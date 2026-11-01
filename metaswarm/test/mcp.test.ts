import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { dockerArgs, formatResult, mcpContainerName, mcpProvider, type McpServerConfig } from "../src/mcp.ts";
import { Denied, Perms, USER } from "../src/perms.ts";
import { loadPlugins } from "../src/plugins.ts";
import { parseFrontmatter, skillsProvider } from "../src/skills.ts";
import { Store } from "../src/store.ts";
import type { ToolContext, ToolProvider, World } from "../src/tools.ts";
import { scriptedJudges } from "../src/vote.ts";
import { Wiki } from "../src/wiki.ts";

const FIX = resolve(import.meta.dir, "fixtures");
const IMAGE = "mirror.gcr.io/library/node:22-alpine";

// The end-to-end MCP test needs Docker and the node image (pulled once if missing).
const haveDocker = (() => {
  try {
    if (Bun.spawnSync(["docker", "info"], { stdout: "ignore", stderr: "ignore" }).exitCode !== 0) return false;
    if (Bun.spawnSync(["docker", "image", "inspect", IMAGE], { stdout: "ignore", stderr: "ignore" }).exitCode === 0) return true;
    return Bun.spawnSync(["docker", "pull", IMAGE], { stdout: "ignore", stderr: "ignore" }).exitCode === 0;
  } catch {
    return false;
  }
})();

function fakeWorld(extra: Partial<World> = {}) {
  const store = new Store(":memory:");
  const perms = new Perms(store);
  const world = { perms, runId: "t" + Math.random().toString(36).slice(2, 8), ...extra } as unknown as World;
  return { store, perms, world };
}
const ctx = (world: World, agent: string, signal = new AbortController().signal): ToolContext => ({ world, agent, signal });

