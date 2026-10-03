// Refresh the selected cinemas' programme for the next N days.
// UGC: one GET per (cinema, day). CGR: one schedule call per cinema for the
// whole range, plus one movies call per 50 films. Sequential with a small
// pause — polite, and tiny: 3 UGC × 8 days + 2 CGR ≈ 30 requests.
import type { Database } from "bun:sqlite";
import { fetchShowings, isoDate, nextDays } from "./ugc";
import { fetchCgrProgramme } from "./cgr";
import { ingestCinema, listCinemas, openDb, pruneBefore, setSetting } from "./db";
import type { Film, Showing } from "./types";

export type RefreshResult = { days: number; cinemas: number; showings: number; errors: string[] };
export type Fetchers = {
  ugc: (cinemaId: number, date: string) => Promise<{ films: Film[]; showings: Showing[] }>;
  cgr: (theaterId: string, from: string, to: string) => Promise<{ films: Film[]; showings: Showing[] }>;
};
const LIVE: Fetchers = { ugc: fetchShowings, cgr: fetchCgrProgramme };

export async function refresh(
  db: Database,
  opts: { days?: number; pauseMs?: number; fetchers?: Partial<Fetchers>; log?: (s: string) => void } = {},
): Promise<RefreshResult> {
  const days = nextDays(opts.days ?? 8);
  const pause = opts.pauseMs ?? 400;
  const f: Fetchers = { ...LIVE, ...(opts.fetchers ?? {}) };
  const log = opts.log ?? (() => {});
  const cinemas = listCinemas(db).filter((c) => c.selected);
  const res: RefreshResult = { days: days.length, cinemas: cinemas.length, showings: 0, errors: [] };
  pruneBefore(db, isoDate(new Date()));
  const fail = (msg: string) => { res.errors.push(msg); log("ERROR " + msg); };
  for (const c of cinemas) {
    const bare = c.id.slice(c.chain.length + 1);
    if (c.chain === "ugc") {
      for (const d of days) {
        try {
          const { films, showings } = await f.ugc(Number(bare), d);
          ingestCinema(db, c.id, [d], films, showings);
          const n = showings.filter((s) => s.cinemaId === c.id && s.date === d).length;
          res.showings += n;
          log(`${c.name} ${d}: ${n} séances`);
        } catch (e) { fail(`${c.name} ${d}: ${(e as Error).message}`); }
        if (pause) await Bun.sleep(pause);
      }
    } else if (c.chain === "cgr") {
      try {
        const { films, showings } = await f.cgr(bare, days[0], days[days.length - 1]);
        ingestCinema(db, c.id, days, films, showings);
        const n = showings.filter((s) => s.cinemaId === c.id && days.includes(s.date)).length;
        res.showings += n;
        log(`${c.name} ${days[0]}..${days[days.length - 1]}: ${n} séances`);
      } catch (e) { fail(`${c.name}: ${(e as Error).message}`); }
      if (pause) await Bun.sleep(pause);
    } else {
      fail(`${c.name}: unknown chain ${c.chain}`);
    }
  }
  setSetting(db, "last_refresh", JSON.stringify({ at: Date.now(), ...res }));
  return res;
}

if (import.meta.main) {
  const db = openDb(process.env.SEANCES_DB ?? "seances.db");
  const r = await refresh(db, { days: Number(process.env.SEANCES_DAYS ?? 8), log: console.log });
  console.log(`done: ${r.showings} séances over ${r.cinemas} cinemas × ${r.days} days, ${r.errors.length} errors`);
  process.exit(r.errors.length && !r.showings ? 1 : 0);
}
