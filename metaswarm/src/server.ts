// The viewer: a small HTTP server for the user to watch the run and step in.
// Everything is read live from the swarm; changes go through the same checks
// as agent actions, with the user as the root of all rights.
import { Denied, USER } from "./perms.ts";
import type { Swarm } from "./swarm.ts";

const HTML = Bun.file(new URL("./viewer.html", import.meta.url));

export function serve(s: Swarm, runId: string, mode: string, port = 7777) {
  const json = (v: unknown, status = 200) => Response.json(v, { status });

  const state = () => ({
    run: { id: runId, done: s.done, summary: s.summary, mode },
    agents: [...s.agents.values()].map((a) => ({
      id: a.id, status: a.status, paused: a.paused, interrupted: a.interrupted, error: a.error, calls: a.calls, maxCalls: s.limits.maxCallsPerAgent,
      spawnedBy: a.spawnedBy, queued: a.userQueue.length, context: { ...a.context, limit: s.limits.contextLimit },
    })),
    grants: s.perms.all(),
    approvals: s.approvals.all(50),
    hooks: s.hooks.configs.map((h) => ({ event: h.event, matcher: h.matcher ?? null, command: h.command, source: h.source ?? "config" })),
    ...Object.assign({}, ...s.registry.providers.map((p) => p.describe?.() ?? {})),
    channels: s.chat.channels().map((c) => ({ ...c, members: s.chat.members(c.id), posts: s.chat.all(c.id).length })),
    pages: s.wiki.list(USER),
    proposals: s.wiki.proposals(),
  });

  const actions: Record<string, (b: any) => unknown | Promise<unknown>> = {
    post: (b) => s.chat.post(USER, String(b.channel), String(b.body), { thread: b.thread ?? null }),
    lock: (b) => s.wiki.setLock(USER, String(b.path), !!b.locked),
    grant: (b) => {
      if (!s.agents.has(String(b.agent))) throw new Denied(`no agent ${b.agent}`);
      const g = s.perms.grant(USER, String(b.agent), String(b.right), { delegable: !!b.delegable, ask: !!b.ask });
      s.notify(g.subject, `user granted you "${g.right}"${g.delegable ? " (delegable)" : ""}${g.ask ? " (each use needs the user's approval)" : ""} as grant #${g.id}.`);
    },
    approve: (b) => s.approvals.decide(Number(b.id), !!b.approve, b.note ? String(b.note) : undefined),
    message: (b) => s.message(String(b.agent), String(b.body ?? "").trim() || (() => { throw new Denied("empty message"); })()),
    interrupt: (b) => s.interrupt(String(b.agent)),
    retry: (b) => s.retry(String(b.agent)),
    revoke: async (b) => {
      const ids = s.perms.revoke(USER, Number(b.grant));
      for (const a of s.perms.subjectsOf(ids)) {
        s.notify(a, `user revoked grant(s) ${ids.map((i) => "#" + i).join(", ")}. Check whoami.`);
        if (!s.perms.holds(a, "bash")) await s.dropSandbox(a);
      }
    },
    pause: (b) => s.setPaused(String(b.agent), !!b.paused),
    finish: () => s.finish(USER, "Ended by the user from the viewer."),
  };

  const server = Bun.serve({
    port,
    hostname: "127.0.0.1",
    idleTimeout: 0, // SSE connections stay open
    async fetch(req) {
      const url = new URL(req.url);
      const p = url.pathname;
      try {
        if (p === "/") return new Response(HTML, { headers: { "content-type": "text/html; charset=utf-8" } });
        if (p === "/api/state") return json(state());
        let m = p.match(/^\/api\/agents\/([^/]+)\/transcript$/);
        if (m) {
          const a = s.agents.get(decodeURIComponent(m[1]!));
          return a ? json(a.messages) : json({ error: "no such agent" }, 404);
        }
        m = p.match(/^\/api\/channels\/([^/]+)\/posts$/);
        if (m) return json(s.chat.all(decodeURIComponent(m[1]!)));
        if (p === "/api/wiki/page") {
          const path = url.searchParams.get("path") ?? "";
          return json({ path, content: (await s.wiki.read(USER, path)) ?? "", locked: s.wiki.isLocked(path), history: await s.wiki.history(path) });
        }
        if (p === "/api/wiki/diff") {
          return new Response(await s.wiki.show(url.searchParams.get("commit") ?? ""), { headers: { "content-type": "text/plain; charset=utf-8" } });
        }
        if (p === "/api/events") return sse(s, url.searchParams.get("after"));
        m = p.match(/^\/api\/user\/(\w+)$/);
        if (m && req.method === "POST" && actions[m[1]!]) {
          await actions[m[1]!]!(await req.json().catch(() => ({})));
          return json({ ok: true });
        }
        return json({ error: "not found" }, 404);
      } catch (e) {
        return json({ error: e instanceof Error ? e.message : String(e) }, e instanceof Denied ? 403 : 500);
      }
    },
  });
  return server;
}

function sse(s: Swarm, after: string | null): Response {
  let off = () => {};
  const stream = new ReadableStream({
    start(ctl) {
      const enc = new TextEncoder();
      const send = (e: { id: number }) => ctl.enqueue(enc.encode(`id: ${e.id}\ndata: ${JSON.stringify(e)}\n\n`));
      for (const e of s.store.events(Number(after ?? 0), 500)) send(e);
      off = s.store.on((e) => {
        try {
          send(e);
        } catch {
          off();
        }
      });
    },
    cancel() {
      off();
    },
  });
  return new Response(stream, { headers: { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" } });
}
