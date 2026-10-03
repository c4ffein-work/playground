// CGR showtimes — the Webedia-built cgrcinemas.fr exposes same-origin JSON
// routes its React widgets call at runtime. Verified live on 2026-10-03.
//
//   /api/gatsby-source-boxofficeapi/schedule?from=YYYY-MM-DD&to=YYYY-MM-DD&theaters={"id":"W3300","timeZone":"Europe/Paris"}
//     → { W3300: { schedule: { <movieId>: { <date>: [showtime…] } }, moviesTags, showtimesDates } }
//   /api/gatsby-source-boxofficeapi/movies?ids=55774&ids=…   (≤ 50 per call, like the site)
//     → [{ id, title, runtime (s), genres, poster, synopsis, direction[], release, … }]
//
// Theaters: the home page's theater <select> (base64 "Theater:W3300" values,
// proper names) + the showtimes index page-data (slugs, which embed the metro
// city for suburbs: "p0664-cgr-villenave-dornon-bordeaux").
import { decodeEntities } from "./ugc";
import { addSeconds, filmKey, hm, type CinemaRef, type Film, type Showing } from "./types";

export const BASE = "https://www.cgrcinemas.fr";
const API = `${BASE}/api/gatsby-source-boxofficeapi`;
const TZ = "Europe/Paris";

export type CgrTheater = CinemaRef & { slug: string };

async function get(url: string): Promise<string> {
  const res = await fetch(url, { headers: { "user-agent": "seances-voting (private, friends-only; contact via repo)" } });
  if (!res.ok) throw new Error(`CGR ${res.status} on ${url}`);
  return await res.text();
}

/** The theater <select> of any cgrcinemas.fr page: base64("Theater:W3300") → name. */
export function parseTheaterOptions(html: string): Map<string, string> {
  const out = new Map<string, string>();
  const re = /<option value="([A-Za-z0-9+/=]+)">([^<]+)<\/option>/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) {
    let decoded = "";
    try { decoded = atob(m[1]); } catch { continue; }
    if (!decoded.startsWith("Theater:")) continue;
    out.set(decoded.slice("Theater:".length), decodeEntities(m[2]).replace(/\s+/g, " ").trim());
  }
  return out;
}

