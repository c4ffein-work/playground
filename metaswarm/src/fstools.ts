// Claude-Code-style file tools: read_file, write_file, edit_file, glob, grep.
//
// Everything runs inside the agent's container through `sandbox.exec`; the
// host filesystem is never touched. Rights are per absolute path:
//   fs:read:<abs>   read_file; glob/grep take fs:read:<dir>/ (the whole subtree)
//   fs:write:<abs>  write_file, edit_file
// Paths are normalized in TS before the check (so `..` cannot climb out of a
// grant), then resolved inside the container (symlinks) and checked again on
// the real path; the operation runs on the real path and re-verifies it.
//
// `Sandbox.exec` clips stdout at 8k chars, so bigger outputs are staged in a
// temp file in the container's /tmp, gzipped, and fetched in base64 chunks.
import { posix } from "node:path";
import { z } from "zod";
import { Denied } from "./perms.ts";
import type { Sandbox } from "./sandbox.ts";
import { parseInput, toolDefs, type ToolContext, type ToolProvider } from "./tools.ts";

export const WORKSPACE = "/workspace";
const OUT_MAX = 30_000; // chars returned to the model
const READ_LINES = 2000;
const LINE_MAX = 2000;
const READ_BUDGET = 60_000; // bytes of file text fetched per read_file call
const EDIT_MAX = 1_000_000; // largest file edit_file will handle
const LIST_MAX = 1_000_000; // bytes of find / grep output fetched
const GLOB_MAX = 200;
const CHUNK = 5700; // raw bytes per exec; base64 (7600 chars) stays under exec's 8k clip

const DEFS = {
  read_file: [
    `Read a file in your container. Returns lines numbered from 1 (cat -n style), up to ${READ_LINES} lines; lines over ${LINE_MAX} chars are clipped. Use offset (1-based line) and limit to page through long files. Relative paths are under ${WORKSPACE}. Needs: fs:read:<absolute path>.`,
    z.object({ path: z.string().min(1), offset: z.number().int().min(1).optional(), limit: z.number().int().min(1).optional() }),
  ],
  write_file: [
    "Create or overwrite a file with exactly this content (parent directories are created). Needs: fs:write:<absolute path>.",
    z.object({ path: z.string().min(1), content: z.string() }),
  ],
  edit_file: [
    "Replace an exact string in a file. old_string must appear exactly once unless replace_all is true; include enough surrounding text to make it unique. You must read_file the file first (or have written it), and it must not have changed since. Needs: fs:write:<absolute path>.",
    z.object({ path: z.string().min(1), old_string: z.string().min(1), new_string: z.string(), replace_all: z.boolean().optional() }),
  ],
  glob: [
    `Find files by glob pattern (**, *, ?, [abc], {a,b}), matched against paths relative to path (default ${WORKSPACE}). "*.ts" matches only the top level; use "**/*.ts" to recurse. Newest first, at most ${GLOB_MAX}. Needs: fs:read:<dir>/.`,
    z.object({ pattern: z.string().min(1), path: z.string().optional() }),
  ],
  grep: [
    `Search file contents with an extended regex (POSIX ERE; \\d \\w \\s work) under path (file or directory, default ${WORKSPACE}). glob filters files ("*.ts" matches by name at any depth, "src/**/*.ts" by relative path). output_mode: "files_with_matches" (default), "content" (file:line:text, with context lines), or "count". head_limit keeps the first N lines/entries. Needs: fs:read:<dir>/ (or fs:read:<file>).`,
    z.object({
      pattern: z.string().min(1),
      path: z.string().optional(),
      glob: z.string().optional(),
      ignore_case: z.boolean().optional(),
      context: z.number().int().min(0).max(20).optional(),
      output_mode: z.enum(["content", "files_with_matches", "count"]).optional(),
      head_limit: z.number().int().min(1).optional(),
    }),
  ],
} as const;

const TOOLS = toolDefs(DEFS);

const PROMPT = `File tools: to work with files in your container, prefer read_file, write_file, edit_file, glob and grep over bash (cat, sed, echo >, find, grep). They need fs:read:<path> / fs:write:<path> rights rather than bash, show line numbers, and edit exactly. Relative paths are under ${WORKSPACE}. edit_file only works on a file you have read (or written) since it last changed.`;

/** Thrown for failures that are not rights problems (missing file, bad pattern...). */
export class FsError extends Error {
  override name = "FsError";
}

// --- helpers ------------------------------------------------------------------

