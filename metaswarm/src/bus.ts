// Message bus: an append-only SQLite log that every agent reads and writes.
// The log is the source of truth: messages, role changes and tool calls all land here.
import { Database } from "bun:sqlite";

export type Role = "overseer" | "worker";

export interface Message {
  id: number;
  ts: string;
  from: string; // agent id, or "harness"
  to: string; // agent id, "@overseer", "@worker" or "*"
  kind: string; // task | result | question | role_request | role_change | note
  body: string;
}

export class Bus {
  private db: Database;
  private listeners = new Set<() => void>();

  constructor(path: string) {
    this.db = new Database(path, { create: true });
    this.db.run("PRAGMA journal_mode = WAL");
    this.db.run(`CREATE TABLE IF NOT EXISTS messages (
      id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
      "from" TEXT NOT NULL, "to" TEXT NOT NULL, kind TEXT NOT NULL, body TEXT NOT NULL)`);
    this.db.run(`CREATE TABLE IF NOT EXISTS events (
      id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
      agent TEXT NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL)`);
  }

  send(from: string, to: string, kind: string, body: string): Message {
    const ts = new Date().toISOString();
    const row = this.db
      .query(`INSERT INTO messages (ts, "from", "to", kind, body) VALUES (?, ?, ?, ?, ?) RETURNING id`)
      .get(ts, from, to, kind, body) as { id: number };
    const msg = { id: row.id, ts, from, to, kind, body };
    for (const fn of this.listeners) fn();
    return msg;
  }

  /** Messages addressed to this agent (directly, by role, or broadcast) after a cursor. */
  inbox(agent: string, role: Role, afterId: number): Message[] {
    return this.db
      .query(
        `SELECT id, ts, "from", "to", kind, body FROM messages
         WHERE id > ? AND "from" != ? AND ("to" = ? OR "to" = ? OR "to" = '*')
         ORDER BY id`,
      )
      .all(afterId, agent, agent, `@${role}`) as Message[];
  }

  /** Record a non-message event (tool call, model call, error) for later replay. */
  event(agent: string, type: string, data: unknown) {
    this.db
      .query(`INSERT INTO events (ts, agent, type, data) VALUES (?, ?, ?, ?)`)
      .run(new Date().toISOString(), agent, type, JSON.stringify(data));
  }

  onMessage(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  close() {
    this.db.close();
  }
}
