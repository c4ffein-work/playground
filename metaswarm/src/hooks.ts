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
