// The egress proxy that runs inside the proxy container (node, type-stripped).
// It must stay self-contained: only node builtins, only erasable TypeScript.
//
//   agent container --(internal network)--> proxy:3128 --(bridge)--> internet
//                                             |
//                                             +-- unix socket --> harness: "may <token> reach <host>?"
//
// Every request (CONNECT tunnel or absolute-URI HTTP request) carries
// Proxy-Authorization: Basic <agent>:<token>. The proxy asks the harness over
// the mounted unix socket before connecting anywhere, so grants and revokes
// apply to the very next request. Open tunnels are re-checked periodically
// and cut when the right goes away. The proxy also refuses private, loopback
// and link-local destinations (after DNS resolution, connecting to the
// address it checked), so `net:*` means "the public internet", not the host.
//
// The helpers below are also imported by the harness (src/net.ts).
import dns from "node:dns/promises";
import http from "node:http";
import https from "node:https";
import net from "node:net";

// --- address classification (shared with src/net.ts) -------------------------

function v4ToInt(ip: string): number {
  return ip.split(".").reduce((a, o) => a * 256 + Number(o), 0);
}

const V4_BLOCKED: [string, number][] = [
  ["0.0.0.0", 8], ["10.0.0.0", 8], ["100.64.0.0", 10], ["127.0.0.0", 8], ["169.254.0.0", 16],
  ["172.16.0.0", 12], ["192.0.0.0", 24], ["192.0.2.0", 24], ["192.168.0.0", 16], ["198.18.0.0", 15],
  ["198.51.100.0", 24], ["203.0.113.0", 24], ["224.0.0.0", 4], ["240.0.0.0", 4],
];

function v6Words(ip: string): number[] | null {
  let s = ip.toLowerCase();
  const pct = s.indexOf("%");
  if (pct >= 0) s = s.slice(0, pct);
  // Trailing dotted quad (::ffff:1.2.3.4).
  const m = s.match(/^(.*:)(\d+\.\d+\.\d+\.\d+)$/);
  if (m) {
    const n = v4ToInt(m[2]!);
    s = `${m[1]}${(n >>> 16).toString(16)}:${(n & 0xffff).toString(16)}`;
  }
  const [head, tail, extra] = s.split("::");
  if (extra !== undefined) return null;
  const h = head ? head.split(":") : [];
  const t = tail !== undefined ? (tail ? tail.split(":") : []) : [];
  const fill = tail !== undefined ? 8 - h.length - t.length : 0;
  if (fill < 0) return null;
  const words = [...h, ...Array(fill).fill("0"), ...t].map((w) => parseInt(w, 16));
  return words.length === 8 && words.every((w) => w >= 0 && w <= 0xffff) ? words : null;
}

/** True for addresses an agent must never reach: private, loopback, link-local, CGNAT, multicast, reserved. */
export function isPrivateIp(ip: string): boolean {
  const fam = net.isIP(ip.replace(/%.*$/, ""));
  if (fam === 4) {
    const n = v4ToInt(ip);
    return V4_BLOCKED.some(([base, bits]) => (n >>> (32 - bits)) === (v4ToInt(base) >>> (32 - bits)));
  }
  if (fam !== 6) return true; // not an IP: refuse rather than guess
  const w = v6Words(ip);
  if (!w) return true;
  if (w.every((x) => x === 0)) return true; // ::
  if (w.slice(0, 7).every((x) => x === 0) && w[7] === 1) return true; // ::1
  const embedded = (hi: number, lo: number) => isPrivateIp(`${hi >>> 8}.${hi & 255}.${lo >>> 8}.${lo & 255}`);
  if (w.slice(0, 5).every((x) => x === 0) && w[5] === 0xffff) return embedded(w[6]!, w[7]!); // ::ffff:v4
  if (w.slice(0, 6).every((x) => x === 0)) return embedded(w[6]!, w[7]!); // ::v4 (deprecated)
  if (w[0] === 0x64 && w[1] === 0xff9b) return embedded(w[6]!, w[7]!); // NAT64
  if (w[0] === 0x2002) return embedded(w[1]!, w[2]!); // 6to4
  if ((w[0]! & 0xfe00) === 0xfc00) return true; // fc00::/7 unique local
  if ((w[0]! & 0xffc0) === 0xfe80) return true; // fe80::/10 link-local
  if ((w[0]! & 0xffc0) === 0xfec0) return true; // fec0::/10 site-local
  if ((w[0]! & 0xff00) === 0xff00) return true; // multicast
  if (w[0] === 0x2001 && w[1] === 0xdb8) return true; // documentation
  return false;
}

