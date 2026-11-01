import { describe, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Chat } from "../src/chat.ts";
import { DEMOS } from "../src/demos.ts";
import { ScriptedModel } from "../src/model.ts";
import { covers, Denied, Perms, USER } from "../src/perms.ts";
import { standardProviders } from "../src/providers.ts";
import { runDemo } from "../src/run.ts";
import type { Sandbox, SandboxFactory } from "../src/sandbox.ts";
import { Store } from "../src/store.ts";
import { scriptedJudges } from "../src/vote.ts";
import { Wiki } from "../src/wiki.ts";

const tmp = () => mkdtempSync(join(tmpdir(), "metaswarm-test-"));
const world = () => {
  const store = new Store(":memory:");
  const perms = new Perms(store);
  return { store, perms, chat: new Chat(store, perms) };
};

describe("rights", () => {
  test("wildcards", () => {
    expect(covers("*", "bash")).toBe(true);
    expect(covers("channel:abc:*", "channel:abc:post")).toBe(true);
    expect(covers("channel:abc:*", "channel:abd:post")).toBe(false);
    expect(covers("page:docs/*:write", "page:docs/a/b.md:write")).toBe(true);
    // A narrower grant never covers a wider pattern.
    expect(covers("channel:abc:post", "channel:abc:*")).toBe(false);
    expect(covers("a*", "*")).toBe(false);
  });

  test("delegation needs a delegable grant, and only narrows", () => {
    const { perms } = world();
    perms.grant(USER, "a", "channel:x:*", { delegable: true });
    perms.grant(USER, "a", "bash");
    expect(perms.grant("a", "b", "channel:x:post").parent).not.toBeNull();
    expect(() => perms.grant("a", "b", "bash")).toThrow(Denied);
    expect(() => perms.grant("a", "b", "channel:*:post")).toThrow(Denied);
    expect(() => perms.grant("a", "a", "channel:x:post")).toThrow(Denied);
  });

  test("revoking cascades down the tree", () => {
    const { perms } = world();
    const g = perms.grant(USER, "a", "wiki:propose", { delegable: true });
    perms.grant("a", "b", "wiki:propose", { delegable: true });
    perms.grant("b", "c", "wiki:propose");
    expect(perms.can("c", "wiki:propose")).toBe(true);
    expect(() => perms.revoke("c", g.id)).toThrow(Denied);
    expect(perms.revoke(USER, g.id)).toHaveLength(3);
    expect(perms.can("b", "wiki:propose") || perms.can("c", "wiki:propose")).toBe(false);
  });

  test("a child grant expires with its parent", async () => {
    const { perms } = world();
    perms.grant(USER, "a", "bash", { delegable: true, ttlS: 1 });
    const child = perms.grant("a", "b", "bash", { ttlS: 3600 });
    expect(child.expiresAt).not.toBeNull();
    await Bun.sleep(1100);
    expect(perms.can("b", "bash")).toBe(false);
  });
});

describe("channels", () => {
  test("open vs invite-only, invite delegation, mentions", () => {
    const { perms, chat } = world();
    perms.grant(USER, "a", "channel:create");
    perms.grant(USER, "b", "channel:join");
    const open = chat.create("a", "open one", "open");
    const priv = chat.create("a", "private", "invite");
    chat.join("b", open.id, "mentions");
    expect(() => chat.join("b", priv.id)).toThrow(Denied);
    expect(chat.visibleTo("b").map((c) => c.id)).toEqual([open.id]);

    chat.invite("a", priv.id, "c");
    expect(perms.can("c", `channel:${priv.id}:post`)).toBe(true);
    // c's rights hang under a's: revoking a's channel grant removes c's too.
    const aGrant = perms.held("a").find((g) => g.right === `channel:${priv.id}:*`)!;
    perms.revoke(USER, aGrant.id);
    expect(perms.can("c", `channel:${priv.id}:read`)).toBe(false);

    chat.post("a", open.id, "hello everyone");
    chat.post("a", open.id, "hey @b look");
    expect(chat.wakeups("b", 0).map((p) => p.body)).toEqual(["hey @b look"]);
  });

  test("threads only nest one level", () => {
    const { perms, chat } = world();
    perms.grant(USER, "a", "channel:create");
    const c = chat.create("a", "t", "open");
    const root = chat.post("a", c.id, "root");
    const reply = chat.post("a", c.id, "reply", { thread: root.id });
    expect(() => chat.post("a", c.id, "nested", { thread: reply.id })).toThrow(Denied);
    expect(chat.read("a", c.id, { thread: root.id })).toHaveLength(2);
  });
});

describe("wiki", () => {
  test("locks beat every right, scratch is private, votes decide", async () => {
    const { store, perms } = world();
    const wiki = new Wiki(store, perms, scriptedJudges(), join(tmp(), "vault"));
    await wiki.init({ "rules.md": "# rules\n", "docs/a.md": "# a\n\nlinks to [[rules]]\n" }, ["rules.md"]);
    perms.grant(USER, "x", "*");
    await expect(wiki.write("x", "rules.md", "pwned", "m")).rejects.toThrow(Denied);
    expect(() => wiki.propose("x", "rules.md", "pwned", "a long enough reason here")).toThrow(Denied);
    expect(() => wiki.setLock("x", "rules.md", false)).toThrow(Denied);

    perms.grant(USER, "y", "wiki:read");
    await wiki.write("y", "scratch/y/n.md", "mine", "m");
    await expect(wiki.write("y", "scratch/x/n.md", "theirs", "m")).rejects.toThrow(Denied);
    expect(() => wiki.norm("../etc/passwd")).toThrow(Denied);
    expect(await wiki.backlinks("y", "rules.md")).toEqual(["docs/a.md"]);

    const decided = new Promise((res) => (wiki.onDecided = res));
    wiki.propose("x", "docs/a.md", "# a\n\nbetter\n", "Clearer wording for the intro.");
    expect(((await decided) as any).status).toBe("approved");
    expect(await wiki.read(USER, "docs/a.md")).toBe("# a\n\nbetter\n");
    expect((await wiki.history("docs/a.md"))[0]!.author).toBe("x");
  });
});

// The demos double as integration tests, with a fake sandbox so Docker isn't needed.
const fakeSandboxes: SandboxFactory = {
  async create(): Promise<Sandbox> {
    return {
      async exec(cmd) {
        const out = cmd.includes("wordfreq") ? "3 the\n2 and\n1 hat\n" : "uid=1000 gid=1000\nRead-only file system\nnetwork: no\n";
        return { code: 0, stdout: out, stderr: "", timedOut: false };
      },
      async destroy() {},
    };
  },
  async shutdown() {},
};

for (const name of ["build", "redteam"]) {
  test(`demo: ${name}`, async () => {
    const demo = DEMOS[name]!;
    const { failures } = await runDemo(demo, { dir: tmp(), model: new ScriptedModel(demo.script, 5), judges: scriptedJudges(), sandboxes: fakeSandboxes, providers: standardProviders() });
    expect(failures).toEqual([]);
  }, 30_000);
}
