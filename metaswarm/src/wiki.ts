// The wiki: a folder of Markdown files in a git repo, i.e. a real Obsidian vault.
// Open runs/<run>/vault in Obsidian to read along; `git log` shows who changed what.
//
// Three tiers of pages:
//   scratch/<agent>/...  the agent's own pages, edited directly
//   any other page       edited through a proposal that a judge panel votes on
//                        (or directly, with an explicit `page:<path>:write` right)
//   locked pages         set by the user; no agent can change them, whatever its rights
import { mkdirSync, existsSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { Denied, type Perms, USER } from "./perms.ts";
import { now, type Store } from "./store.ts";
import { runPanel, type Judge, type Vote } from "./vote.ts";

export interface Proposal {
  id: number;
  path: string;
  author: string;
  reason: string;
  status: "voting" | "approved" | "rejected";
  votes: Vote[];
  ts: string;
}

const PATH_RE = /^(?!.*(^|\/)\.)[A-Za-z0-9_][A-Za-z0-9_ .\/-]*\.md$/;

export class Wiki {
  readonly dir: string;
  private gitQueue: Promise<unknown> = Promise.resolve();
  onDecided: (p: Proposal) => void = () => {};

  constructor(private store: Store, private perms: Perms, private judges: Judge[], dir: string) {
    this.dir = dir;
    store.db.run(`CREATE TABLE IF NOT EXISTS locks (path TEXT PRIMARY KEY, ts TEXT NOT NULL)`);
    store.db.run(`CREATE TABLE IF NOT EXISTS proposals (id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL, author TEXT NOT NULL,
      reason TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, votes TEXT NOT NULL, ts TEXT NOT NULL)`);
  }

  async init(seed: Record<string, string>, locked: string[]) {
    mkdirSync(this.dir, { recursive: true });
    await this.git("init", "-q", "-b", "main");
    for (const [p, content] of Object.entries(seed)) await Bun.write(join(this.dir, this.norm(p)), content);
    if (Object.keys(seed).length) await this.commit(USER, ".", "Seed vault");
    for (const p of locked) this.setLock(USER, p, true);
  }

  // --- paths & locks --------------------------------------------------------

  norm(path: string): string {
    const p = path.replace(/^\/+/, "").replace(/\/+/g, "/");
    const withExt = p.endsWith(".md") ? p : `${p}.md`;
    if (!PATH_RE.test(withExt) || withExt.split("/").includes("..")) throw new Denied(`bad page path "${path}"`);
    return withExt;
  }

  isLocked(path: string): boolean {
    return !!this.store.db.query(`SELECT 1 FROM locks WHERE path = ?`).get(this.norm(path));
  }

  setLock(by: string, path: string, locked: boolean) {
    if (by !== USER) throw new Denied("only the user can lock or unlock pages");
    const p = this.norm(path);
    if (locked) this.store.db.query(`INSERT OR IGNORE INTO locks VALUES (?, ?)`).run(p, now());
    else this.store.db.query(`DELETE FROM locks WHERE path = ?`).run(p);
    this.store.log(by, locked ? "wiki.lock" : "wiki.unlock", { path: p });
  }

  // --- reading --------------------------------------------------------------

  private checkRead(agent: string) {
    if (agent !== USER) this.perms.check(agent, "wiki:read");
  }

  list(agent: string): { path: string; locked: boolean }[] {
    this.checkRead(agent);
    const out: string[] = [];
    const walk = (d: string, rel: string) => {
      if (!existsSync(d)) return;
      for (const name of readdirSync(d)) {
        if (name.startsWith(".")) continue;
        const full = join(d, name);
        const r = rel ? `${rel}/${name}` : name;
        if (statSync(full).isDirectory()) walk(full, r);
        else if (name.endsWith(".md")) out.push(r);
      }
    };
    walk(this.dir, "");
    return out.sort().map((path) => ({ path, locked: this.isLocked(path) }));
  }

  async read(agent: string, path: string): Promise<string | null> {
    this.checkRead(agent);
    const f = Bun.file(join(this.dir, this.norm(path)));
    return (await f.exists()) ? f.text() : null;
  }

  async search(agent: string, query: string): Promise<{ path: string; line: string }[]> {
    this.checkRead(agent);
    const q = query.toLowerCase();
    const hits: { path: string; line: string }[] = [];
    for (const { path } of this.list(agent)) {
      const text = (await this.read(agent, path)) ?? "";
      for (const line of text.split("\n")) if (line.toLowerCase().includes(q)) hits.push({ path, line: line.slice(0, 200) });
    }
    return hits.slice(0, 50);
  }

  /** Pages that link to `path` with [[name]] or [[name|alias]]. */
  async backlinks(agent: string, path: string): Promise<string[]> {
    const target = this.norm(path).replace(/\.md$/, "");
    const short = target.split("/").pop()!;
    const out: string[] = [];
    for (const { path: p } of this.list(agent)) {
      const text = (await this.read(agent, p)) ?? "";
      const links = [...text.matchAll(/\[\[([^\]|#]+)/g)].map((m) => m[1]!.trim().replace(/\.md$/, ""));
      if (links.some((l) => l === target || l === short)) out.push(p);
    }
    return out;
  }

  // --- writing --------------------------------------------------------------

  private canWriteDirect(agent: string, p: string): boolean {
    if (agent === USER || p.startsWith(`scratch/${agent}/`)) return true;
    if (this.perms.mode(agent, `page:${p}:write`) === "ask") this.perms.check(agent, `page:${p}:write`); // throws NeedsApproval
    return this.perms.can(agent, `page:${p}:write`);
  }

  async write(agent: string, path: string, content: string, message: string) {
    const p = this.norm(path);
    if (agent !== USER && this.isLocked(p)) throw new Denied(`${p} is locked by the user`);
    if (!this.canWriteDirect(agent, p)) throw new Denied(`direct edits to ${p} need page:${p}:write; use wiki_propose instead`);
    await this.apply(agent, p, content, message);
  }

  propose(agent: string, path: string, content: string, reason: string): Proposal {
    const p = this.norm(path);
    if (this.isLocked(p)) throw new Denied(`${p} is locked by the user`);
    if (!this.perms.can(agent, `page:${p}:propose`)) this.perms.check(agent, "wiki:propose");
    const { id } = this.store.db
      .query(`INSERT INTO proposals (path, author, reason, content, status, votes, ts) VALUES (?, ?, ?, ?, 'voting', '[]', ?) RETURNING id`)
      .get(p, agent, reason, content, now()) as { id: number };
    this.store.log(agent, "wiki.propose", { id, path: p, reason });
    void this.decide(id);
    return this.proposal(id)!;
  }

  private async decide(id: number) {
    const row = this.store.db.query(`SELECT * FROM proposals WHERE id = ?`).get(id) as any;
    const before = (await this.read(USER, row.path)) ?? "";
    const { approved, votes } = await runPanel(this.judges, { path: row.path, author: row.author, reason: row.reason, before, after: row.content });
    let status: Proposal["status"] = approved ? "approved" : "rejected";
    // The user may have locked the page while the vote ran: the lock wins.
    if (approved && this.isLocked(row.path)) {
      status = "rejected";
      votes.push({ judge: "harness", approve: false, reason: "page was locked by the user during the vote" });
    }
    if (status === "approved") await this.apply(row.author, row.path, row.content, `${row.reason} (proposal #${id})`);
    this.store.db.query(`UPDATE proposals SET status = ?, votes = ? WHERE id = ?`).run(status, JSON.stringify(votes), id);
    this.store.log("harness", "wiki.decided", { id, path: row.path, status, votes });
    this.onDecided(this.proposal(id)!);
  }

  proposal(id: number): Proposal | undefined {
    return this.proposals().find((p) => p.id === id);
  }

  proposals(): Proposal[] {
    return (this.store.db.query(`SELECT id, path, author, reason, status, votes, ts FROM proposals ORDER BY id`).all() as any[]).map((r) => ({
      ...r,
      votes: JSON.parse(r.votes),
    }));
  }

  pendingVotes(): number {
    return this.proposals().filter((p) => p.status === "voting").length;
  }

  private async apply(author: string, p: string, content: string, message: string) {
    const file = join(this.dir, p);
    mkdirSync(dirname(file), { recursive: true });
    await Bun.write(file, content.endsWith("\n") ? content : content + "\n");
    await this.commit(author, p, message);
    this.store.log(author, "wiki.write", { path: p, message });
  }

  // --- git ------------------------------------------------------------------

  private git(...args: string[]): Promise<string> {
    // Serialize: concurrent git commands would fight over index.lock.
    const run = async () => {
      const proc = Bun.spawn(["git", "-C", this.dir, ...args], { stdout: "pipe", stderr: "pipe" });
      const [out, err, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
      if (code !== 0) throw new Error(`git ${args[0]} failed: ${err.trim()}`);
      return out;
    };
    const next = this.gitQueue.then(run, run);
    this.gitQueue = next.catch(() => {});
    return next;
  }

  private async commit(author: string, path: string, message: string) {
    await this.git("add", "--", path);
    const id = `${author} <${author}@metaswarm.local>`;
    await this.git("-c", `user.name=${author}`, "-c", `user.email=${author}@metaswarm.local`, "commit", "-q", "--allow-empty", "--author", id, "-m", message);
  }

  async history(path: string): Promise<{ commit: string; author: string; date: string; subject: string }[]> {
    const out = await this.git("log", "--format=%H%x09%an%x09%aI%x09%s", "--", this.norm(path));
    return out.trim().split("\n").filter(Boolean).map((l) => {
      const [commit, author, date, subject] = l.split("\t");
      return { commit: commit!, author: author!, date: date!, subject: subject! };
    });
  }

  async show(commit: string): Promise<string> {
    if (!/^[0-9a-f]{7,40}$/.test(commit)) throw new Denied("bad commit id");
    return this.git("show", "--format=%h %an %aI%n%s%n", commit);
  }
}
