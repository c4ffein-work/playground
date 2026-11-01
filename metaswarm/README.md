# metaswarm (PoC)

A small harness where several Claude conversations work together. Each agent is
an **overseer** or a **worker**, they talk through a shared message bus, and
roles can change mid-run. Tool calls only ever run inside Docker containers.

```sh
bun install
bun run src/main.ts --scripted           # offline demo, no API key needed
ANTHROPIC_API_KEY=... bun run src/main.ts "your goal here"
```

Env: `METASWARM_IMAGE` (default `alpine:3.20`), `METASWARM_MODEL` (default `claude-opus-5-5`).

## How it works

- **Bus** (`src/bus.ts`): an append-only SQLite log under `runs/`. It stores every
  message plus every turn, tool call and error, so a run can be inspected afterwards.
  Addresses: an agent id, `@overseer`, `@worker`, or `*`.
- **Agent loop** (`src/swarm.ts`): one Claude tool-use loop per agent. New mail is
  appended to the agent's next user turn. An agent with nothing to do ends its turn
  and sleeps until mail arrives.
- **Roles** (`src/roles.ts`): every agent sees the same tools and system prompt, so the
  prompt prefix stays cacheable. The harness checks the current role before running any
  tool. Role changes are announced in a `system` message, a channel peers can't forge.
  - Overseer tools: `spawn_worker`, `set_role`, `finish`
  - Worker tools: `bash`
  - Everyone: `send_message`, `request_role`
- **Role switching**: an agent can only *ask* (`request_role`); an overseer grants it
  (`set_role`). The last overseer can't be demoted. A promoted agent loses its container,
  and a demoted one gets a fresh container on its next `bash` call.
- **Sandbox** (`src/sandbox.ts`): one container per worker, sharing a `/workspace` volume.
  Locked down by default: no network, read-only root filesystem, uid 1000, all
  capabilities dropped, memory/CPU/process limits, and a 30s limit per command.
  Any backend implementing `Sandbox` can replace Docker.
- **Guards**: limits on agent count and model calls per agent. A watchdog nudges the
  overseers when everyone is idle, then ends the run.

## The scripted demo

`boss` (overseer) spawns `alice` and `bob`. alice writes `notes.md`, reports, and asks
to become an overseer. boss approves. alice then tries `bash` and is refused, since
overseers have no sandbox. bob sums 1..100 and confirms the sandbox has no network.
alice checks in with bob and reports to boss, and boss finishes the run.

## Not done yet

- Context management for long runs (compaction or summarized hand-offs)
- A replay/viewer for the `runs/*.sqlite` logs
- Streaming, and running agents as separate processes
- Shared-volume races (workers can overwrite each other's files)