const LABEL = "[a-z0-9_](?:[a-z0-9_-]{0,61}[a-z0-9_])?";
const HOST_RE = new RegExp(`^${LABEL}(?:\\.${LABEL})*$`);

/** Lower-case, strip brackets and a trailing dot; null if it isn't a plausible hostname or IP. */
export function normHost(raw: string): string | null {
  let h = raw.trim().toLowerCase();
  if (h.startsWith("[") && h.endsWith("]")) h = h.slice(1, -1);
  if (h.endsWith(".")) h = h.slice(0, -1);
  if (!h || h.length > 253) return null;
  if (net.isIP(h)) return h;
  return HOST_RE.test(h) ? h : null;
}

/** Name-level refusal, usable without DNS: localhost, mDNS/internal zones, private IP literals. */
export function refusedByName(host: string): string | null {
  if (net.isIP(host)) return isPrivateIp(host) ? `${host} is a private or local address` : null;
  if (host === "localhost" || /\.(localhost|local|internal|localdomain|home\.arpa|lan)$/.test(host) || !host.includes("."))
    return `${host} looks like a local name`;
  return null;
}

// --- the proxy ----------------------------------------------------------------

const HOP = new Set(["connection", "keep-alive", "proxy-authorization", "proxy-authenticate", "proxy-connection", "te", "trailer", "transfer-encoding", "upgrade"]);

interface Who {
  agent: string;
  token: string;
}

