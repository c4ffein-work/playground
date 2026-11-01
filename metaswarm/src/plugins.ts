// Plugins: a folder bundling skills, MCP servers and hooks.
//
//   <plugin>/plugin.json   optional { "name", "description" } (name defaults to the folder name)
//   <plugin>/skills/       optional, each subfolder a skill (see skills.ts)
//   <plugin>/mcp.json      optional { "mcpServers": { "<name>": McpServerConfig } }
//   <plugin>/hooks.json    optional { "hooks": HookConfig[] }
//
// Relative paths (hook commands, MCP mount sources) resolve against the plugin
// folder, and `${PLUGIN_DIR}` in hook argv / mount sources expands to it.
// A folder passed to loadPlugins is either a plugin itself or a folder of plugins.
// Plugins are installed by the user, so they are configuration: their hooks run
// on the host like other hooks, and their MCP servers in containers like other
// MCP servers.
import { existsSync, readdirSync, statSync } from "node:fs";
import { basename, isAbsolute, join, resolve } from "node:path";
import { z } from "zod";
import type { HookConfig } from "./hooks.ts";
import { McpServerConfigSchema, type McpServerConfig } from "./mcp.ts";

export interface LoadedPlugins {
  skillDirs: string[];
  mcpServers: Record<string, McpServerConfig>;
  hooks: HookConfig[];
  names: string[];
}

const PluginJson = z.object({ name: z.string().regex(/^[A-Za-z0-9_-]{1,40}$/).optional(), description: z.string().optional() });
const McpJson = z.object({ mcpServers: z.record(z.string(), McpServerConfigSchema) });
const HooksJson = z.object({
  hooks: z.array(
    z.object({
      event: z.enum(["PreToolUse", "PostToolUse", "AgentIdle", "Notice", "RunEnd"]),
      matcher: z.string().optional(),
      command: z.array(z.string()).min(1),
      timeoutS: z.number().positive().optional(),
    }),
  ),
});

const isPlugin = (d: string) => ["plugin.json", "skills", "mcp.json", "hooks.json"].some((f) => existsSync(join(d, f)));

async function readJson<T>(file: string, schema: z.ZodType<T>): Promise<T | undefined> {
  if (!existsSync(file)) return undefined;
  const parsed = schema.safeParse(await Bun.file(file).json());
  if (!parsed.success) throw new Error(`${file}: ${parsed.error.message}`);
  return parsed.data;
}

export async function loadPlugins(dirs: string[]): Promise<LoadedPlugins> {
  const out: LoadedPlugins = { skillDirs: [], mcpServers: {}, hooks: [], names: [] };
  const folders: string[] = [];
  for (const d of dirs.map((x) => resolve(x))) {
    if (!existsSync(d)) throw new Error(`no plugin folder ${d}`);
    if (isPlugin(d)) folders.push(d);
    else for (const n of readdirSync(d).sort()) if (!n.startsWith(".") && statSync(join(d, n)).isDirectory() && isPlugin(join(d, n))) folders.push(join(d, n));
  }

  for (const dir of folders) {
    const meta = (await readJson(join(dir, "plugin.json"), PluginJson)) ?? {};
    const name = meta.name ?? basename(dir).replace(/[^A-Za-z0-9_-]/g, "_");
    if (out.names.includes(name)) throw new Error(`two plugins named ${name}`);
    out.names.push(name);

    const expand = (s: string) => s.replaceAll("${PLUGIN_DIR}", dir);
    const local = (p: string) => (isAbsolute(p) ? p : resolve(dir, p));

    if (existsSync(join(dir, "skills"))) out.skillDirs.push(join(dir, "skills"));

    const mcp = await readJson(join(dir, "mcp.json"), McpJson);
    for (const [server, cfg] of Object.entries(mcp?.mcpServers ?? {})) {
      const key = server in out.mcpServers ? `${name}-${server}` : server;
      if (key in out.mcpServers) throw new Error(`plugin ${name}: MCP server ${server} clashes twice`);
      out.mcpServers[key] = "command" in cfg && cfg.mounts ? { ...cfg, mounts: cfg.mounts.map((m) => ({ ...m, host: local(expand(m.host)) })) } : cfg;
    }

    const hooks = await readJson(join(dir, "hooks.json"), HooksJson);
    for (const h of hooks?.hooks ?? []) {
      const [cmd, ...args] = h.command.map(expand) as [string, ...string[]];
      // A bare program name ("node") goes through PATH; anything path-like is plugin-relative.
      const resolved = [cmd.includes("/") ? local(cmd) : cmd, ...args.map((a) => (a.startsWith("./") || a.startsWith("../") ? local(a) : a))];
      out.hooks.push({ ...h, command: resolved, source: name });
    }
  }
  return out;
}
