// The swarm: runs one loop per agent, routes tool calls, enforces roles and budgets.
import type Anthropic from "@anthropic-ai/sdk";
import { Bus, type Message, type Role } from "./bus.ts";
import type { Model } from "./model.ts";
import { INPUTS, isToolName, ROLE_BRIEF, ROLE_TOOLS, type ToolName } from "./roles.ts";
import type { Sandbox, SandboxFactory } from "./sandbox.ts";

type Param = Anthropic.Beta.BetaMessageParam;
type UserBlock = Anthropic.Beta.BetaTextBlockParam | Anthropic.Beta.BetaToolResultBlockParam;

export interface Limits {
  maxAgents: number;
  maxCallsPerAgent: number;
  idleNudges: number; // how many times to poke idle overseers before giving up
}

class Agent {
  messages: Param[] = [];
  cursor = 0;
  announced: Role | null = null;
  sandbox: Sandbox | null = null;
  status: "running" | "idle" | "stopped" = "running";
  calls = 0;
  constructor(public id: string, public role: Role, public spawnedBy: string) {}
}

const COLORS = [36, 33, 35, 32, 34, 31];
const ROLE_TAG: Record<Role, string> = { overseer: "OVR", worker: "WRK" };

export class Swarm {
  private agents = new Map<string, Agent>();
  private loops: Promise<void>[] = [];
  private waiters = new Set<() => void>();
  private done = false;
  summary: string | null = null;

  constructor(
    private bus: Bus,
    private model: Model,
    private sandboxes: SandboxFactory,
    private limits: Limits,
  ) {
    bus.onMessage(() => this.wakeAll());
  }

  // --- public ---------------------------------------------------------------

  async run(goal: string, overseer = "boss"): Promise<string> {
    this.spawn(overseer, "overseer", "human", `The human's goal:\n\n${goal}`);
    const watchdog = this.watchdog();
    while (this.loops.length) {
      // Loops can be added while we wait (spawn_worker), so drain repeatedly.
      const pending = this.loops.splice(0);
      await Promise.all(pending);
    }
    clearInterval(watchdog);
    await this.sandboxes.shutdown();
    return this.summary ?? "(run ended without a summary)";
  }

  // --- agents ---------------------------------------------------------------

  private spawn(id: string, role: Role, by: string, intro: string) {
    const a = new Agent(id, role, by);
    this.agents.set(id, a);
    this.log("harness", `spawned ${id} as ${role} (by ${by})`);
    this.bus.event(id, "spawn", { role, by });
    this.loops.push(
      this.loop(a, `Your agent id is "${id}". You were started by ${by}.\n\n${intro}`).catch((e) => {
        a.status = "stopped";
        this.log(id, `crashed: ${e instanceof Error ? e.message : e}`);
        this.bus.event(id, "crash", String(e));
        this.bus.send("harness", "@overseer", "note", `${id} crashed and was stopped: ${e}`);
      }),
    );
  }

  private async loop(a: Agent, intro: string) {
    let pending: UserBlock[] = [{ type: "text", text: intro }];
    while (!this.done) {
      const inbox = this.bus.inbox(a.id, a.role, a.cursor);
      if (inbox.length) {
        a.cursor = inbox.at(-1)!.id;
        pending.push({ type: "text", text: inbox.map(formatMessage).join("\n") });
      }
      if (!pending.length) {
        a.status = "idle";
        await this.waitForMail(a);
        a.status = "running";
        continue;
      }

      if (a.calls >= this.limits.maxCallsPerAgent) {
        a.status = "stopped";
        this.log(a.id, "budget exhausted, stopping");
        this.bus.send("harness", "@overseer", "note", `${a.id} ran out of model calls and was stopped.`);
        return;
      }

      a.messages.push({ role: "user", content: pending });
      pending = [];
      // Role announcements go in a system message: the harness's own channel,
      // which peers cannot forge by writing text in a <message>.
      if (a.announced !== a.role) {
        a.messages.push({ role: "system", content: roleAnnouncement(a.role) });
        a.announced = a.role;
      }

      a.calls++;
      const turn = await this.model.next(a.id, a.messages);
      a.messages.push({ role: "assistant", content: turn.content });
      this.bus.event(a.id, "turn", { stop_reason: turn.stop_reason, content: turn.content });

      for (const b of turn.content) if (b.type === "text" && b.text.trim()) this.log(a.id, b.text.trim());
      if (turn.stop_reason === "refusal") this.log(a.id, "(model declined this turn)");

      const uses = turn.content.filter((b): b is Anthropic.Beta.BetaToolUseBlock => b.type === "tool_use");
      if (uses.length && turn.stop_reason === "tool_use") {
        // All results go back in one user message, as the API expects.
        pending = await Promise.all(uses.map((u) => this.runTool(a, u)));
      }
    }
    a.status = "stopped";
  }

