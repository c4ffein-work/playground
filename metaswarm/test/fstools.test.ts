// File tools against a real Docker sandbox. Skipped when no docker daemon is reachable.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { absPath, fsTools, globToRegExp, shq } from "../src/fstools.ts";
import { Denied, Perms, USER } from "../src/perms.ts";
import { DockerSandboxes, type Sandbox } from "../src/sandbox.ts";
import { Store } from "../src/store.ts";
import type { ToolContext, World } from "../src/tools.ts";

const dockerOk = Bun.spawnSync(["docker", "info"], { stdout: "ignore", stderr: "ignore" }).exitCode === 0;
const image = process.env.METASWARM_IMAGE ?? "mirror.gcr.io/library/alpine:3.20";
const runId = `fstest-${Date.now().toString(36)}`;
const factory = new DockerSandboxes({ image, runId });
const perms = new Perms(new Store(":memory:"));
const boxes = new Map<string, Promise<Sandbox>>();
const world = {
  perms,
  runId,
  sandbox(agent: string) {
    if (!boxes.has(agent)) boxes.set(agent, factory.create(agent));
    return boxes.get(agent)!;
  },
} as unknown as World;

const ctx = (agent: string): ToolContext => ({ world, agent, signal: new AbortController().signal });
const call = (agent: string, name: string, input: unknown) => fsTools.run(ctx(agent), name, input);
const sh = async (cmd: string) => (await world.sandbox("a")).exec(cmd);

