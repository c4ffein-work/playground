// Sandboxes: the only place tool calls run. The harness itself never touches the host.
// Docker is the first backend; anything implementing Sandbox can replace it.

export interface ExecResult {
  code: number;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

export interface Sandbox {
  exec(command: string, timeoutS?: number): Promise<ExecResult>;
  destroy(): Promise<void>;
}

export interface SandboxFactory {
  create(agent: string): Promise<Sandbox>;
  /** Tear down anything shared (volumes). */
  shutdown(): Promise<void>;
}

const MAX_OUTPUT = 8_000;

async function run(args: string[], timeoutMs = 60_000): Promise<ExecResult> {
  const proc = Bun.spawn(["docker", ...args], { stdout: "pipe", stderr: "pipe" });
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    proc.kill("SIGKILL");
  }, timeoutMs);
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  clearTimeout(timer);
  return { code, stdout, stderr, timedOut };
}

function clip(s: string): string {
  return s.length > MAX_OUTPUT ? s.slice(0, MAX_OUTPUT) + `\n…[${s.length - MAX_OUTPUT} chars cut]` : s;
}

export interface DockerOptions {
  image: string;
  runId: string;
  network?: "none" | "bridge"; // default none
  memory?: string; // default 256m
  cpus?: string; // default 0.5
}

/**
 * One locked-down container per agent, plus one volume shared by all of them
 * at /workspace, which is how workers hand files to each other.
 */
export class DockerSandboxes implements SandboxFactory {
  private volume: string;
  private ready: Promise<void> | null = null;
  private live = new Set<DockerSandbox>();

  constructor(private opts: DockerOptions) {
    this.volume = `metaswarm-${opts.runId}`;
  }

  private init(): Promise<void> {
    // A fresh volume is root-owned; hand it to the unprivileged user once.
    this.ready ??= (async () => {
      const r = await run(["volume", "create", this.volume]);
      if (r.code !== 0) throw new Error(`docker volume create failed: ${r.stderr}`);
      const c = await run(["run", "--rm", "--network", "none", "-v", `${this.volume}:/w`, this.opts.image, "chown", "1000:1000", "/w"]);
      if (c.code !== 0) throw new Error(`volume init failed: ${c.stderr}`);
    })();
    return this.ready;
  }

  async create(agent: string): Promise<Sandbox> {
    await this.init();
    const name = `metaswarm-${this.opts.runId}-${agent}`.replace(/[^a-zA-Z0-9_.-]/g, "_");
    const r = await run([
      "run", "-d", "--name", name,
      "--network", this.opts.network ?? "none",
      "--read-only", "--tmpfs", "/tmp:rw,size=64m",
      "--user", "1000:1000",
      "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
      "--memory", this.opts.memory ?? "256m", "--cpus", this.opts.cpus ?? "0.5",
      "--pids-limit", "128",
      // No `-w` here: `docker run -w` re-owns the volume root as root.
      "-v", `${this.volume}:/workspace`,
      "--label", `metaswarm.run=${this.opts.runId}`,
      this.opts.image, "sleep", "infinity",
    ]);
    if (r.code !== 0) throw new Error(`docker run failed: ${r.stderr}`);
    const sb = new DockerSandbox(name, () => this.live.delete(sb));
    this.live.add(sb);
    return sb;
  }

  async shutdown() {
    await Promise.all([...this.live].map((s) => s.destroy()));
    if (this.ready) await run(["volume", "rm", "-f", this.volume]);
  }
}

class DockerSandbox implements Sandbox {
  constructor(private name: string, private onDestroy: () => void) {}

  async exec(command: string, timeoutS = 30): Promise<ExecResult> {
    // `timeout` inside the container kills the command itself; the outer
    // timer is a backstop in case docker exec hangs.
    const r = await run(
      ["exec", "-w", "/workspace", this.name, "timeout", "-s", "KILL", String(timeoutS), "sh", "-c", command],
      (timeoutS + 10) * 1000,
    );
    return { ...r, timedOut: r.timedOut || r.code === 137, stdout: clip(r.stdout), stderr: clip(r.stderr) };
  }

  async destroy() {
    this.onDestroy();
    await run(["rm", "-f", this.name]);
  }
}
