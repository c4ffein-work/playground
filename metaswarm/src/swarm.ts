// The swarm: one loop per agent. Delivers posts, notices and user messages,
// runs tools through hooks and approvals, persists transcripts so a run can be
// resumed, retries failed model calls, and lets the user pause, interrupt and
// message agents.
import Anthropic from "@anthropic-ai/sdk";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { Approvals } from "./approvals.ts";
import type { Chat } from "./chat.ts";
import type { Hooks } from "./hooks.ts";
import type { Model, Turn } from "./model.ts";
import { Denied, NeedsApproval, type Perms, USER } from "./perms.ts";
import type { Sandbox, SandboxFactory } from "./sandbox.ts";
import type { Store } from "./store.ts";
import { fmtPost, type ToolRegistry, type World } from "./tools.ts";
import type { Wiki } from "./wiki.ts";

type Param = Anthropic.Beta.BetaMessageParam;
type UserBlock = Anthropic.Beta.BetaTextBlockParam | Anthropic.Beta.BetaToolResultBlockParam;

export interface Limits {
  maxAgents: number;
  maxCallsPerAgent: number;
  idleNudges: number;
  /** Tool results longer than this are saved to a file and shown truncated. */
  maxToolResult: number;
  /** Context window used for the meter. */
  contextLimit: number;
}
export const DEFAULT_LIMITS: Limits = { maxAgents: 8, maxCallsPerAgent: 30, idleNudges: 2, maxToolResult: 25_000, contextLimit: 1_000_000 };

export interface AgentSpec {
  id: string;
  brief: string;
  rights: { right: string; delegable?: boolean; ask?: boolean }[];
}

export interface SwarmDeps {
  store: Store;
  perms: Perms;
  chat: Chat;
  wiki: Wiki;
  approvals: Approvals;
  hooks: Hooks;
  model: Model;
  sandboxes: SandboxFactory;
  registry: ToolRegistry;
  limits: Limits;
  runId: string;
  dir: string;
}

export type AgentStatus = "running" | "idle" | "stopped" | "errored";

export class Agent {
  messages: Param[] = [];
  notices: string[] = [];
  userQueue: string[] = [];
  wakeCursor = 0;
  sandbox: Sandbox | null = null;
  status: AgentStatus = "running";
  error: string | null = null;
  paused = false;
  /** Interrupted by the user: wait for a user message before calling the model again. */
  interrupted = false;
  calls = 0;
  outputs = 0;
  context = { tokens: 0, compactions: 0 };
  abort = new AbortController();
  constructor(public id: string, public spawnedBy: string, public brief: string) {}
}

const COLORS = [36, 33, 35, 32, 34, 31, 96, 93];
const RETRY_DELAYS_MS = [2_000, 8_000, 30_000];

export class Swarm implements World {
  readonly agents = new Map<string, Agent>();
  private loops: Promise<void>[] = [];
  private waiters = new Set<() => void>();
  done = false;
  summary: string | null = null;

  readonly store: Store;
  readonly perms: Perms;
  readonly chat: Chat;
  readonly wiki: Wiki;
  readonly approvals: Approvals;
  readonly hooks: Hooks;
  readonly registry: ToolRegistry;
  readonly limits: Limits;
  readonly runId: string;
  readonly dir: string;
  private model: Model;
  private sandboxes: SandboxFactory;

