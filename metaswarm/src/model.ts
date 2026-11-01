// Model backends. ClaudeModel calls the API; ScriptedModel replays a fixed demo
// so the harness can be run and tested with no API key.
import Anthropic from "@anthropic-ai/sdk";
import { SYSTEM_PROMPT, TOOLS } from "./roles.ts";

type Block = Anthropic.Beta.BetaContentBlock;
export interface Turn {
  content: Block[];
  stop_reason: string | null;
}

export interface Model {
  next(agent: string, messages: Anthropic.Beta.BetaMessageParam[]): Promise<Turn>;
}

export class ClaudeModel implements Model {
  private client = new Anthropic();
  constructor(
    private model = "claude-opus-5-5",
    private effort: "low" | "medium" | "high" | "xhigh" | "max" = "medium",
  ) {}

  async next(_agent: string, messages: Anthropic.Beta.BetaMessageParam[]): Promise<Turn> {
    const res = await this.client.beta.messages.create({
      model: this.model,
      max_tokens: 16000,
      system: SYSTEM_PROMPT,
      tools: TOOLS,
      messages,
      thinking: { type: "adaptive" },
      output_config: { effort: this.effort },
      cache_control: { type: "ephemeral" },
      // If a safety classifier declines, the API retries on a recommended
      // fallback model inside the same call.
      betas: ["server-side-fallback-2026-07-01"],
      fallbacks: "default",
    });
    return { content: res.content, stop_reason: res.stop_reason };
  }
}

// ---------------------------------------------------------------------------
// Scripted demo: one overseer (boss) and two workers (alice, bob).
// alice asks for a promotion, boss grants it, and alice then finds she can no
// longer run bash, which shows that permissions follow the role.

type Msgs = Anthropic.Beta.BetaMessageParam[];
interface Ctx {
  inbox: string; // every text the agent has received (user + system turns)
  role: string; // latest role the harness announced
}
interface Step {
  when: (c: Ctx) => boolean;
  turn: (c: Ctx) => Turn;
}

let nextId = 0;
const say = (text: string): Block => ({ type: "text", text, citations: null }) as Block;
const call = (name: string, input: object): Block =>
  ({ type: "tool_use", id: `toolu_scripted_${nextId++}`, name, input }) as Block;
const tools = (...blocks: Block[]): Turn => ({ content: blocks, stop_reason: "tool_use" });

const always = () => true;
const has = (s: string) => (c: Ctx) => c.inbox.includes(s);

const SCRIPTS: Record<string, Step[]> = {
  boss: [
    {
      when: always,
      turn: () =>
        tools(
          say("Splitting the goal in two."),
          call("spawn_worker", { id: "alice", task: "Write /workspace/notes.md with a 3-line summary of what metaswarm is, then report back to boss." }),
          call("spawn_worker", { id: "bob", task: "Compute the sum of 1..100 in your sandbox and check whether you have network access. Report both to boss." }),
        ),
    },
    {
      when: has('kind="role_request"'),
      turn: () =>
        tools(
          say("alice finished first and wants to help coordinate. Approving."),
          call("set_role", { agent: "alice", role: "overseer" }),
        ),
    },
    {
      // Wait for alice, now an overseer, to confirm bob is done too.
      when: has('from="alice" kind="note"'),
      turn: () =>
        tools(call("finish", { summary: "alice wrote notes.md, bob computed 5050 and confirmed the sandbox has no network. alice was promoted to overseer." })),
    },
  ],
  alice: [
    {
      when: (c) => c.role === "worker",
      turn: () =>
        tools(call("bash", { command: "printf '# metaswarm\\n- agents talk over a shared bus\\n- roles can change mid-run\\n- tools only run in containers\\n' > notes.md && cat notes.md && id" })),
    },
    {
      when: (c) => c.role === "worker",
      turn: () =>
        tools(
          call("send_message", { to: "boss", kind: "result", body: "notes.md written (4 lines) in /workspace." }),
          call("request_role", { role: "overseer", reason: "I'm done and can help review bob's work." }),
        ),
    },
    {
      when: (c) => c.role === "overseer",
      turn: () => tools(say("Let me double-check notes.md myself."), call("bash", { command: "cat notes.md" })),
    },
    {
      when: (c) => c.role === "overseer",
      turn: () => tools(call("send_message", { to: "bob", kind: "question", body: "Overseer alice here: are you done? notes.md is in /workspace if you need it." })),
    },
    {
      when: has('from="bob" kind="note"'),
      turn: () => tools(call("send_message", { to: "boss", kind: "note", body: "bob confirmed he is done. Everything is reported; ready to finish." })),
    },
  ],
  bob: [
    {
      when: always,
      turn: () =>
        tools(call("bash", { command: "seq 1 100 | awk '{s+=$1} END {print \"sum:\", s}'; wget -q -T 3 -O- http://example.com >/dev/null 2>&1 && echo 'network: yes' || echo 'network: no'" })),
    },
    {
      when: always,
      turn: () => tools(call("send_message", { to: "boss", kind: "result", body: "sum(1..100) = 5050. No network in the sandbox, as expected." })),
    },
    {
      when: has('from="alice" kind="question"'),
      turn: () => tools(call("send_message", { to: "alice", kind: "note", body: "Yes, done. Already reported to boss." })),
    },
  ],
};

function texts(messages: Msgs): Ctx {
  let inbox = "";
  let role = "";
  for (const m of messages) {
    if (m.role === "assistant") continue;
    const parts = typeof m.content === "string" ? [m.content] : (m.content ?? []).map((b) => ("text" in b && typeof b.text === "string" ? b.text : ""));
    for (const p of parts) {
      inbox += p + "\n";
      const r = m.role === "system" && /Your role is now (\w+)/.exec(p);
      if (r) role = r[1]!.toLowerCase();
    }
  }
  return { inbox, role };
}

export class ScriptedModel implements Model {
  private done = new Map<string, Set<number>>();

  async next(agent: string, messages: Msgs): Promise<Turn> {
    await Bun.sleep(150); // pretend to think, so agents interleave
    const ctx = texts(messages);
    const done = this.done.get(agent) ?? new Set<number>();
    this.done.set(agent, done);
    const steps = SCRIPTS[agent] ?? [];
    const i = steps.findIndex((s, i) => !done.has(i) && s.when(ctx));
    if (i === -1) return { content: [say("Nothing to do, waiting.")], stop_reason: "end_turn" };
    done.add(i);
    return steps[i]!.turn(ctx);
  }
}
