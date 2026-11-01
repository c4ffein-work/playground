// Hooks: user commands that run on harness events, like Claude Code's hooks.
// A hook gets the event as JSON on stdin. For PreToolUse it may answer with JSON
// on stdout: {"decision": "deny", "reason": "..."} blocks the call,
// {"decision": "ask"} sends it to the user for approval, and
// {"input": {...}} replaces the tool input. Exit code 2 also denies (stderr = reason).
//
// Hooks are configuration written by the user, so they run on the host, not in
// a container. Agents cannot add hooks.

export type HookEvent = "PreToolUse" | "PostToolUse" | "AgentIdle" | "Notice" | "RunEnd";

export interface HookConfig {
  event: HookEvent;
  /** Regex on the tool name (tool events) or agent id (others). Default: everything. */
  matcher?: string;
  /** argv; run with cwd = the run directory. */
  command: string[];
  timeoutS?: number; // default 10
  /** Where it came from, for logs (a plugin name, "config"...). */
  source?: string;
}

export interface PreToolResult {
  decision?: "allow" | "deny" | "ask";
  reason?: string;
  input?: unknown;
}

export interface HookRun {
  hook: HookConfig;
  code: number;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

export class Hooks {
  constructor(
    readonly configs: HookConfig[],
    private cwd: string,
    private onRun: (r: HookRun, event: HookEvent) => void = () => {},
  ) {}

  private matching(event: HookEvent, subject: string): HookConfig[] {
    return this.configs.filter((h) => h.event === event && (!h.matcher || new RegExp(h.matcher).test(subject)));
  }

  private async exec(h: HookConfig, payload: unknown): Promise<HookRun> {
    const proc = Bun.spawn(h.command, { cwd: this.cwd, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    proc.stdin.write(JSON.stringify(payload));
    proc.stdin.end();
    let timedOut = false;
    const t = setTimeout(() => {
      timedOut = true;
      proc.kill("SIGKILL");
    }, (h.timeoutS ?? 10) * 1000);
    const [stdout, stderr, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
    clearTimeout(t);
    return { hook: h, code, stdout, stderr, timedOut };
  }

  /** Fire-and-forget style events. Errors are logged, never thrown. */
  async emit(event: Exclude<HookEvent, "PreToolUse">, subject: string, payload: object) {
    for (const h of this.matching(event, subject)) {
      try {
        this.onRun(await this.exec(h, { event, ...payload }), event);
      } catch (e) {
        this.onRun({ hook: h, code: -1, stdout: "", stderr: String(e), timedOut: false }, event);
      }
    }
  }

  /**
   * Run PreToolUse hooks in order. The first deny wins; "ask" is remembered;
   * an `input` replaces the tool input for the following hooks and the call.
   * A hook that crashes or times out denies the call: hooks are policy, so they fail closed.
   */
  async preToolUse(agent: string, tool: string, input: unknown): Promise<{ decision: "allow" | "deny" | "ask"; reason: string; input: unknown }> {
    let decision: "allow" | "ask" = "allow";
    let reason = "";
    for (const h of this.matching("PreToolUse", tool)) {
      let r: HookRun;
      try {
        r = await this.exec(h, { event: "PreToolUse", agent, tool, input });
      } catch (e) {
        r = { hook: h, code: -1, stdout: "", stderr: String(e), timedOut: false };
      }
      this.onRun(r, "PreToolUse");
      const who = h.source ? `hook (${h.source})` : "hook";
      if (r.code === 2) return { decision: "deny", reason: `${who}: ${r.stderr.trim() || "denied"}`, input };
      if (r.code !== 0) return { decision: "deny", reason: `${who} failed (exit ${r.code}${r.timedOut ? ", timed out" : ""})`, input };
      let out: PreToolResult = {};
      if (r.stdout.trim()) {
        try {
          out = JSON.parse(r.stdout);
        } catch {
          return { decision: "deny", reason: `${who} printed invalid JSON`, input };
        }
      }
      if (out.input !== undefined) input = out.input;
      if (out.decision === "deny") return { decision: "deny", reason: `${who}: ${out.reason ?? "denied"}`, input };
      if (out.decision === "ask") {
        decision = "ask";
        reason = `${who}: ${out.reason ?? "asks for approval"}`;
      }
    }
    return { decision, reason, input };
  }
}
