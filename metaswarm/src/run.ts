// Wires a demo (or a saved run) into a running swarm. Shared by the CLI and the tests.
import { existsSync, mkdirSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { Approvals } from "./approvals.ts";
import { Chat } from "./chat.ts";
import type { Demo } from "./demos.ts";
import { Hooks, type HookConfig } from "./hooks.ts";
import type { Model } from "./model.ts";
import { Perms, USER } from "./perms.ts";
import type { SandboxFactory } from "./sandbox.ts";
import { Store } from "./store.ts";
import { DEFAULT_LIMITS, Swarm, type Limits } from "./swarm.ts";
import { coreTools, ToolRegistry, type ToolProvider } from "./tools.ts";
import type { Judge } from "./vote.ts";
import { Wiki } from "./wiki.ts";

export interface RunOptions {
  dir: string; // run directory: db, vault, saved outputs
  model: Model;
  judges: Judge[];
  sandboxes: SandboxFactory;
  limits?: Partial<Limits>;
  /** Extra tool providers, after the core tools. */
  providers?: ToolProvider[];
  hooks?: HookConfig[];
  approvalTimeoutMs?: number;
  /** Called once everything is set up, before agents start (e.g. to start the viewer). */
  onReady?: (s: Swarm) => void;
}

async function build(o: RunOptions, seed: Record<string, string>, locked: string[]): Promise<Swarm> {
  mkdirSync(o.dir, { recursive: true });
  const dir = resolve(o.dir);
  const store = new Store(join(dir, "run.sqlite"));
  const perms = new Perms(store);
  const chat = new Chat(store, perms);
  const wiki = new Wiki(store, perms, o.judges, join(dir, "vault"));
  await wiki.init(seed, locked);
  const hooks = new Hooks(o.hooks ?? [], dir, (r, event) =>
    store.log("harness", "hook", { event, command: r.hook.command, source: r.hook.source ?? null, code: r.code, timedOut: r.timedOut, stderr: r.stderr.slice(0, 2000) }),
  );
  const swarm = new Swarm({
    store, perms, chat, wiki, hooks,
    approvals: new Approvals(store, o.approvalTimeoutMs),
    model: o.model,
    sandboxes: o.sandboxes,
    registry: new ToolRegistry([coreTools, ...(o.providers ?? [])]),
    limits: { ...DEFAULT_LIMITS, ...o.limits },
    runId: basename(dir),
    dir,
  });
  await swarm.registry.init(swarm);
  return swarm;
}

export async function setup(demo: Demo, o: RunOptions): Promise<{ swarm: Swarm; start: () => Promise<string> }> {
  const swarm = await build(o, demo.seed, demo.locked);
  const ids: Record<string, string> = {};
  for (const c of demo.channels) ids[c.name] = swarm.chat.create(USER, c.topic, c.visibility).id;
  const agents = demo.agents.map((a) => ({ ...a, brief: a.brief.replace(/\{\{(\w+)\}\}/g, (_, n) => ids[n] ?? `{{${n}}}`) }));

  return {
    swarm,
    start: async () => {
      o.onReady?.(swarm);
      const run = swarm.run(agents);
      for (const c of demo.channels) {
        for (const m of c.members) {
          swarm.chat.invite(USER, ids[c.name]!, m);
          swarm.notify(m, `user added you to channel ${ids[c.name]} "${c.topic}".`);
        }
      }
      return run;
    },
  };
}

export async function runDemo(demo: Demo, o: RunOptions) {
  const { swarm, start } = await setup(demo, o);
  const summary = await start();
  const failures = demo.verify ? await demo.verify(swarm) : [];
  return { swarm, summary, failures };
}

/** Continue a run from its directory, after a crash or Ctrl-C. */
export async function resumeRun(o: RunOptions): Promise<{ swarm: Swarm; summary: string }> {
  if (!existsSync(join(o.dir, "run.sqlite"))) throw new Error(`no run found in ${o.dir}`);
  const swarm = await build(o, {}, []);
  o.onReady?.(swarm);
  const summary = await swarm.resume();
  return { swarm, summary };
}
