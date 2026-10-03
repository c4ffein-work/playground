// Refresh the selected cinemas' programme for the next N days.
// One GET per (cinema, day), sequential with a small pause — polite, and
// tiny: 3 cinemas × 8 days = 24 requests.
import type { Database } from "bun:sqlite";
import { fetchShowings, isoDate, nextDays } from "./ugc";
import { ingestDay, listCinemas, openDb, pruneBefore, setSetting } from "./db";

export type RefreshResult = { days: number; cinemas: number; showings: number; errors: string[] };

export async function refresh(
  db: Database,
  opts: { days?: number; pauseMs?: number; fetch?: typeof fetchShowings; log?: (s: string) => void } = {},
): Promise<RefreshResult> {
  const days = nextDays(opts.days ?? 8);
  const pause = opts.pauseMs ?? 400;
  const get = opts.fetch ?? fetchShowings;
  const log = opts.log ?? (() => {});
  const cinemas = listCinemas(db).filter((c) => c.selected);
  const res: RefreshResult = { days: days.length, cinemas: cinemas.length, showings: 0, errors: [] };
  pruneBefore(db, isoDate(new Date()));
  for (const c of cinemas) {
    for (const d of days) {
      try {
        const { films, showings } = await get(c.id, d);
        ingestDay(db, c.id, d, films, showings);
        const n = showings.filter((s) => s.cinemaId === c.id && s.date === d).length;
        res.showings += n;
        log(`${c.name} ${d}: ${n} séances`);
      } catch (e) {
        const msg = `${c.name} ${d}: ${(e as Error).message}`;
        res.errors.push(msg);
        log("ERROR " + msg);
      }
      if (pause) await Bun.sleep(pause);
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
