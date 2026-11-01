// Fine-grained rights with delegation.
//
// A right is a string such as `bash`, `spawn`, `channel:<uuid>:post`, or
// `page:docs/*:write`. A grant may use `*` as a wildcard matching any text.
//
// Rules:
// - "user" (the human) holds everything and can grant anything.
// - An agent can grant a right only if it holds a *delegable* grant covering it.
//   The new grant hangs under that parent grant in a tree.
// - Revoking a grant revokes everything granted from it, recursively.
// - Expired or revoked grants count for nothing.
// - An "ask" grant means the right is held, but every use needs the user's
//   approval. Anything delegated from an ask grant is ask too.
// So rights can move around between agents but never grow beyond what the user gave out.
import { now, type Store } from "./store.ts";

export const USER = "user";

export interface Grant {
  id: number;
  subject: string;
  right: string;
  delegable: boolean;
  ask: boolean;
  grantedBy: string;
  parent: number | null;
  revoked: boolean;
  expiresAt: string | null;
  ts: string;
}

const RIGHT_RE = /^[A-Za-z0-9_.*:\/@-]{1,200}$/;

function globToRegex(glob: string): RegExp {
  const esc = glob.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replaceAll("*", ".*");
  return new RegExp(`^${esc}$`, "s");
}

/** Does pattern `p` cover right (or pattern) `r`? `*` in `r` is treated as a literal, so this is conservative. */
export function covers(p: string, r: string): boolean {
  return globToRegex(p).test(r);
}

export class Perms {
  constructor(private store: Store) {
    store.db.run(`CREATE TABLE IF NOT EXISTS grants (
      id INTEGER PRIMARY KEY AUTOINCREMENT, subject TEXT NOT NULL, "right" TEXT NOT NULL,
      delegable INTEGER NOT NULL, granted_by TEXT NOT NULL, parent INTEGER,
      revoked INTEGER NOT NULL DEFAULT 0, expires_at TEXT, ts TEXT NOT NULL)`);
    const cols = store.db.query(`PRAGMA table_info(grants)`).all() as { name: string }[];
    if (!cols.some((c) => c.name === "ask")) store.db.run(`ALTER TABLE grants ADD COLUMN ask INTEGER NOT NULL DEFAULT 0`);
  }

  /** Rights approved by the user for the call in progress, per agent (an agent runs one tool at a time). */
  private approved = new Map<string, Set<string>>();

  private rows(where: string, ...args: any[]): Grant[] {
    return (this.store.db.query(`SELECT * FROM grants ${where}`).all(...args) as any[]).map((r) => ({
      id: r.id, subject: r.subject, right: r.right, delegable: !!r.delegable, ask: !!r.ask, grantedBy: r.granted_by,
      parent: r.parent, revoked: !!r.revoked, expiresAt: r.expires_at, ts: r.ts,
    }));
  }

  all(): Grant[] {
    return this.rows("ORDER BY id");
  }

  /** Live grants held by an agent. */
  held(subject: string): Grant[] {
    const t = now();
    return this.rows("WHERE subject = ? AND revoked = 0 AND (expires_at IS NULL OR expires_at > ?) ORDER BY id", subject, t);
  }

  /** "allow": usable now; "ask": held, but each use needs the user's approval; "deny": not held. */
  mode(subject: string, right: string): "allow" | "ask" | "deny" {
    if (subject === USER) return "allow";
    const gs = this.held(subject).filter((g) => covers(g.right, right));
    if (gs.some((g) => !g.ask)) return "allow";
    if (this.approved.get(subject)?.has(right)) return "allow";
    return gs.length ? "ask" : "deny";
  }

  /** Usable right now, without asking. */
  can(subject: string, right: string): boolean {
    return this.mode(subject, right) === "allow";
  }

  /** Held at all (possibly needing approval). */
  holds(subject: string, right: string): boolean {
    return this.mode(subject, right) !== "deny";
  }

  /** Throws Denied, or NeedsApproval for ask rights. */
  check(subject: string, right: string) {
    const m = this.mode(subject, right);
    if (m === "ask") throw new NeedsApproval(subject, right);
    if (m === "deny") throw new Denied(`${subject} lacks right "${right}"`);
  }

  /** Run `fn` with `rights` approved for `subject`. */
  async withApproved<T>(subject: string, rights: string[], fn: () => Promise<T>): Promise<T> {
    this.approved.set(subject, new Set(rights));
    try {
      return await fn();
    } finally {
      this.approved.delete(subject);
    }
  }

  grant(by: string, subject: string, right: string, opts: { delegable?: boolean; ttlS?: number; ask?: boolean } = {}): Grant {
    if (!RIGHT_RE.test(right)) throw new Denied(`malformed right "${right}"`);
    const delegable = opts.delegable ?? false;
    let parent: number | null = null;
    if (by !== USER) {
      if (subject === by) throw new Denied("agents cannot grant rights to themselves");
      const p = this.held(by).find((g) => g.delegable && covers(g.right, right));
      if (!p) throw new Denied(`${by} has no delegable right covering "${right}"`);
      parent = p.id;
    }
    // A child grant never outlives its parent.
    let expiresAt = opts.ttlS ? new Date(Date.now() + opts.ttlS * 1000).toISOString() : null;
    const pg = parent !== null ? this.rows("WHERE id = ?", parent)[0] : undefined;
    const ask = !!opts.ask || !!pg?.ask;
    if (pg?.expiresAt && (!expiresAt || pg.expiresAt < expiresAt)) expiresAt = pg.expiresAt;

    const { id } = this.store.db
      .query(`INSERT INTO grants (subject, "right", delegable, ask, granted_by, parent, expires_at, ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id`)
      .get(subject, right, delegable ? 1 : 0, ask ? 1 : 0, by, parent, expiresAt, now()) as { id: number };
    this.store.log(by, "grant", { id, subject, right, delegable, ask, parent, expiresAt });
    return this.rows("WHERE id = ?", id)[0]!;
  }

  /** Revoke a grant and all its descendants. Only the granter, an ancestor's holder, or the user may do it. */
  revoke(by: string, id: number): number[] {
    const g = this.rows("WHERE id = ?", id)[0];
    if (!g) throw new Denied(`no grant #${id}`);
    if (by !== USER && !this.isAncestorHolder(by, g)) throw new Denied(`${by} did not grant #${id} (nor anything above it)`);
    const ids: number[] = [];
    const walk = (gid: number) => {
      ids.push(gid);
      for (const c of this.rows("WHERE parent = ? AND revoked = 0", gid)) walk(c.id);
    };
    walk(id);
    this.store.db.run(`UPDATE grants SET revoked = 1 WHERE id IN (${ids.join(",")})`);
    this.store.log(by, "revoke", { ids });
    return ids;
  }

  private isAncestorHolder(by: string, g: Grant): boolean {
    let cur: Grant | undefined = g;
    while (cur) {
      if (cur.grantedBy === by) return true;
      cur = cur.parent !== null ? this.rows("WHERE id = ?", cur.parent)[0] : undefined;
    }
    return false;
  }

  /** Subjects affected when these grants are revoked (to notify them). */
  subjectsOf(ids: number[]): string[] {
    if (!ids.length) return [];
    return [...new Set(this.rows(`WHERE id IN (${ids.join(",")})`).map((g) => g.subject))];
  }
}

export class Denied extends Error {
  override name = "Denied";
}

/** The agent holds the right through an "ask" grant: the user must approve this use. */
export class NeedsApproval extends Error {
  override name = "NeedsApproval";
  constructor(readonly agent: string, readonly right: string) {
    super(`"${right}" needs the user's approval`);
  }
}
