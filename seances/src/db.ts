import { Database } from "bun:sqlite";
import type { Film, Showing } from "./ugc";

export type User = { id: string; name: string; email: string; phone: string; created_at: number };

export function openDb(path = ":memory:"): Database {
  const db = new Database(path, { create: true });
  db.exec("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;");
  db.exec(`
    CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS cinemas (
      id INTEGER PRIMARY KEY, name TEXT NOT NULL, selected INTEGER NOT NULL DEFAULT 1
    );
    CREATE TABLE IF NOT EXISTS films (
      id INTEGER PRIMARY KEY, title TEXT NOT NULL, genre TEXT, duration TEXT, release TEXT,
      director TEXT, synopsis TEXT, poster TEXT, label TEXT, rating REAL
    );
    CREATE TABLE IF NOT EXISTS showings (
      id TEXT PRIMARY KEY, film_id INTEGER NOT NULL REFERENCES films(id),
      cinema_id INTEGER NOT NULL, date TEXT NOT NULL, time TEXT NOT NULL, end_time TEXT,
      version TEXT, room TEXT, booking_url TEXT, scraped_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS showings_day ON showings(cinema_id, date);
    CREATE TABLE IF NOT EXISTS scrapes (
      cinema_id INTEGER NOT NULL, date TEXT NOT NULL, fetched_at INTEGER NOT NULL, count INTEGER NOT NULL,
      PRIMARY KEY (cinema_id, date)
    );
    CREATE TABLE IF NOT EXISTS users (
      id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', email TEXT NOT NULL DEFAULT '',
      phone TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS availability (
      user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      showing_id TEXT NOT NULL REFERENCES showings(id) ON DELETE CASCADE,
      PRIMARY KEY (user_id, showing_id)
    );
    CREATE TABLE IF NOT EXISTS film_votes (
      user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      film_id INTEGER NOT NULL REFERENCES films(id) ON DELETE CASCADE,
      vote INTEGER NOT NULL CHECK (vote IN (-1, 1)),
      PRIMARY KEY (user_id, film_id)
    );
  `);
  return db;
}

// ---- settings ----
export const getSetting = (db: Database, key: string): string | null =>
  (db.query("SELECT value FROM settings WHERE key = ?").get(key) as { value: string } | null)?.value ?? null;
export const setSetting = (db: Database, key: string, value: string) =>
  db.query("INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value").run(key, value);

// ---- cinemas ----
export type Cinema = { id: number; name: string; selected: number };
export const listCinemas = (db: Database) => db.query("SELECT * FROM cinemas ORDER BY name").all() as Cinema[];
export function replaceCinemas(db: Database, cinemas: { id: number; name: string }[]) {
  db.transaction(() => {
    db.query("DELETE FROM availability").run();
    db.query("DELETE FROM film_votes").run();
    db.query("DELETE FROM showings").run();
    db.query("DELETE FROM scrapes").run();
    db.query("DELETE FROM films").run();
    db.query("DELETE FROM cinemas").run();
    const ins = db.query("INSERT INTO cinemas(id, name, selected) VALUES (?, ?, 1)");
    for (const c of cinemas) ins.run(c.id, c.name);
  })();
}
export const setCinemaSelected = (db: Database, id: number, selected: boolean) =>
  db.query("UPDATE cinemas SET selected = ? WHERE id = ?").run(selected ? 1 : 0, id);

