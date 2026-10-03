// Bun HTTP server: static page + JSON API + background refresh.
import type { Database } from "bun:sqlite";
import { join } from "node:path";
import { REGIONS, fetchCityCinemas, isoDate } from "./ugc";
import {
  createUser, findUserByContact, getSetting, getUser, isBlankUser, lastScrapeAt, listCinemas, listUsers, openDb,
  rankPlans, replaceCinemas, setAvailability, setCinemaSelected, setFilmVote, setSetting, snapshot, updateUser,
} from "./db";
import { refresh } from "./scrape";

const PUBLIC = join(import.meta.dir, "..", "public");
const COOKIE = "seances_uid";
const ONE_YEAR = 60 * 60 * 24 * 365;

export type AppOptions = { db: Database; port?: number; refreshEveryMs?: number; autoRefresh?: boolean; cityFetcher?: typeof fetchCityCinemas };

function json(data: unknown, init: ResponseInit = {}) {
  return new Response(JSON.stringify(data), { ...init, headers: { "content-type": "application/json; charset=utf-8", ...(init.headers ?? {}) } });
}
const bad = (msg: string, status = 400) => json({ error: msg }, { status });

function cookieOf(req: Request, name: string): string | null {
  const raw = req.headers.get("cookie") ?? "";
  for (const part of raw.split(";")) {
    const [k, ...v] = part.trim().split("=");
    if (k === name) return decodeURIComponent(v.join("="));
  }
  return null;
}
const setCookie = (id: string) => `${COOKIE}=${id}; Path=/; Max-Age=${ONE_YEAR}; SameSite=Lax; HttpOnly`;

