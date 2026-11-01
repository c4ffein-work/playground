// The store: one SQLite database per run. Every state change is also appended
// to `events`, which is what the viewer streams and what a replay would read.
import { Database } from "bun:sqlite";

export interface Event {
  id: number;
  ts: string;
  actor: string;
  type: string;
  data: unknown;
}

export const now = () => new Date().toISOString();

export class Store {
  readonly db: Database;
  private listeners = new Set<(e: Event) => void>();

  constructor(path: string) {
    this.db = new Database(path, { create: true });
    this.db.run("PRAGMA journal_mode = WAL");
    this.db.run(`CREATE TABLE IF NOT EXISTS events (
      id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
      actor TEXT NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL)`);
  }

  log(actor: string, type: string, data: unknown): Event {
    const ts = now();
    const { id } = this.db
      .query(`INSERT INTO events (ts, actor, type, data) VALUES (?, ?, ?, ?) RETURNING id`)
      .get(ts, actor, type, JSON.stringify(data)) as { id: number };
    const e = { id, ts, actor, type, data };
    for (const fn of this.listeners) fn(e);
    return e;
  }

  events(afterId = 0, limit = 1000): Event[] {
    return (
      this.db.query(`SELECT * FROM events WHERE id > ? ORDER BY id LIMIT ?`).all(afterId, limit) as {
        id: number; ts: string; actor: string; type: string; data: string;
      }[]
    ).map((r) => ({ ...r, data: JSON.parse(r.data) }));
  }

  on(fn: (e: Event) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  close() {
    this.db.close();
  }
}