// ---- scrape ingestion ----
/** Replace the showings of one (cinema, day) with a fresh scrape; keeps votes on ids that survive. */
export function ingestDay(db: Database, cinemaId: number, date: string, films: Film[], showings: Showing[], now = Date.now()) {
  db.transaction(() => {
    const upFilm = db.query(`
      INSERT INTO films(id, title, genre, duration, release, director, synopsis, poster, label, rating)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET title = excluded.title, genre = excluded.genre, duration = excluded.duration,
        release = excluded.release, director = excluded.director, synopsis = excluded.synopsis,
        poster = excluded.poster, label = excluded.label, rating = excluded.rating`);
    for (const f of films) upFilm.run(f.id, f.title, f.genre, f.duration, f.release, f.director, f.synopsis, f.poster, f.label, f.rating);
    const keep = showings.filter((s) => s.cinemaId === cinemaId && s.date === date);
    const ids = keep.map((s) => s.id);
    const stale = db.query("SELECT id FROM showings WHERE cinema_id = ? AND date = ?").all(cinemaId, date) as { id: string }[];
    const del = db.query("DELETE FROM showings WHERE id = ?");
    for (const s of stale) if (!ids.includes(s.id)) del.run(s.id);
    const up = db.query(`
      INSERT INTO showings(id, film_id, cinema_id, date, time, end_time, version, room, booking_url, scraped_at)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET film_id = excluded.film_id, time = excluded.time, end_time = excluded.end_time,
        version = excluded.version, room = excluded.room, booking_url = excluded.booking_url, scraped_at = excluded.scraped_at`);
    for (const s of keep) up.run(s.id, s.filmId, s.cinemaId, s.date, s.time, s.endTime, s.version, s.room, s.bookingUrl, now);
    db.query("INSERT INTO scrapes(cinema_id, date, fetched_at, count) VALUES (?, ?, ?, ?) ON CONFLICT(cinema_id, date) DO UPDATE SET fetched_at = excluded.fetched_at, count = excluded.count")
      .run(cinemaId, date, now, keep.length);
  })();
}

/** Drop days that are gone and films nothing points at anymore. */
export function pruneBefore(db: Database, isoToday: string) {
  db.transaction(() => {
    db.query("DELETE FROM showings WHERE date < ?").run(isoToday);
    db.query("DELETE FROM scrapes WHERE date < ?").run(isoToday);
    db.query("DELETE FROM films WHERE id NOT IN (SELECT DISTINCT film_id FROM showings)").run();
  })();
}

export const lastScrapeAt = (db: Database): number | null =>
  (db.query("SELECT MAX(fetched_at) AS t FROM scrapes").get() as { t: number | null }).t;

// ---- users ----
export const newId = () => crypto.randomUUID().replace(/-/g, "");
export function createUser(db: Database, name = ""): User {
  const u: User = { id: newId(), name, email: "", phone: "", created_at: Date.now() };
  db.query("INSERT INTO users(id, name, email, phone, created_at) VALUES (?, ?, ?, ?, ?)").run(u.id, u.name, u.email, u.phone, u.created_at);
  return u;
}
export const getUser = (db: Database, id: string) => db.query("SELECT * FROM users WHERE id = ?").get(id) as User | null;
export const normEmail = (s: string) => s.trim().toLowerCase();
export const normPhone = (s: string) => {
  let d = s.replace(/[^\d+]/g, "");
  if (d.startsWith("00")) d = "+" + d.slice(2);
  if (/^0\d{9}$/.test(d)) d = "+33" + d.slice(1); // French national -> E.164
  return d;
};
export function updateUser(db: Database, id: string, fields: { name?: string; email?: string; phone?: string }): User {
  const cur = getUser(db, id);
  if (!cur) throw new Error("no such user");
  const name = (fields.name ?? cur.name).trim().slice(0, 40);
  const email = normEmail(fields.email ?? cur.email).slice(0, 120);
  const phone = normPhone(fields.phone ?? cur.phone).slice(0, 20);
  if (email && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) throw new Error("invalid email");
  if (phone && !/^\+?\d{6,15}$/.test(phone)) throw new Error("invalid phone");
  const clash = (col: "email" | "phone", v: string) =>
    v && (db.query(`SELECT id FROM users WHERE ${col} = ? AND id != ?`).get(v, id) as { id: string } | null);
  if (clash("email", email)) throw new Error("email already linked to another member");
  if (clash("phone", phone)) throw new Error("phone already linked to another member");
  db.query("UPDATE users SET name = ?, email = ?, phone = ? WHERE id = ?").run(name, email, phone, id);
  return getUser(db, id)!;
}
/** No name, no contact, no vote: a cookie that never became anyone. */
export const isBlankUser = (db: Database, id: string): boolean =>
  !!db.query(`SELECT 1 FROM users u WHERE u.id = ? AND u.name = '' AND u.email = '' AND u.phone = ''
    AND NOT EXISTS (SELECT 1 FROM availability WHERE user_id = u.id)
    AND NOT EXISTS (SELECT 1 FROM film_votes WHERE user_id = u.id)`).get(id);
