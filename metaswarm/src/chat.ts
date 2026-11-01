// Discord-like channels. A channel is a UUID with a topic and members.
// Rights per channel: channel:<id>:read | post | invite | manage.
// Creating a channel needs `channel:create` and gives the creator `channel:<id>:*` (delegable).
// Joining an *open* channel needs `channel:join`; invite-only channels need an invite.
import { Denied, type Perms, USER } from "./perms.ts";
import { now, type Store } from "./store.ts";

export interface Channel {
  id: string;
  topic: string;
  visibility: "open" | "invite";
  createdBy: string;
  ts: string;
}
export interface Post {
  id: number;
  channel: string;
  thread: number | null;
  author: string;
  body: string;
  kind: string; // post | ad | system
  ts: string;
}
export type Notify = "all" | "mentions";

const RATE_WINDOW_MS = 60_000;
const RATE_MAX = 12; // posts per agent per channel per minute

export class Chat {
  private recent = new Map<string, number[]>();

  constructor(private store: Store, private perms: Perms) {
    const db = store.db;
    db.run(`CREATE TABLE IF NOT EXISTS channels (id TEXT PRIMARY KEY, topic TEXT NOT NULL, visibility TEXT NOT NULL, created_by TEXT NOT NULL, ts TEXT NOT NULL)`);
    db.run(`CREATE TABLE IF NOT EXISTS members (channel TEXT NOT NULL, agent TEXT NOT NULL, notify TEXT NOT NULL, read_cursor INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (channel, agent))`);
    db.run(`CREATE TABLE IF NOT EXISTS posts (id INTEGER PRIMARY KEY AUTOINCREMENT, channel TEXT NOT NULL, thread INTEGER, author TEXT NOT NULL, body TEXT NOT NULL, kind TEXT NOT NULL, ts TEXT NOT NULL)`);
  }

  channels(): Channel[] {
    return (this.store.db.query(`SELECT * FROM channels ORDER BY ts`).all() as any[]).map((r) => ({
      id: r.id, topic: r.topic, visibility: r.visibility, createdBy: r.created_by, ts: r.ts,
    }));
  }

  channel(id: string): Channel {
    const c = this.channels().find((c) => c.id === id);
    if (!c) throw new Denied(`no channel ${id}`);
    return c;
  }

  members(channel: string): { agent: string; notify: Notify }[] {
    return this.store.db.query(`SELECT agent, notify FROM members WHERE channel = ? ORDER BY agent`).all(channel) as any[];
  }

  isMember(channel: string, agent: string) {
    return !!this.store.db.query(`SELECT 1 FROM members WHERE channel = ? AND agent = ?`).get(channel, agent);
  }

  /** Channels an agent may see listed: open ones, plus any it belongs to. */
  visibleTo(agent: string): (Channel & { member: boolean; unread: number })[] {
    return this.channels()
      .filter((c) => agent === USER || c.visibility === "open" || this.isMember(c.id, agent))
      .map((c) => ({ ...c, member: this.isMember(c.id, agent), unread: this.unread(c.id, agent) }));
  }

  create(by: string, topic: string, visibility: "open" | "invite"): Channel {
    this.perms.check(by, "channel:create");
    const id = crypto.randomUUID();
    this.store.db.query(`INSERT INTO channels VALUES (?, ?, ?, ?, ?)`).run(id, topic, visibility, by, now());
    this.store.log(by, "channel.create", { id, topic, visibility });
    if (by !== USER) {
      this.perms.grant(USER, by, `channel:${id}:*`, { delegable: true });
      this.addMember(id, by, "all");
    }
    return this.channel(id);
  }

  join(agent: string, id: string, notify: Notify = "all") {
    const c = this.channel(id);
    if (this.isMember(id, agent)) return;
    if (!this.perms.can(agent, `channel:${id}:read`)) {
      if (c.visibility !== "open") throw new Denied(`channel ${id} is invite-only`);
      this.perms.check(agent, "channel:join");
      this.perms.grant(USER, agent, `channel:${id}:read`);
      this.perms.grant(USER, agent, `channel:${id}:post`);
    }
    this.addMember(id, agent, notify);
  }

  invite(by: string, id: string, agent: string, post = true) {
    this.channel(id);
    this.perms.check(by, `channel:${id}:invite`);
    // The invitee's rights hang under the inviter's grant, so revoking the
    // inviter also removes everyone they let in.
    if (!this.perms.can(agent, `channel:${id}:read`)) this.perms.grant(by, agent, `channel:${id}:read`);
    if (post && !this.perms.can(agent, `channel:${id}:post`)) this.perms.grant(by, agent, `channel:${id}:post`);
    this.addMember(id, agent, "all");
    this.store.log(by, "channel.invite", { id, agent });
  }

