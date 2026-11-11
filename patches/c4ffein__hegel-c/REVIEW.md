# hegel-c: four series to review (2026-11-11)

Cut against `c4ffein/hegel-c` main at `ceefde0`. Each directory is independent
and applies on its own; all four also apply together, in the order below, with
no conflict.

**Nothing here was built or run.** The sandbox refused `make`, so the only
check was `gcc -fsyntax-only -Wall -Wextra` on every changed C file (clean,
also with all four applied). No engine, no test suite, no pins.

| series | what | to run before keeping it |
|---|---|---|
| `2026-11-11-doc-drift` | README selftest count (34 -> 58), dead `HEGEL_ENGINE_VERSION` in `ci.yml`, "Linux only" in the README | nothing (docs and a comment) |
| `2026-11-11-draw-bool-and-url-alloc` | `HEGEL_DRAW_BOOL ()` expands to `hegel_draw_bool`; new `hegel_draw_url_alloc (tc)` | `make test`, the `bootconfig_rediscovery` pin |
| `2026-11-11-case-timeout` | a forked body still running at its deadline is killed and the case fails; on by default, 10 s | `make test`, then MPI, Scotch and Linux pins |
| `2026-11-11-graph-isolated-bench` | `hc_graph_iso.c`: the graph pair under nofork + `HEGEL_ISOLATED` | `make -C experiments/libhegel-vs-hegel-c run` |

## Decisions taken in the session

- Timeout: on by default, both modes (fork-mode case, `HEGEL_ISOLATED` child),
  default 10 s.
- `HEGEL_DRAW_BOOL ()` becomes the primitive (the choice sequence changes).
- URL: a new `hegel_draw_url_alloc`; `hegel_draw_url` unchanged.

## Choices made without asking

- `HEGEL_C_TIMEOUT_MS` in the environment overrides `timeout_ms` in
  `hegel_run_options` (like `HEGEL_C_FORK`), although the engine settings go
  the other way (typed fields over `HEGEL_*`). Reason: raising a timeout on a
  slow machine or under a debugger should not need a rebuild.
- Timeout origin: `timeout` in fork mode, the block's own origin inside
  `HEGEL_ISOLATED` (so hang-or-crash from one bug shrinks as one).
- Only the forked child is killed, not its process group: MPI workers or
  shells it started are left running.
- The timed span is wall time of the whole forked body, engine time serving
  its draws included.

## What to look at first

1. **The 10 s default against existing tests.** Any fork-mode case that
   honestly takes more than 10 s now fails. Candidates: MPI spawn tests, ASan
   kernel harnesses, fork-clean mode, fork-mode repeat loops that run to the
   choice budget.
2. **`test_timeout_hang` (TESTS_SHRINK, expects `shrunk: 8 8`).** It uses a
   300 ms timeout; on a loaded machine a slow honest case could be killed and
   the shrink come out wrong. It also assumes the last execution of a failing
   run is the final replay of the shrunk case, as `test_repeat_crash_shrinks`
   does.
3. **Shrinking a hang at the default timeout is slow**: every hanging
   execution costs 10 s. The header says so; a smarter policy (a shorter
   deadline once a run has seen one timeout) is not implemented.
4. **The bool change invalidates stored blobs** of tests using the macro
   (`bootconfig_rediscovery`, the schema experiments).

## Follow-ups when applying more than one

- `doc-drift` sets the README selftest count to 58 and `case-timeout` adds a
  59th test (it updates CLAUDE.md, not the README): with both, the README row
  is `59 (31 PASS, 8 FAIL, 4 CRASH, 8 HEALTH, 8 SHRINK)`.
- `graph-isolated-bench` adds the row to `run.sh` only. The Results block in
  the experiment's README and the figures in `docs/libhegel-vs-hegel-c.md`
  need the rerun's output.

## Seen in review, not patched

- `docs/benchmarking.md` still describes the Rust-bridge server; its numbers
  predate the current runtime.
- SPDX headers say "see hegel/LICENSE"; the file is `LICENSE` at the root.
  It is the convention CLAUDE.md documents, so it was left alone.
- `hegel_fail` / `hegel_assert` take no `tc` and use process globals: fine
  today, in the way of the multi-threaded generation item in TODO.md.
- `make test` is red about one run in thirty from the upstream float bug
  (TODO.md): worth settling before the first tag.
