// Approvals: tool calls waiting for the user, because the agent's right is an
// "ask" grant or a hook asked. The agent blocks until the user decides in the
// viewer, or until the request expires (which counts as a refusal).
import { now, type Store } from "./store.ts";

export interface Approval {
  id: number;
  agent: string;
  tool: string;
  input: unknown;
  right: string;
  reason: string;
  ts: string;
  status: "pending" | "approved" | "denied" | "expired";
  note: string | null;
}

export class Approvals {
  private waiting = new Map<number, (a: Approval) => void>();

  constructor(private store: Store, private timeoutMs = 10 * 60_000) {
    store.db.run(`CREATE TABLE IF NOT EXISTS approvals (id INTEGER PRIMARY KEY AUTOINCREMENT, agent TEXT NOT NULL, tool TEXT NOT NULL,
      input TEXT NOT NULL, "right" TEXT NOT NULL, reason TEXT NOT NULL, ts TEXT NOT NULL, status TEXT NOT NULL, note TEXT)`);
    // Requests left pending by a previous process can't be answered any more.
    store.db.run(`UPDATE approvals SET status = 'expired' WHERE status = 'pending'`);
  }

  all(limit = 100): Approval[] {
    return (this.store.db.query(`SELECT * FROM approvals ORDER BY id DESC LIMIT ?`).all(limit) as any[]).map((r) => ({ ...r, input: JSON.parse(r.input) }));
  }

  get(id: number): Approval | undefined {
    return this.all(1_000_000).find((a) => a.id === id);
  }

  /** Create a request and wait for the decision. */
  request(agent: string, tool: string, input: unknown, right: string, reason: string, signal?: AbortSignal): Promise<Approval> {
    const { id } = this.store.db
      .query(`INSERT INTO approvals (agent, tool, input, "right", reason, ts, status) VALUES (?, ?, ?, ?, ?, ?, 'pending') RETURNING id`)
      .get(agent, tool, JSON.stringify(input), right, reason, now()) as { id: number };
    this.store.log(agent, "approval.request", { id, tool, input, right, reason });
    return new Promise((resolve) => {
      const done = (a: Approval) => {
        clearTimeout(t);
        signal?.removeEventListener("abort", onAbort);
        this.waiting.delete(id);
        resolve(a);
      };
      const t = setTimeout(() => this.settle(id, "expired", "no answer in time"), this.timeoutMs);
      const onAbort = () => this.settle(id, "expired", "agent interrupted");
      signal?.addEventListener("abort", onAbort, { once: true });
      this.waiting.set(id, done);
    });
  }

  decide(id: number, approve: boolean, note?: string) {
    const a = this.get(id);
    if (!a) throw new Error(`no approval #${id}`);
    if (a.status !== "pending") throw new Error(`approval #${id} is already ${a.status}`);
    this.settle(id, approve ? "approved" : "denied", note ?? null);
  }

  private settle(id: number, status: Approval["status"], note: string | null) {
    this.store.db.query(`UPDATE approvals SET status = ?, note = ? WHERE id = ? AND status = 'pending'`).run(status, note, id);
    const a = this.get(id)!;
    this.store.log("user", "approval.decided", { id, status: a.status, note });
    this.waiting.get(id)?.(a);
  }

  pending(): number {
    return (this.store.db.query(`SELECT COUNT(*) AS n FROM approvals WHERE status = 'pending'`).get() as any).n;
  }
}
