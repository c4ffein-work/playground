// Chain-agnostic records every scraper produces and the DB stores.

export type Chain = "ugc" | "cgr";

export type CinemaRef = {
  /** Namespaced: "ugc:32", "cgr:W3300". */
  id: string;
  chain: Chain;
  name: string;
};

export type Film = {
  /** Cross-chain key: the normalized title (see `filmKey`), so one card shows every chain's séances. */
  key: string;
  title: string;
  genre: string;
  duration: string; // "1h49"
  release: string; // free text, chain-specific
  director: string;
  synopsis: string;
  poster: string;
  label: string; // chain's editorial label ("Sélection UGC Culte", …)
  rating: number | null; // audience average /5
};

export type Showing = {
  /** Namespaced, stable across scrapes: "ugc:330401306442", "cgr:W3300:55774:2026-10-04:10:30". */
  id: string;
  filmKey: string;
  cinemaId: string;
  date: string; // ISO yyyy-mm-dd
  time: string; // "18:30"
  endTime: string; // "20:35" or ""
  version: string; // VF / VO / VOST / VFST …
  extra: string; // "ICE", "3D", "IV" … or ""
  room: string;
  bookingUrl: string;
};

/** "DELIVRE-NOUS DU MAL" and "Délivre-nous du mal" collapse to the same key. */
export function filmKey(title: string): string {
  return title
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/&/g, " et ")
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .replace(/ /g, "-");
}

export function chainOf(id: string): Chain {
  return id.split(":")[0] as Chain;
}

/** "1h49" from seconds. */
export function hm(seconds: number): string {
  const m = Math.round(seconds / 60);
  return `${Math.floor(m / 60)}h${String(m % 60).padStart(2, "0")}`;
}

/** "20:35" = "18:30" + 7500 s, same day (wraps past midnight). */
export function addSeconds(time: string, seconds: number): string {
  const [h, m] = time.split(":").map(Number);
  const t = (h * 60 + m + Math.round(seconds / 60)) % (24 * 60);
  return `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`;
}
