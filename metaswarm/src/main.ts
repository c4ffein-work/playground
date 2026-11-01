// metaswarm CLI.
//
//   bun run src/main.ts [demo] [--claude | --scripted] [--serve] [--port N] [--goal "..."] [--config file]
//   bun run src/main.ts --resume runs/<id> [--serve]
//
// demo: build (default) | redteam | open. "open" takes --goal and only makes
// sense with --claude: one lead agent with broad rights works on your goal.
// Config (default ./metaswarm.config.json, optional): see README.
import { existsSync } from "node:fs";
import { basename, join } from "node:path";
import { DEMOS, type Demo } from "./demos.ts";
import type { HookConfig } from "./hooks.ts";
import { mcpProvider, type McpServerConfig } from "./mcp.ts";
import { ClaudeModel, ScriptedModel, type Model } from "./model.ts";
import { Egress } from "./egress.ts";
import { loadPlugins } from "./plugins.ts";
import { standardProviders } from "./providers.ts";
import { resumeRun, runDemo, type RunOptions } from "./run.ts";
import { DockerSandboxes } from "./sandbox.ts";
import { serve } from "./server.ts";
import type { Swarm } from "./swarm.ts";
import type { ToolProvider } from "./tools.ts";
import { ClaudeJudge, scriptedJudges } from "./vote.ts";

export interface Config {
  hooks?: HookConfig[];
  /** Folders of SKILL.md skills. */
  skills?: string[];
  /** Plugin folders (skills/, mcp.json, hooks.json). */
  plugins?: string[];
  mcpServers?: Record<string, McpServerConfig>;
  /** Web access, per agent, through net:<host> rights. */
  network?: {
    /** Give containers an egress proxy (bash can reach allowed hosts). Default false: no network at all. */
    containers?: boolean;
    /** Extra CA bundle the proxy trusts (e.g. a corporate TLS proxy's). */
    caFile?: string;
  };
  limits?: RunOptions["limits"];
  image?: string;
}

const argv = process.argv.slice(2);
const opt = (name: string) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const flag = (name: string) => argv.includes(name);
const valued = ["--port", "--goal", "--config", "--resume"];
const positional = argv.filter((a, i) => !a.startsWith("--") && !valued.includes(argv[i - 1] ?? ""));

const configPath = opt("--config") ?? "metaswarm.config.json";
const config: Config = existsSync(configPath) ? await Bun.file(configPath).json() : {};
const useClaude = flag("--claude") || (!flag("--scripted") && !!process.env.ANTHROPIC_API_KEY);
const mode = useClaude ? "claude" : "scripted";
const port = Number(opt("--port") ?? 7777);

const resumeDir = opt("--resume");
let demo: Demo | undefined;
if (!resumeDir) {
  const demoName = positional[0] ?? "build";
  demo = DEMOS[demoName];
  if (demoName === "open") {
    const goal = opt("--goal") ?? "Build something small and useful, document it in the wiki, then finish.";
    const r = (right: string, delegable = true) => ({ right, delegable });
    demo = {
      ...DEMOS.build!,
      name: "open",
      description: "Your goal, one lead with broad rights.",
      agents: [{ id: "lead", brief: `Goal from the user:\n\n${goal}`, rights: [r("spawn"), r("channel:create"), r("channel:join"), r("wiki:read"), r("wiki:propose"), r("bash"), r("fs:read:*"), r("fs:write:*"), r("run:finish", false)] }],
      script: {},
      verify: undefined,
    };
  }
  if (!demo) {
    console.error(`unknown demo "${demoName}". Try: ${[...Object.keys(DEMOS), "open"].join(", ")}`);
    process.exit(2);
  }
} else if (!useClaude) {
  console.error("--resume needs a real model (--claude or ANTHROPIC_API_KEY): scripted demos can't pick up mid-script.");
  process.exit(2);
}

const model: Model = useClaude ? new ClaudeModel(process.env.METASWARM_MODEL ?? "claude-opus-5-5") : new ScriptedModel(demo!.script);
const judgeModels = (process.env.METASWARM_JUDGES ?? "claude-fable-5-1,claude-opus-5-5,claude-sonnet-5-5").split(",");
const judges = useClaude ? judgeModels.map((m) => new ClaudeJudge(m.trim())) : scriptedJudges();

const dir = resumeDir ?? join("runs", new Date().toISOString().replace(/[:.]/g, "-"));
const runId = basename(dir);
let swarm: Swarm | undefined;
const netLog = (agent: string, host: string, allowed: boolean) => swarm?.store.log("harness", "net", { agent, host, allowed });
const egress = config.network?.containers
  ? new Egress({ runId, can: (a, r) => swarm?.perms.can(a, r) ?? false, onDecision: netLog, caFile: config.network.caFile })
  : undefined;
const sandboxes = new DockerSandboxes({ image: process.env.METASWARM_IMAGE ?? config.image ?? "alpine:3.20", runId, egress });
const plugins = await loadPlugins(config.plugins ?? []);
const mcpServers = { ...plugins.mcpServers };
for (const [name, cfg] of Object.entries(config.mcpServers ?? {})) {
  if (mcpServers[name]) console.error(`mcp: config server "${name}" overrides the plugin server with the same name`);
  mcpServers[name] = cfg;
}
const providers: ToolProvider[] = standardProviders({ skillDirs: [...(config.skills ?? []), ...plugins.skillDirs], onNet: netLog, bashProxy: !!egress });
if (Object.keys(mcpServers).length) providers.push(mcpProvider(mcpServers, { runId }));
if (plugins.names.length) console.log(`plugins: ${plugins.names.join(", ")}`);

process.on("SIGINT", async () => {
  console.log(`\ninterrupted, removing containers (keeping /workspace)… resume later with: bun run src/main.ts --resume ${dir}`);
  await swarm?.registry.shutdown();
  await sandboxes.shutdown({ keepVolume: true });
  process.exit(130);
});

const options: RunOptions = {
  dir, model, judges, sandboxes, providers,
  hooks: [...(config.hooks ?? []), ...plugins.hooks],
  limits: config.limits,
  onReady: (s) => {
    swarm = s;
    if (flag("--serve")) {
      serve(s, runId, mode, port);
      console.log(`viewer: http://127.0.0.1:${port}\n`);
    }
  },
};

let failures: string[] = [];
if (resumeDir) {
  console.log(`metaswarm · resuming ${dir} · ${mode}\n`);
  const r = await resumeRun(options);
  console.log(`\nsummary: ${r.summary}`);
} else {
  console.log(`metaswarm · demo ${demo!.name} · ${mode} · run ${runId}\n${demo!.description}\nvault: ${dir}/vault (open it in Obsidian)\n`);
  const r = await runDemo(demo!, options);
  failures = r.failures;
  console.log(`\nsummary: ${r.summary}`);
  if (demo!.verify) console.log(failures.length ? `\n✗ verify failed:\n${failures.map((f) => "  - " + f).join("\n")}` : "\n✓ verify passed");
}
if (flag("--serve")) console.log(`run is over; viewer stays up at http://127.0.0.1:${port} (Ctrl-C to quit)`);
else process.exit(failures.length ? 1 : 0);
