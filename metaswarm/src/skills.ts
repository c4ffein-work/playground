// Skills: folders with a SKILL.md (agentskills.io / Claude Code format). The
// YAML frontmatter has `name` and `description`; the body is instructions, and
// may point at other files in the folder.
//
// Two sources:
// - builtin: host folders given at startup. Skill files are data, read on the host.
// - vault:   wiki pages `skills/<name>/SKILL.md`. Agents write new skills as wiki
//            pages, judges vote on them, and from then on every agent can load
//            them. Read through the wiki with the caller's identity, so
//            `wiki:read` applies. Re-scanned on every call.
// Loading skill <name> needs the right `skill:<name>`.
import { existsSync, readdirSync, realpathSync, statSync } from "node:fs";
import { basename, join, relative, resolve, sep } from "node:path";
import { z } from "zod";
import { Denied } from "./perms.ts";
import { parseInput, toolDefs, type ToolContext, type ToolProvider } from "./tools.ts";

export interface SkillMeta {
  name: string;
  description: string;
  source: "builtin" | "vault";
}

const NAME_RE = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$/;
const MAX_OUTPUT = 50_000;

/**
 * Parse simple YAML frontmatter: `key: value` lines, optional quotes, indented
 * continuation lines (and `>` / `|` block markers) folded into the value.
 */