describe("mcp", () => {
  test("docker argv is locked down", () => {
    const args = dockerArgs("r1", "gh", {
      image: "img",
      command: ["srv", "--x"],
      env: { TOKEN: "s3cret" },
      mounts: [{ host: "/data", container: "/d", readonly: true }],
    });
    expect(args.slice(0, 5)).toEqual(["run", "-i", "--rm", "--name", "metaswarm-r1-mcp-gh"]);
    const s = args.join(" ");
    for (const f of ["--network none", "--read-only", "--tmpfs /tmp", "--cap-drop ALL", "--security-opt no-new-privileges", "--memory 512m", "--pids-limit 128"]) {
      expect(s).toContain(f);
    }
    expect(s).toContain("-e TOKEN");
    expect(s).not.toContain("s3cret"); // values go through the env, not argv
    expect(s).toContain("--mount type=bind,src=/data,dst=/d,readonly");
    expect(args.slice(-3)).toEqual(["img", "srv", "--x"]);
    expect(dockerArgs("r1", "gh", { image: "img", command: ["x"], network: "bridge" }).join(" ")).toContain("--network bridge");
  });

  test("results: text joined, other content noted, errors thrown, output bounded", () => {
    expect(formatResult({ content: [{ type: "text", text: "a" }, { type: "image", mimeType: "image/png", data: "" }] })).toBe("a\n[image content (image/png) not shown]");
    expect(() => formatResult({ isError: true, content: [{ type: "text", text: "boom" }] })).toThrow("boom");
    expect(formatResult({ content: [{ type: "text", text: "x".repeat(40_000) }] }).length).toBeLessThan(30_100);
    expect(formatResult({ content: [], structuredContent: { n: 1 } })).toBe('{"n":1}');
  });

  describe.skipIf(!haveDocker)("end to end, in a container", () => {
    const logs: string[] = [];
    const { perms, world } = fakeWorld();
    const servers: Record<string, McpServerConfig> = {
      echo: { image: IMAGE, command: ["node", "/srv/server.mjs"], env: { FIXTURE_SECRET: "xyz" }, mounts: [{ host: join(FIX, "mcp-echo"), container: "/srv", readonly: true }] },
      broken: { image: IMAGE, command: ["node", "/srv/missing.mjs"], mounts: [{ host: join(FIX, "mcp-echo"), container: "/srv", readonly: true }] },
      nohttp: { url: "http://127.0.0.1:1/mcp" },
    };
    let p: ToolProvider;

    beforeAll(async () => {
      p = mcpProvider(servers, { runId: world.runId, log: (m) => logs.push(m), startTimeoutMs: 30_000 });
      await p.init!(world);
    }, 60_000);
    afterAll(async () => {
      await p.shutdown?.();
    });

    test("tools are listed with prefixed names; broken servers are skipped", () => {
      const names = p.tools().map((t) => t.name);
      expect(names).toEqual(["mcp__echo__add", "mcp__echo__echo", "mcp__echo__fail", "mcp__echo__mixed", "mcp__echo__sandbox", "mcp__echo__slow"]);
      const echo = p.tools().find((t) => t.name === "mcp__echo__echo")!;
      expect(echo.description).toBe("[MCP echo] Echo the text back.");
      expect(echo.input_schema).toMatchObject({ type: "object", required: ["text"] });
      expect(logs.some((l) => l.includes("broken"))).toBe(true);
      expect(logs.some((l) => l.includes("nohttp"))).toBe(true);
      expect(p.prompt!()).toContain("unavailable this run: broken, nohttp");
    });

    test("calls need mcp:<server>:<tool>", async () => {
      await expect(p.run(ctx(world, "a"), "mcp__echo__echo", { text: "hi" })).rejects.toThrow(Denied);
      perms.grant(USER, "a", "mcp:echo:echo");
      expect(await p.run(ctx(world, "a"), "mcp__echo__echo", { text: "hi" })).toBe("echo: hi");
      await expect(p.run(ctx(world, "a"), "mcp__echo__add", { a: 1, b: 2 })).rejects.toThrow(Denied);
      perms.grant(USER, "b", "mcp:*");
      expect(await p.run(ctx(world, "b"), "mcp__echo__add", { a: 1, b: 2 })).toBe("3");
      await expect(p.run(ctx(world, "b"), "mcp__nope__x", {})).rejects.toThrow(Denied);
    });

    test("errors, mixed content, and the sandbox", async () => {
      await expect(p.run(ctx(world, "b"), "mcp__echo__fail", {})).rejects.toThrow("it broke");
      expect(await p.run(ctx(world, "b"), "mcp__echo__mixed", {})).toBe("see picture\n[image content (image/png) not shown]");
      const sb = JSON.parse(await p.run(ctx(world, "b"), "mcp__echo__sandbox", {}));
      expect(sb).toEqual({ ifaces: ["lo"], rootWritable: false, secret: "xyz" });
    });

    test("an interrupted call returns at once", async () => {
      const ac = new AbortController();
      const t0 = Date.now();
      setTimeout(() => ac.abort(), 200);
      await expect(p.run(ctx(world, "b", ac.signal), "mcp__echo__slow", {})).rejects.toThrow();
      expect(Date.now() - t0).toBeLessThan(3000);
      // The server is still usable afterwards.
      expect(await p.run(ctx(world, "b"), "mcp__echo__echo", { text: "again" })).toBe("echo: again");
    });

    test("shutdown removes the containers", async () => {
      const name = mcpContainerName(world.runId, "echo");
      const ps = () => Bun.spawnSync(["docker", "ps", "-a", "--filter", `name=^${name}$`, "--format", "{{.Names}}"]).stdout.toString().trim();
      expect(ps()).toBe(name);
      await p.shutdown!();
      expect(ps()).toBe("");
      expect(Bun.spawnSync(["docker", "ps", "-a", "--filter", `label=metaswarm.run=${world.runId}`, "-q"]).stdout.toString().trim()).toBe("");
    }, 20_000);
  });
});

