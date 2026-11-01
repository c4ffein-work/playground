// Wires a demo into a running swarm. Shared by the CLI and the tests.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { Chat } from "./chat.ts";
import type { Demo } from "./demos.ts";
import type { Model } from "./model.ts";
import { Perms, USER } from "./perms.ts";
import type { SandboxFactory } from "./sandbox.ts";
import { Store } from "./store.ts";
import { Swarm, type Limits } from "./swarm.ts";
import { coreTools, ToolRegistry, type ToolProvider } from "./tools.ts";
import type { Judge } from "./vote.ts";
import { Wiki } from "./wiki.ts";

export interface RunOptions {
  dir: string; // run directory: db + vault
  model: Model;
  judges: Judge[];
  sandboxes: SandboxFactory;
  limits?: Partial<Limits>;
  /** Extra tool providers, after the core tools. */
  providers?: ToolProvider[];
  /** Called once everything is set up, before agents start (e.g. to start the viewer). */
  onReady?: (s: Swarm) => void;
}

export async function setup(demo: Demo, o: RunOptions): Promise<{ swarm: Swarm; start: () => Promise<string> }> {
  mkdirSync(o.dir, { recursive: true });
  const store = new Store(join(o.dir, "run.sqlite"));
  const perms = new Perms(store);
  const chat = new Chat(store, perms);
  const wiki = new Wiki(store, perms, o.judges, join(o.dir, "vault"));
  await wiki.init(demo.seed, demo.locked);

  const limits: Limits = { maxAgents: 8, maxCallsPerAgent: 30, idleNudges: 2, ...o.limits };
  const registry = new ToolRegistry([coreTools, ...(o.providers ?? [])]);
  const runId = o.dir.split("/").filter(Boolean).pop()!;
  const swarm = new Swarm(store, perms, chat, wiki, o.model, o.sandboxes, limits, registry, runId, o.dir);
  await registry.init(swarm);

  const ids: Record<string, string> = {};
  for (const c of demo.channels) ids[c.name] = chat.create(USER, c.topic, c.visibility).id;
  const agents = demo.agents.map((a) => ({ ...a, brief: a.brief.replace(/\{\{(\w+)\}\}/g, (_, n) => ids[n] ?? `{{${n}}}`) }));

  return {
    swarm,
    start: async () => {
      o.onReady?.(swarm);
      const run = swarm.run(agents);
      for (const c of demo.channels) {
        for (const m of c.members) {
          chat.invite(USER, ids[c.name]!, m);
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
