# Hypothesis on libhegel: the draw-level experiment

Session notes for the two-patch series in this directory (cut against
Hypothesis master 44b82b24, v6.168.3, with libhegel / hegeltest-c 0.44.1).
The question was: can Hypothesis's Python-to-Rust engine migration
(HypothesisWorks/hypothesis issue #4740) be solved by consuming Hegel's
native engine instead of porting module by module, and how would one check
equivalence? This is the cheapest real step: libhegel linked into
`hypothesis._native`, exposed as `settings(backend="hegel")`, and measured.

## What the patches do

1. **Link libhegel into hypothesis-native.** `hegeltest-c` (the crate behind
   the `libhegel` shared library) is a normal Rust dependency of the PyO3
   extension, statically linked, and driven through its `hegel_*` C ABI
   functions from Rust, exactly as hegel-rust's `static-engine` mode does.
   `hypothesis._native.internal.hegel` wraps Engine (context + settings),
   Run (`next_test_case`), TestCase (typed draws, spans, collection sizing,
   `mark_complete`), StringGenerator, run results with reproduce blobs, and
   `test_case_from_blob` for replay. No ctypes, no per-draw marshalling
   beyond the Python call itself.
2. **`backend="hegel"`.** A `PrimitiveProvider` whose `draw_*` go to
   libhegel's generate phase. Hypothesis keeps the run loop, database,
   health checks and shrinking (shrinking replays the recorded choices
   through the Python provider, as for any alternative backend). libhegel
   runs with only its generate phase enabled, a huge `test_cases` budget,
   no database and all health checks suppressed. Plus an equivalence test
   file and a `RELEASE.rst` (minor) in case anyone wants to upstream it.

To try it: apply the series (`scripts/apply-patch.sh`), then in
`hypothesis/` run `maturin develop` (or `pip install -e hypothesis/`,
which needs cargo and crates.io) and use `settings(backend="hegel")` or
`HYPOTHESIS_PROFILE=hegel` for the test suite.

## Equivalence checks that now exist

| Check | Result |
|---|---|
| Provider contract: every libhegel draw is permitted by the Hypothesis constraints (`tests/conjecture/test_hegel_provider.py`, stressed to 3000 sequences) | passes, 0 overruns |
| Replay semantics: a Hypothesis choice sequence encoded as a libhegel reproduce blob (base64 of `0x00` + `serialize_choices`) replays through libhegel value for value for the same draw calls (3000 sequences, ~19.6k draws over integers, booleans, floats, bytes, single-range strings) | passes |
| NaN, -0.0, inf round-trip through a blob | passes |
| Pipeline: `minimal()` reaches the same minimal example as the Python engine for lists, integers, text, floats, bytes | passes |
| Hypothesis `tests/cover` (about 4000 tests) with `HYPOTHESIS_PROFILE=hegel` | 3969 passed, 15 failed (was 22 before the fixes below) |
| Hypothesis `tests/quality` under the hegel backend | see the section at the end |

The replay result is the one that matters for the migration: the two
engines agree on what a choice sequence means, so databases and
`reproduce_failure` blobs would be portable across a runner swap.

## Where the two engines differ (found by the tests)

Vocabulary differences, all mapped in the provider and documented on it:

- libhegel has no unbounded or half-bounded integer draw. The provider
  clamps to 2**128 around the bound or origin, as HypothesisProvider does.
- libhegel has no weighted integer draw. Emulated with a boolean plus an
  index; the weighted keys are picked uniformly, not by relative weight.
- libhegel wants `allow_infinity` spelled out and rejects it with finite
  bounds; Hypothesis expresses it only through an infinite bound.
- libhegel refuses `allow_nan` together with a finite bound; Hypothesis
  permits NaN regardless of bounds. Bounded-with-NaN draws never produce
  NaN under the hegel backend.
- libhegel's text generator takes a codepoint range, categories and
  include / exclude character lists, not an arbitrary interval set. Single
  ranges, ranges split only by the surrogate block, and sets with at most
  256 characters or gaps map directly; anything else (735-range `\W`
  classes, surrogates) falls back to libhegel's collection sizing plus one
  character index per element.
- `hegel_stop_span` on a discarded span can spend choice budget and return
  `HEGEL_E_STOP_TEST`; a provider has to treat span calls like draws.
- Constant injection (5% of integer / string / bytes draws and 15% of float
  draws come from a pool mined from the user's source) is a frontend
  feature, not an engine one. Without it the datetimes tests could not find
  the 2038 rollover; the provider now does it in Python with a private PRNG.
- Hypothesis's `derandomize` / `seed` do not reach libhegel; only that PRNG
  is fixed. A real integration would pass the seed through.
- PyO3 `unsendable` wrappers break under Hypothesis's threading tests,
  which drop providers from other threads; the wrappers are `Send + Sync`
  on the strength of libhegel's internal locking plus the GIL.

Distribution differences (generation only, 2000 examples each, derandomized;
these are libhegel's choices, not mapping artefacts, except the unbounded
integer row which is the 2**128 clamp meeting libhegel's bounded draw):

| strategy | statistic | hypothesis | hegel |
|---|---|---|---|
| integers() | == 0 | 0.1% | 6.0% |
| integers() | abs > 2**32 | 21.9% | 71.3% |
| integers(0, 10**6) | <= 10 | 0.5% | 15.7% |
| floats() | == 0 | 0.1% | 14.8% |
| floats() | finite, abs <= 1e3 | 25.5% | 52.8% |
| floats(0, 1) | == 0 / == 1 | 0.1% / 0.1% | 14.7% / 4.2% |
| text() | empty | 0.1% | 13.3% |
| text() | ascii only | 9.1% | 48.4% |
| text() | distinct values | 2000 | 820 |
| lists(integers()) | empty | 0.1% | 10.3% |
| binary() | empty | 0.1% | 15.0% |

libhegel is much more biased towards the simplest values than the current
Python engine, which moved to smooth distributions in 2025-26. Neither is
"right", but the quality suite (below) measures which finds bugs.

Speed, generation only (2000 examples, ms per example, Python provider
versus hegel backend): integers 0.47 vs 0.87, text 0.63 vs 0.60, lists of
integers 1.15 vs 1.48, floats 0.56 vs 0.60, dicts 1.98 vs 2.34. A draw-level
backend cannot be faster than the Python provider: every draw still costs a
Python call, plus the PyO3 hop. The performance case for the migration only
exists at the runner level, where the engine loop, the choice tree and the
shrinker leave Python.

## The 15 remaining `tests/cover` failures, by cause

- **Search-space exhaustion is an engine feature the draw level cannot
  see** (5): `test_notes_exhausted_search_space_in_unsatisfiable_error`,
  `test_unsatisfiable_explicit_filteredstrategy_sampled`,
  `test_unsat_filtered_sampling_in_rejection_stage`,
  `test_raises_unsatisfiable_if_all_false_in_finite_set`,
  `test_given_usable_inline_on_lambdas` (expects `booleans()` to stop after
  2 examples). The Python engine's DataTree knows the space is exhausted;
  libhegel has the same knowledge internally but a provider never sees it.
  Two of these are already `xfail_on_crosshair`.
- **Alternative-backend semantics in the engine** (6): the flakiness and
  slippage tests (`test_fails_differently_is_flaky`,
  `test_gives_flaky_error_if_assumption_is_flaky`,
  `test_flaky_stateful_reports_steps`,
  `test_handles_flaky_tests_where_only_one_is_flaky`, `test_flaky_exit`) and
  `test_error_is_in_finally`. With any backend other than "hypothesis" the
  engine re-executes a failure through its own provider before trusting it,
  which changes execution counts and turns a `finally: raise` that masks a
  StopTest into a `FlakyBackendFailure`. crosshair carries the same marks.
- **By design** (1): `test_find_uses_provided_random` (the provided Random
  is not the source of libhegel's entropy; also xfail on crosshair).
- **Distribution / discovery** (3): `test_triangular_modes` (needs both
  sides of 0.5 from `randoms()`), `test_fullmatch_generates_example[[ab]*]`
  and `test_generates_unix_rollover_adjacent_times` (found sometimes, not
  every run). All are "did 100 examples hit X", and libhegel's bias towards
  simplest values makes that flaky.

Nothing in the remaining list is a wrong value or a crash.

## What this says about the migration plan

- The draw-level flag works, is cheap, and is a good differential-testing
  harness. It is not a migration path: the things that would make the
  engine swap worthwhile (the run loop, the tree and exhaustion tracking,
  shrinking, the database, targeting) all sit above the draw boundary.
- The next experiment is the runner-level flag: substitute
  `ConjectureRunner` (constructed in one place, `core.py`) with a
  libhegel-driven runner, keep `ConjectureData` as the shim strategies draw
  from, and keep the Python runner selected whenever `settings.backend` is
  not "hypothesis" until libhegel grows a delegating backend for CrossHair.
  The replay-blob test above is the seam that makes this checkable.
- Things libhegel would need first: a seed / derandomize hand-off, NaN with
  finite bounds (or an agreed rule), weighted integers, an arbitrary
  codepoint-set text generator (or a way to pass Hypothesis's category
  queries), and an `explain`-phase equivalent or an agreement that it stays
  in Python.
- The distribution gap is a product decision, not a bug: the Python
  engine's smooth distributions are recent and deliberate; libhegel is a
  port of the older shape. The quality suite is where that argument should
  be had.

## tests/quality under the hegel backend

(filled in below once the run finished)
