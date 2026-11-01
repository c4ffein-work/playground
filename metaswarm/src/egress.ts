// Egress for agent containers: an HTTP(S) proxy that lets each agent reach
// exactly the hosts it holds `net:<host>` for, and nothing else.
//
// Layout, per run:
// - an *internal* docker network `metaswarm-<runId>`: no route out;
// - agent containers join it, with HTTP(S)_PROXY=http://<agent>:<token>@proxy:3128;
// - one proxy container (src/egress-proxy.ts under node) on that network
//   (alias `proxy`) and on the default bridge, which is its only way out;
// - the harness listens on a unix socket in a private temp dir that is
//   bind-mounted into the proxy container. For every request the proxy asks
//   POST /check {agent, token, host}; the harness maps token -> agent, asks
//   the rights system *now*, and reports the decision. No state is cached in
//   the proxy, so grants and revokes apply to the next request, and open
//   tunnels are re-checked every few seconds and cut on revoke.
//
// Why a unix socket: nothing listens on a host TCP port (no need for
// host-gateway, no firewall rules, nothing other containers can reach), and
// the proxy holds no secrets: it only forwards the credential it was given.
// It needs the docker daemon to be on the same machine as the harness.
import { randomBytes } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { normHost } from "./egress-proxy.ts";

export interface EgressOptions {
  runId: string;
  /** Rights lookup, called on every request (e.g. `(a, r) => perms.can(a, r)`). */
  can: (agent: string, right: string) => boolean;
  /** Called for every allow/deny, and when a re-check cuts an open connection. */
  onDecision?: (agent: string, host: string, allowed: boolean) => void;
  /** Image with node >= 22.18 (type stripping). Default mirror.gcr.io/library/node:22-alpine. */
  image?: string;
  /** Let the proxy reach private/loopback addresses (tests against local servers only). */
  allowPrivate?: boolean;
  /** Extra /etc/hosts entries in the proxy container, name -> ip (tests). */
  extraHosts?: Record<string, string>;
  /** Extra CA bundle (PEM) the proxy trusts when it makes TLS connections itself (absolute https:// requests). */
  caFile?: string;
  /** How often open connections are re-checked, ms. Default 5000. */
  recheckMs?: number;
  /** How long start() waits for the proxy to come up, ms. Default 30000. */
  startTimeoutMs?: number;
}

const SCRIPT = join(import.meta.dir, "egress-proxy.ts");