  constructor(d: SwarmDeps) {
    ({ store: this.store, perms: this.perms, chat: this.chat, wiki: this.wiki, approvals: this.approvals, hooks: this.hooks } = d);
    ({ registry: this.registry, limits: this.limits, runId: this.runId, dir: this.dir, model: this.model, sandboxes: this.sandboxes } = d);
    const db = this.store.db;
    db.run(`CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, spawned_by TEXT NOT NULL, brief TEXT NOT NULL,
      wake_cursor INTEGER NOT NULL DEFAULT 0, calls INTEGER NOT NULL DEFAULT 0, outputs INTEGER NOT NULL DEFAULT 0, stopped INTEGER NOT NULL DEFAULT 0,
      pending TEXT NOT NULL DEFAULT '[]')`);
    db.run(`CREATE TABLE IF NOT EXISTS messages (agent TEXT NOT NULL, seq INTEGER NOT NULL, json TEXT NOT NULL, PRIMARY KEY (agent, seq))`);
    db.run(`CREATE TABLE IF NOT EXISTS run (key TEXT PRIMARY KEY, value TEXT NOT NULL)`);

    this.store.on((e) => {
      if (e.type === "post" || e.type === "notice" || e.type === "user.message") this.wakeAll();
    });
    this.wiki.onDecided = (p) => {
      const votes = p.votes.map((v) => `${v.judge}: ${v.approve ? "yes" : "no"} (${v.reason})`).join("; ");
      this.notify(p.author, `Your proposal #${p.id} for ${p.path} was ${p.status.toUpperCase()}. Votes: ${votes}`);
    };
  }

  // --- run ------------------------------------------------------------------

  async run(initial: AgentSpec[]): Promise<string> {
    for (const s of initial) this.spawn(USER, s.id, s.brief, s.rights);
    return this.wait();
  }

  /** Bring back every agent of a previous process from the database and continue. */
  async resume(): Promise<string> {
    const rows = this.store.db.query(`SELECT * FROM agents ORDER BY rowid`).all() as any[];
    if (this.store.db.query(`SELECT value FROM run WHERE key = 'done'`).get()) throw new Error("this run already finished");
    for (const r of rows) {
      const a = new Agent(r.id, r.spawned_by, r.brief);
      a.wakeCursor = r.wake_cursor;
      a.calls = r.calls;
      a.outputs = r.outputs;
      a.messages = (this.store.db.query(`SELECT json FROM messages WHERE agent = ? ORDER BY seq`).all(r.id) as any[]).map((m) => JSON.parse(m.json));
      this.agents.set(a.id, a);
      if (r.stopped) {
        a.status = "stopped";
        continue;
      }
      this.store.log("harness", "resume", { agent: a.id, messages: a.messages.length });
      this.log("harness", `resumed ${a.id} (${a.messages.length} messages)`);
      this.notify(a.id, "The run was restarted. Your containers are new: anything outside /workspace is gone. Check where you were and carry on.");
      this.start(a, this.repair(a, JSON.parse(r.pending)));
    }
    return this.wait();
  }

  /**
   * Rebuild what the agent was about to send. Tool results produced before the
   * crash were saved; calls that never finished get an error result, so the
   * transcript stays valid.
   */
  private repair(a: Agent, saved: UserBlock[]): UserBlock[] {
    const last = a.messages.at(-1);
    if (!last || last.role !== "assistant" || typeof last.content === "string") return saved;
    const have = new Set(saved.flatMap((b) => (b.type === "tool_result" ? [b.tool_use_id] : [])));
    const missing = last.content
      .filter((b): b is Anthropic.Beta.BetaToolUseBlockParam => b.type === "tool_use" && !have.has(b.id))
      .map((b) => ({ type: "tool_result" as const, tool_use_id: b.id, is_error: true, content: "No result: the run was restarted while this was running." }));
    return [...saved, ...missing];
  }

  private async wait(): Promise<string> {
    const watchdog = this.watchdog();
    while (this.loops.length) await Promise.all(this.loops.splice(0));
    clearInterval(watchdog);
    await this.hooks.emit("RunEnd", "run", { summary: this.summary });
    await this.registry.shutdown();
    await this.sandboxes.shutdown();
    return this.summary ?? "(run ended without a summary)";
  }

  // --- World (used by tools) -----------------------------------------------

  agentIds() {
    return [...this.agents.keys()];
  }

  agentInfo() {
    return [...this.agents.values()].map((a) => ({ id: a.id, status: a.paused ? "paused" : a.status, spawnedBy: a.spawnedBy, calls: a.calls }));
  }

  budget(id: string) {
    return { used: this.agents.get(id)?.calls ?? 0, max: this.limits.maxCallsPerAgent };
  }