  private waitForMail(a: Agent): Promise<void> {
    return new Promise((resolve) => {
      const check = () => {
        if (this.done || this.bus.inbox(a.id, a.role, a.cursor).length) {
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

  /** If everyone is idle with no mail, poke the overseers; give up after a few pokes. */
  private watchdog() {
    let quietTicks = 0;
    let nudges = 0;
    return setInterval(() => {
      const live = [...this.agents.values()].filter((a) => a.status !== "stopped");
      const stalled = live.every((a) => a.status === "idle" && !this.bus.inbox(a.id, a.role, a.cursor).length);
      if (!stalled) return void (quietTicks = 0);
      if (++quietTicks < 3) return;
      quietTicks = 0;
      if (++nudges > this.limits.idleNudges || !live.some((a) => a.role === "overseer")) {
        this.log("harness", "swarm is stalled, ending run");
        this.end("Run ended by the watchdog: every agent went idle.");
        return;
      }
      this.bus.send("harness", "@overseer", "note", "Every agent is idle and no messages are pending. Call finish if the goal is met, or assign more work.");
    }, 1000);
  }

  private end(summary: string) {
    this.done = true;
    this.summary ??= summary;
    this.wakeAll();
  }

  // --- tools ----------------------------------------------------------------

  private async runTool(a: Agent, use: Anthropic.Beta.BetaToolUseBlock): Promise<Anthropic.Beta.BetaToolResultBlockParam> {
    const fail = (msg: string) => {
      this.log(a.id, `✗ ${use.name}: ${msg}`);
      this.bus.event(a.id, "tool_error", { tool: use.name, input: use.input, error: msg });
      return { type: "tool_result" as const, tool_use_id: use.id, is_error: true, content: msg };
    };
    if (!isToolName(use.name)) return fail(`unknown tool ${use.name}`);
    if (!ROLE_TOOLS[a.role].has(use.name)) {
      return fail(`${use.name} is not allowed for role ${a.role}. Allowed: ${[...ROLE_TOOLS[a.role]].join(", ")}`);
    }
    const parsed = INPUTS[use.name].safeParse(use.input);
    if (!parsed.success) return fail(`invalid input: ${parsed.error.message}`);

    try {
      const out = await this.tool(a, use.name, parsed.data);
      this.bus.event(a.id, "tool", { tool: use.name, input: use.input, output: out });
      return { type: "tool_result", tool_use_id: use.id, content: out };
    } catch (e) {
      return fail(e instanceof Error ? e.message : String(e));
    }
  }

  private async tool(a: Agent, name: ToolName, input: any): Promise<string> {
    switch (name) {
      case "send_message": {
        const { to, kind, body } = input as { to: string; kind: string; body: string };
        if (!["*", "@overseer", "@worker"].includes(to) && !this.agents.has(to)) throw new Error(`no agent named ${to}`);
        const m = this.bus.send(a.id, to, kind, body);
        this.log(a.id, `→ ${to} [${kind}] ${body}`);
        return `sent (#${m.id})`;
      }
      case "request_role": {
        const { role, reason } = input as { role: Role; reason: string };
        if (role === a.role) throw new Error(`you already are a ${role}`);
        this.bus.send(a.id, "@overseer", "role_request", `${a.id} asks to become ${role}: ${reason}`);
        this.log(a.id, `asks to become ${role}`);
        return "request sent to the overseers; wait for a role announcement";
      }
      case "spawn_worker": {
        const { id, task } = input as { id: string; task: string };
        if (this.agents.has(id)) throw new Error(`agent ${id} already exists`);
        if (this.agents.size >= this.limits.maxAgents) throw new Error(`agent limit (${this.limits.maxAgents}) reached`);
        this.spawn(id, "worker", a.id, "Your task will arrive as a message.");
        this.bus.send(a.id, id, "task", task);
        this.log(a.id, `→ ${id} [task] ${task}`);
        return `spawned ${id}`;
      }
      case "set_role": {
        const { agent, role } = input as { agent: string; role: Role };
        const t = this.agents.get(agent);
        if (!t || t.status === "stopped") throw new Error(`no live agent named ${agent}`);
        if (t.role === role) return `${agent} already is a ${role}`;
        const overseers = [...this.agents.values()].filter((x) => x.role === "overseer" && x.status !== "stopped");
        if (t.role === "overseer" && overseers.length === 1) throw new Error("cannot demote the last overseer");
        t.role = role;
        // Overseers have no sandbox. A demoted agent gets a fresh one on its next bash call.
        if (role === "overseer" && t.sandbox) {
          await t.sandbox.destroy();
          t.sandbox = null;
        }
        this.log("harness", `${agent} is now ${role} (set by ${a.id})`);
        this.bus.send("harness", agent, "role_change", `${a.id} changed your role to ${role}.`);
        return `${agent} is now a ${role}`;
      }
      case "finish": {
        const { summary } = input as { summary: string };
        this.log(a.id, `finished: ${summary}`);
        this.summary = summary;
        this.end(summary);
        return "run finished";
      }
      case "bash": {
        const { command } = input as { command: string };
        a.sandbox ??= await this.sandboxes.create(a.id);
        this.log(a.id, `$ ${command}`);
        const r = await a.sandbox.exec(command);
        const out = [`exit ${r.code}${r.timedOut ? " (timed out)" : ""}`, r.stdout && `stdout:\n${r.stdout}`, r.stderr && `stderr:\n${r.stderr}`]
          .filter(Boolean)
          .join("\n");
        this.log(a.id, out.split("\n").map((l) => "  " + l).join("\n"));
        return out;
      }
    }
  }

  // --- output ---------------------------------------------------------------

  private log(who: string, text: string) {
    const a = this.agents.get(who);
    const idx = a ? [...this.agents.keys()].indexOf(who) : -1;
    const color = idx >= 0 ? COLORS[idx % COLORS.length] : 90;
    const tag = a ? ` ${ROLE_TAG[a.role]}` : "";
    console.log(`\x1b[${color}m[${who}${tag}]\x1b[0m ${text}`);
  }
}

function roleAnnouncement(role: Role): string {
  return `Your role is now ${role.toUpperCase()}. ${ROLE_BRIEF[role]} Tools you may use: ${[...ROLE_TOOLS[role]].join(", ")}.`;
}

function formatMessage(m: Message): string {
  const esc = (s: string) => s.replaceAll("</message>", "<\\/message>");
  return `<message id="${m.id}" from="${m.from}" kind="${m.kind}" to="${m.to}">\n${esc(m.body)}\n</message>`;
}
