import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Egress } from "../src/egress.ts";
import { isPrivateIp, normHost, refusedByName } from "../src/egress-proxy.ts";
import { checkAddress, htmlToText, webProvider } from "../src/net.ts";
import { Denied, Perms, USER } from "../src/perms.ts";
import { DockerSandboxes, type Sandbox } from "../src/sandbox.ts";
import { Store } from "../src/store.ts";
import type { ToolContext } from "../src/tools.ts";

const perms = () => new Perms(new Store(":memory:"));
const ctxFor = (p: Perms, agent: string, signal = new AbortController().signal) =>
  ({ agent, signal, world: { perms: p } }) as unknown as ToolContext;

describe("addresses", () => {
  test("private, loopback, link-local and friends are refused", () => {
    for (const ip of ["127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "::1", "::", "fe80::1", "fd00::1", "::ffff:127.0.0.1", "::ffff:7f00:1", "64:ff9b::a00:1", "2002:a00:1::"])
      expect(isPrivateIp(ip)).toBe(true);
    for (const ip of ["93.184.216.34", "1.1.1.1", "172.32.0.1", "2606:4700::1111", "::ffff:8.8.8.8"]) expect(isPrivateIp(ip)).toBe(false);
  });

  test("hostnames are normalised and local names refused without DNS", () => {
    expect(normHost("Example.COM.")).toBe("example.com");
    expect(normHost("[::1]")).toBe("::1");
    expect(normHost("evil.com/x")).toBeNull();
    expect(normHost("a b")).toBeNull();
    for (const h of ["localhost", "foo.localhost", "db.internal", "printer.local", "intranet", "10.0.0.1"]) expect(refusedByName(h)).not.toBeNull();
    expect(refusedByName("example.com")).toBeNull();
    expect(refusedByName("8.8.8.8")).toBeNull();
  });

  test("resolution: private answers refused; DNS failure tolerated only behind a proxy", async () => {
    const priv = { lookup: async () => ["10.0.0.5"] };
    await expect(checkAddress(new URL("https://sneaky.example/"), priv)).rejects.toThrow(/private address 10.0.0.5/);
    const fail = { lookup: async () => Promise.reject(new Error("ENOTFOUND")) };
    const saved = { ...process.env };
    try {
      for (const k of ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]) delete process.env[k];
      await expect(checkAddress(new URL("https://nowhere.example/"), fail)).rejects.toThrow(/cannot resolve/);
      process.env.HTTPS_PROXY = "http://127.0.0.1:9";
      process.env.NO_PROXY = "";
      expect(await checkAddress(new URL("https://nowhere.example/"), fail)).toBeUndefined();
      // Name checks still apply.
      await expect(checkAddress(new URL("https://metadata.internal/"), fail)).rejects.toThrow(Denied);
    } finally {
      process.env = saved;
    }
  });
});

describe("html to text", () => {
  test("strips scripts, styles and tags, keeps links", () => {
    const html = `<!doctype html><html><head><title>T &amp; t</title><style>p{color:red}</style><script>alert(1)</script></head>
      <body><h1>Hello</h1><p>Some   <b>bold</b>
      text &lt;3 &#x263A;</p><ul><li>one</li><li><a href="/two?a=1&amp;b=2">two</a></li></ul>
      <script type="x">nope()</script><a href="https://x.org/">https://x.org/</a></body></html>`;
    const t = htmlToText(html, "https://ex.com/dir/");
    expect(t).toContain("# T & t");
    expect(t).toContain("# Hello");
    expect(t).toContain("Some bold text <3 ☺");
    expect(t).toContain("- one\n- two (https://ex.com/two?a=1&b=2)");
    expect(t).toContain("https://x.org/");
    expect(t).not.toMatch(/alert|nope|color|<b>/);
    expect(t).not.toMatch(/\n{3}/);
  });
});

describe("web_fetch", () => {
  let server: ReturnType<typeof Bun.serve>;
  let base: string;
  const decisions: [string, string, boolean][] = [];
  beforeAll(() => {
    server = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch(req) {
        const u = new URL(req.url);
        if (u.pathname === "/page") return new Response(`<html><body><p>hi <a href="/next">next</a></p><script>x()</script></body></html>`, { headers: { "content-type": "text/html; charset=utf-8" } });
        if (u.pathname === "/json") return Response.json({ a: 1 });
        if (u.pathname === "/hop") return Response.redirect(`http://127.0.0.1:${server.port}/page`, 302);
        if (u.pathname === "/away") return Response.redirect(`http://localhost:${server.port}/page`, 302);
        if (u.pathname === "/loop") return Response.redirect(`/loop`, 302);
        if (u.pathname === "/big") return new Response("x".repeat(3_000_000), { headers: { "content-type": "text/plain" } });
        if (u.pathname === "/bin") return new Response(new Uint8Array(100), { headers: { "content-type": "image/png" } });
        if (u.pathname === "/slow") return Bun.sleep(3000).then(() => new Response("late"));
        return new Response("nope", { status: 404 });
      },
    });
    base = `http://127.0.0.1:${server.port}`;
  });
  afterAll(() => server.stop(true));

  const setup = (opts = {}) => {
    const p = perms();
    p.grant(USER, "a", "net:127.0.0.1");
    const web = webProvider({ allowPrivate: true, onDecision: (...d) => decisions.push(d), ...opts });
    return { p, web };
  };

  test("allowed host: status, final url, type, readable text", async () => {
    const { p, web } = setup();
    const out = await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/page` });
    expect(out).toContain("status: 200");
    expect(out).toContain(`url: ${base}/page`);
    expect(out).toContain("content-type: text/html");
    expect(out).toContain(`hi next (${base}/next)`);
    expect(out).not.toContain("x()");
    expect(await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/json` })).toContain('{"a":1}');
    expect(await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/bin` })).toContain("binary content, 100 bytes");
    expect(await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/missing` })).toContain("status: 404");
    expect(web.prompt!()).toContain("net:<host>");
  });

  test("denied host is refused before any request", async () => {
    const { p, web } = setup();
    await expect(web.run(ctxFor(p, "b"), "web_fetch", { url: `${base}/page` })).rejects.toThrow(/b lacks right "net:127.0.0.1"/);
    expect(decisions).toContainEqual(["b", "127.0.0.1", false]);
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: "file:///etc/passwd" })).rejects.toThrow(Denied);
  });

  test("redirects are followed and every hop is checked", async () => {
    const { p, web } = setup();
    const out = await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/hop` });
    expect(out).toContain(`redirects: ${base}/hop -> ${base}/page`);
    expect(out).toContain("hi next");
    // /away redirects to localhost, which "a" holds no right for.
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/away` })).rejects.toThrow(/lacks right "net:localhost"/);
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/loop` })).rejects.toThrow(/too many redirects/);
  });

  test("private addresses refused even with the right", async () => {
    const p = perms();
    p.grant(USER, "a", "net:*");
    const web = webProvider();
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/page` })).rejects.toThrow(/private or local/);
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: `http://localhost:${server.port}/page` })).rejects.toThrow(/local name/);
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: "http://169.254.169.254/latest/meta-data/" })).rejects.toThrow(/private/);
    await expect(web.run(ctxFor(p, "a"), "web_fetch", { url: "http://[::ffff:127.0.0.1]/" })).rejects.toThrow(/private/);
    const hidden = webProvider({ lookup: async () => ["127.0.0.1"] });
    await expect(hidden.run(ctxFor(p, "a"), "web_fetch", { url: "http://rebind.example/" })).rejects.toThrow(/resolves to private address/);
  });

  test("max_chars, download cap, timeout and abort", async () => {
    const { p, web } = setup({ maxBytes: 1000, timeoutMs: 500 });
    const out = await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/big`, max_chars: 100 });
    expect(out).toContain("download stopped at 1000 bytes");
    expect(out).toContain("[900 chars cut]");
    expect(await web.run(ctxFor(p, "a"), "web_fetch", { url: `${base}/slow` })).toContain("timed out");
    const ac = new AbortController();
    const pending = setup().web.run(ctxFor(p, "a", ac.signal), "web_fetch", { url: `${base}/slow` });
    setTimeout(() => ac.abort(), 100);
    expect(await pending).toBe("interrupted");
  });
});

describe("egress check (no docker)", () => {
  test("token maps to agent; rights are read at request time", () => {
    const p = perms();
    const seen: [string, string, boolean][] = [];
    const eg = new Egress({ runId: "unit", can: (a, r) => p.can(a, r), onDecision: (...d) => seen.push(d) });
    const env = eg.register("alice");
    const token = new URL(env.HTTPS_PROXY!).password;
    expect(eg.check("alice", token, "pypi.org").allowed).toBe(false);
    const g = p.grant(USER, "alice", "net:*.pypi.org");
    expect(eg.check("alice", token, "files.pypi.org").allowed).toBe(true);
    expect(eg.check("alice", token, "pypi.org.evil.com").allowed).toBe(false);
    expect(eg.check("bob", token, "files.pypi.org").allowed).toBe(false); // credential bound to alice
    p.revoke(USER, g.id);
    expect(eg.check("alice", token, "files.pypi.org").allowed).toBe(false);
    expect(seen).toEqual([["alice", "pypi.org", false], ["alice", "files.pypi.org", true], ["alice", "pypi.org.evil.com", false], ["alice", "files.pypi.org", false]]);
    eg.register("alice"); // new container, new token: the old one is dead
    p.grant(USER, "alice", "net:*");
    expect(eg.check("alice", token, "example.com").allowed).toBe(false);
  });
});

// --- real containers ------------------------------------------------------------

const hasDocker = Bun.spawnSync(["docker", "info"], { stdout: "ignore", stderr: "ignore" }).exitCode === 0;
const IMAGE = process.env.METASWARM_TEST_IMAGE ?? "mirror.gcr.io/library/alpine:3.20";
const NODE = "mirror.gcr.io/library/node:22-alpine";

const TARGET_JS = `
require("http").createServer((req, res) => {
  if (req.url === "/slow") {
    let i = 0;
    const t = setInterval(() => { res.write("x\\n"); if (++i >= 40) { clearInterval(t); res.end(); } }, 200);
    req.on("close", () => clearInterval(t));
    return;
  }
  res.end("hello from target " + req.headers.host + "\\n");
}).listen(8080);
const fs = require("fs");
if (fs.existsSync("/tls/key.pem"))
  require("https").createServer({ key: fs.readFileSync("/tls/key.pem"), cert: fs.readFileSync("/tls/cert.pem") },
    (req, res) => res.end("tls hello " + req.headers.host + "\\n")).listen(8443);`;

describe.skipIf(!hasDocker)("egress proxy (docker)", () => {
  const runId = `nettest-${process.pid}-${Date.now().toString(36)}`;
  const target = `${runId}-target`;
  const p = perms();
  const decisions: [string, string, boolean][] = [];
  let sandboxes: DockerSandboxes;
  let egress: Egress;
  let sb: Sandbox;
  let targetIp = "";
  let grantId = 0;

  const sh = async (cmd: string, timeoutS = 20) => sb.exec(cmd, { timeoutS });
  const wget = (url: string) => sh(`wget -q -O- -T 5 ${url} 2>&1; echo "rc=$?"`);

  // A self-signed cert for the https target, trusted by the proxy through caFile.
  const tls = mkdtempSync(join(tmpdir(), "metaswarm-tls-"));
  const hasTls =
    Bun.spawnSync(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", join(tls, "key.pem"), "-out", join(tls, "cert.pem"),
      "-days", "1", "-subj", "/CN=allowed.test", "-addext", "subjectAltName=DNS:allowed.test"], { stdout: "ignore", stderr: "ignore" }).exitCode === 0;

  beforeAll(async () => {
    chmodSync(tls, 0o755);
    if (hasTls) for (const f of ["key.pem", "cert.pem"]) chmodSync(join(tls, f), 0o644);
    const t = Bun.spawnSync(["docker", "run", "-d", "--rm", "--name", target, "--label", `metaswarm.run=${runId}`, "-v", `${tls}:/tls:ro`, NODE, "node", "-e", TARGET_JS]);
    if (t.exitCode !== 0) throw new Error(t.stderr.toString());
    targetIp = Bun.spawnSync(["docker", "inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", target]).stdout.toString().trim();
    egress = new Egress({
      runId, can: (a, r) => p.can(a, r), onDecision: (...d) => decisions.push(d),
      allowPrivate: true, // the target is a container on the bridge, i.e. a private address
      extraHosts: { "allowed.test": targetIp, "denied.test": targetIp },
      recheckMs: 300,
      ...(hasTls ? { caFile: join(tls, "cert.pem") } : {}),
    });
    sandboxes = new DockerSandboxes({ image: IMAGE, runId, egress });
    p.grant(USER, "alice", "bash");
    grantId = p.grant(USER, "alice", "net:allowed.test").id;
    sb = await sandboxes.create("alice");
    // The target needs a moment to listen.
    for (let i = 0; i < 50 && !(await wget("http://allowed.test:8080/")).stdout.includes("hello"); i++) await Bun.sleep(200);
  }, 120_000);

  afterAll(async () => {
    await sandboxes?.shutdown();
    Bun.spawnSync(["docker", "rm", "-f", target]);
    rmSync(tls, { recursive: true, force: true });
  }, 60_000);

  test("allowed host passes through the proxy (plain HTTP and CONNECT)", async () => {
    const r = await wget("http://allowed.test:8080/");
    expect(r.stdout).toContain("hello from target allowed.test:8080");
    expect(decisions).toContainEqual(["alice", "allowed.test", true]);
    // CONNECT, as curl/pip do for https: open a tunnel by hand, speak HTTP inside it.
    const c = await sh(
      `auth=$(echo "$HTTPS_PROXY" | sed -E 's#^http://([^@]*)@.*#\\1#' | tr -d '\\n' | base64 | tr -d '\\n')
       (printf 'CONNECT allowed.test:8080 HTTP/1.1\\r\\nProxy-Authorization: Basic %s\\r\\n\\r\\n' "$auth"; sleep 1;
        printf 'GET / HTTP/1.0\\r\\nHost: tunnel\\r\\n\\r\\n'; sleep 1) | nc proxy 3128`,
    );
    expect(c.stdout).toContain("200 Connection Established");
    expect(c.stdout).toContain("hello from target tunnel");
  }, 30_000);

  test.skipIf(!hasTls)("https:// through the proxy (busybox sends absolute https URLs; the proxy does TLS and verifies)", async () => {
    const r = await wget("https://allowed.test:8443/");
    expect(r.stdout).toContain("tls hello allowed.test:8443");
  }, 30_000);

  test("denied host is blocked, with no credentials too", async () => {
    const r = await wget("http://denied.test:8080/");
    expect(r.stdout).not.toContain("hello");
    expect(r.stdout).toMatch(/403|Forbidden/);
    expect(decisions).toContainEqual(["alice", "denied.test", false]);
    const anon = await sh(`http_proxy=http://proxy:3128 wget -q -O- -T 5 http://allowed.test:8080/ 2>&1; echo "rc=$?"`);
    expect(anon.stdout).not.toContain("hello");
    expect(anon.stdout).toMatch(/407|Proxy/);
  }, 30_000);

  test("without the proxy, the container cannot get out", async () => {
    const r = await sh(`unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY; wget -q -O- -T 3 http://${targetIp}:8080/ 2>&1; echo "rc=$?"; wget -q -O- -T 3 http://1.1.1.1/ 2>&1; echo "rc=$?"`);
    expect(r.stdout).not.toContain("hello");
    expect(r.stdout.match(/rc=1/g)?.length).toBe(2);
    // Bypassing the env var with -Y off changes nothing either.
    const y = await sh(`wget -Y off -q -O- -T 3 http://${targetIp}:8080/ 2>&1; echo "rc=$?"`);
    expect(y.stdout).not.toContain("hello");
  }, 30_000);

  test("a revoke applies to the next request and cuts open tunnels", async () => {
    // Stream from /slow through a tunnel (8s worth of lines); revoke after ~1.5s.
    const streaming = sh(
      `auth=$(echo "$HTTPS_PROXY" | sed -E 's#^http://([^@]*)@.*#\\1#' | tr -d '\\n' | base64 | tr -d '\\n')
       (printf 'CONNECT allowed.test:8080 HTTP/1.1\\r\\nProxy-Authorization: Basic %s\\r\\n\\r\\n' "$auth"; sleep 0.5;
        printf 'GET /slow HTTP/1.0\\r\\nHost: x\\r\\n\\r\\n'; sleep 9) | nc proxy 3128 | grep -c '^x'`,
    );
    await Bun.sleep(1500);
    p.revoke(USER, grantId);
    const after = await wget("http://allowed.test:8080/");
    expect(after.stdout).not.toContain("hello");
    expect(after.stdout).toMatch(/403|Forbidden/);
    const lines = Number((await streaming).stdout.trim());
    expect(lines).toBeGreaterThan(0);
    expect(lines).toBeLessThan(20); // 40 without the revoke
    // Granting again works at once.
    grantId = p.grant(USER, "alice", "net:allowed.test").id;
    expect((await wget("http://allowed.test:8080/")).stdout).toContain("hello");
  }, 40_000);

  test("shutdown removes the proxy and the network", async () => {
    await sandboxes.shutdown();
    expect(Bun.spawnSync(["docker", "inspect", egress.proxyName]).exitCode).not.toBe(0);
    expect(Bun.spawnSync(["docker", "network", "inspect", egress.network]).exitCode).not.toBe(0);
  }, 60_000);
});