export function parseFrontmatter(text: string): { data: Record<string, string>; body: string } {
  const m = text.match(/^﻿?---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/);
  if (!m) return { data: {}, body: text };
  const data: Record<string, string> = {};
  let key: string | null = null;
  let block: "" | ">" | "|" = "";
  for (const line of m[1]!.split(/\r?\n/)) {
    const kv = line.match(/^([A-Za-z0-9_-]+):\s*(.*)$/);
    if (kv) {
      key = kv[1]!;
      let v = kv[2]!.trim();
      block = /^[>|][+-]?$/.test(v) ? (v[0] as ">" | "|") : "";
      if (block) v = "";
      else if (/^(["']).*\1$/.test(v)) v = v.slice(1, -1);
      data[key] = v;
    } else if (key && /^\s+\S/.test(line)) {
      const t = line.trim();
      data[key] = data[key] ? data[key] + (block === "|" ? "\n" : " ") + t : t;
    }
  }
  return { data, body: text.slice(m[0].length) };
}

const clip = (s: string) => (s.length > MAX_OUTPUT ? s.slice(0, MAX_OUTPUT) + `\n…[${s.length - MAX_OUTPUT} chars cut]` : s);

interface Builtin extends SkillMeta {
  dir: string;
}

const DEFS = {
  list_skills: ["List the skills you can load: builtin ones and ones written in the wiki under skills/<name>/SKILL.md.", z.object({})],
  load_skill: [
    "Load a skill's instructions (its SKILL.md), or another file of that skill. Needs: skill:<name> (wiki skills also need wiki:read).",
    z.object({ name: z.string(), file: z.string().optional() }),
  ],
} as const;

function scanDir(dir: string, log: (m: string) => void): Builtin[] {
  let folders: string[];
  if (existsSync(join(dir, "SKILL.md"))) folders = [dir]; // the folder is itself one skill
  else if (existsSync(dir)) {
    folders = readdirSync(dir)
      .filter((n) => !n.startsWith("."))
      .map((n) => join(dir, n))
      .filter((p) => statSync(p).isDirectory() && existsSync(join(p, "SKILL.md")));
  } else {
    log(`skills: no folder ${dir}`);
    folders = [];
  }
  return folders.map((f) => ({ dir: realpathSync(f), name: basename(f), description: "", source: "builtin" as const }));
}

/** Files of a host skill folder, relative, excluding dotfiles. */
function listFiles(dir: string): string[] {
  const out: string[] = [];
  const walk = (d: string, rel: string) => {
    for (const n of readdirSync(d)) {
      if (n.startsWith(".")) continue;
      const full = join(d, n);
      const r = rel ? `${rel}/${n}` : n;
      if (statSync(full).isDirectory()) walk(full, r);
      else out.push(r);
    }
  };
  walk(dir, "");
  return out.sort();
}

export function skillsProvider(opts: { dirs: string[]; log?: (msg: string) => void }): ToolProvider {
  const log = opts.log ?? ((m: string) => console.error(m));
  const builtins = new Map<string, Builtin>();

  async function vaultSkills(ctx: ToolContext): Promise<Map<string, SkillMeta & { files: string[] }>> {
    const out = new Map<string, SkillMeta & { files: string[] }>();
    const pages = ctx.world.wiki.list(ctx.agent).map((p) => p.path);
    for (const p of pages) {
      const m = p.match(/^skills\/([^/]+)\/SKILL\.md$/);
      if (!m || !NAME_RE.test(m[1]!) || builtins.has(m[1]!)) continue; // builtins can't be shadowed
      const text = (await ctx.world.wiki.read(ctx.agent, p)) ?? "";
      const files = pages.filter((q) => q.startsWith(`skills/${m[1]}/`) && q !== p).map((q) => q.slice(`skills/${m[1]}/`.length));
      out.set(m[1]!, { name: m[1]!, description: parseFrontmatter(text).data.description ?? "", source: "vault", files });
    }
    return out;
  }

  return {
    id: "skills",

    async init() {
      for (const d of opts.dirs) {
        for (const s of scanDir(resolve(d), log)) {
          const { data } = parseFrontmatter(await Bun.file(join(s.dir, "SKILL.md")).text());
          const name = data.name || s.name;
          if (!NAME_RE.test(name)) {
            log(`skills: ${s.dir}: bad skill name "${name}", skipped`);
            continue;
          }
          if (builtins.has(name)) {
            log(`skills: ${s.dir}: duplicate skill "${name}", skipped`);
            continue;
          }
          if (!data.description) log(`skills: ${name} has no description`);
          builtins.set(name, { ...s, name, description: data.description ?? "" });
        }
      }
    },

    tools: () => toolDefs(DEFS),

    prompt() {
      const lines = [...builtins.values()].map((s) => `- ${s.name}: ${s.description}`);
      return [
        "Skills. A skill is a set of instructions (and maybe extra files) for a kind of task. Load one with load_skill before doing that kind of task; loading skill <name> needs the right skill:<name>.",
        lines.length ? `Builtin skills:\n${lines.join("\n")}` : "No builtin skills.",
        "More skills live in the wiki: list them with list_skills. You can propose a new skill as a wiki page skills/<name>/SKILL.md, starting with frontmatter (---, name: <name>, description: <when to use it>, ---) followed by the instructions; once the judges approve it, every agent can load it.",
      ].join("\n");
    },

    async run(ctx, name, raw) {
      const { perms } = ctx.world;
      if (name === "list_skills") {
        parseInput(DEFS, name, raw);
        const all: SkillMeta[] = [...builtins.values()];
        let note = "";
        if (perms.can(ctx.agent, "wiki:read")) all.push(...(await vaultSkills(ctx)).values());
        else note = "\n(wiki skills hidden: you lack wiki:read)";
        const fmt = (s: SkillMeta) => `${s.name} [${s.source}]${perms.can(ctx.agent, `skill:${s.name}`) ? "" : ` (needs skill:${s.name})`}: ${s.description}`;
        return (all.map(fmt).join("\n") || "(no skills)") + note;
      }
      if (name !== "load_skill") throw new Denied(`unknown tool ${name}`);
      const input = parseInput(DEFS, name, raw);
      if (!NAME_RE.test(input.name)) throw new Denied(`bad skill name "${input.name}"`);
      perms.check(ctx.agent, `skill:${input.name}`);
      const file = input.file?.replace(/^\.?\/+/, "");

      const b = builtins.get(input.name);
      if (b) {
        const target = resolve(b.dir, file || "SKILL.md");
        const real = existsSync(target) ? realpathSync(target) : null;
        if (!real || relative(b.dir, real).startsWith("..") || !(real + sep).startsWith(b.dir + sep) || statSync(real).isDirectory()) {
          throw new Denied(`no file ${file} in skill ${input.name}`);
        }
        const text = await Bun.file(real).text();
        if (file && file !== "SKILL.md") return clip(text);
        const others = listFiles(b.dir).filter((f) => f !== "SKILL.md");
        return clip(fmtSkill(input.name, "builtin", parseFrontmatter(text).body, others));
      }

      perms.check(ctx.agent, "wiki:read");
      const v = (await vaultSkills(ctx)).get(input.name);
      if (!v) throw new Denied(`no skill ${input.name}`);
      const path = `skills/${input.name}/${file || "SKILL.md"}`;
      const text = await ctx.world.wiki.read(ctx.agent, path);
      if (text === null) throw new Denied(`no file ${file} in skill ${input.name}`);
      if (file && file !== "SKILL.md") return clip(text);
      return clip(fmtSkill(input.name, "vault", parseFrontmatter(text).body, v.files));
    },
  };
}

function fmtSkill(name: string, source: string, body: string, files: string[]): string {
  const extra = files.length ? `\n\n(other files in this skill, load with load_skill {name: "${name}", file}: ${files.join(", ")})` : "";
  return `<skill name="${name}" source="${source}">\n${body.trim()}\n</skill>${extra}`;
}
