// The swarm: one loop per agent. Delivers posts and harness notices, runs tools,
// enforces budgets, and lets the user pause agents or end the run.
import type Anthropic from "@anthropic-ai/sdk";
import { Chat } from "./chat.ts";
import type { Model } from "./model.ts";
import { Denied, Perms, USER } from "./perms.ts";
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
}

export interface AgentSpec {
  id: string;
  brief: string;
  rights: { right: string; delegable?: boolean }[];
}

export class Agent {
  messages: Param[] = [];
  notices: string[] = [];
  wakeCursor = 0;
  sandbox: Sandbox | null = null;
  status: "running" | "idle" | "stopped" = "running";
  paused = false;
  calls = 0;
  /** Aborts the current model call or tool when the user interrupts. */
  abort = new AbortController();
  constructor(public id: string, public spawnedBy: string, public brief: string) {}
}

const COLORS = [36, 33, 35, 32, 34, 31, 96, 93];

export class Swarm implements World {
  readonly agents = new Map<string, Agent>();
  private loops: Promise<void>[] = [];
  private waiters = new Set<() => void>();
  done = false;
  summary: string | null = null;

  constructor(
    readonly store: Store,
    readonly perms: Perms,
    readonly chat: Chat,
    readonly wiki: Wiki,
    private model: Model,
    private sandboxes: SandboxFactory,
    readonly limits: Limits,
    readonly registry: ToolRegistry,
    readonly runId: string,
    readonly dir: string,
  ) {
    store.on((e) => {
      if (e.type === "post" || e.type === "notice") this.wakeAll();
    });
    wiki.onDecided = (p) => {
      const votes = p.votes.map((v) => `${v.judge}: ${v.approve ? "yes" : "no"} (${v.reason})`).join("; ");
      this.notify(p.author, `Your proposal #${p.id} for ${p.path} was ${p.status.toUpperCase()}. Votes: ${votes}`);
    };
  }

  // --- run ------------------------------------------------------------------

