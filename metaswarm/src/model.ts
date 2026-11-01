// Model backends. ClaudeModel calls the API; ScriptedModel plays back a demo
// script so the harness runs and can be tested with no API key.
import Anthropic from "@anthropic-ai/sdk";
import { SYSTEM_PROMPT, TOOLS } from "./tools.ts";

type Block = Anthropic.Beta.BetaContentBlock;
type Msgs = Anthropic.Beta.BetaMessageParam[];
export interface Turn {
  content: Block[];
  stop_reason: string | null;
}
export interface Model {
  next(agent: string, messages: Msgs): Promise<Turn>;
}

export class ClaudeModel implements Model {
  private client = new Anthropic();
  constructor(
    private model = "claude-opus-5-5",
    private effort: "low" | "medium" | "high" | "xhigh" | "max" = "medium",
  ) {}

  async next(_agent: string, messages: Msgs): Promise<Turn> {
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

// --- scripted demos ----------------------------------------------------------

/** What a script step can see: everything the agent has received so far. */
export interface Ctx {
  text: string;
  has(s: string | RegExp): boolean;
  /** First capture group of the last match, or "". */
  last(re: RegExp): string;
}
export interface Step {
  when?: (c: Ctx) => boolean;
  turn: (c: Ctx) => Turn;
}
export type Script = Record<string, Step[]>;

let nextId = 0;
export const say = (text: string): Block => ({ type: "text", text, citations: null }) as Block;
export const call = (name: string, input: object): Block =>
  ({ type: "tool_use", id: `toolu_scripted_${nextId++}`, name, input }) as Block;
export const act = (...blocks: Block[]): Turn => ({ content: blocks, stop_reason: "tool_use" });

function ctxOf(messages: Msgs): Ctx {
  let text = "";
  for (const m of messages) {
    if (m.role === "assistant") continue;
    if (typeof m.content === "string") text += m.content + "\n";
    else
      for (const b of m.content ?? []) {
        if (b.type === "text") text += b.text + "\n";
        else if (b.type === "tool_result") text += (typeof b.content === "string" ? b.content : JSON.stringify(b.content)) + "\n";
      }
  }
  return {
    text,
    has: (s) => (typeof s === "string" ? text.includes(s) : s.test(text)),
    last: (re) => {
      const g = new RegExp(re.source, re.flags.includes("g") ? re.flags : re.flags + "g");
      return [...text.matchAll(g)].at(-1)?.[1] ?? "";
    },
  };
}

/** Steps run once each, strictly in order; a step waits until its condition holds. */
export class ScriptedModel implements Model {
  private done = new Map<string, Set<number>>();
  constructor(private script: Script, private delayMs = 120) {}

  async next(agent: string, messages: Msgs): Promise<Turn> {
    await Bun.sleep(this.delayMs);
    const ctx = ctxOf(messages);
    const done = this.done.get(agent) ?? new Set<number>();
    this.done.set(agent, done);
    const steps = this.script[agent] ?? [];
    const i = steps.findIndex((_, i) => !done.has(i));
    if (i === -1 || !(steps[i]!.when?.(ctx) ?? true)) return { content: [say("Nothing to do, waiting.")], stop_reason: "end_turn" };
    done.add(i);
    return steps[i]!.turn(ctx);
  }
}
