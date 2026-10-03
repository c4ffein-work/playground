// UGC showtimes scraper — plain HTTP, no browser.
//
// ugc.fr is a Struts app whose cinema page loads its screenings through a GET
// endpoint returning an HTML fragment. Every screening card carries clean
// data-attributes, so parsing is regex work on well-known hooks, not DOM
// surgery. Verified against the live site on 2026-10-03.

export const BASE = "https://www.ugc.fr";

/** UGC's "regions" = the city tabs of /cinemas.html (ids as of 2026-10). */
export const REGIONS: { id: number; name: string }[] = [
  { id: 1, name: "Paris" },
  { id: 2, name: "Région parisienne" },
  { id: 3, name: "Bordeaux" },
  { id: 4, name: "Caen" },
  { id: 5, name: "Lille Métropole" },
  { id: 6, name: "Lyon" },
  { id: 7, name: "Nancy" },
  { id: 8, name: "Nantes" },
  { id: 10, name: "Strasbourg" },
  { id: 11, name: "Toulouse" },
];

export type Film = {
  id: number;
  title: string;
  genre: string;
  duration: string; // "1h49"
  release: string; // "30 septembre 2026"
  director: string;
  synopsis: string;
  poster: string;
  label: string; // "Sélection UGC Culte", "UGC Aime", ...
  rating: number | null; // audience average /5
};

export type Showing = {
  id: string; // UGC's showing id, also the booking id
  filmId: number;
  cinemaId: number;
  date: string; // ISO yyyy-mm-dd
  time: string; // "18:30"
  endTime: string; // "20:35" or ""
  version: string; // VF / VOSTF / ...
  room: string; // "Salle 1"
  bookingUrl: string;
};

const ENTITIES: Record<string, string> = {
  amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", hellip: "…",
  rsquo: "’", lsquo: "‘", rdquo: "”", ldquo: "“", laquo: "«", raquo: "»", ndash: "–", mdash: "—",
  eacute: "é", egrave: "è", ecirc: "ê", euml: "ë", Eacute: "É", Egrave: "È", Ecirc: "Ê",
  agrave: "à", acirc: "â", auml: "ä", Agrave: "À", Acirc: "Â",
  icirc: "î", iuml: "ï", ocirc: "ô", ouml: "ö", Ocirc: "Ô",
  ugrave: "ù", ucirc: "û", uuml: "ü", ccedil: "ç", Ccedil: "Ç", oelig: "œ", OElig: "Œ", aelig: "æ",
};

export function decodeEntities(s: string): string {
  return s.replace(/&(#x[0-9a-fA-F]+|#\d+|[A-Za-z]+);/g, (m, code: string) => {
    if (code[0] === "#") {
      const n = code[1] === "x" || code[1] === "X" ? parseInt(code.slice(2), 16) : parseInt(code.slice(1), 10);
      return Number.isFinite(n) ? String.fromCodePoint(n) : m;
    }
    return ENTITIES[code] ?? m;
  });
}