async function docker(args: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
  const p = Bun.spawn(["docker", ...args], { stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  const [stdout, stderr, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  return { code, stdout, stderr };
}

export class Egress {
  /** The internal network agent containers join. */
  readonly network: string;
  readonly proxyName: string;
  readonly port = 3128;
  private tokens = new Map<string, string>(); // token -> agent
  private byAgent = new Map<string, string>(); // agent -> token
  private dir: string | null = null;
  private server: ReturnType<typeof Bun.serve> | null = null;
  private started: Promise<void> | null = null;
  private onReady: (() => void) | null = null;

  constructor(private opts: EgressOptions) {
    const safe = opts.runId.replace(/[^a-zA-Z0-9_.-]/g, "_");
    this.network = `metaswarm-${safe}`;
    this.proxyName = `metaswarm-${safe}-proxy`;
  }

  /** Create the network, the check socket and the proxy container. Idempotent. */
  start(): Promise<void> {
    this.started ??= this.boot().catch(async (e) => {
      await this.stop();
      throw e;
    });
    return this.started;
  }

  private async boot() {
    const o = this.opts;
    this.dir = mkdtempSync(join(tmpdir(), "metaswarm-egress-"));
    chmodSync(this.dir, 0o711); // the proxy runs as uid 1000
    const sock = join(this.dir, "check.sock");
    const ready = new Promise<void>((r) => (this.onReady = r));
    this.server = Bun.serve({ unix: sock, fetch: (req) => this.handle(req) });
    chmodSync(sock, 0o666);

    await docker(["rm", "-f", this.proxyName]);
    await docker(["network", "rm", this.network]);
    const n = await docker(["network", "create", "--internal", "--label", `metaswarm.run=${o.runId}`, this.network]);
    if (n.code !== 0) throw new Error(`docker network create failed: ${n.stderr}`);
    const hosts = Object.entries(o.extraHosts ?? {}).flatMap(([h, ip]) => ["--add-host", `${h}:${ip}`]);
    const r = await docker([
      "run", "-d", "--name", this.proxyName,
      "--network", this.network, "--network-alias", "proxy",
      "--read-only", "--user", "1000:1000", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
      "--memory", "128m", "--cpus", "0.5", "--pids-limit", "64",
      "-v", `${this.dir}:/run/egress`, "-v", `${SCRIPT}:/proxy/egress-proxy.ts:ro`,
      "-e", "METASWARM_EGRESS_SOCKET=/run/egress/check.sock",
      "-e", `METASWARM_EGRESS_PORT=${this.port}`,
      "-e", `METASWARM_EGRESS_ALLOW_PRIVATE=${o.allowPrivate ? 1 : 0}`,
      "-e", `METASWARM_EGRESS_RECHECK_MS=${o.recheckMs ?? 5000}`,
      ...hosts,
      ...(o.caFile ? ["-v", `${o.caFile}:/proxy/ca.pem:ro`, "-e", "NODE_EXTRA_CA_CERTS=/proxy/ca.pem"] : []),
      "--label", `metaswarm.run=${o.runId}`,
      o.image ?? "mirror.gcr.io/library/node:22-alpine",
      "node", "--experimental-strip-types", "--no-warnings", "/proxy/egress-proxy.ts",
    ]);
    if (r.code !== 0) throw new Error(`proxy container failed to start: ${r.stderr}`);
    // Its only way out. Connected after creation so `proxy` resolves on the internal network.
    const c = await docker(["network", "connect", "bridge", this.proxyName]);
    if (c.code !== 0) throw new Error(`docker network connect failed: ${c.stderr}`);

    const ms = o.startTimeoutMs ?? 30_000;
    let timer: Timer | undefined;
    const late = new Promise<never>((_, rej) => (timer = setTimeout(() => rej(new Error(`egress proxy not ready after ${ms}ms`)), ms)));
    try {
      await Promise.race([ready, late]);
    } catch (e) {
      const logs = await docker(["logs", "--tail", "20", this.proxyName]);
      throw new Error(`${(e as Error).message}\n${logs.stdout}${logs.stderr}`);
    } finally {
      clearTimeout(timer);
    }
  }

  private async handle(req: Request): Promise<Response> {
    const path = new URL(req.url).pathname;
    if (path === "/ready") {
      this.onReady?.();
      return Response.json({ ok: true });
    }
    if (path !== "/check") return new Response("not found", { status: 404 });
    const b = (await req.json().catch(() => ({}))) as { agent?: string; token?: string; host?: string; recheck?: boolean };
    return Response.json(this.check(String(b.agent ?? ""), String(b.token ?? ""), String(b.host ?? ""), !!b.recheck));
  }

  /** The decision the proxy asks for. Public so it can be tested without docker. */
  check(agent: string, token: string, rawHost: string, recheck = false): { allowed: boolean; agent?: string } {
    const owner = this.tokens.get(token);
    if (!owner || owner !== agent) return { allowed: false }; // unknown or stale credential: nobody to report
    const host = normHost(rawHost);
    const allowed = host !== null && this.opts.can(owner, `net:${host}`);
    // Re-checks of open connections are only worth reporting when they cut one.
    if (!recheck || !allowed) this.opts.onDecision?.(owner, host ?? rawHost, allowed);
    return { allowed, agent: owner };
  }

  /** A fresh credential for an agent's container (replacing any earlier one); returns the env to set. */
  register(agent: string): Record<string, string> {
    this.unregister(agent);
    const token = randomBytes(24).toString("hex");
    this.tokens.set(token, agent);
    this.byAgent.set(agent, token);
    const url = `http://${encodeURIComponent(agent)}:${token}@proxy:${this.port}`;
    return { HTTP_PROXY: url, HTTPS_PROXY: url, http_proxy: url, https_proxy: url, NO_PROXY: "", no_proxy: "" };
  }

  /** Invalidate an agent's credential (its container is going away). */
  unregister(agent: string) {
    const t = this.byAgent.get(agent);
    if (t !== undefined) this.tokens.delete(t);
    this.byAgent.delete(agent);
  }

  /** Remove the proxy container and the network. Agent containers must be gone first. */
  async stop() {
    await docker(["rm", "-f", this.proxyName]);
    await docker(["network", "rm", this.network]);
    this.server?.stop(true);
    this.server = null;
    if (this.dir) rmSync(this.dir, { recursive: true, force: true });
    this.dir = null;
    this.tokens.clear();
    this.byAgent.clear();
    this.started = null;
  }
}
