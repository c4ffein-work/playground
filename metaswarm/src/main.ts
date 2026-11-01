// metaswarm PoC entry point.
//   bun run src/main.ts [--scripted | --claude] [goal...]
import { mkdirSync } from "node:fs";
import { Bus } from "./bus.ts";
import { ClaudeModel, ScriptedModel, type Model } from "./model.ts";
import { DockerSandboxes } from "./sandbox.ts";
import { Swarm } from "./swarm.ts";

const args = process.argv.slice(2);
const flag = (f: string) => args.includes(f);
const goal =
  args.filter((a) => !a.startsWith("--")).join(" ") ||
  "Write a short notes file about metaswarm, check the sandbox's limits, and report back.";

// Default to the real API when a key is set, otherwise run the offline demo.
const useClaude = flag("--claude") || (!flag("--scripted") && !!process.env.ANTHROPIC_API_KEY);
const model: Model = useClaude ? new ClaudeModel(process.env.METASWARM_MODEL ?? "claude-opus-5-5") : new ScriptedModel();

const runId = new Date().toISOString().replace(/[:.]/g, "-");
mkdirSync("runs", { recursive: true });
const dbPath = `runs/${runId}.sqlite`;
const bus = new Bus(dbPath);

const sandboxes = new DockerSandboxes({ image: process.env.METASWARM_IMAGE ?? "alpine:3.20", runId });
const swarm = new Swarm(bus, model, sandboxes, { maxAgents: 6, maxCallsPerAgent: 25, idleNudges: 2 });

console.log(`metaswarm run ${runId} (${useClaude ? "claude" : "scripted demo"})\ngoal: ${goal}\n`);
const stop = async () => {
  console.log("\ninterrupted, cleaning up containers…");
  await sandboxes.shutdown();
  process.exit(130);
};
process.on("SIGINT", stop);

const summary = await swarm.run(goal);
console.log(`\nsummary: ${summary}\nlog: ${dbPath}`);
bus.close();