function clean(s: string | undefined | null): string {
  return decodeEntities(s ?? "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
}

function attr(tag: string, name: string): string {
  const m = new RegExp(`\\b${name}\\s*=\\s*"([^"]*)"`, "i").exec(tag);
  return m ? decodeEntities(m[1]) : "";
}

function between(src: string, after: RegExp, until: string): string {
  const m = after.exec(src);
  if (!m) return "";
  const start = m.index + m[0].length;
  const end = src.indexOf(until, start);
  return end < 0 ? "" : src.slice(start, end);
}

/** "04/10/2026" -> "2026-10-04" */
export function frDateToIso(d: string): string {
  const m = /^(\d{2})\/(\d{2})\/(\d{4})$/.exec(d.trim());
  return m ? `${m[3]}-${m[2]}-${m[1]}` : d.trim();
}

/** Parse one `getShowingsForCinemaPage` fragment into films + screenings. */
export function parseShowings(html: string): { films: Film[]; showings: Showing[] } {
  const films: Film[] = [];
  const showings: Showing[] = [];
  const blocks = html.split(/<div id="bloc-showing-film-(?=\d+")/).slice(1);
  for (const block of blocks) {
    const id = parseInt(block, 10);
    if (!Number.isFinite(id)) continue;
    const anchor = /<a id="goToFilm_\d+_visu_img"[^>]*>/.exec(block)?.[0] ?? "";
    const info = between(block, /class="group-info">\s*<p[^>]*>\s*Sortie le/, "</div>");
    const durM = /\((\d+h\d{2})\)/.exec(info);
    const relM = /<span[^>]*>\s*([^<]*?)\s*<\/span>/.exec(info);
    const dirM = /\bDe\s*<span[^>]*>([^<]*)<\/span>/.exec(block);
    const synM = /Synopsis\s*<span[^>]*>([\s\S]*?)<\/span>/.exec(block);
    const rateM = /<h1 class="average">\s*([\d,.]+)/.exec(block);
    const posterM = /<img class="lozad[^"]*"\s+data-src="([^"]*)"/.exec(block);
    const kind = attr(anchor, "data-film-kind");
    films.push({
      id,
      title: clean(attr(anchor, "title")),
      genre: clean(kind),
      duration: durM ? durM[1] : "",
      release: clean(relM?.[1]).replace(/\s*\(\d+h\d{2}\)$/, ""),
      director: clean(dirM?.[1]),
      synopsis: clean(synM?.[1]),
      poster: decodeEntities(posterM?.[1] ?? ""),
      label: clean(attr(anchor, "data-film-label")),
      rating: rateM ? parseFloat(rateM[1].replace(",", ".")) : null,
    });

    const cardRe = /<button[^>]*\bdata-showing="(\d+)"[^>]*>/g;
    let m: RegExpExecArray | null;
    while ((m = cardRe.exec(block))) {
      const tag = m[0];
      const tail = block.slice(m.index + tag.length, m.index + tag.length + 3000);
      const endM = /screening-time-end">\s*\(fin\s+(\d{1,2}:\d{2})\)/.exec(tail);
      const roomM = /screening-room">\s*([^<]*?)\s*</.exec(tail);
      const sid = m[1];
      showings.push({
        id: sid,
        filmId: parseInt(attr(tag, "data-filmId"), 10) || id,
        cinemaId: parseInt(attr(tag, "data-cinemaId"), 10),
        date: frDateToIso(attr(tag, "data-seanceDate")),
        time: attr(tag, "data-seanceHour"),
        endTime: endM ? endM[1] : "",
        version: attr(tag, "data-version").toUpperCase(),
        room: clean(roomM?.[1]),
        bookingUrl: `${BASE}/reservationSeances.html?id=${sid}`,
      });
    }
  }
  return { films, showings };
}

async function get(url: string, init?: RequestInit): Promise<string> {
  const res = await fetch(url, {
    ...init,
    headers: { "user-agent": "seances-voting (private, friends-only; contact via repo)", ...(init?.headers ?? {}) },
  });
  if (!res.ok) throw new Error(`UGC ${res.status} on ${url}`);
  return await res.text();
}

export type CinemaRef = { id: number; name: string };

/** Every UGC cinema with its numeric id (a JSON endpoint). */
export async function fetchAllCinemas(): Promise<CinemaRef[]> {
  const json = JSON.parse(await get(`${BASE}/inscriptionNewsletterAction!getCinemaList.action`));
  if (!Array.isArray(json?.cinemas)) throw new Error("UGC: unexpected cinema list payload");
  return json.cinemas.map((c: any) => ({ id: Number(c.id), name: String(c.name).trim() }));
}

/** Cinema names listed under one city tab (no ids in that fragment). */
export function parseCityCinemaNames(html: string): string[] {
  const names = new Set<string>();
  const re = /<a href="cinema-[^"]*\.html" class="zoom" title="([^"]*)"/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) names.add(clean(m[1]));
  return [...names];
}

const norm = (s: string) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();

/** Cinemas of a city, resolved to ids by name against the global list. */
export async function fetchCityCinemas(regionId: number): Promise<CinemaRef[]> {
  const [names, all] = await Promise.all([
    get(`${BASE}/cinemasAjaxAction!getCinemasList.action?id=${regionId}`).then(parseCityCinemaNames),
    fetchAllCinemas(),
  ]);
  const byName = new Map(all.map((c) => [norm(c.name), c]));
  const out: CinemaRef[] = [];
  for (const n of names) {
    const hit = byName.get(norm(n));
    if (hit) out.push({ id: hit.id, name: n });
  }
  if (names.length && !out.length) throw new Error("UGC: could not match any city cinema to an id");
  return out;
}

/** One cinema, one day. `date` is ISO yyyy-mm-dd. */
export async function fetchShowings(cinemaId: number, date: string) {
  const url = `${BASE}/showingsCinemaAjaxAction!getShowingsForCinemaPage.action?cinemaId=${cinemaId}&date=${date}`;
  const html = await get(url);
  if (!/component--cinema-list-item|bloc-showing-film-|Aucune s/i.test(html)) {
    throw new Error(`UGC: unrecognized showings page for cinema ${cinemaId} on ${date} (markup changed?)`);
  }
  return parseShowings(html);
}

export function isoDate(d: Date): string {
  const y = d.getFullYear(), m = String(d.getMonth() + 1).padStart(2, "0"), day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

export function nextDays(n: number, from = new Date()): string[] {
  const out: string[] = [];
  for (let i = 0; i < n; i++) {
    const d = new Date(from);
    d.setDate(d.getDate() + i);
    out.push(isoDate(d));
  }
  return out;
}
