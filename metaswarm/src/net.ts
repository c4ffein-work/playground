// web_fetch: the harness fetches a URL for an agent holding `net:<host>`.
//
// Every hop is checked: the right before any request, then the address
// (private, loopback and link-local destinations are refused after DNS
// resolution), and redirects are followed by hand so each new host is
// checked again. Responses are capped in size and time; HTML becomes text.
import dns from "node:dns/promises";
import { isIP } from "node:net";
import { z } from "zod";
import { isPrivateIp, normHost, refusedByName } from "./egress-proxy.ts";
import { Denied } from "./perms.ts";
import { parseInput, toolDefs, type ToolProvider } from "./tools.ts";

export interface WebOptions {
  /** Skip the private-address checks (tests against local servers only). */
  allowPrivate?: boolean;
  timeoutMs?: number; // whole fetch, all hops; default 20000
  maxBytes?: number; // download cap; default 2 MB
  maxRedirects?: number; // default 5
  /** Called for every rights decision, one per hop. */
  onDecision?: (agent: string, host: string, allowed: boolean) => void;
  /** Mention in the prompt that bash also reaches allowed hosts (egress proxy is on). */
  bashProxy?: boolean;
  /** DNS lookup; default node:dns. */
  lookup?: (host: string) => Promise<string[]>;
}

const DEFS = {
  web_fetch: [
    "Fetch a web page or API (http/https, GET) and get its status and text; HTML is converted to readable text with links as `text (url)`. Needs: net:<host>.",
    z.object({ url: z.string().min(1), max_chars: z.number().int().positive().max(200_000).optional() }),
  ],
} as const;

const defaultLookup = async (host: string) => (await dns.lookup(host, { all: true })).map((a) => a.address);

// --- proxy env (Bun's fetch honours HTTP(S)_PROXY / NO_PROXY) ----------------

function proxyFor(url: URL): string | undefined {
  const env = process.env;
  const p = url.protocol === "https:" ? env.HTTPS_PROXY ?? env.https_proxy : env.HTTP_PROXY ?? env.http_proxy;
  if (!p) return undefined;
  const host = url.hostname.toLowerCase();
  const no = (env.NO_PROXY ?? env.no_proxy ?? "").split(",").map((s) => s.trim().toLowerCase()).filter(Boolean);
  for (const n of no) {
    if (n === "*") return undefined;
    const d = n.replace(/^\*?\./, "");
    if (host === d || host.endsWith("." + d)) return undefined;
  }
  return p;
}

/** Throws Denied unless `url`'s host is a public destination (as far as we can tell from here). */
export async function checkAddress(url: URL, o: WebOptions): Promise<void> {
  if (o.allowPrivate) return;
  const host = normHost(url.hostname)!;
  const byName = refusedByName(host);
  if (byName) throw new Denied(`refused: ${byName}`);
  if (isIP(host)) return;
  let addrs: string[];
  try {
    addrs = await (o.lookup ?? defaultLookup)(host);
  } catch (e) {
    // Behind a proxy the harness may not resolve names itself; the proxy
    // does, and the name checks above already ran. Without one, give up.
    if (proxyFor(url)) return;
    throw new Denied(`cannot resolve ${host}: ${(e as Error).message}`);
  }
  const bad = addrs.find(isPrivateIp);
  if (bad) throw new Denied(`refused: ${host} resolves to private address ${bad}`);
}

// --- HTML -> text ---------------------------------------------------------------

const ENT: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", mdash: "—", ndash: "–", hellip: "…", copy: "©", laquo: "«", raquo: "»" };

function decode(s: string): string {
  return s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (m, e: string) => {
    if (e[0] === "#") {
      const n = e[1] === "x" || e[1] === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      return n > 0 && n < 0x110000 ? String.fromCodePoint(n) : m;
    }
    return ENT[e.toLowerCase()] ?? m;
  });
}

/** Readable text from HTML: no scripts/styles/tags, links kept as `text (url)`, whitespace collapsed. */
export function htmlToText(html: string, base?: string): string {
  let s = html
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<(script|style|noscript|template|svg|iframe|object|canvas)\b[\s\S]*?<\/\1\s*>/gi, "")
    .replace(/<head\b[\s\S]*?<\/head\s*>/gi, (h) => {
      const t = h.match(/<title\b[^>]*>([\s\S]*?)<\/title\s*>/i);
      return t ? `\n# ${t[1]}\n` : "";
    });
  s = s.replace(/\s+/g, " "); // source newlines are just spaces; structure comes from tags
  s = s.replace(/<a\b([^>]*)>([\s\S]*?)<\/a\s*>/gi, (_, attrs: string, inner: string) => {
    const text = inner.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
    const m = attrs.match(/\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i);
    let href = m ? decode(m[1] ?? m[2] ?? m[3] ?? "").trim() : "";
    if (!href || /^(javascript|data):/i.test(href) || href.startsWith("#")) return ` ${text} `;
    try {
      href = new URL(href, base).href;
    } catch {}
    return text && text !== href ? ` ${text} (${href}) ` : ` ${href} `;
  });
  s = s
    .replace(/<(h[1-6])\b[^>]*>/gi, (_, h: string) => `\n\n${"#".repeat(Number(h[1]))} `)
    .replace(/<li\b[^>]*>/gi, "\n- ")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/?(p|div|section|article|header|footer|nav|main|aside|ul|ol|table|tr|blockquote|pre|form|h[1-6]|dl|dt|dd|figure|figcaption|hr)\b[^>]*>/gi, "\n")
    .replace(/<\/?(td|th)\b[^>]*>/gi, " | ")
    .replace(/<[^>]*>/g, "");
  return decode(s)
    .split("\n")
    .map((l) => l.replace(/[ \t\f\v\r ]+/g, " ").trim())
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