  async run(initial: AgentSpec[]): Promise<string> {
    for (const s of initial) this.spawn(USER, s.id, s.brief, s.rights);
    const watchdog = this.watchdog();
    while (this.loops.length) await Promise.all(this.loops.splice(0));
    clearInterval(watchdog);
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

  spawn(by: string, id: string, brief: string, rights: { right: string; delegable?: boolean }[]) {
    if (this.agents.has(id)) throw new Denied(`agent ${id} already exists`);
    if (this.agents.size >= this.limits.maxAgents) throw new Denied(`agent limit (${this.limits.maxAgents}) reached`);
    const a = new Agent(id, by, brief);
    this.agents.set(id, a);
    for (const r of rights) this.perms.grant(by, id, r.right, { delegable: r.delegable });
    this.store.log(by, "spawn", { id, brief, rights });
    this.log("harness", `spawned ${id} (by ${by}) with ${rights.map((r) => r.right + (r.delegable ? "+" : "")).join(", ") || "no rights"}`);
    const intro = `Your agent id is "${id}". You were started by ${by}.\n\n<brief>\n${brief}\n</brief>`;
    this.loops.push(
      this.loop(a, intro).catch((e) => {
        a.status = "stopped";
        this.log(id, `crashed: ${e instanceof Error ? e.message : e}`);
        this.store.log(id, "crash", String(e));
      }),
    );
  }

  notify(agent: string, text: string) {
    const a = this.agents.get(agent);
    if (!a) return;
    a.notices.push(text);
    this.store.log("harness", "notice", { agent, text });
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
    this.summary = summary;
    this.done = true;
    this.wakeAll();
  }

  setPaused(agent: string, paused: boolean) {
    const a = this.agents.get(agent);
    if (!a) throw new Denied(`no agent ${agent}`);
    a.paused = paused;
    this.store.log(USER, paused ? "pause" : "resume", { agent });
    this.wakeAll();
  }

  // --- agent loop -----------------------------------------------------------

  private async loop(a: Agent, intro: string) {
    let pending: UserBlock[] = [{ type: "text", text: intro }];
    a.wakeCursor = this.chat.lastPostId();
    while (!this.done) {
      if (a.paused) {
        a.status = "idle";
        await this.waitFor(() => !a.paused);
        continue;
      }
      const posts = this.chat.wakeups(a.id, a.wakeCursor);
      if (posts.length) {
        a.wakeCursor = posts.at(-1)!.id;
        pending.push({ type: "text", text: this.deliver(a, posts) });
      }
      const notices = a.notices.splice(0);
      if (!pending.length && !notices.length) {
        a.status = "idle";
        await this.waitFor(() => a.notices.length > 0 || this.chat.wakeups(a.id, a.wakeCursor).length > 0);
        a.status = "running";
        continue;
      }
      if (a.calls >= this.limits.maxCallsPerAgent) {
        a.status = "stopped";
        this.log(a.id, "budget exhausted, stopping");
        this.store.log(a.id, "budget", {});
        return;
      }

      // A user turn must come first; notices then follow as a system message,
      // the channel that only the harness can write to.
      if (!pending.length) pending.push({ type: "text", text: "(harness notice below)" });
      a.messages.push({ role: "user", content: pending });
      if (notices.length) a.messages.push({ role: "system", content: notices.map((n) => `- ${n}`).join("\n") });
      pending = [];

      a.calls++;
      const turn = await this.model.next(a.id, a.messages, { system: this.registry.systemPrompt(), tools: this.registry.tools(), signal: a.abort.signal });
      a.messages.push({ role: "assistant", content: turn.content });
      this.store.log(a.id, "turn", { stop_reason: turn.stop_reason, content: turn.content });
      for (const b of turn.content) if (b.type === "text" && b.text.trim()) this.log(a.id, b.text.trim());
      if (turn.stop_reason === "refusal") this.log(a.id, "(model declined this turn)");

      const uses = turn.content.filter((b): b is Anthropic.Beta.BetaToolUseBlock => b.type === "tool_use");
      if (uses.length && turn.stop_reason === "tool_use") {
        // Run in order: tools in one turn often depend on each other (create, then invite).
        for (const u of uses) pending.push(await this.tool(a, u));
      }
    }
    a.status = "stopped";
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

  private async tool(a: Agent, u: Anthropic.Beta.BetaToolUseBlock): Promise<Anthropic.Beta.BetaToolResultBlockParam> {
    try {
      const out = await this.registry.run({ world: this, agent: a.id, signal: a.abort.signal }, u.name, u.input);
      this.log(a.id, `${u.name} ${short(u.input)} → ${short(out)}`);
      this.store.log(a.id, "tool", { tool: u.name, input: u.input, output: out });
      return { type: "tool_result", tool_use_id: u.id, content: out };
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      this.log(a.id, `✗ ${u.name} ${short(u.input)}: ${msg}`);
      this.store.log(a.id, "tool_error", { tool: u.name, input: u.input, error: msg, denied: e instanceof Denied });
      return { type: "tool_result", tool_use_id: u.id, is_error: true, content: msg };
    }
  }

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

  private wakeAll() {
    for (const w of [...this.waiters]) w();
  }

  /** When nothing is happening, nudge whoever can finish; then give up. */
  private watchdog() {
    let quiet = 0;
    let nudges = 0;
    return setInterval(() => {
      if (this.done) return;
      const all = [...this.agents.values()].filter((a) => a.status !== "stopped");
      if (all.some((a) => a.paused)) return void (quiet = 0); // the user is inspecting: don't end the run under them
      const live = all;
      const busy = live.some((a) => a.status === "running") || this.wiki.pendingVotes() > 0;
      if (busy) return void (quiet = 0);
      if (++quiet < 3) return;
      quiet = 0;
      const finishers = live.filter((a) => this.perms.can(a.id, "run:finish"));
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