  spawn(by: string, id: string, brief: string, rights: { right: string; delegable?: boolean; ask?: boolean }[]) {
    if (this.agents.has(id)) throw new Denied(`agent ${id} already exists`);
    if (this.agents.size >= this.limits.maxAgents) throw new Denied(`agent limit (${this.limits.maxAgents}) reached`);
    const a = new Agent(id, by, brief);
    this.agents.set(id, a);
    this.store.db.query(`INSERT INTO agents (id, spawned_by, brief) VALUES (?, ?, ?)`).run(id, by, brief);
    for (const r of rights) this.perms.grant(by, id, r.right, { delegable: r.delegable, ask: r.ask });
    this.store.log(by, "spawn", { id, brief, rights });
    this.log("harness", `spawned ${id} (by ${by}) with ${rights.map((r) => r.right + (r.delegable ? "+" : "") + (r.ask ? "?" : "")).join(", ") || "no rights"}`);
    a.wakeCursor = this.chat.lastPostId();
    this.start(a, [{ type: "text", text: `Your agent id is "${id}". You were started by ${by}.\n\n<brief>\n${brief}\n</brief>` }]);
  }

  private start(a: Agent, pending: UserBlock[]) {
    this.loops.push(
      this.loop(a, pending).catch((e) => {
        a.status = "stopped";
        this.log(a.id, `crashed: ${e instanceof Error ? e.stack : e}`);
        this.store.log(a.id, "crash", String(e));
      }),
    );
  }

  notify(agent: string, text: string) {
    const a = this.agents.get(agent);
    if (!a) return;
    a.notices.push(text);
    this.store.log("harness", "notice", { agent, text });
    void this.hooks.emit("Notice", agent, { agent, text });
  }

  async sandbox(agent: string): Promise<Sandbox> {
    const a = this.agents.get(agent)!;
    a.sandbox ??= await this.sandboxes.create(agent);
    return a.sandbox;
  }

  async dropSandbox(agent: string) {
    const a = this.agents.get(agent);
    if (a?.sandbox) {
      await a.sandbox.destroy();
      a.sandbox = null;
    }
  }

  finish(by: string, summary: string) {
    this.log(by, `finished: ${summary}`);
    this.store.log(by, "finish", { summary });
    this.store.db.query(`INSERT OR REPLACE INTO run VALUES ('done', ?)`).run(summary);
    this.summary = summary;
    this.done = true;
    for (const a of this.agents.values()) a.abort.abort();
    this.wakeAll();
  }

  async readOutput(agent: string, id: number, offset: number, limit: number): Promise<string> {
    const f = Bun.file(join(this.dir, "outputs", agent, `${id}.txt`));
    if (!(await f.exists())) throw new Denied(`no saved output #${id}`);
    const text = await f.text();
    const part = text.slice(offset, offset + limit);
    const more = offset + limit < text.length ? `\n[… ${text.length - offset - limit} more chars; next offset ${offset + limit}]` : "";
    return part + more;
  }

  // --- user controls --------------------------------------------------------

  private agent(id: string): Agent {
    const a = this.agents.get(id);
    if (!a) throw new Denied(`no agent ${id}`);
    return a;
  }

  setPaused(agent: string, paused: boolean) {
    this.agent(agent).paused = paused;
    this.store.log(USER, paused ? "pause" : "resume", { agent });
    this.wakeAll();
  }

  /** A direct message, delivered at the agent's next model call, even mid-task. */
  message(agent: string, body: string) {
    const a = this.agent(agent);
    if (a.status === "stopped") throw new Denied(`${agent} is stopped`);
    a.userQueue.push(body);
    this.store.log(USER, "user.message", { agent, body });
  }

  /** Abort the current model call or command; the agent then waits for a user message. */
  interrupt(agent: string) {
    const a = this.agent(agent);
    a.interrupted = true;
    a.abort.abort();
    a.abort = new AbortController();
    this.store.log(USER, "interrupt", { agent });
    this.log(agent, "interrupted by the user");
    this.wakeAll();
  }