  setNotify(agent: string, id: string, notify: Notify) {
    this.store.db.query(`UPDATE members SET notify = ? WHERE channel = ? AND agent = ?`).run(notify, id, agent);
  }

  private addMember(id: string, agent: string, notify: Notify) {
    this.store.db.query(`INSERT OR IGNORE INTO members (channel, agent, notify) VALUES (?, ?, ?)`).run(id, agent, notify);
    this.store.log(agent, "channel.join", { id });
  }

  post(author: string, id: string, body: string, opts: { thread?: number | null; kind?: string } = {}): Post {
    this.channel(id);
    if (author !== USER) {
      this.perms.check(author, `channel:${id}:post`);
      const key = `${author}|${id}`;
      const t = Date.now();
      const times = (this.recent.get(key) ?? []).filter((x) => t - x < RATE_WINDOW_MS);
      if (times.length >= RATE_MAX) throw new Denied(`rate limit: at most ${RATE_MAX} posts per minute in a channel`);
      times.push(t);
      this.recent.set(key, times);
    }
    const thread = opts.thread ?? null;
    if (thread !== null) {
      const root = this.store.db.query(`SELECT channel, thread FROM posts WHERE id = ?`).get(thread) as any;
      if (!root || root.channel !== id) throw new Denied(`no post #${thread} in this channel`);
      if (root.thread !== null) throw new Denied(`#${thread} is a reply; reply to the thread root #${root.thread}`);
    }
    const kind = opts.kind ?? "post";
    const ts = now();
    const { id: pid } = this.store.db
      .query(`INSERT INTO posts (channel, thread, author, body, kind, ts) VALUES (?, ?, ?, ?, ?, ?) RETURNING id`)
      .get(id, thread, author, body, kind, ts) as { id: number };
    const post = { id: pid, channel: id, thread, author, body, kind, ts };
    this.store.log(author, "post", post);
    return post;
  }

  read(agent: string, id: string, opts: { thread?: number; limit?: number; markRead?: boolean } = {}): Post[] {
    this.channel(id);
    if (agent !== USER) this.perms.check(agent, `channel:${id}:read`);
    const limit = Math.min(opts.limit ?? 30, 200);
    const rows = (
      opts.thread
        ? this.store.db.query(`SELECT * FROM posts WHERE channel = ? AND (id = ? OR thread = ?) ORDER BY id DESC LIMIT ?`).all(id, opts.thread, opts.thread, limit)
        : this.store.db.query(`SELECT * FROM posts WHERE channel = ? ORDER BY id DESC LIMIT ?`).all(id, limit)
    ) as Post[];
    rows.reverse();
    if (opts.markRead !== false && rows.length && agent !== USER && !opts.thread) {
      this.store.db.query(`UPDATE members SET read_cursor = MAX(read_cursor, ?) WHERE channel = ? AND agent = ?`).run(rows.at(-1)!.id, id, agent);
    }
    return rows;
  }

  all(id: string): Post[] {
    return this.store.db.query(`SELECT * FROM posts WHERE channel = ? ORDER BY id`).all(id) as Post[];
  }

  unread(id: string, agent: string): number {
    const m = this.store.db.query(`SELECT read_cursor FROM members WHERE channel = ? AND agent = ?`).get(id, agent) as any;
    if (!m) return 0;
    return (this.store.db.query(`SELECT COUNT(*) AS n FROM posts WHERE channel = ? AND id > ? AND author != ?`).get(id, m.read_cursor, agent) as any).n;
  }

  /** Posts since `afterId` that should wake this agent: any post in an "all" channel, or a mention anywhere it can read. */
  wakeups(agent: string, afterId: number): Post[] {
    const rows = this.store.db
      .query(
        `SELECT p.*, m.notify FROM posts p JOIN members m ON m.channel = p.channel AND m.agent = ?
         WHERE p.id > ? AND p.author != ? ORDER BY p.id`,
      )
      .all(agent, afterId, agent) as (Post & { notify: Notify })[];
    const mention = new RegExp(`(^|\\W)@${agent}(\\W|$)`);
    return rows.filter((p) => p.notify === "all" || mention.test(p.body));
  }

  lastPostId(): number {
    return (this.store.db.query(`SELECT COALESCE(MAX(id), 0) AS n FROM posts`).get() as any).n;
  }
}