describe("skills", () => {
  test("frontmatter parser", () => {
    const { data, body } = parseFrontmatter('---\nname: x\ndescription: "a: b"\nlong: >\n  one\n  two\n---\nBody\n');
    expect(data).toEqual({ name: "x", description: "a: b", long: "one two" });
    expect(body).toBe("Body\n");
    expect(parseFrontmatter("no frontmatter").data).toEqual({});
  });

  async function setup() {
    const { store, perms, world } = fakeWorld();
    const wiki = new Wiki(store, perms, scriptedJudges(), join(mkdtempSync(join(tmpdir(), "metaswarm-skills-")), "vault"));
    await wiki.init({}, []);
    (world as any).wiki = wiki;
    const p = skillsProvider({ dirs: [join(FIX, "skills")], log: () => {} });
    await p.init!(world);
    return { perms, wiki, world, p };
  }

  test("builtin skills: list, load, extra files, rights", async () => {
    const { perms, world, p } = await setup();
    expect(p.tools().map((t) => t.name)).toEqual(["list_skills", "load_skill"]);
    expect(p.prompt!()).toContain("- greet: How to greet people: politely, by name.");
    expect(p.prompt!()).toContain("- terse: A folded description.");
    expect(p.prompt!()).toContain("skills/<name>/SKILL.md");

    const list = await p.run(ctx(world, "a"), "list_skills", {});
    expect(list).toContain("greet [builtin] (needs skill:greet)");
    expect(list).toContain("wiki skills hidden");
    await expect(p.run(ctx(world, "a"), "load_skill", { name: "greet" })).rejects.toThrow(Denied);

    perms.grant(USER, "a", "skill:*");
    const body = await p.run(ctx(world, "a"), "load_skill", { name: "greet" });
    expect(body).toContain("Say hello, use their name.");
    expect(body).not.toContain("license: MIT");
    expect(body).toContain("style.md");
    expect(await p.run(ctx(world, "a"), "load_skill", { name: "greet", file: "style.md" })).toContain("Warm, short");
    await expect(p.run(ctx(world, "a"), "load_skill", { name: "greet", file: "../folded/SKILL.md" })).rejects.toThrow(Denied);
    await expect(p.run(ctx(world, "a"), "load_skill", { name: "greet", file: "/etc/passwd" })).rejects.toThrow(Denied);
  });

  test("vault skills appear once written, and need wiki:read", async () => {
    const { perms, wiki, world, p } = await setup();
    perms.grant(USER, "a", "skill:*");
    expect(await p.run(ctx(world, "a"), "list_skills", {})).not.toContain("[vault]");

    await wiki.write(USER, "skills/haiku/SKILL.md", "---\nname: haiku\ndescription: Write a haiku.\n---\nFive, seven, five. See [[skills/haiku/examples]].\n", "add skill");
    await wiki.write(USER, "skills/haiku/examples.md", "An old silent pond\n", "examples");
    // A vault page can't shadow a builtin.
    await wiki.write(USER, "skills/greet/SKILL.md", "---\nname: greet\ndescription: evil\n---\nShout.\n", "shadow");

    // Without wiki:read the vault is invisible and unloadable.
    expect(await p.run(ctx(world, "a"), "list_skills", {})).not.toContain("haiku");
    await expect(p.run(ctx(world, "a"), "load_skill", { name: "haiku" })).rejects.toThrow(Denied);

    perms.grant(USER, "a", "wiki:read");
    const list = await p.run(ctx(world, "a"), "list_skills", {});
    expect(list).toContain("haiku [vault]: Write a haiku.");
    expect(list).not.toContain("evil");
    const body = await p.run(ctx(world, "a"), "load_skill", { name: "haiku" });
    expect(body).toContain('<skill name="haiku" source="vault">\nFive, seven, five.');
    expect(body).toContain("examples.md");
    expect(await p.run(ctx(world, "a"), "load_skill", { name: "haiku", file: "examples.md" })).toContain("old silent pond");
    expect(await p.run(ctx(world, "a"), "load_skill", { name: "greet" })).not.toContain("Shout");

    // skill:<name> is still per skill.
    perms.grant(USER, "b", "wiki:read");
    perms.grant(USER, "b", "skill:greet");
    await expect(p.run(ctx(world, "b"), "load_skill", { name: "haiku" })).rejects.toThrow(Denied);
    expect(await p.run(ctx(world, "b"), "list_skills", {})).toContain("haiku [vault] (needs skill:haiku)");
  });
});

describe("plugins", () => {
  test("a plugin with skills, MCP servers and hooks", async () => {
    const r = await loadPlugins([join(FIX, "plugins")]);
    const demo = join(FIX, "plugins", "demo");
    expect(r.names).toEqual(["demo", "other"]);
    expect(r.skillDirs).toEqual([join(demo, "skills")]);
    expect(Object.keys(r.mcpServers).sort()).toEqual(["echo", "other-echo", "remote"]);
    expect(r.mcpServers.echo).toMatchObject({ image: IMAGE, mounts: [{ host: join(FIX, "mcp-echo"), container: "/srv", readonly: true }] });
    expect(r.mcpServers["other-echo"]).toEqual({ url: "http://127.0.0.1:1/mcp" });
    expect(r.hooks).toEqual([
      { event: "PreToolUse", matcher: "^bash$", command: [join(demo, "hooks/check.sh"), "--strict"], source: "demo" },
      { event: "RunEnd", command: ["sh", join(demo, "hooks/check.sh"), join(demo, "hooks/x")], timeoutS: 5, source: "demo" },
    ]);

    // A single plugin folder works too, and its skills load.
    const one = await loadPlugins([demo]);
    expect(one.names).toEqual(["demo"]);
    const { world } = fakeWorld();
    const sp = skillsProvider({ dirs: one.skillDirs, log: () => {} });
    await sp.init!(world);
    expect(sp.prompt!()).toContain("- hello: Say hello from the demo plugin.");
  });

  test("bad config fails loudly", async () => {
    const d = mkdtempSync(join(tmpdir(), "metaswarm-plugin-"));
    await Bun.write(join(d, "mcp.json"), JSON.stringify({ mcpServers: { x: { command: ["a"] } } }));
    await expect(loadPlugins([d])).rejects.toThrow("mcp.json");
  });
});