function nowHM(d = new Date()) {
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function createApp(opts: AppOptions) {
  const { db } = opts;
  const cityFetcher = opts.cityFetcher ?? fetchCityCinemas;
  let refreshing: Promise<unknown> | null = null;

  function kickRefresh(days?: number) {
    if (!refreshing) {
      refreshing = refresh(db, { days, log: (s) => console.log("[scrape]", s) })
        .catch((e) => console.error("[scrape] failed:", e))
        .finally(() => (refreshing = null));
    }
    return refreshing;
  }

  function identify(req: Request): { user: ReturnType<typeof getUser>; fresh: boolean } {
    const id = cookieOf(req, COOKIE);
    const user = id ? getUser(db, id) : null;
    if (user) return { user, fresh: false };
    return { user: createUser(db), fresh: true };
  }

  function stateFor(userId: string) {
    const today = isoDate(new Date());
    const { films, showings } = snapshot(db, today, nowHM());
    const last = getSetting(db, "last_refresh");
    return {
      me: getUser(db, userId),
      city: { id: Number(getSetting(db, "region_id") ?? 0) || null, name: getSetting(db, "region_name") },
      cities: REGIONS,
      cinemas: listCinemas(db),
      users: listUsers(db),
      films,
      showings,
      plans: rankPlans(showings, films).map((s) => s.id),
      lastScrape: lastScrapeAt(db),
      lastRefresh: last ? JSON.parse(last) : null,
      refreshing: !!refreshing,
      today,
    };
  }

  async function body(req: Request): Promise<any> {
    try { return await req.json(); } catch { return {}; }
  }

  async function api(req: Request, path: string): Promise<Response> {
    const { user, fresh } = identify(req);
    const uid = user!.id;
    const withCookie = (res: Response) => { if (fresh) res.headers.append("set-cookie", setCookie(uid)); return res; };
    const m = req.method;

    if (m === "GET" && path === "/api/state") return withCookie(json(stateFor(uid)));

    if (m === "POST" && path === "/api/me") {
      const b = await body(req);
      try { updateUser(db, uid, { name: b.name, email: b.email, phone: b.phone }); }
      catch (e) { return withCookie(bad((e as Error).message)); }
      return withCookie(json(stateFor(uid)));
    }

    if (m === "POST" && path === "/api/login") {
      // Trust-based: whoever types a linked email/phone becomes that member on this device.
      const b = await body(req);
      const target = findUserByContact(db, String(b.contact ?? ""));
      if (!target) return withCookie(bad("no member linked to that email / phone", 404));
      if (isBlankUser(db, uid)) db.query("DELETE FROM users WHERE id = ?").run(uid); // drop the throwaway identity
      const res = json(stateFor(target.id));
      res.headers.append("set-cookie", setCookie(target.id));
      return res;
    }

    if (m === "POST" && path === "/api/city") {
      const b = await body(req);
      const region = REGIONS.find((r) => r.id === Number(b.regionId));
      if (!region) return withCookie(bad("unknown city"));
      let cinemas;
      try { cinemas = await cityFetcher(region.id); }
      catch (e) { return withCookie(bad("UGC lookup failed: " + (e as Error).message, 502)); }
      if (!cinemas.length) return withCookie(bad("no UGC cinema found for that city", 404));
      replaceCinemas(db, cinemas);
      setSetting(db, "region_id", String(region.id));
      setSetting(db, "region_name", region.name);
      if (opts.autoRefresh !== false) kickRefresh();
      return withCookie(json(stateFor(uid)));
    }

    if (m === "POST" && path === "/api/cinema") {
      const b = await body(req);
      if (!listCinemas(db).some((c) => c.id === Number(b.id))) return withCookie(bad("unknown cinema"));
      setCinemaSelected(db, Number(b.id), !!b.selected);
      if (b.selected && opts.autoRefresh !== false) kickRefresh();
      return withCookie(json(stateFor(uid)));
    }

    if (m === "POST" && path === "/api/refresh") {
      kickRefresh(Number(await body(req).then((b) => b.days)) || undefined);
      return withCookie(json({ ok: true, refreshing: true }));
    }

    if (m === "POST" && path === "/api/availability") {
      const b = await body(req);
      const sid = String(b.showingId ?? "");
      if (!db.query("SELECT 1 FROM showings WHERE id = ?").get(sid)) return withCookie(bad("unknown showing", 404));
      setAvailability(db, uid, sid, !!b.on);
      return withCookie(json(stateFor(uid)));
    }

    if (m === "POST" && path === "/api/film-vote") {
      const b = await body(req);
      const fid = Number(b.filmId), v = Number(b.vote);
      if (!db.query("SELECT 1 FROM films WHERE id = ?").get(fid)) return withCookie(bad("unknown film", 404));
      if (![-1, 0, 1].includes(v)) return withCookie(bad("vote must be -1, 0 or 1"));
      setFilmVote(db, uid, fid, v as -1 | 0 | 1);
      return withCookie(json(stateFor(uid)));
    }

    return withCookie(bad("not found", 404));
  }

  async function handler(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const path = url.pathname;
    if (path.startsWith("/api/")) return api(req, path);
    const file = path === "/" ? "index.html" : path.replace(/^\/+/, "");
    if (file.includes("..")) return bad("nope", 403);
    const f = Bun.file(join(PUBLIC, file));
    if (await f.exists()) return new Response(f, { headers: { "cache-control": "no-store" } });
    return new Response("not found", { status: 404 });
  }

  let timer: ReturnType<typeof setInterval> | null = null;
  function start() {
    const server = Bun.serve({ port: opts.port ?? 0, fetch: handler });
    if (opts.autoRefresh !== false) {
      const every = opts.refreshEveryMs ?? 6 * 3600_000;
      const last = lastScrapeAt(db) ?? 0;
      if (listCinemas(db).length && Date.now() - last > every) kickRefresh();
      timer = setInterval(() => listCinemas(db).length && kickRefresh(), every);
    }
    return server;
  }
  function stop(server: ReturnType<typeof Bun.serve>) {
    if (timer) clearInterval(timer);
    server.stop(true);
  }
  return { handler, start, stop, kickRefresh };
}

if (import.meta.main) {
  const db = openDb(process.env.SEANCES_DB ?? "seances.db");
  const app = createApp({ db, port: Number(process.env.PORT ?? 3000) });
  const server = app.start();
  console.log(`seances listening on http://${server.hostname}:${server.port}`);
}