/** Find the member a contact (email or phone) is linked to. */
export function findUserByContact(db: Database, contact: string): User | null {
  const c = contact.trim();
  if (!c) return null;
  return (
    (db.query("SELECT * FROM users WHERE email = ? AND email != ''").get(normEmail(c)) as User | null) ??
    (db.query("SELECT * FROM users WHERE phone = ? AND phone != ''").get(normPhone(c)) as User | null)
  );
}
/** Members = people who gave themselves a name; cookie-only visitors stay invisible. */
export const listUsers = (db: Database) => db.query("SELECT id, name, email != '' AS has_email, phone != '' AS has_phone FROM users WHERE name != '' ORDER BY created_at").all() as
  { id: string; name: string; has_email: number; has_phone: number }[];

// ---- votes ----
export function setAvailability(db: Database, userId: string, showingId: string, on: boolean) {
  if (on) db.query("INSERT OR IGNORE INTO availability(user_id, showing_id) VALUES (?, ?)").run(userId, showingId);
  else db.query("DELETE FROM availability WHERE user_id = ? AND showing_id = ?").run(userId, showingId);
}
export function setFilmVote(db: Database, userId: string, filmId: number, vote: -1 | 0 | 1) {
  if (vote === 0) db.query("DELETE FROM film_votes WHERE user_id = ? AND film_id = ?").run(userId, filmId);
  else db.query("INSERT INTO film_votes(user_id, film_id, vote) VALUES (?, ?, ?) ON CONFLICT(user_id, film_id) DO UPDATE SET vote = excluded.vote").run(userId, filmId, vote);
}

// ---- read model ----
export type ShowingRow = {
  id: string; film_id: number; cinema_id: number; date: string; time: string; end_time: string;
  version: string; room: string; booking_url: string; available: string[];
};
export type FilmRow = Film & { votes: Record<string, number> };

export function snapshot(db: Database, isoToday: string, nowHM: string) {
  const films = (db.query("SELECT * FROM films").all() as any[]).map((f) => ({ ...f, votes: {} as Record<string, number> }));
  const byFilm = new Map<number, FilmRow>(films.map((f) => [f.id, f]));
  for (const v of db.query("SELECT user_id, film_id, vote FROM film_votes").all() as { user_id: string; film_id: number; vote: number }[]) {
    byFilm.get(v.film_id)!.votes[v.user_id] = v.vote;
  }
  const showings = (db.query(
    `SELECT s.* FROM showings s JOIN cinemas c ON c.id = s.cinema_id
     WHERE c.selected = 1 AND (s.date > ? OR (s.date = ? AND s.time >= ?)) ORDER BY s.date, s.time`,
  ).all(isoToday, isoToday, nowHM) as any[]).map((s) => ({ ...s, available: [] as string[] }));
  const byShowing = new Map<string, ShowingRow>(showings.map((s) => [s.id, s]));
  for (const a of db.query("SELECT user_id, showing_id FROM availability").all() as { user_id: string; showing_id: string }[]) {
    byShowing.get(a.showing_id)?.available.push(a.user_id);
  }
  // Films with no upcoming showing in a selected cinema are noise.
  const live = new Set(showings.map((s) => s.film_id));
  return { films: films.filter((f) => live.has(f.id)), showings };
}

/**
 * Rank plans: most people available first, then the film's net thumbs among
 * everyone, then the sooner séance.
 */
export function rankPlans(showings: ShowingRow[], films: FilmRow[], limit = 5): ShowingRow[] {
  const score = new Map(films.map((f) => [f.id, Object.values(f.votes).reduce((a, b) => a + b, 0)]));
  return [...showings]
    .filter((s) => s.available.length > 0)
    .sort((a, b) =>
      b.available.length - a.available.length ||
      (score.get(b.film_id) ?? 0) - (score.get(a.film_id) ?? 0) ||
      (a.date + a.time).localeCompare(b.date + b.time),
    )
    .slice(0, limit);
}
