// Approvals, hooks, big outputs, interrupt + direct messages, errors + retry, resume.
import { expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type Anthropic from "@anthropic-ai/sdk";
import type { Demo } from "../src/demos.ts";
import type { HookConfig } from "../src/hooks.ts";
import { act, call, say, type Model, type Turn } from "../src/model.ts";
import { resumeRun, setup } from "../src/run.ts";
import type { Sandbox, SandboxFactory } from "../src/sandbox.ts";
import type { Swarm } from "../src/swarm.ts";
import { scriptedJudges } from "../src/vote.ts";

type Msgs = Anthropic.Beta.BetaMessageParam[];
const tmp = () => mkdtempSync(join(tmpdir(), "metaswarm-ctl-"));
const until = async (cond: () => boolean, ms = 5000) => {
  const t = Date.now();
  while (!cond()) {
    if (Date.now() - t > ms) throw new Error("timed out waiting");
    await Bun.sleep(10);
  }
};
const lastText = (m: Msgs) => JSON.stringify(m.at(-1)) + JSON.stringify(m.at(-2) ?? "");
const done = (summary = "ok"): Turn => act(call("finish", { summary }));
const idle = (): Turn => ({ content: [say("waiting")], stop_reason: "end_turn" });

/** A model written as a function of the call number and the messages. */
class FnModel implements Model {
  n = 0;
  seen: Msgs[] = [];
  constructor(private fn: (n: number, m: Msgs, signal?: AbortSignal) => Promise<Turn> | Turn) {}
  async next(_a: string, m: Msgs, req?: { signal?: AbortSignal }) {
    this.seen.push(m);
    return this.fn(this.n++, m, req?.signal);
  }
}

const echo: SandboxFactory = {
  async create(): Promise<Sandbox> {
    return {
      async exec(cmd) {
        return { code: 0, stdout: cmd.startsWith("big") ? "x".repeat(60_000) : `ran: ${cmd}\n`, stderr: "", timedOut: false };
      },
      async destroy() {},
    };
  },
  async shutdown() {},
};

function demo(rights: { right: string; ask?: boolean }[]): Demo {
  return { name: "t", description: "", channels: [], agents: [{ id: "a", brief: "test", rights: [...rights, { right: "run:finish" }] }], seed: {}, locked: [], script: {} };
}

async function start(model: Model, rights: { right: string; ask?: boolean }[], extra: { hooks?: HookConfig[]; dir?: string } = {}) {
  const dir = extra.dir ?? tmp();
  const { swarm, start } = await setup(demo(rights), { dir, model, judges: scriptedJudges(), sandboxes: echo, hooks: extra.hooks, limits: { maxToolResult: 5_000 } });
  return { swarm, dir, run: start() };
}

const results = (s: Swarm) => s.store.events().filter((e) => e.type === "tool" || e.type === "tool_error").map((e) => ({ type: e.type, ...(e.data as any) }));

test("ask rights wait for the user; approve runs, deny refuses", async () => {
  const m = new FnModel((n) => (n === 0 ? act(call("bash", { command: "one" })) : n === 1 ? act(call("bash", { command: "two" })) : done()));
  const { swarm, run } = await start(m, [{ right: "bash", ask: true }]);
  await until(() => swarm.approvals.pending() === 1);
  const first = swarm.approvals.all()[0]!;
  expect(first.right).toBe("bash");
  swarm.approvals.decide(first.id, true);
  await until(() => swarm.approvals.all().length === 2 && swarm.approvals.pending() === 1);
  swarm.approvals.decide(swarm.approvals.all()[0]!.id, false, "not now");
  await run;
  const r = results(swarm);
  expect(r[0]).toMatchObject({ type: "tool", tool: "bash", output: expect.stringContaining("ran: one") });
  expect(r[1]).toMatchObject({ type: "tool_error", tool: "bash" });
  expect(r[1].error).toContain("not now");
});

test("an ask grant can only be delegated as ask", async () => {
  const m = new FnModel(() => idle());
  const { swarm } = await start(m, [{ right: "bash", ask: true }]);
  swarm.perms.grant("user", "a", "wiki:read", { delegable: true, ask: true });
  expect(swarm.perms.grant("a", "b", "wiki:read").ask).toBe(true);
  swarm.finish("user", "x");
});

test("hooks: deny, rewrite input, ask, post", async () => {
  const dir = tmp();
  const log = join(dir, "post.log");
  writeFileSync(join(dir, "pre.sh"), `#!/bin/sh
input=$(cat)
case "$input" in
  *rm*) echo "no rm" >&2; exit 2 ;;
  *rewrite*) echo '{"input": {"command": "rewritten"}}' ;;
  *please-ask*) echo '{"decision": "ask", "reason": "looks risky"}' ;;
esac
`);
  const hooks: HookConfig[] = [
    { event: "PreToolUse", matcher: "^bash$", command: ["sh", join(dir, "pre.sh")] },
    { event: "PostToolUse", command: ["sh", "-c", `cat >> ${log}; echo >> ${log}`] },
  ];
  const cmds = ["rm -rf /", "rewrite me", "please-ask"];
  const m = new FnModel((n) => (n < 3 ? act(call("bash", { command: cmds[n] })) : done()));
  const { swarm, run } = await start(m, [{ right: "bash" }], { hooks, dir });
  await until(() => swarm.approvals.pending() === 1);
  expect(swarm.approvals.all()[0]!.right).toBe("hook:bash");
  swarm.approvals.decide(swarm.approvals.all()[0]!.id, true);
  await run;
  const r = results(swarm);
  expect(r[0]).toMatchObject({ type: "tool_error" });
  expect(r[0].error).toContain("no rm");
  expect(r[1]).toMatchObject({ type: "tool", output: expect.stringContaining("ran: rewritten") });
  expect(r[2]).toMatchObject({ type: "tool", output: expect.stringContaining("ran: please-ask") });
  // PostToolUse hooks run in the background: give the last one a moment.
  const lines = async () => (await Bun.file(log).text().catch(() => "")).split("\n").filter(Boolean).length;
  for (let i = 0; i < 100 && (await lines()) < 4; i++) await Bun.sleep(20);
  expect(await lines()).toBeGreaterThanOrEqual(4);
});

test("big outputs are saved and paged with read_output", async () => {
  const m = new FnModel((n, msgs) => {
    if (n === 0) return act(call("bash", { command: "big" }));
    if (n === 1) {
      expect(lastText(msgs)).toContain("Saved as output #1");
      return act(call("read_output", { id: 1, offset: 59_990 }));
    }
    expect(lastText(msgs)).toContain("xxxxxxxxxx");
    return done();
  });
  const { run } = await start(m, [{ right: "bash" }]);
  await run;
  expect(m.n).toBe(3);
});

test("interrupt aborts a call; a direct message resumes the agent mid-task", async () => {
  const m = new FnModel(async (n, msgs, signal) => {
    if (n === 0) {
      await new Promise((_, rej) => signal!.addEventListener("abort", () => rej(new Error("aborted"))));
    }
    expect(lastText(msgs)).toContain("[user → you] stop and finish");
    return done("finished after message");
  });
  const { swarm, run } = await start(m, [{ right: "bash" }]);
  await until(() => m.n === 1);
  swarm.interrupt("a");
  await until(() => swarm.agents.get("a")!.status === "idle");
  await Bun.sleep(50);
  expect(m.n).toBe(1); // waits for the user instead of calling again
  swarm.message("a", "stop and finish");
  expect(await run).toBe("finished after message");
  // The aborted turn left nothing behind: brief + message went out together once.
  const sent = m.seen.at(-1)!;
  expect(sent.filter((x) => x.role === "user").length).toBe(1);
});

test("a failing model call marks the agent errored until the user retries", async () => {
  const m = new FnModel((n) => {
    if (n === 0) throw new Error("400 bad request");
    return done("recovered");
  });
  const { swarm, run } = await start(m, []);
  await until(() => swarm.agents.get("a")!.status === "errored");
  expect(swarm.agents.get("a")!.error).toContain("400");
  swarm.retry("a");
  expect(await run).toBe("recovered");
});

test("resume picks up a crashed run from its database", async () => {
  // First process: one tool turn, then the model hangs forever ("crash").
  const m1 = new FnModel((n) => (n === 0 ? act(say("step one"), call("bash", { command: "first" })) : new Promise<Turn>(() => {})));
  const { swarm: s1, dir } = await start(m1, [{ right: "bash" }]);
  await until(() => m1.n === 2);
  expect(s1.agents.get("a")!.messages.length).toBe(2); // user brief + assistant tool call; the result was never sent

  // Second process on the same directory.
  const m2 = new FnModel((_, msgs) => {
    const all = JSON.stringify(msgs);
    expect(all).toContain("step one"); // history is back
    expect(all).toContain("ran: first"); // the pending result was kept... or repaired
    expect(all).toContain("The run was restarted");
    return done("resumed fine");
  });
  const { summary } = await resumeRun({ dir, model: m2, judges: scriptedJudges(), sandboxes: echo });
  expect(summary).toBe("resumed fine");
  s1.finish("user", "stop the first one");
});

test("connection errors are retried automatically", async () => {
  const { default: Anthropic } = await import("@anthropic-ai/sdk");
  const m = new FnModel((n) => {
    if (n === 0) throw new Anthropic.APIConnectionError({ message: "socket hang up" });
    return done("after retry");
  });
  const { swarm, run } = await start(m, []);
  expect(await run).toBe("after retry");
  expect(swarm.store.events().some((e) => e.type === "retry.model")).toBe(true);
}, 10_000);
