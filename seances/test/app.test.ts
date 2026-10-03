import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createUser, ingestCinema, openDb, rankPlans, setAvailability, setFilmVote, snapshot, updateUser } from "../src/db";
import { createApp } from "../src/server";
import { parseShowings } from "../src/ugc";
import { refresh } from "../src/scrape";

const fixture = await Bun.file(new URL("./fixtures/showings.html", import.meta.url)).text();
const parsed = parseShowings(fixture);

// The fixture is cinema 32 on 2026-10-04; shift it onto a future day so the
// "upcoming only" filter keeps it whatever the clock says.
function futureDay(offset = 3) {
  const d = new Date();
  d.setDate(d.getDate() + offset);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
const DAY = futureDay();
const showings = parsed.showings.map((s) => ({ ...s, date: DAY, cinemaId: "ugc:1" }));
const fakeUgc = async (cinemaId: number, date: string) =>
  date === DAY && cinemaId === 1 ? { films: parsed.films, showings } : { films: [], showings: [] };
const none = async () => ({ films: [], showings: [] });
const DOC = "delivre-nous-du-mal", FILM2 = parsed.films[1].key;

describe("db", () => {
  test("ingest, re-ingest and prune keep votes on surviving showings", () => {
    const db = openDb();
    db.query("INSERT INTO cinemas(id, chain, name) VALUES ('ugc:1', 'ugc', 'Gambetta')").run();
    ingestCinema(db, "ugc:1", [DAY], parsed.films, showings);
    const u = createUser(db, "Ana");
    setAvailability(db, u.id, "ugc:330401306442", true);
    setFilmVote(db, u.id, DOC, 1);
    // Second scrape: the first showing disappeared, the others stay.
    ingestCinema(db, "ugc:1", [DAY], parsed.films, showings.slice(1));
    const snap = snapshot(db, "2000-01-01", "00:00");
    expect(snap.showings).toHaveLength(4);
    expect(snap.films.map((f) => f.key)).toEqual([FILM2]); // film w/o showings is dropped from the view
    expect(db.query("SELECT COUNT(*) AS n FROM availability").get()).toEqual({ n: 0 }); // cascaded
  });

  test("a film seen by two chains keeps the nicer-cased title and the fuller metadata", () => {
    const db = openDb();
    db.query("INSERT INTO cinemas(id, chain, name) VALUES ('ugc:1', 'ugc', 'Gambetta'), ('cgr:W', 'cgr', 'Le Français')").run();
    const cgrFilm = { key: "cars", title: "Cars", genre: "Animation", duration: "1h57", release: "", director: "John Lasseter", synopsis: "short", poster: "", label: "", rating: null };
    const ugcFilm = { ...cgrFilm, title: "CARS", genre: "", director: "", synopsis: "a much longer synopsis from UGC", poster: "p.jpg", label: "UGC Family", rating: 3.5 };
    const show = (id: string, cinemaId: string) => ({ id, filmKey: "cars", cinemaId, date: DAY, time: "10:00", endTime: "", version: "VF", extra: "", room: "", bookingUrl: "" });
    ingestCinema(db, "cgr:W", [DAY], [cgrFilm], [show("cgr:1", "cgr:W")]);
    ingestCinema(db, "ugc:1", [DAY], [ugcFilm], [show("ugc:1", "ugc:1")]);
    const [f] = snapshot(db, "2000-01-01", "00:00").films;
    expect(f).toMatchObject({ title: "Cars", genre: "Animation", director: "John Lasseter", synopsis: "a much longer synopsis from UGC", poster: "p.jpg", label: "UGC Family", rating: 3.5 });
  });

  test("rankPlans: availability first, then film thumbs, then soonest", () => {
    const db = openDb();
    db.query("INSERT INTO cinemas(id, chain, name) VALUES ('ugc:1', 'ugc', 'Gambetta')").run();
    ingestCinema(db, "ugc:1", [DAY], parsed.films, showings);
    const a = createUser(db, "Ana"), b = createUser(db, "Bob"), c = createUser(db, "Cy");
    const ids = showings.map((s) => s.id);
    setAvailability(db, a.id, ids[1], true); setAvailability(db, b.id, ids[1], true); // 2 people, film 17879
    setAvailability(db, a.id, ids[0], true); setAvailability(db, b.id, ids[0], true); setAvailability(db, c.id, ids[0], true); // 3 people, film 18319
    setAvailability(db, c.id, ids[4], true); // 1 person
    setAvailability(db, a.id, ids[2], true); setAvailability(db, b.id, ids[2], true); // 2 people, 17879, later slot
    setFilmVote(db, c.id, DOC, -1);
    const { films, showings: live } = snapshot(db, "2000-01-01", "00:00");
    expect(rankPlans(live, films).map((s) => s.id)).toEqual([ids[0], ids[1], ids[2], ids[4]]);
  });

  test("updateUser validates and normalizes contacts, refuses duplicates", () => {
    const db = openDb();
    const a = createUser(db, "Ana"), b = createUser(db, "Bob");
    expect(updateUser(db, a.id, { email: " Ana@Example.com ", phone: "06 12 34 56 78" })).toMatchObject({ email: "ana@example.com", phone: "+33612345678" });
    expect(() => updateUser(db, b.id, { email: "ana@example.com" })).toThrow(/already linked/);
    expect(() => updateUser(db, b.id, { email: "nope" })).toThrow(/invalid email/);
    expect(() => updateUser(db, b.id, { phone: "12" })).toThrow(/invalid phone/);
  });
});

describe("refresh", () => {
  test("walks selected cinemas × days and records errors without aborting", async () => {
    const db = openDb();
    db.query("INSERT INTO cinemas(id, chain, name, selected) VALUES ('ugc:1', 'ugc', 'Gambetta', 1), ('ugc:2', 'ugc', 'Talence', 1), ('ugc:3', 'ugc', 'Off', 0), ('cgr:W3300', 'cgr', 'Le Français', 1)").run();
    let calls = 0, cgrCalls: string[] = [];
    const r = await refresh(db, {
      days: 4, pauseMs: 0,
      fetchers: {
        ugc: async (cid, d) => { calls++; if (cid === 2) throw new Error("boom"); return fakeUgc(cid, d); },
        cgr: async (id, from, to) => { cgrCalls.push(`${id} ${from} ${to}`); return none(); },
      },
    });
    expect(calls).toBe(8);
    expect(cgrCalls).toEqual([`W3300 ${futureDay(0)} ${futureDay(3)}`]);
    expect(r.showings).toBe(5);
    expect(r.errors).toHaveLength(4);
    expect(r.errors[0]).toMatch(/Talence .* boom/);
  });
});

describe("api", () => {
  const db = openDb();
  const app = createApp({
    db, autoRefresh: false,
    cityFetcher: async (id) => (id === 3 ? [
      { id: "ugc:1", chain: "ugc", name: "UGC Ciné Cité Bordeaux Gambetta" },
      { id: "ugc:42", chain: "ugc", name: "UGC Talence" },
      { id: "cgr:W3300", chain: "cgr", name: "CGR Bordeaux - Le Français" },
    ] : []),
  });
  let server: ReturnType<typeof app.start>;
  let base = "";
  beforeAll(() => { server = app.start(); base = `http://localhost:${server.port}`; });
  afterAll(() => app.stop(server));

  const jar: Record<string, string> = {};
  async function call(path: string, body?: unknown, who = "me") {
    const res = await fetch(base + path, {
      method: body === undefined ? "GET" : "POST",
      headers: { "content-type": "application/json", ...(jar[who] ? { cookie: jar[who] } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const sc = res.headers.get("set-cookie");
    if (sc) jar[who] = sc.split(";")[0];
    return { status: res.status, data: (await res.json()) as any };
  }

  test("first visit mints an identity cookie and an empty state", async () => {
    const { status, data } = await call("/api/state");
    expect(status).toBe(200);
    expect(jar.me).toMatch(/^seances_uid=[0-9a-f]{32}$/);
    expect(data.me.name).toBe("");
    expect(data.city.id).toBeNull();
    expect(data.cities.some((c: any) => c.name === "Bordeaux")).toBe(true);
    expect(data.films).toEqual([]);
  });

  test("choosing a city resolves its cinemas", async () => {
    const bad = await call("/api/city", { regionId: 999 });
    expect(bad.status).toBe(400);
    const { status, data } = await call("/api/city", { regionId: 3 });
    expect(status).toBe(200);
    expect(data.city).toEqual({ id: 3, name: "Bordeaux" });
    expect(data.cinemas.map((c: any) => c.id).sort()).toEqual(["cgr:W3300", "ugc:1", "ugc:42"]);
  });

  test("votes flow through and rank plans", async () => {
    await refresh(db, { days: 5, pauseMs: 0, fetchers: { ugc: fakeUgc, cgr: none } });
    let r = await call("/api/me", { name: "Ana", email: "ana@example.com" });
    expect(r.data.me).toMatchObject({ name: "Ana", email: "ana@example.com" });
    expect(r.data.showings).toHaveLength(5);
    r = await call("/api/availability", { showingId: "ugc:330401306442", on: true });
    expect(r.data.showings.find((s: any) => s.id === "ugc:330401306442").available).toEqual([r.data.me.id]);
    expect(r.data.plans).toEqual(["ugc:330401306442"]);
    r = await call("/api/film-vote", { filmKey: DOC, vote: 1 });
    expect(r.data.films.find((f: any) => f.key === DOC).votes[r.data.me.id]).toBe(1);
    expect((await call("/api/film-vote", { filmKey: DOC, vote: 5 })).status).toBe(400);
    expect((await call("/api/availability", { showingId: "nope", on: true })).status).toBe(404);
  });

  test("another device takes over an identity by its linked contact", async () => {
    const miss = await call("/api/login", { contact: "nobody@example.com" }, "phone");
    expect(miss.status).toBe(404);
    const hit = await call("/api/login", { contact: "ANA@example.com" }, "phone");
    expect(hit.status).toBe(200);
    expect(hit.data.me.name).toBe("Ana");
    expect(jar.phone).toBe(jar.me);
    expect(hit.data.users).toHaveLength(1); // the throwaway identity was dropped
  });

  test("deselecting a cinema hides its showings", async () => {
    const r = await call("/api/cinema", { id: "ugc:1", selected: false });
    expect(r.data.showings).toEqual([]);
    expect(r.data.films).toEqual([]);
  });

  test("static page and traversal guard", async () => {
    expect((await fetch(base + "/")).status).toBe(200);
    expect((await fetch(base + "/..%2Fpackage.json")).status).not.toBe(200);
  });
});