// --- fetching ------------------------------------------------------------------------

async function readCapped(res: Response, max: number): Promise<{ bytes: Uint8Array; truncated: boolean }> {
  if (!res.body) return { bytes: new Uint8Array(), truncated: false };
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let n = 0;
  let truncated = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (n + value.length > max) {
      chunks.push(value.subarray(0, max - n));
      n = max;
      truncated = true;
      await reader.cancel().catch(() => {});
      break;
    }
    chunks.push(value);
    n += value.length;
  }
  const bytes = new Uint8Array(n);
  let off = 0;
  for (const c of chunks) {
    bytes.set(c, off);
    off += c.length;
  }
  return { bytes, truncated };
}

const isText = (ct: string) => /^text\/|json|xml|javascript|ecmascript|x-www-form-urlencoded|csv|yaml|toml/i.test(ct) || ct === "";

export function webProvider(opts: WebOptions = {}): ToolProvider {
  const timeoutMs = opts.timeoutMs ?? 20_000;
  const maxBytes = opts.maxBytes ?? 2_000_000;
  const maxRedirects = opts.maxRedirects ?? 5;

  return {
    id: "web",
    tools: () => toolDefs(DEFS),
    prompt: () =>
      `- Web. web_fetch fetches a URL for you if you hold net:<host> (e.g. net:api.github.com, net:*.pypi.org; net:* is any public host). Private and local addresses are always refused.` +
      (opts.bashProxy ? ` Your bash container reaches the same hosts through an HTTP proxy already set in its environment (curl, wget, pip...); every other destination is blocked.` : ""),
    async run(ctx, name, raw) {
      const input = parseInput(DEFS, name as "web_fetch", raw);
      const maxChars = input.max_chars ?? 20_000;
      const signal = AbortSignal.any([ctx.signal, AbortSignal.timeout(timeoutMs)]);
      let url: URL;
      try {
        url = new URL(input.url);
      } catch {
        throw new Denied(`invalid URL ${input.url}`);
      }
      const hops: string[] = [];
      try {
        for (let hop = 0; ; hop++) {
          if (url.protocol !== "http:" && url.protocol !== "https:") throw new Denied(`only http and https URLs (got ${url.protocol})`);
          if (url.username || url.password) throw new Denied("URLs with credentials are not allowed");
          const host = normHost(url.hostname);
          if (!host) throw new Denied(`bad host ${url.hostname}`);
          const allowed = ctx.world.perms.can(ctx.agent, `net:${host}`);
          opts.onDecision?.(ctx.agent, host, allowed);
          if (!allowed) throw new Denied(`${ctx.agent} lacks right "net:${host}"${hops.length ? ` (redirected from ${hops[0]})` : ""}`);
          await checkAddress(url, opts);
          hops.push(url.href);

          const res = await fetch(url, {
            redirect: "manual",
            signal,
            headers: { "user-agent": "metaswarm-web-fetch/0.1", accept: "text/html,application/xhtml+xml,application/json,text/plain;q=0.9,*/*;q=0.5" },
          });
          const loc = res.headers.get("location");
          if (res.status >= 300 && res.status < 400 && loc) {
            await res.body?.cancel().catch(() => {});
            if (hop >= maxRedirects) throw new Denied(`too many redirects (${maxRedirects})`);
            url = new URL(loc, url);
            continue;
          }

          const ct = (res.headers.get("content-type") ?? "").split(";")[0]!.trim().toLowerCase();
          const { bytes, truncated } = await readCapped(res, maxBytes);
          let text: string;
          if (!isText(ct)) text = `(binary content, ${bytes.length}${truncated ? "+" : ""} bytes, not shown)`;
          else {
            text = new TextDecoder().decode(bytes);
            if (/html/.test(ct) || (!ct && /^\s*<(!doctype html|html)/i.test(text))) text = htmlToText(text, url.href);
          }
          const clipped = text.length > maxChars ? `${text.slice(0, maxChars)}\n…[${text.length - maxChars} chars cut]` : text;
          return [
            `status: ${res.status}`,
            `url: ${url.href}`,
            `content-type: ${ct || "(none)"}`,
            hops.length > 1 ? `redirects: ${hops.join(" -> ")}` : "",
            truncated ? `(download stopped at ${maxBytes} bytes)` : "",
            "",
            clipped,
          ].filter((l, i) => l !== "" || i === 5).join("\n");
        }
      } catch (e) {
        if (e instanceof Denied) throw e;
        if (ctx.signal.aborted) return "interrupted";
        if (signal.aborted) return `error: timed out after ${timeoutMs}ms fetching ${url.href}`;
        return `error: ${(e as Error).message} (${url.href})`;
      }
    },
  };
}
