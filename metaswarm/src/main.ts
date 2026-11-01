// metaswarm CLI.
//
//   bun run src/main.ts [demo] [--claude | --scripted] [--serve] [--port N] [--goal "..."]
//
// demo: build (default) | redteam | open. "open" takes --goal and only makes
// sense with --claude: one lead agent with broad rights works on your goal.
import { join } from "node:path";
import { DEMOS, type Demo } from "./demos.ts";
import { ClaudeModel, ScriptedModel, type Model } from "./model.ts";
import { DockerSandboxes } from "./sandbox.ts";
import { runDemo } from "./run.ts";
import { serve } from "./server.ts";
import { ClaudeJudge, scriptedJudges } from "./vote.ts";

const argv = process.argv.slice(2);
const opt = (name: string) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const flag = (name: string) => argv.includes(name);
const positional = argv.filter((a, i) => !a.startsWith("--") && !["--port", "--goal"].includes(argv[i - 1] ?? ""));

const demoName = positional[0] ?? "build";
const useClaude = flag("--claude") || (!flag("--scripted") && !!process.env.ANTHROPIC_API_KEY);

let demo: Demo | undefined = DEMOS[demoName];
if (demoName === "open") {
  const goal = opt("--goal") ?? "Build something small and useful, document it in the wiki, then finish.";
  const r = (right: string, delegable = true) => ({ right, delegable });
  demo = {
    ...DEMOS.build!,
    name: "open",
    description: "Your goal, one lead with broad rights.",
    agents: [{ id: "lead", brief: `Goal from the user:\n\n${goal}`, rights: [r("spawn"), r("channel:create"), r("channel:join"), r("wiki:read"), r("wiki:propose"), r("bash"), r("run:finish", false)] }],
    script: {},
    verify: undefined,
  };
}
if (!demo) {
  console.error(`unknown demo "${demoName}". Try: ${[...Object.keys(DEMOS), "open"].join(", ")}`);
  process.exit(2);
}

const model: Model = useClaude ? new ClaudeModel(process.env.METASWARM_MODEL ?? "claude-opus-5-5") : new ScriptedModel(demo.script);
const judgeModels = (process.env.METASWARM_JUDGES ?? "claude-fable-5-1,claude-opus-5-5,claude-sonnet-5-5").split(",");
const judges = useClaude ? judgeModels.map((m) => new ClaudeJudge(m.trim())) : scriptedJudges();

const runId = new Date().toISOString().replace(/[:.]/g, "-");
const dir = join("runs", runId);
const sandboxes = new DockerSandboxes({ image: process.env.METASWARM_IMAGE ?? "alpine:3.20", runId });
const mode = useClaude ? "claude" : "scripted";

console.log(`metaswarm · demo ${demo.name} · ${mode} · run ${runId}\n${demo.description}\nvault: ${dir}/vault (open it in Obsidian)\n`);
process.on("SIGINT", async () => {
  console.log("\ninterrupted, removing containers…");
  await sandboxes.shutdown();
  process.exit(130);
});

const port = Number(opt("--port") ?? 7777);
const { summary, failures } = await runDemo(demo, {
  dir, model, judges, sandboxes,
  onReady: flag("--serve")
    ? (s) => {
        serve(s, runId, mode, port);
        console.log(`viewer: http://127.0.0.1:${port}\n`);
      }
    : undefined,
});

console.log(`\nsummary: ${summary}`);
if (demo.verify) {
  console.log(failures.length ? `\n✗ verify failed:\n${failures.map((f) => "  - " + f).join("\n")}` : "\n✓ verify passed");
}
if (flag("--serve")) {
  console.log(`run is over; viewer stays up at http://127.0.0.1:${port} (Ctrl-C to quit)`);
} else {
  process.exit(failures.length ? 1 : 0);
}

