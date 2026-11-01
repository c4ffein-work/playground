// MCP servers as a tool provider.
//
// stdio servers are code: they run inside a locked-down container (no network
// unless asked, read-only root, no capabilities), and we speak MCP over the
// container's stdio. HTTP servers are remote: the harness connects to them.
// Each server tool is exposed as `mcp__<server>__<tool>` and needs the right
// `mcp:<server>:<tool>`, so `mcp:github:*` or `mcp:*` work as grants.
import { resolve } from "node:path";
import type Anthropic from "@anthropic-ai/sdk";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { getDefaultEnvironment, StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import type { Transport } from "@modelcontextprotocol/sdk/shared/transport.js";
import { z } from "zod";
import { Denied } from "./perms.ts";
import type { ToolProvider } from "./tools.ts";

export const McpServerConfigSchema = z.union([
  z.object({
    command: z.array(z.string()).min(1),
    image: z.string().min(1),
    network: z.enum(["none", "bridge"]).optional(),
    env: z.record(z.string(), z.string()).optional(),
    mounts: z.array(z.object({ host: z.string(), container: z.string(), readonly: z.boolean().optional() })).optional(),
  }),
  z.object({ url: z.string().url(), headers: z.record(z.string(), z.string()).optional() }),
]);
export type McpServerConfig = z.infer<typeof McpServerConfigSchema>;
export type McpStdioConfig = Extract<McpServerConfig, { command: string[] }>;

export interface McpOptions {
  runId: string;
  /** Where failures go (default: console.error). */
  log?: (msg: string) => void;
  /** Connect + list timeout per server (default 60s; the first run may pull the image). */
  startTimeoutMs?: number;
  /** Per-call timeout (default 120s). */
  callTimeoutMs?: number;
}

const MAX_OUTPUT = 30_000;
const SERVER_RE = /^[A-Za-z0-9_-]{1,40}$/;
const safe = (s: string) => s.replace(/[^A-Za-z0-9_-]/g, "_");

/** Container name for a stdio server. */
export const mcpContainerName = (runId: string, server: string) => safe(`metaswarm-${runId}-mcp-${server}`).replace(/^_+/, "");

/** `docker run` argv for a stdio server (without the leading "docker"). Env values are passed through the process env, not argv. */
export function dockerArgs(runId: string, server: string, c: McpStdioConfig): string[] {
  const args = [
    "run", "-i", "--rm", "--name", mcpContainerName(runId, server),
    "--network", c.network ?? "none",
    "--read-only", "--tmpfs", "/tmp",
    "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "--memory", "512m", "--pids-limit", "128",
    "--label", `metaswarm.run=${runId}`, "--label", `metaswarm.mcp=${server}`,
  ];
  for (const k of Object.keys(c.env ?? {})) args.push("-e", k);
  for (const m of c.mounts ?? []) {
    if (m.host.includes(",") || m.container.includes(",")) throw new Error(`mount paths may not contain commas: ${m.host}`);
    args.push("--mount", `type=bind,src=${resolve(m.host)},dst=${m.container}${m.readonly ? ",readonly" : ""}`);
  }
  args.push(c.image, ...c.command);
  return args;
}

/** The docker CLI's own settings, so `docker` finds the daemon (the server's env is added on top). */
function dockerEnv(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) if (v !== undefined && (k.startsWith("DOCKER_") || k === "XDG_RUNTIME_DIR")) out[k] = v;
  return out;
}

async function dockerRm(name: string) {
  const p = Bun.spawn(["docker", "rm", "-f", name], { stdout: "ignore", stderr: "ignore" });
  await p.exited;
}

interface Server {
  name: string;
  client: Client;
  container?: string;
}

interface Entry {
  server: Server;
  tool: string; // the server's own name for it
  right: string;
}

/** Turn an MCP tool result into text for the model. Throws when the server flagged an error. */
export function formatResult(res: any): string {
  const parts: string[] = [];
  for (const c of (res?.content ?? []) as any[]) {
    if (c.type === "text") parts.push(c.text);
    else if (c.type === "resource" && typeof c.resource?.text === "string") parts.push(`[resource ${c.resource.uri}]\n${c.resource.text}`);
    else if (c.type === "resource") parts.push(`[resource ${c.resource?.uri ?? "?"} (${c.resource?.mimeType ?? "binary"}) not shown]`);
    else if (c.type === "resource_link") parts.push(`[resource link: ${c.uri}${c.name ? ` "${c.name}"` : ""}]`);
    else parts.push(`[${c.type} content${c.mimeType ? ` (${c.mimeType})` : ""} not shown]`);
  }
  if (!parts.length && res?.structuredContent !== undefined) parts.push(JSON.stringify(res.structuredContent));
  if (!parts.length && "toolResult" in (res ?? {})) parts.push(JSON.stringify(res.toolResult));
  let text = parts.join("\n");
  if (text.length > MAX_OUTPUT) text = text.slice(0, MAX_OUTPUT) + `\n…[${text.length - MAX_OUTPUT} chars cut]`;
  if (res?.isError) throw new Error(text || "MCP tool returned an error");
  return text || "(no output)";
}