function main() {
  const SOCK = process.env.METASWARM_EGRESS_SOCKET!;
  const PORT = Number(process.env.METASWARM_EGRESS_PORT ?? 3128);
  const ALLOW_PRIVATE = process.env.METASWARM_EGRESS_ALLOW_PRIVATE === "1";
  const RECHECK_MS = Number(process.env.METASWARM_EGRESS_RECHECK_MS ?? 5000);

  function rpc(path: string, body: unknown): Promise<any> {
    return new Promise((resolve, reject) => {
      const data = JSON.stringify(body);
      const req = http.request(
        { socketPath: SOCK, path, method: "POST", headers: { "content-type": "application/json", "content-length": Buffer.byteLength(data) }, timeout: 5000 },
        (res) => {
          let s = "";
          res.on("data", (d) => (s += d));
          res.on("end", () => {
            try {
              resolve(JSON.parse(s));
            } catch (e) {
              reject(e);
            }
          });
        },
      );
      req.on("timeout", () => req.destroy(new Error("harness timeout")));
      req.on("error", reject);
      req.end(data);
    });
  }

  async function allowed(who: Who, host: string, recheck = false): Promise<boolean> {
    try {
      const r = await rpc("/check", { agent: who.agent, token: who.token, host, recheck });
      return r?.allowed === true;
    } catch {
      return false; // harness unreachable: fail closed
    }
  }

  function auth(header: string | undefined): Who | null {
    const m = header?.match(/^Basic\s+(\S+)$/i);
    if (!m) return null;
    const dec = Buffer.from(m[1]!, "base64").toString();
    const i = dec.indexOf(":");
    return i > 0 ? { agent: dec.slice(0, i), token: dec.slice(i + 1) } : null;
  }

  async function resolve(host: string): Promise<string> {
    const addrs = net.isIP(host) ? [host] : (await dns.lookup(host, { all: true })).map((a) => a.address);
    if (!addrs.length) throw new Error(`cannot resolve ${host}`);
    if (!ALLOW_PRIVATE) {
      const bad = addrs.find(isPrivateIp);
      if (bad) throw new Error(`${host} resolves to private address ${bad}`);
    }
    return addrs[0]!;
  }

  /** Auth + rights + address checks. Returns the address to connect to, or an [status, message] refusal. */
  async function vet(authHeader: string | undefined, rawHost: string): Promise<{ ip: string; host: string; who: Who } | [number, string]> {
    const who = auth(authHeader);
    if (!who) return [407, "metaswarm egress: proxy credentials required"];
    const host = normHost(rawHost);
    if (!host) return [400, `metaswarm egress: bad host ${JSON.stringify(rawHost)}`];
    // Rights first, so a denied host costs no DNS lookup.
    if (!(await allowed(who, host))) return [403, `metaswarm egress: ${who.agent} may not reach ${host} (needs net:${host})`];
    try {
      return { ip: await resolve(host), host, who };
    } catch (e) {
      return [502, `metaswarm egress: ${(e as Error).message}`];
    }
  }

  /** Cut a connection as soon as its right is revoked. */
  function watch(who: Who, host: string, close: () => void): () => void {
    const t = setInterval(async () => {
      if (!(await allowed(who, host, true))) close();
    }, RECHECK_MS);
    return () => clearInterval(t);
  }

  const reply = (status: number, msg: string, extra: Record<string, string> = {}) => {
    const body = msg + "\n";
    const head = Object.entries({ "content-type": "text/plain", "content-length": String(Buffer.byteLength(body)), connection: "close", ...extra })
      .map(([k, v]) => `${k}: ${v}\r\n`)
      .join("");
    return `HTTP/1.1 ${status} ${http.STATUS_CODES[status]}\r\n${head}\r\n${body}`;
  };
  const authHdr: Record<string, string> = { "proxy-authenticate": 'Basic realm="metaswarm"' };

  const server = http.createServer(async (req, res) => {
    let url: URL;
    try {
      url = new URL(req.url ?? "");
      if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error();
    } catch {
      res.writeHead(400, { "content-type": "text/plain" }).end("metaswarm egress: absolute http(s) URL required\n");
      return;
    }
    const v = await vet(req.headers["proxy-authorization"], url.hostname);
    if (Array.isArray(v)) {
      res.writeHead(v[0], { "content-type": "text/plain", ...(v[0] === 407 ? authHdr : {}) }).end(v[1] + "\n");
      return;
    }
    const headers: Record<string, string | string[]> = {};
    for (const [k, val] of Object.entries(req.headers)) if (!HOP.has(k) && val !== undefined) headers[k] = val;
    headers.host = url.host;
    const lib = url.protocol === "https:" ? https : http;
    const up = lib.request(
      {
        host: v.ip, port: url.port || (url.protocol === "https:" ? 443 : 80), method: req.method, path: url.pathname + url.search,
        headers, agent: false, timeout: 20_000, ...(url.protocol === "https:" && !net.isIP(v.host) ? { servername: v.host } : {}),
      },
      (ur) => {
        const h: Record<string, string | string[]> = {};
        for (const [k, val] of Object.entries(ur.headers)) if (!HOP.has(k) && val !== undefined) h[k] = val;
        res.writeHead(ur.statusCode ?? 502, h);
        ur.pipe(res);
      },
    );
    const stop = watch(v.who, v.host, () => {
      up.destroy();
      res.destroy();
    });
    res.on("close", stop);
    up.on("timeout", () => up.destroy(new Error("upstream timeout")));
    up.on("error", (e) => {
      if (!res.headersSent) res.writeHead(502, { "content-type": "text/plain" }).end(`metaswarm egress: ${e.message}\n`);
      else res.destroy();
    });
    req.pipe(up);
  });

  // busybox wget half-closes its socket after an https:// request to a proxy;
  // by default node would then drop the connection before we can answer.
  (server as unknown as { httpAllowHalfOpen: boolean }).httpAllowHalfOpen = true;

  server.on("connect", async (req: http.IncomingMessage, client: net.Socket, head: Buffer) => {
    client.on("error", () => {});
    const m = (req.url ?? "").match(/^(\[[^\]]+\]|[^:]+):(\d{1,5})$/);
    if (!m) {
      client.end(reply(400, "metaswarm egress: CONNECT host:port required"));
      return;
    }
    const v = await vet(req.headers["proxy-authorization"], m[1]!);
    if (Array.isArray(v)) {
      client.end(reply(v[0], v[1], v[0] === 407 ? authHdr : {}));
      return;
    }
    const upstream = net.connect({ host: v.ip, port: Number(m[2]), timeout: 20_000 });
    const stop = watch(v.who, v.host, () => {
      upstream.destroy();
      client.destroy();
    });
    let connected = false;
    upstream.once("connect", () => {
      connected = true;
      upstream.setTimeout(0);
      client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      if (head.length) upstream.write(head);
      upstream.pipe(client);
      client.pipe(upstream);
    });
    upstream.on("timeout", () => upstream.destroy(new Error("connect timeout")));
    upstream.on("error", (e) => {
      if (!connected) client.end(reply(502, `metaswarm egress: ${e.message}`));
      else client.destroy();
    });
    client.on("close", () => {
      stop();
      upstream.destroy();
    });
    upstream.on("close", () => {
      stop();
      if (connected) client.destroy(); // otherwise the 502 above is still being written
    });
  });

  server.listen(PORT, "0.0.0.0", () => {
    rpc("/ready", {}).catch(() => {});
  });
}

if (process.env.METASWARM_EGRESS_SOCKET) main();