describe.skipIf(!dockerOk)("fs tools (docker)", () => {
  beforeAll(async () => {
    await fsTools.init?.(world);
    perms.grant(USER, "a", "fs:read:/workspace/*");
    perms.grant(USER, "a", "fs:write:/workspace/*");
    perms.grant(USER, "b", "fs:read:/workspace/b/*");
    perms.grant(USER, "b", "fs:write:/workspace/b/*");
  });

  afterAll(async () => {
    await fsTools.shutdown?.();
    await factory.shutdown();
  });

  describe("helpers", () => {
    test("shq survives any string", async () => {
      const nasty = `it's "$(echo pwned)" \`id\` \\ $HOME\n'';`;
      const r = await sh(`printf '%s' ${shq(nasty)}`);
      expect(r.stdout).toBe(nasty);
    });

    test("absPath", () => {
      expect(absPath("x/y")).toBe("/workspace/x/y");
      expect(absPath("../etc/passwd")).toBe("/etc/passwd");
      expect(absPath("/workspace/a/../../etc/")).toBe("/etc");
      expect(absPath("/")).toBe("/");
    });

    test("globToRegExp", () => {
      const m = (g: string, p: string) => globToRegExp(g).test(p);
      expect(m("**/*.ts", "a.ts")).toBe(true);
      expect(m("**/*.ts", "x/y/a.ts")).toBe(true);
      expect(m("*.ts", "x/a.ts")).toBe(false);
      expect(m("src/**", "src/a/b")).toBe(true);
      expect(m("a?.{js,ts}", "ab.js")).toBe(true);
      expect(m("a?.{js,ts}", "ab.md")).toBe(false);
      expect(m("[!x]*.md", "y.md")).toBe(true);
      expect(m("[!x]*.md", "x.md")).toBe(false);
      expect(m("a.b", "axb")).toBe(false);
    });
  });

  describe("fs tools", () => {
    test("write then read, with line numbers", async () => {
      expect(await call("a", "write_file", { path: "dir/sub/hello.txt", content: "one\ntwo\nthree\n" })).toContain("created /workspace/dir/sub/hello.txt");
      const out = await call("a", "read_file", { path: "/workspace/dir/sub/hello.txt" });
      expect(out).toBe("     1\tone\n     2\ttwo\n     3\tthree\n");
    });

    test("offset and limit", async () => {
      const content = Array.from({ length: 50 }, (_, i) => `line ${i + 1}`).join("\n");
      await call("a", "write_file", { path: "long.txt", content });
      const out = await call("a", "read_file", { path: "long.txt", offset: 10, limit: 3 });
      expect(out).toContain("    10\tline 10\n    11\tline 11\n    12\tline 12\n");
      expect(out).not.toContain("line 13");
      expect(out).toContain("showing lines 10-12 of 50; read more with offset=13");
      expect(await call("a", "read_file", { path: "long.txt", offset: 99 })).toContain("past the end");
    });

    test("big file: default 2000 lines, long lines clipped, output bounded", async () => {
      // Built inside the container: far over exec's 8k output clip.
      await sh(`mkdir -p /workspace/big && awk 'BEGIN{for(i=1;i<=3000;i++) print "row " i}' > /workspace/big/rows.txt`);
      const out = await call("a", "read_file", { path: "big/rows.txt" });
      expect(out).toContain("     1\trow 1\n");
      expect(out.length).toBeLessThan(31_000);
      expect(out).toMatch(/showing lines 1-\d+ of 3000/);
      const tail = await call("a", "read_file", { path: "big/rows.txt", offset: 2990 });
      expect(tail).toContain("  3000\trow 3000\n");
      await sh(`awk 'BEGIN{s=""; for(i=0;i<5000;i++) s=s "x"; print s; print "short"}' > /workspace/big/wide.txt`);
      const wide = await call("a", "read_file", { path: "big/wide.txt" });
      expect(wide).toContain("…[line clipped]");
      expect(wide).toContain("     2\tshort");
    });

    test("missing files and directories", async () => {
      await expect(call("a", "read_file", { path: "nope.txt" })).rejects.toThrow(/no such file/);
      await expect(call("a", "read_file", { path: "dir" })).rejects.toThrow(/is a directory/);
    });

    test("content with quotes, newlines and $() is written verbatim", async () => {
      const content = `a 'single' "double" $(touch /workspace/pwned) \`id\` $HOME \\n\nEOF\n\ttab ünïcödé 🎉`;
      await call("a", "write_file", { path: "weird.txt", content });
      const r = await sh(`cat /workspace/weird.txt; test -e /workspace/pwned && echo PWNED`);
      expect(r.stdout).toBe(content);
      expect(await call("a", "read_file", { path: "weird.txt" })).toContain("     3\t\ttab ünïcödé 🎉");
    });

    test("edit after read", async () => {
      await call("a", "write_file", { path: "e.txt", content: "alpha\nbeta\ngamma\n" });
      await call("a", "read_file", { path: "e.txt" });
      expect(await call("a", "edit_file", { path: "e.txt", old_string: "beta", new_string: "BETA $& $1" })).toContain("1 replacement");
      expect((await sh("cat /workspace/e.txt")).stdout).toBe("alpha\nBETA $& $1\ngamma\n");
      // Our own edit keeps the file "read": a second edit works.
      await call("a", "edit_file", { path: "e.txt", old_string: "gamma", new_string: "delta" });
      expect((await sh("cat /workspace/e.txt")).stdout).toBe("alpha\nBETA $& $1\ndelta\n");
    });

    test("edit refused when not read", async () => {
      await sh(`printf 'x\\n' > /workspace/unread.txt`);
      await expect(call("a", "edit_file", { path: "unread.txt", old_string: "x", new_string: "y" })).rejects.toThrow(/read .* before editing/);
      // Another agent's read doesn't count either.
      perms.grant(USER, "c", "fs:*");
      await call("c", "read_file", { path: "unread.txt" });
      await expect(call("a", "edit_file", { path: "unread.txt", old_string: "x", new_string: "y" })).rejects.toThrow(/before editing/);
    });

    test("edit refused when the file changed after the read", async () => {
      await call("a", "write_file", { path: "chg.txt", content: "v1\n" });
      await call("a", "read_file", { path: "chg.txt" });
      await sh(`echo v2 > /workspace/chg.txt`);
      await expect(call("a", "edit_file", { path: "chg.txt", old_string: "v", new_string: "w" })).rejects.toThrow(/changed since you last read it/);
      await call("a", "read_file", { path: "chg.txt" });
      await call("a", "edit_file", { path: "chg.txt", old_string: "v2", new_string: "v3" });
      expect((await sh("cat /workspace/chg.txt")).stdout).toBe("v3\n");
    });

    test("ambiguous old_string, replace_all, not found, no-op", async () => {
      await call("a", "write_file", { path: "dup.txt", content: "foo foo\nfoo\n" });
      await expect(call("a", "edit_file", { path: "dup.txt", old_string: "foo", new_string: "bar" })).rejects.toThrow(/appears 3 times/);
      await expect(call("a", "edit_file", { path: "dup.txt", old_string: "zzz", new_string: "bar" })).rejects.toThrow(/not found/);
      await expect(call("a", "edit_file", { path: "dup.txt", old_string: "foo", new_string: "foo" })).rejects.toThrow(/same/);
      expect(await call("a", "edit_file", { path: "dup.txt", old_string: "foo", new_string: "bar", replace_all: true })).toContain("3 replacements");
      expect((await sh("cat /workspace/dup.txt")).stdout).toBe("bar bar\nbar\n");
    });

    test("glob with **, newest first", async () => {
      await sh(
        `mkdir -p /workspace/g/src/deep && cd /workspace/g && echo > top.ts && echo > src/a.ts && echo > src/deep/b.ts && echo > src/c.md` +
          ` && touch -d '2020-01-01 00:00:00' src/a.ts && touch -d '2021-01-01 00:00:00' top.ts`,
      );
      const all = (await call("a", "glob", { pattern: "**/*.ts", path: "g" })).split("\n");
      expect(all).toEqual(["/workspace/g/src/deep/b.ts", "/workspace/g/top.ts", "/workspace/g/src/a.ts"]);
      expect(await call("a", "glob", { pattern: "*.ts", path: "/workspace/g" })).toBe("/workspace/g/top.ts");
      expect(await call("a", "glob", { pattern: "/workspace/g/src/**/*.{md,ts}" })).toContain("/workspace/g/src/c.md");
      expect(await call("a", "glob", { pattern: "**/*.rs", path: "g" })).toContain("no files match");
    });

    test("grep modes", async () => {
      await sh(`mkdir -p /workspace/s/x && cd /workspace/s && printf 'hello world\\nbye\\nHello again\\n' > one.txt && printf 'nothing\\n' > two.txt && printf 'hello there\\n' > x/three.md`);
      const files = await call("a", "grep", { pattern: "hello", path: "s" });
      expect(files.split("\n").sort()).toEqual(["/workspace/s/one.txt", "/workspace/s/x/three.md"]);
      const content = await call("a", "grep", { pattern: "h[e]llo", path: "s", output_mode: "content", ignore_case: true, glob: "*.txt" });
      expect(content).toBe("/workspace/s/one.txt:1:hello world\n/workspace/s/one.txt:3:Hello again");
      const ctxOut = await call("a", "grep", { pattern: "bye", path: "s/one.txt", output_mode: "content", context: 1 });
      expect(ctxOut).toBe("/workspace/s/one.txt-1-hello world\n/workspace/s/one.txt:2:bye\n/workspace/s/one.txt-3-Hello again");
      const count = await call("a", "grep", { pattern: "hello", path: "s", output_mode: "count", ignore_case: true });
      expect(count).toContain("/workspace/s/one.txt:2");
      expect(count).toContain("/workspace/s/x/three.md:1");
      expect(count).not.toContain("two.txt");
      expect(count).toContain("[3 matching lines in 2 files]");
      expect(await call("a", "grep", { pattern: "\\d{5}", path: "s" })).toBe("no matches");
      expect(await call("a", "grep", { pattern: "hello", path: "s", output_mode: "content", head_limit: 1 })).toContain("more lines not shown");
      await expect(call("a", "grep", { pattern: "(", path: "s" })).rejects.toThrow(/grep failed/);
      // Patterns are never interpreted by the shell.
      expect(await call("a", "grep", { pattern: "$(touch /workspace/pwned2)'", path: "s" })).toBe("no matches");
      expect((await sh("test -e /workspace/pwned2 || echo clean")).stdout).toBe("clean\n");
    });

    test("rights: denial and .. escapes", async () => {
      await sh(`mkdir -p /workspace/b && echo secret > /workspace/top-secret.txt && echo mine > /workspace/b/mine.txt`);
      expect(await call("b", "read_file", { path: "/workspace/b/mine.txt" })).toContain("mine");
      await expect(call("b", "read_file", { path: "/workspace/top-secret.txt" })).rejects.toThrow(Denied);
      await expect(call("b", "read_file", { path: "/workspace/b/../top-secret.txt" })).rejects.toThrow(/lacks right "fs:read:\/workspace\/top-secret.txt"/);
      await expect(call("b", "read_file", { path: "b/../../etc/passwd" })).rejects.toThrow(Denied);
      await expect(call("b", "write_file", { path: "/workspace/b/../evil.txt", content: "x" })).rejects.toThrow(Denied);
      await expect(call("b", "glob", { pattern: "**", path: "/workspace" })).rejects.toThrow(Denied);
      await expect(call("b", "grep", { pattern: "secret" })).rejects.toThrow(Denied);
      expect(await call("b", "glob", { pattern: "**", path: "/workspace/b" })).toBe("/workspace/b/mine.txt");
      await expect(call("nobody", "read_file", { path: "x" })).rejects.toThrow(Denied);
    });

    test("rights: symlinks are resolved before the check", async () => {
      await sh(`ln -sf /workspace/top-secret.txt /workspace/b/link && ln -sfn /workspace /workspace/b/up`);
      await expect(call("b", "read_file", { path: "/workspace/b/link" })).rejects.toThrow(/lacks right "fs:read:\/workspace\/top-secret.txt"/);
      await expect(call("b", "write_file", { path: "/workspace/b/up/evil.txt", content: "x" })).rejects.toThrow(Denied);
      await expect(call("b", "grep", { pattern: "secret", path: "/workspace/b/up" })).rejects.toThrow(Denied);
      expect((await sh("test -e /workspace/evil.txt || echo clean")).stdout).toBe("clean\n");
    });

    test("tool definitions and prompt", async () => {
      expect(fsTools.tools().map((t) => t.name)).toEqual(["read_file", "write_file", "edit_file", "glob", "grep"]);
      expect(fsTools.prompt?.()).toContain("prefer read_file");
      await expect(call("a", "read_file", { path: 3 })).rejects.toThrow(/invalid input/);
    });
  });
});