export function mcpProvider(servers: Record<string, McpServerConfig>, opts: McpOptions): ToolProvider {
  const log = opts.log ?? ((m: string) => console.error(m));
  const live: Server[] = [];
  const containers = new Set<string>();
  const entries = new Map<string, Entry>();
  const defs: Anthropic.Beta.BetaTool[] = [];
  const failed: string[] = [];
  const errors = new Map<string, string>();

  async function connect(name: string, cfg: McpServerConfig): Promise<void> {
    if (!SERVER_RE.test(name)) throw new Error(`bad server name (use letters, digits, _ and -)`);
    const parsed = McpServerConfigSchema.parse(cfg);
    const client = new Client({ name: "metaswarm", version: "0.1.0" });
    let transport: Transport;
    let container: string | undefined;
    let stderr = "";
    if ("command" in parsed) {
      container = mcpContainerName(opts.runId, name);
      containers.add(container);
      await dockerRm(container); // leftover from a crash with the same run id
      const t = new StdioClientTransport({
        command: "docker",
        args: dockerArgs(opts.runId, name, parsed),
        env: { ...getDefaultEnvironment(), ...dockerEnv(), ...(parsed.env ?? {}) },
        stderr: "pipe",
      });
      // Keep the tail of stderr for error messages (and keep the pipe drained).
      t.stderr?.on("data", (d: Buffer) => (stderr = (stderr + d.toString()).slice(-2000)));
      transport = t;
    } else {
      transport = new StreamableHTTPClientTransport(new URL(parsed.url), { requestInit: { headers: parsed.headers ?? {} } });
    }
    const timeout = opts.startTimeoutMs ?? 60_000;
    try {
      await client.connect(transport, { timeout });
      const tools: any[] = [];
      let cursor: string | undefined;
      do {
        const page = await client.listTools(cursor ? { cursor } : undefined, { timeout });
        tools.push(...page.tools);
        cursor = page.nextCursor;
      } while (cursor);
      const server: Server = { name, client, container };
      live.push(server);
      for (const t of tools) {
        const exposed = `mcp__${name}__${safe(t.name)}`.slice(0, 128);
        if (entries.has(exposed)) {
          log(`mcp: ${name}: tool ${t.name} clashes with another tool after renaming, skipped`);
          continue;
        }
        entries.set(exposed, { server, tool: t.name, right: `mcp:${name}:${safe(t.name)}` });
        const schema = { ...(t.inputSchema ?? {}), type: "object" } as Anthropic.Beta.BetaTool.InputSchema;
        defs.push({ name: exposed, description: `[MCP ${name}] ${t.description ?? t.title ?? t.name}`, input_schema: schema });
      }
    } catch (e) {
      await client.close().catch(() => {});
      if (container) await dockerRm(container);
      const msg = e instanceof Error ? e.message : String(e);
      throw new Error(stderr.trim() ? `${msg}\n${stderr.trim()}` : msg);
    }
  }

  return {
    id: "mcp",

    async init() {
      const names = Object.keys(servers);
      const results = await Promise.allSettled(names.map((n) => connect(n, servers[n]!)));
      results.forEach((r, i) => {
        if (r.status === "rejected") {
          failed.push(names[i]!);
          errors.set(names[i]!, r.reason instanceof Error ? r.reason.message : String(r.reason));
          log(`mcp: server ${names[i]} failed to start, skipped: ${r.reason instanceof Error ? r.reason.message : r.reason}`);
        }
      });
      // Stable order regardless of which server answered first.
      defs.sort((a, b) => a.name.localeCompare(b.name));
    },

    tools: () => defs,

    describe() {
      return {
        mcp: [
          ...live.map((sv) => ({ server: sv.name, ok: true, tools: [...entries].filter(([, e]) => e.server === sv).map(([n]) => n) })),
          ...failed.map((n) => ({ server: n, ok: false, tools: [], error: errors.get(n) })),
        ],
      };
    },

    prompt() {
      if (!live.length && !failed.length) return "";
      const lines = live.map((s) => `- ${s.name}: tools mcp__${s.name}__*, right mcp:${s.name}:<tool>`);
      if (failed.length) lines.push(`(unavailable this run: ${failed.join(", ")})`);
      return `MCP servers. Tools named mcp__<server>__<tool> come from MCP servers; each call needs the right "mcp:<server>:<tool>".\n${lines.join("\n")}`;
    },

    async run(ctx, name, input) {
      const e = entries.get(name);
      if (!e) throw new Denied(`unknown tool ${name}`);
      ctx.world.perms.check(ctx.agent, e.right);
      if (ctx.signal.aborted) throw new Error("interrupted");
      const res = await e.server.client.callTool(
        { name: e.tool, arguments: (input ?? {}) as Record<string, unknown> },
        undefined,
        { signal: ctx.signal, timeout: opts.callTimeoutMs ?? 120_000 },
      );
      return formatResult(res);
    },

    async shutdown() {
      await Promise.all(live.map((s) => s.client.close().catch(() => {})));
      live.length = 0;
      await Promise.all([...containers].map(dockerRm));
    },
  };
}