  retry(agent: string) {
    const a = this.agent(agent);
    if (a.status !== "errored") throw new Denied(`${agent} is not errored`);
    a.status = "running";
    a.error = null;
    this.store.log(USER, "retry", { agent });
    this.wakeAll();
  }

  // --- agent loop -----------------------------------------------------------

  private async loop(a: Agent, pending: UserBlock[]) {
    while (!this.done) {
      if (a.paused || a.status === "errored") {
        if (a.paused) a.status = a.status === "errored" ? "errored" : "idle";
        await this.waitFor(() => !a.paused && a.status !== "errored");
        a.status = "running";
        continue;
      }
      const posts = this.chat.wakeups(a.id, a.wakeCursor);
      if (posts.length) {
        a.wakeCursor = posts.at(-1)!.id;
        this.saveAgent(a);
        pending.push({ type: "text", text: this.deliver(a, posts) });
      }
      for (const body of a.userQueue.splice(0)) pending.push({ type: "text", text: `[user → you] ${body}` });
      if (pending.some((b) => b.type === "text" && b.text.startsWith("[user → you]"))) a.interrupted = false;
      const notices = a.notices.splice(0);

      if (a.interrupted || (!pending.length && !notices.length)) {
        a.notices.unshift(...notices);
        a.status = "idle";
        void this.hooks.emit("AgentIdle", a.id, { agent: a.id, interrupted: a.interrupted });
        await this.waitFor(
          () => a.userQueue.length > 0 || a.paused || (!a.interrupted && (a.notices.length > 0 || this.chat.wakeups(a.id, a.wakeCursor).length > 0)),
        );
        a.status = "running";
        continue;
      }
      if (a.calls >= this.limits.maxCallsPerAgent) {
        a.status = "stopped";
        this.saveAgent(a, true);
        this.log(a.id, "budget exhausted, stopping");
        this.store.log(a.id, "budget", {});
        return;
      }

      // A user turn comes first; notices follow as a system message, the
      // channel only the harness writes to.
      if (!pending.length) pending.push({ type: "text", text: "(harness notice below)" });
      const add: Param[] = [{ role: "user", content: pending }];
      if (notices.length) add.push({ role: "system", content: notices.map((n) => `- ${n}`).join("\n") });

      const signal = a.abort.signal;
      let turn: Turn;
      try {
        turn = await this.callModel(a, [...a.messages, ...add], signal);
      } catch (e) {
        // Nothing was appended: keep what we meant to send for the next attempt.
        a.notices.unshift(...notices);
        if (signal.aborted) {
          if (this.done) break;
          continue; // interrupted: `pending` is kept, the loop waits for the user
        }
        a.status = "errored";
        a.error = e instanceof Error ? e.message : String(e);
        this.store.log(a.id, "agent.error", { error: a.error });
        this.log(a.id, `model call failed: ${a.error}`);
        continue;
      }
      pending = [];
      for (const m of add) this.push(a, m);
      this.saveAgent(a, false, []);
      this.push(a, { role: "assistant", content: turn.content });
      a.calls++;
      this.track(a, turn);
      this.saveAgent(a);
      this.store.log(a.id, "turn", { stop_reason: turn.stop_reason, content: turn.content });
      for (const b of turn.content) if (b.type === "text" && b.text.trim()) this.log(a.id, b.text.trim());
      if (turn.stop_reason === "refusal") this.log(a.id, "(model declined this turn)");

      const uses = turn.content.filter((b): b is Anthropic.Beta.BetaToolUseBlock => b.type === "tool_use");
      if (uses.length && turn.stop_reason === "tool_use") {
        // In order: tools in one turn often depend on each other.
        for (const u of uses) {
          pending.push(signal.aborted ? { type: "tool_result", tool_use_id: u.id, is_error: true, content: "Interrupted by the user before this ran." } : await this.tool(a, u, signal));
          this.saveAgent(a, false, pending); // so a crash doesn't lose results
        }
      }
    }
    if (a.status !== "stopped") a.status = "stopped";
  }