/** childPages of /horaire-film: [{ id: "W3300", slug: "horaire-film/w3300-cgr-bordeaux-le-francais" }]. */
export function parseTheaterSlugs(pageData: any): { id: string; slug: string }[] {
  const pages = pageData?.result?.data?.page?.childPages;
  if (!Array.isArray(pages)) throw new Error("CGR: unexpected horaire-film page-data");
  return pages
    .filter((p: any) => p?.relatedEntity?.__typename === "Theater" && p.relatedEntity.id && p.slug)
    .map((p: any) => ({ id: String(p.relatedEntity.id), slug: String(p.slug).replace(/^horaire-film\//, "") }));
}

export async function fetchCgrTheaters(): Promise<CgrTheater[]> {
  const [names, slugs] = await Promise.all([
    get(`${BASE}/`).then(parseTheaterOptions),
    get(`${BASE}/page-data/horaire-film/page-data.json`).then((t) => parseTheaterSlugs(JSON.parse(t))),
  ]);
  const out: CgrTheater[] = [];
  for (const { id, slug } of slugs) {
    const name = names.get(id) ?? titleFromSlug(slug);
    out.push({ id: `cgr:${id}`, chain: "cgr", name, slug });
  }
  if (!out.length) throw new Error("CGR: no theater found");
  return out;
}

function titleFromSlug(slug: string): string {
  return slug.replace(/^[a-z]\d{4}-/, "").split("-").map((w) => w[0]?.toUpperCase() + w.slice(1)).join(" ");
}

const norm = (s: string) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, "-");

/**
 * Theaters of a city: the slug carries the metro city for suburbs
 * ("villenave-dornon-bordeaux", "brignais-lyon"). A trailing "le-paris" /
 * "la-…" is a cinema's name, not a city.
 */
export function theatersInCity(theaters: CgrTheater[], city: string): CgrTheater[] {
  const c = norm(city).replace(/^region-parisienne$/, "paris"); // best effort for the Paris suburbs
  return theaters.filter((t) => {
    const parts = t.slug.replace(/^[a-z]\d{4}-cgr-/, "").split("-");
    const cityParts = c.split("-");
    for (let i = 0; i + cityParts.length <= parts.length; i++) {
      if (cityParts.every((p, j) => parts[i + j] === p)) {
        const prev = parts[i - 1];
        if (prev === "le" || prev === "la" || prev === "les") continue;
        return true;
      }
    }
    return false;
  });
}

const tagVersion = (tags: string[]): string => {
  const original = tags.includes("Localization.Version.Original");
  const subtitled = tags.includes("Showtime.Accessibility.Subtitled");
  if (original) return subtitled ? "VOST" : "VO";
  return subtitled ? "VFST" : "VF";
};
const tagExtra = (tags: string[]): string =>
  tags.map((t) => (t === "Auditorium.Experience.Ice" ? "ICE" : t === "Auditorium.Experience.InfinityVision" ? "IV" : t === "Format.Projection.3d" ? "3D" : ""))
    .filter(Boolean).join(" ");

export function movieToFilm(m: any): Film {
  const title = String(m?.title ?? m?.originalTitle ?? "").trim();
  const rel = typeof m?.release === "string" ? m.release.slice(0, 10) : "";
  return {
    key: filmKey(title),
    title,
    genre: String(m?.genres ?? "").trim(),
    duration: typeof m?.runtime === "number" && m.runtime > 0 ? hm(m.runtime) : "",
    release: rel ? rel.split("-").reverse().join("/") : "",
    director: Array.isArray(m?.direction) ? m.direction.join(", ") : "",
    synopsis: String(m?.synopsis ?? m?.locale?.synopsis ?? "").replace(/\s+/g, " ").trim(),
    poster: String(m?.poster ?? ""),
    label: "",
    rating: null,
  };
}

/** Pure: a schedule payload + the movies it references → films + showings for `theaterId`. */
export function parseCgrSchedule(schedule: any, movies: any[], theaterId: string): { films: Film[]; showings: Showing[] } {
  const sched = schedule?.[theaterId]?.schedule;
  if (!sched || typeof sched !== "object") throw new Error(`CGR: no schedule for ${theaterId} in payload`);
  const byId = new Map<string, any>(movies.map((m) => [String(m.id), m]));
  const films = new Map<string, Film>();
  const showings: Showing[] = [];
  const cinemaId = `cgr:${theaterId}`;
  for (const [movieId, days] of Object.entries<any>(sched)) {
    const m = byId.get(movieId);
    if (!m) continue; // unknown movie (metadata call failed): nothing to show
    const film = movieToFilm(m);
    if (!film.title) continue;
    films.set(film.key, film);
    for (const [date, list] of Object.entries<any>(days)) {
      if (!Array.isArray(list)) continue;
      for (const s of list) {
        const starts = String(s?.startsAt ?? "");
        const mm = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/.exec(starts);
        if (!mm) continue;
        const tags: string[] = Array.isArray(s?.tags) ? s.tags : [];
        const time = mm[2];
        const booking = s?.data?.ticketing?.find?.((t: any) => t?.provider === "default")?.urls?.[0] ?? s?.data?.ticketing?.[0]?.urls?.[0] ?? `${BASE}/horaire-film/`;
        showings.push({
          id: `cgr:${theaterId}:${movieId}:${mm[1]}:${time}`,
          filmKey: film.key,
          cinemaId,
          date: mm[1] || date,
          time,
          endTime: typeof m.runtime === "number" && m.runtime > 0 ? addSeconds(time, m.runtime) : "",
          version: tagVersion(tags),
          extra: tagExtra(tags),
          room: String(s?.screen?.name ?? "").trim(),
          bookingUrl: String(booking),
        });
      }
    }
  }
  return { films: [...films.values()], showings };
}

export async function fetchCgrMovies(ids: string[]): Promise<any[]> {
  const out: any[] = [];
  for (let i = 0; i < ids.length; i += 50) {
    const qs = ids.slice(i, i + 50).map((id) => `ids=${encodeURIComponent(id)}`).join("&");
    const arr = JSON.parse(await get(`${API}/movies?${qs}`));
    if (!Array.isArray(arr)) throw new Error("CGR: unexpected movies payload");
    out.push(...arr);
  }
  return out;
}

/** One theater, a date range (inclusive). `theaterId` is the bare CGR id ("W3300"). */
export async function fetchCgrProgramme(theaterId: string, from: string, to: string) {
  const theaters = encodeURIComponent(JSON.stringify({ id: theaterId, timeZone: TZ }));
  const schedule = JSON.parse(await get(`${API}/schedule?from=${from}&to=${to}&theaters=${theaters}`));
  const sched = schedule?.[theaterId]?.schedule;
  if (!sched || typeof sched !== "object") throw new Error(`CGR: unrecognized schedule payload for ${theaterId} (API changed?)`);
  const movieIds = Object.keys(sched);
  const movies = movieIds.length ? await fetchCgrMovies(movieIds) : [];
  return parseCgrSchedule(schedule, movies, theaterId);
}