/** Quote any string for sh: 'it'\''s'. */
export function shq(s: string): string {
  if (s.includes("\0")) throw new FsError("NUL byte in argument");
  return `'${s.replaceAll("'", `'\\''`)}'`;
}

/** Absolute, normalized path; relative paths are under /workspace. */
export function absPath(p: string): string {
  if (p.includes("\0")) throw new FsError("NUL byte in path");
  const n = posix.normalize(posix.isAbsolute(p) ? p : posix.join(WORKSPACE, p));
  return n.length > 1 && n.endsWith("/") ? n.slice(0, -1) : n;
}

/** Right for a whole directory tree: fs:read:/a/b/ (covered by fs:read:/a/b/* or fs:read:/a/*). */
const dirRight = (kind: string, dir: string) => `fs:${kind}:${dir === "/" ? "/" : dir + "/"}`;

export function clipOut(s: string, max = OUT_MAX): string {
  return s.length > max ? `${s.slice(0, max)}\n[truncated: ${s.length - max} more chars]` : s;
}

const sha256 = (data: string | Uint8Array) => new Bun.CryptoHasher("sha256").update(data).digest("hex");
const utf8 = new TextDecoder();

/** Glob → RegExp over '/'-separated relative paths. */
export function globToRegExp(glob: string): RegExp {
  let re = "";
  let depth = 0; // inside {a,b}
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i]!;
    if (c === "*") {
      if (glob[i + 1] === "*") {
        const atStart = i === 0 || glob[i - 1] === "/";
        i++;
        while (glob[i + 1] === "*") i++;
        if (atStart && glob[i + 1] === "/") {
          re += "(?:.*/)?"; // "**/": zero or more directories
          i++;
        } else re += ".*";
      } else re += "[^/]*";
    } else if (c === "?") re += "[^/]";
    else if (c === "[") {
      const end = glob.indexOf("]", i + 2);
      if (end < 0) re += "\\[";
      else {
        let body = glob.slice(i + 1, end);
        if (body.startsWith("!")) body = "^" + body.slice(1);
        re += `[${body.replaceAll("\\", "\\\\")}]`;
        i = end;
      }
    } else if (c === "{") {
      depth++;
      re += "(?:";
    } else if (c === "}" && depth > 0) {
      depth--;
      re += ")";
    } else if (c === "," && depth > 0) re += "|";
    else if (c === "\\" && i + 1 < glob.length) re += "\\" + glob[++i];
    else re += c.replace(/[.+^$()|\\/]/g, "\\$&").replace(/[{}\]]/g, "\\$&");
  }
  re += ")".repeat(depth);
  return new RegExp(`^${re}$`, "s");
}

/** Shell function: print the real path of $1, resolving symlinks in the part that exists. */
const RES = `res() { p=$1; s=; while [ ! -e "$p" ] && [ ! -L "$p" ]; do s="/\${p##*/}$s"; p=\${p%/*}; [ -z "$p" ] && p=/; done; r=$(readlink -f "$p") || return 1; [ "$r" = / ] && r=; printf '%s%s' "$r" "$s"; }`;

interface Captured {
  rc: number;
  out: Uint8Array;
  truncated: boolean;
  stderr: string;
}

function execFailed(r: { code: number; timedOut: boolean; stderr: string }): never {
  if (r.timedOut) throw new FsError("timed out");
  if (r.code === 130) throw new FsError("interrupted");
  throw new FsError(`container exec failed (exit ${r.code}): ${r.stderr.trim()}`);
}

/**
 * Run `script` in the container and return all its stdout (up to `max` bytes),
 * working around exec's output clip: small outputs come back inline as base64,
 * larger ones are gzipped to /tmp and fetched in chunks.
 */
async function capture(sb: Sandbox, script: string, opts: { max: number; stdin?: string; signal?: AbortSignal; timeoutS?: number }): Promise<Captured> {
  const wrapper = [
    `t=$(mktemp) || exit 90`,
    `{ ( ${script}
) 2>"$t.e"; echo $? >"$t.c"; } | head -c ${opts.max + 1} >"$t"`,
    `rc=$(cat "$t.c" 2>/dev/null || echo 99); n=$(wc -c <"$t")`,
    `if [ "$n" -le ${CHUNK} ]; then printf 'rc=%s n=%s\\n' "$rc" "$n"; base64 -w0 "$t"; rm -f "$t"`,
    `else gzip -c "$t" >"$t.z"; rm -f "$t"; printf 'rc=%s n=%s z=%s t=%s\\n' "$rc" "$n" "$(wc -c <"$t.z")" "$t.z"; fi`,
    `head -c 3000 "$t.e" >&2; rm -f "$t.e" "$t.c"`,
  ].join("\n");
  const r = await sb.exec(wrapper, { stdin: opts.stdin, signal: opts.signal, timeoutS: opts.timeoutS });
  const nl = r.stdout.indexOf("\n");
  const head = r.stdout.slice(0, nl < 0 ? undefined : nl);
  const m = /^rc=(\d+) n=(\d+)(?: z=(\d+) t=(\S+))?$/.exec(head);
  if (r.code !== 0 || !m) execFailed(r);
  let out: Uint8Array;
  if (!m[3]) out = Buffer.from(r.stdout.slice(nl + 1).trim(), "base64");
  else {
    const tmp = m[4]!;
    try {
      const parts: Buffer[] = [];
      for (let i = 0; i * CHUNK < Number(m[3]); i++) {
        const c = await sb.exec(`dd if=${shq(tmp)} bs=${CHUNK} skip=${i} count=1 2>/dev/null | base64 -w0`, { signal: opts.signal });
        if (c.code !== 0) execFailed(c);
        parts.push(Buffer.from(c.stdout.trim(), "base64"));
      }
      out = Bun.gunzipSync(Buffer.concat(parts));
    } finally {
      await sb.exec(`rm -f ${shq(tmp)}`).catch(() => {});
    }
  }
  const truncated = out.length > opts.max;
  return { rc: Number(m[1]), out: truncated ? out.subarray(0, opts.max) : out, truncated, stderr: r.stderr.trim() };
}

type Kind = "file" | "dir" | "missing" | "other";

// --- the provider -----------------------------------------------------------------

class FsTools implements ToolProvider {
  readonly id = "fs";
  /** Content hash of each file as of the agent's last read_file/write_file/edit_file, keyed run\0agent\0real path. */
  private seen = new Map<string, string>();

  async init() {
    this.seen.clear();
  }

  tools() {
    return TOOLS;
  }

  prompt() {
    return PROMPT;
  }

  async run(ctx: ToolContext, name: string, raw: unknown): Promise<string> {
    if (!(name in DEFS)) throw new Denied(`unknown tool ${name}`);
    const n = name as keyof typeof DEFS;
    switch (n) {
      case "read_file":
        return this.read(ctx, parseInput(DEFS, n, raw));
      case "write_file":
        return this.write(ctx, parseInput(DEFS, n, raw));
      case "edit_file":
        return this.edit(ctx, parseInput(DEFS, n, raw));
      case "glob":
        return this.glob(ctx, parseInput(DEFS, n, raw));
      case "grep":
        return this.grep(ctx, parseInput(DEFS, n, raw));
    }
  }

  private key(ctx: ToolContext, real: string) {
    return `${ctx.world.runId}\0${ctx.agent}\0${real}`;
  }

  /**
   * Check `right(abs)`, resolve symlinks in the container, check `right(real)`.
   * Returns the sandbox, the real path and what is there.
   */
  private async open(ctx: ToolContext, path: string, right: (p: string) => string) {
    const abs = absPath(path);
    ctx.world.perms.check(ctx.agent, right(abs));
    const sb = await ctx.world.sandbox(ctx.agent);
    const r = await sb.exec(
      `${RES}\nr=$(res ${shq(abs)}) || { echo E; exit 0; }\nif [ -f "$r" ]; then k=file; elif [ -d "$r" ]; then k=dir; elif [ -e "$r" ]; then k=other; else k=missing; fi\nprintf '%s\\n%s' "$k" "$r"`,
      { signal: ctx.signal },
    );
    if (r.code !== 0) execFailed(r);
    const nl = r.stdout.indexOf("\n");
    if (nl < 0) throw new FsError(`cannot resolve ${abs}`);
    const kind = r.stdout.slice(0, nl) as Kind;
    const real = r.stdout.slice(nl + 1) || "/";
    if (real !== abs) ctx.world.perms.check(ctx.agent, right(real));
    // Operations re-check that the real path still resolves to itself (no symlink swapped in).
    const guard = `${RES}\n[ "$(res ${shq(real)})" = ${shq(real === "/" ? "" : real)} ] || { echo "path changed while in use" >&2; exit 97; }`;
    return { sb, abs, real, kind, guard };
  }

  private async read(ctx: ToolContext, input: { path: string; offset?: number; limit?: number }) {
    const { sb, abs, real, kind, guard } = await this.open(ctx, input.path, (p) => `fs:read:${p}`);
    if (kind === "missing") throw new FsError(`${abs}: no such file`);
    if (kind === "dir") throw new FsError(`${abs} is a directory; use glob to list it`);
    if (kind !== "file") throw new FsError(`${abs} is not a regular file`);
    const from = input.offset ?? 1;
    const to = from + (input.limit ?? READ_LINES) - 1;
    const f = shq(real);
    const script = [
      guard,
      // Metadata line: hash, line count, byte count, binary flag.
      `nl=$(wc -l <${f}); [ -s ${f} ] && [ "$(tail -c1 ${f} | wc -l)" = 0 ] && nl=$((nl+1))`,
      `bin=0; [ "$(head -c 8000 ${f} | tr -d '\\000' | wc -c)" = "$(head -c 8000 ${f} | wc -c)" ] || bin=1`,
      `printf '%s %s %s %s\\n' "$(sha256sum <${f} | cut -c1-64)" "$nl" "$(wc -c <${f})" "$bin"`,
      `[ "$bin" = 1 ] && exit 0`,
      // Each line is prefixed with 1 if it was clipped, else 0.
      `awk -v a=${from} -v b=${to} -v w=${LINE_MAX} -v m=${READ_BUDGET} 'NR<a{next} NR>b{exit} {c=0; if(length($0)>w){$0=substr($0,1,w); c=1}; t+=length($0)+1; if(t>m) exit; print c $0}' ${f}`,
    ].join("\n");
    const c = await capture(sb, script, { max: READ_BUDGET + 4 * LINE_MAX, signal: ctx.signal });
    if (c.rc !== 0) throw new FsError(`cannot read ${abs}: ${c.stderr || `exit ${c.rc}`}`);
    const text = utf8.decode(c.out);
    const nl = text.indexOf("\n");
    const [hash, total, size, bin] = text.slice(0, nl).split(" ");
    const lines = Number(total);
    if (bin === "1") return `${abs} looks like a binary file (${size} bytes); not shown.`;
    this.seen.set(this.key(ctx, real), hash!);
    if (lines === 0) return `${abs} is empty.`;
    if (from > lines) return `${abs} has ${lines} lines; offset ${from} is past the end.`;
    const body = text.slice(nl + 1).split("\n");
    if (body.at(-1) === "") body.pop();
    let out = "";
    let last = from - 1;
    let clipped = 0;
    for (const l of body) {
      const line = `${String(last + 1).padStart(6)}\t${l.slice(1)}${l[0] === "1" ? " …[line clipped]" : ""}\n`;
      if (out.length + line.length > OUT_MAX) break;
      out += line;
      last++;
      if (l[0] === "1") clipped++;
    }
    const notes: string[] = [];
    if (last < lines) notes.push(`[showing lines ${from}-${last} of ${lines}; read more with offset=${last + 1}]`);
    if (clipped) notes.push(`[${clipped} line(s) longer than ${LINE_MAX} chars clipped]`);
    return out + notes.join("\n");
  }

  private async write(ctx: ToolContext, input: { path: string; content: string }) {
    const { sb, abs, real, kind, guard } = await this.open(ctx, input.path, (p) => `fs:write:${p}`);
    if (kind === "dir") throw new FsError(`${abs} is a directory`);
    if (kind === "other") throw new FsError(`${abs} is not a regular file`);
    const r = await sb.exec(`${guard}\nmkdir -p ${shq(posix.dirname(real))} && cat >${shq(real)}`, { stdin: input.content, signal: ctx.signal });
    if (r.code !== 0) {
      if (r.timedOut || r.code === 130) execFailed(r);
      throw new FsError(`cannot write ${abs}: ${r.stderr.trim() || `exit ${r.code}`}`);
    }
    this.seen.set(this.key(ctx, real), sha256(input.content));
    const lines = input.content === "" ? 0 : input.content.split("\n").length - (input.content.endsWith("\n") ? 1 : 0);
    return `${kind === "missing" ? "created" : "overwrote"} ${abs} (${lines} lines, ${Buffer.byteLength(input.content)} bytes)`;
  }

  private async edit(ctx: ToolContext, input: { path: string; old_string: string; new_string: string; replace_all?: boolean }) {
    if (input.old_string === input.new_string) throw new FsError("old_string and new_string are the same; nothing to do");
    const { sb, abs, real, kind, guard } = await this.open(ctx, input.path, (p) => `fs:write:${p}`);
    if (kind === "missing") throw new FsError(`${abs}: no such file (use write_file to create it)`);
    if (kind !== "file") throw new FsError(`${abs} is not a regular file`);
    const known = this.seen.get(this.key(ctx, real));
    if (!known) throw new FsError(`read ${abs} with read_file before editing it`);
    const c = await capture(sb, `${guard}\ncat ${shq(real)}`, { max: EDIT_MAX, signal: ctx.signal });
    if (c.rc !== 0) throw new FsError(`cannot read ${abs}: ${c.stderr || `exit ${c.rc}`}`);
    if (c.truncated) throw new FsError(`${abs} is over ${EDIT_MAX} bytes; edit it with bash`);
    const hash = sha256(c.out);
    if (hash !== known) {
      this.seen.delete(this.key(ctx, real));
      throw new FsError(`${abs} has changed since you last read it; read it again with read_file before editing`);
    }
    let text: string;
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(c.out);
    } catch {
      throw new FsError(`${abs} is not valid UTF-8 text; edit it with bash`);
    }
    const parts = text.split(input.old_string);
    const count = parts.length - 1;
    if (count === 0) throw new FsError(`old_string not found in ${abs}`);
    if (count > 1 && !input.replace_all) {
      throw new FsError(`old_string appears ${count} times in ${abs}; add surrounding context to make it unique, or set replace_all`);
    }
    const next = input.replace_all ? parts.join(input.new_string) : parts[0] + input.new_string + parts.slice(1).join(input.old_string);
    // Write only if the file is still what we just read.
    const r = await sb.exec(`${guard}\n[ "$(sha256sum <${shq(real)} | cut -c1-64)" = ${hash} ] || { echo "file changed during edit" >&2; exit 98; }\ncat >${shq(real)}`, {
      stdin: next,
      signal: ctx.signal,
    });
    if (r.code !== 0) {
      if (r.timedOut || r.code === 130) execFailed(r);
      throw new FsError(`cannot write ${abs}: ${r.stderr.trim() || `exit ${r.code}`}`);
    }
    this.seen.set(this.key(ctx, real), sha256(next));
    // Show the edited region of the first replacement.
    const at = parts[0]!.split("\n").length;
    const nextLines = next.split("\n");
    const end = Math.min(nextLines.length, at + input.new_string.split("\n").length + 2);
    const start = Math.max(1, at - 3);
    const snippet = nextLines.slice(start - 1, end).map((l, i) => `${String(start + i).padStart(6)}\t${l}`).join("\n");
    return clipOut(`edited ${abs} (${count} replacement${count > 1 ? "s" : ""})\n${snippet}`, 4000);
  }

  /** `find dir -type f` with mtimes, as [{mtime, path}] (paths absolute). */
  private async listFiles(ctx: ToolContext, sb: Sandbox, guard: string, dir: string) {
    const c = await capture(sb, `${guard}\nfind ${shq(dir)} -type f -exec stat -c '%Y %n' {} +`, { max: LIST_MAX, signal: ctx.signal });
    if (c.rc !== 0 && c.out.length === 0) throw new FsError(`cannot list ${dir}: ${c.stderr || `exit ${c.rc}`}`);
    const lines = utf8.decode(c.out).split("\n");
    if (c.truncated) lines.pop(); // last line may be cut
    const files = lines.filter(Boolean).map((l) => {
      const sp = l.indexOf(" ");
      return { mtime: Number(l.slice(0, sp)), path: l.slice(sp + 1) };
    });
    return { files, truncated: c.truncated };
  }

  private async glob(ctx: ToolContext, input: { pattern: string; path?: string }) {
    let { pattern } = input;
    let base = input.path;
    if (pattern.startsWith("/")) {
      // Absolute pattern: its leading literal directories become the base.
      const segs = pattern.split("/");
      const i = segs.findIndex((s, j) => j > 0 && /[*?[{]/.test(s));
      const lit = i < 0 ? segs.length - 1 : i;
      base = segs.slice(0, lit).join("/") || "/";
      pattern = segs.slice(lit).join("/");
    }
    const { sb, abs, real, kind, guard } = await this.open(ctx, base ?? WORKSPACE, (p) => dirRight("read", p));
    if (kind !== "dir") throw new FsError(`${abs} is not a directory`);
    const re = globToRegExp(pattern);
    const { files, truncated } = await this.listFiles(ctx, sb, guard, real);
    const prefix = real === "/" ? "/" : real + "/";
    const hits = files.filter((f) => re.test(f.path.slice(prefix.length))).sort((a, b) => b.mtime - a.mtime || a.path.localeCompare(b.path));
    const shown = hits.slice(0, GLOB_MAX).map((f) => (real === abs ? f.path : abs + f.path.slice(real.length)));
    const notes: string[] = [];
    if (hits.length > GLOB_MAX) notes.push(`[${hits.length - GLOB_MAX} more matches not shown; narrow the pattern]`);
    if (truncated) notes.push(`[file listing truncated; results may be incomplete]`);
    if (!hits.length) return `no files match ${pattern} under ${abs}` + (notes.length ? "\n" + notes.join("\n") : "");
    return clipOut([...shown, ...notes].join("\n"));
  }

  private async grep(
    ctx: ToolContext,
    input: { pattern: string; path?: string; glob?: string; ignore_case?: boolean; context?: number; output_mode?: "content" | "files_with_matches" | "count"; head_limit?: number },
  ) {
    const mode = input.output_mode ?? "files_with_matches";
    const target = absPath(input.path ?? WORKSPACE);
    // A file needs fs:read:<file>; a directory needs the subtree right. We don't know which until resolved,
    // so first require the file-or-dir right that applies to the literal path.
    const { sb, abs, real, kind, guard } = await this.open(ctx, target, (p) =>
      ctx.world.perms.can(ctx.agent, `fs:read:${p}`) ? `fs:read:${p}` : dirRight("read", p),
    );
    if (kind === "missing") throw new FsError(`${abs}: no such file or directory`);
    if (kind === "dir") ctx.world.perms.check(ctx.agent, dirRight("read", real));
    else if (kind !== "file") throw new FsError(`${abs} is not a regular file or directory`);

    const flags = ["-E", "-s"];
    if (input.ignore_case) flags.push("-i");
    if (mode === "content") {
      flags.push("-n", "-H");
      if (input.context) flags.push("-C", String(input.context));
    } else if (mode === "files_with_matches") flags.push("-l");
    else flags.push("-c", "-H");
    const grepArgs = `${flags.join(" ")} -e ${shq(input.pattern)} --`;

    let script: string;
    let stdin: string | undefined;
    if (input.glob && kind === "dir") {
      const re = globToRegExp(input.glob);
      const byName = !input.glob.includes("/");
      const { files } = await this.listFiles(ctx, sb, guard, real);
      const prefix = real === "/" ? "/" : real + "/";
      const picked = files.map((f) => f.path).filter((p) => re.test(byName ? posix.basename(p) : p.slice(prefix.length))).sort();
      if (!picked.length) return `no files match glob ${input.glob} under ${abs}`;
      script = `${guard}\nxargs -0 -r grep ${grepArgs}`;
      stdin = picked.join("\0") + "\0";
    } else {
      script = `${guard}\ngrep ${kind === "dir" ? "-r " : ""}${grepArgs} ${shq(real)}`;
    }
    const c = await capture(sb, script, { max: LIST_MAX, stdin, signal: ctx.signal, timeoutS: 60 });
    // grep: 0 = matches, 1 = none, 2 = error. xargs: 123 = some grep exited 1-125.
    if (c.rc === 2 || (c.rc > 2 && c.rc !== 123) || (c.rc === 123 && c.out.length === 0 && c.stderr)) {
      throw new FsError(`grep failed: ${c.stderr || `exit ${c.rc}`}`);
    }
    let lines = utf8.decode(c.out).split("\n");
    if (c.truncated) lines.pop();
    lines = lines.filter((l) => l !== "");
    if (mode === "count") lines = lines.filter((l) => !l.endsWith(":0"));
    if (real !== abs) lines = lines.map((l) => (l.startsWith(real) ? abs + l.slice(real.length) : l));
    if (!lines.length) return "no matches";
    const notes: string[] = [];
    if (input.head_limit && lines.length > input.head_limit) {
      notes.push(`[${lines.length - input.head_limit} more lines not shown (head_limit)]`);
      lines = lines.slice(0, input.head_limit);
    }
    if (c.truncated) notes.push("[grep output truncated; narrow the search]");
    if (mode === "count") {
      const total = lines.reduce((s, l) => s + Number(l.slice(l.lastIndexOf(":") + 1)), 0);
      notes.push(`[${total} matching lines in ${lines.length} files]`);
    }
    return clipOut([...lines, ...notes].join("\n"));
  }
}

export const fsTools: ToolProvider = new FsTools();