  private async callModel(a: Agent, messages: Param[], signal: AbortSignal): Promise<Turn> {
    for (let attempt = 0; ; attempt++) {
      try {
        return await this.model.next(a.id, messages, { system: this.registry.systemPrompt(), tools: this.registry.tools(), signal });
      } catch (e) {
        if (signal.aborted || e instanceof Anthropic.APIUserAbortError) throw e;
        const retryable =
          e instanceof Anthropic.APIConnectionError ||
          e instanceof Anthropic.RateLimitError ||
          e instanceof Anthropic.InternalServerError ||
          (e instanceof Anthropic.APIError && (e.status === 529 || e.status === 408));
        if (!retryable || attempt >= RETRY_DELAYS_MS.length) throw e;
        const delay = RETRY_DELAYS_MS[attempt]!;
        this.log(a.id, `model call failed (${e instanceof Error ? e.message : e}); retrying in ${delay / 1000}s`);
        this.store.log(a.id, "retry.model", { attempt: attempt + 1, error: String(e) });
        await Promise.race([Bun.sleep(delay), new Promise((r) => signal.addEventListener("abort", r, { once: true }))]);
        if (signal.aborted) throw e;
      }
    }
  }

  private track(a: Agent, turn: Turn) {
    const u = turn.usage;
    a.context.tokens = u
      ? u.input_tokens + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0) + u.output_tokens
      : Math.round(JSON.stringify(a.messages).length / 4);
    const compactions = turn.content.filter((b) => (b as { type: string }).type === "compaction").length;
    if (compactions) {
      a.context.compactions += compactions;
      this.store.log(a.id, "compaction", { tokens: a.context.tokens });
      this.log(a.id, "context compacted");
    }
  }

  private push(a: Agent, m: Param) {
    this.store.db.query(`INSERT INTO messages VALUES (?, ?, ?)`).run(a.id, a.messages.length, JSON.stringify(m));
    a.messages.push(m);
  }

  private saveAgent(a: Agent, stopped = false, pending?: UserBlock[]) {
    this.store.db
      .query(`UPDATE agents SET wake_cursor = ?, calls = ?, outputs = ?, stopped = ?, pending = COALESCE(?, pending) WHERE id = ?`)
      .run(a.wakeCursor, a.calls, a.outputs, stopped ? 1 : 0, pending ? JSON.stringify(pending) : null, a.id);
  }

  private deliver(a: Agent, posts: ReturnType<Chat["wakeups"]>): string {
    const byChannel = new Map<string, typeof posts>();
    for (const p of posts) byChannel.set(p.channel, [...(byChannel.get(p.channel) ?? []), p]);
    const parts: string[] = [];
    for (const [cid, ps] of byChannel) {
      const c = this.chat.channel(cid);
      parts.push(`New in channel ${cid} "${c.topic}":\n${ps.slice(-10).map(fmtPost).join("\n")}`);
      this.chat.read(a.id, cid, { limit: 1 }); // advance the read cursor
    }
    return parts.join("\n\n");
  }

  // --- tools ----------------------------------------------------------------

  /** Hooks → approvals → the tool itself → output size guard → post hooks. */
  private async tool(a: Agent, u: Anthropic.Beta.BetaToolUseBlock, signal: AbortSignal): Promise<Anthropic.Beta.BetaToolResultBlockParam> {
    let input: unknown = u.input;
    try {
      const pre = await this.hooks.preToolUse(a.id, u.name, input);
      if (pre.decision === "deny") throw new Denied(pre.reason);
      input = pre.input;
      if (pre.decision === "ask") await this.askUser(a, u.name, input, `hook:${u.name}`, pre.reason, signal);

      const approved: string[] = [];
      let out: string;
      for (let i = 0; ; i++) {
        try {
          out = await this.perms.withApproved(a.id, approved, () => this.registry.run({ world: this, agent: a.id, signal }, u.name, input));
          break;
        } catch (e) {
          if (!(e instanceof NeedsApproval) || i >= 4) throw e;
          await this.askUser(a, u.name, input, e.right, `uses "${e.right}", granted with "ask"`, signal);
          approved.push(e.right);
        }
      }
      out = await this.clip(a, out);
      this.log(a.id, `${u.name} ${short(input)} → ${short(out)}`);
      this.store.log(a.id, "tool", { tool: u.name, input, output: out });
      void this.hooks.emit("PostToolUse", u.name, { agent: a.id, tool: u.name, input, output: out, error: false });
      return { type: "tool_result", tool_use_id: u.id, content: out };
    } catch (e) {
      const msg = signal.aborted ? "Interrupted by the user." : e instanceof Error ? e.message : String(e);
      this.log(a.id, `✗ ${u.name} ${short(input)}: ${msg}`);
      this.store.log(a.id, "tool_error", { tool: u.name, input, error: msg, denied: e instanceof Denied });
      void this.hooks.emit("PostToolUse", u.name, { agent: a.id, tool: u.name, input, output: msg, error: true });
      return { type: "tool_result", tool_use_id: u.id, is_error: true, content: msg };
    }
  }

  private async askUser(a: Agent, tool: string, input: unknown, right: string, reason: string, signal: AbortSignal) {
    this.log(a.id, `waiting for user approval: ${tool} (${right})`);
    const ap = await this.approvals.request(a.id, tool, input, right, reason, signal);
    if (ap.status !== "approved") throw new Denied(`the user ${ap.status === "denied" ? "refused" : "did not approve"} this call${ap.note ? `: ${ap.note}` : ""}`);
  }

  /** Big outputs go to a file the agent can page through with read_output. */
  private async clip(a: Agent, out: string): Promise<string> {
    if (out.length <= this.limits.maxToolResult) return out;
    const id = ++a.outputs;
    this.saveAgent(a);
    const dir = join(this.dir, "outputs", a.id);
    mkdirSync(dir, { recursive: true });
    await Bun.write(join(dir, `${id}.txt`), out);
    const head = Math.floor(this.limits.maxToolResult * 0.4);
    return `${out.slice(0, head)}\n\n[output truncated: ${out.length} chars in total. Saved as output #${id}; read more with read_output {"id": ${id}, "offset": ${head}}]`;
  }

  // --- waiting ----------------------------------------------------------------

  private waitFor(cond: () => boolean): Promise<void> {
    return new Promise((resolve) => {
      const check = () => {
        if (this.done || cond()) {
          this.waiters.delete(check);
          resolve();
        }
      };
      this.waiters.add(check);
      check();
    });
  }

  wakeAll() {
    for (const w of [...this.waiters]) w();
  }

  /** When nothing is happening, nudge whoever can finish; then give up. */
  private watchdog() {
    let quiet = 0;
    let nudges = 0;
    return setInterval(() => {
      if (this.done) return;
      const all = [...this.agents.values()].filter((a) => a.status !== "stopped");
      // The user is involved (inspecting, deciding, or must fix an error): don't end the run under them.
      if (all.some((a) => a.paused || a.interrupted || a.status === "errored") || this.approvals.pending() > 0) return void (quiet = 0);
      const busy = all.some((a) => a.status === "running") || this.wiki.pendingVotes() > 0;
      if (busy) return void (quiet = 0);
      if (++quiet < 3) return;
      quiet = 0;
      const finishers = all.filter((a) => this.perms.holds(a.id, "run:finish"));
      if (++nudges > this.limits.idleNudges || !finishers.length) {
        this.log("harness", "everyone is idle, ending run");
        this.finish("harness", this.summary ?? "Run ended by the watchdog: every agent went idle.");
        return;
      }
      for (const f of finishers) this.notify(f.id, "Every agent is idle. Call finish if the goal is met, or get things moving again.");
    }, 1000);
  }

  log(who: string, text: string) {
    const idx = [...this.agents.keys()].indexOf(who);
    const color = idx >= 0 ? COLORS[idx % COLORS.length] : 90;
    console.log(`\x1b[${color}m[${who}]\x1b[0m ${text}`);
  }
}

function short(v: unknown): string {
  const s = typeof v === "string" ? v : JSON.stringify(v);
  const one = s.replace(/\s+/g, " ");
  return one.length > 140 ? one.slice(0, 140) + "…" : one;
}
