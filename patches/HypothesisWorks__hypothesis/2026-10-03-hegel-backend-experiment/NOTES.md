# Hypothesis on libhegel: from a draw-level backend to a runner-level engine

Session notes for two patch series: this one against Hypothesis master
(44b82b24, v6.168.3) and `patches/hegeldev__hegel-rust/2026-10-03-hypothesis-needs`
against hegel-rust main (ebfd9d53, hegeltest 0.48.1 / libhegel 0.44.1).
The question was whether Hypothesis's Python-to-Rust engine migration
(HypothesisWorks/hypothesis issue #4740) can be solved by consuming Hegel's
native engine, and how to check equivalence. Three rounds:

1. **Draw level** (2026-09-29): libhegel linked into `hypothesis._native`
   and exposed as `settings(backend="hegel")`; Hypothesis keeps the loop.
2. **Engine level** (2026-10-01): three additions to libhegel itself, then
   `HYPOTHESIS_ENGINE=hegel` hands libhegel the whole run loop (generation,
   shrinking, targeting, the choice budget) while Python keeps
   `ConjectureData`, test execution and reporting.
3. **Whole strategies natively** (2026-10-03): the built-in strategies
   compiled to a small IR and interpreted in Rust against the libhegel
   case, so a test's inputs are generated in one crossing instead of one
   per primitive; everything else falls back to the interactive path. Plus
   the forced-draw gap closed and a 50x fix to libhegel's unbounded
   integer draw. See "Round three" below.

## The patches

hegel-rust, four commits:

- `hegel_generate_integer_weighted(min, max, shrink_towards, keys, weights)`:
  one plain integer choice whose fresh draws take `keys[i]` with probability
  `weights[i]` and shrink toward `shrink_towards`. Replay and shrinking never
  consult the weights. (HypothesisProvider's `weights` and `shrink_towards`;
  the existing `hegel_generate_integer` always shrinks toward 0.)
- `allow_nan` alongside finite float bounds is accepted: NaN comes with the
  usual NaN probability, in-range values otherwise (Hypothesis's rule).
- `hegel_string_generator_text_ranges`: an alphabet given as explicit sorted,
  disjoint codepoint ranges, i.e. Hypothesis's `IntervalSet` as it is.
- Header and frontend FFI list regenerated; C ABI tests for the three.
- `hegel_test_case_choices`: the choices a live case has made so far, as
  the same base64 blob a failure's reproduction uses (round three: the
  frontend no longer sees individual draws, so it asks).
- `biguint_sample_in_range` builds its "diffuse" pool (256 bignum powers of
  two, sorted) only when that category is selected, as the "interesting"
  pool already was. Same RNG consumption, same values; an unbounded
  integer draw goes from ~160 us to ~3 us. Hypothesis's `integers()` is
  the most common strategy and clamps to 2**128, past libhegel's i128
  fast path, so this was most of the engine's cost per example.
- `hegel_generate_integer_forced(min, max, forced)`: records a
  caller-chosen value as a forced integer choice, as the forced boolean
  already could. For the draws Hypothesis forces (filtered
  `sampled_from`, feature flags, `many` at its bounds) and, later, its
  constant injection.

Hypothesis, four commits:

1. Link `hegeltest-c` (a path dependency on the fork) into the PyO3 extension
   and expose the pieces of its C ABI a runner needs: engine and settings
   (phases, health checks, seed, multiple failures, nondeterminism
   strictness), the run loop, typed draws including the new ones, spans,
   collection sizing, targeting, completion, results with blobs, blob replay.
2. `backend="hegel"`: a `PrimitiveProvider` whose `draw_*` go to libhegel,
   now one-to-one with the new primitives. Replay-equivalence tests.
3. `HYPOTHESIS_ENGINE=hegel`: `core.py` builds a `HegelRunner` instead of
   `ConjectureRunner` for tests on the default backend. It implements what
   `core.py` reads (counters, interesting cases, statistics, exit reason,
   `new_conjecture_data`), maps settings onto libhegel's, executes each
   libhegel case through the draw-level provider bound to that case, and
   translates libhegel's run errors (exhausted space, health checks, a
   nondeterministic test) into Hypothesis's `Unsatisfiable`,
   `FailedHealthCheck` and `FlakyFailure`.
4. Whole strategies natively: `hegel_ir.py` compiles the declarative
   strategies into a tree, `hegel_gen.rs` interprets it against the live
   libhegel case, `ConjectureData.draw` consults the provider's
   `draw_strategy` first and lets it see forced draws (`draw_forced`);
   `HYPOTHESIS_HEGEL_BATCH=0` turns batching off for comparison.
   `tests/conjecture/test_hegel_ir.py` is the equivalence check.

To try: apply both series (`scripts/apply-patch.sh`), build hegel-rust's
`hegel-c` once (the Hypothesis crate points at it by path), `maturin develop`
in `hypothesis/`, then `settings(backend="hegel")`, `HYPOTHESIS_PROFILE=hegel`
for the suite, or `HYPOTHESIS_ENGINE=hegel` for the engine flag.

## Equivalence checks

| Check | Draw level | Engine level |
|---|---|---|
| Provider contract: every libhegel draw permitted by its constraints (3000 random sequences) | passes | same provider |
| Replay: a Hypothesis choice sequence encoded as a libhegel blob replays through libhegel value for value, now for every kind of choice: weights, NaN with finite bounds, multi-range alphabets (3000 sequences, ~17k draws, 383 weighted, 1724 bounded-NaN, 2241 multi-range strings) | passes | same |
| `minimal()` reaches the Python engine's minimum for lists, integers, text, floats, bytes, unique lists | passes (Hypothesis shrinks) | passes (libhegel shrinks) |
| `tests/cover` (3985 tests) | 3967 passed, 17 failed | 3938 passed, 46 failed |
| `tests/quality` | 254 passed, 15 failed | 240 passed, 30 failed |

The replay result is the one that makes an engine swap checkable: both
engines agree on what a choice sequence means, so databases and reproduce
blobs are portable across them, and the quality suite can compare shrinkers
on equal terms.

## Round three: whole strategies natively

The question from round two was whether the whole logic of a test case
should move to Rust rather than one primitive draw at a time. The census
first: over the strategy draws `tests/cover` makes, three quarters are
of built-in strategies whose draw is a pure function of their attributes
(integers, text, lists, tuples, fixed dictionaries, one_of, sampled_from,
map, builds, filter...), a sixth are `composite` / `data` / stateful
draws that run arbitrary Python between draws, and the rest are
specialised strategies with their own Python logic (unique lists,
recursive, dates, regex). So the engine can own most of the work without
owning any user code.

Design:

- `hegel_ir.py` walks a strategy object graph and emits a tuple tree, one
  node per built-in `do_draw` (`int`, `float`, `bool`, `bytes`, `text`,
  `list`, `tuple`, `fixeddict`, `one_of`, `sampled`, `just`, `map`,
  `builds`, `shared`, `filter`, plus `text_list` and `empty_list`
  specialisations). A strategy it cannot express becomes an `opaque` leaf
  holding the strategy object; a root that is opaque is "not batchable".
  Compiled once per strategy object and cached on it.
- `hegel_gen.rs` interprets the tree against the libhegel case: spans with
  Hypothesis's labels, the `many` continuation protocol with its forced
  booleans, the fixed-dictionary shuffle, the filter rule (three tries then
  invalid) and the map rule (three retries on `UnsatisfiedAssumption`),
  `shared` through `ConjectureData`'s shared-draw table. It records
  `(kind, value)` pairs and flushes them to `ConjectureData` as choice
  nodes before any Python callback and at the end. An opaque leaf is
  handed back to `ConjectureData.draw`, which runs the strategy's own
  `do_draw` interactively against the same case, and any batchable
  strategy drawn underneath it is batched again.
- `ConjectureData.draw` asks the provider's `draw_strategy` before a
  strategy's `do_draw`; `ConjectureData._draw` tells a provider about
  forced draws (`draw_forced`) so a provider that keeps its own choice
  sequence stays aligned. The runner takes the case's nodes from
  libhegel's own record (`choices_blob`) rather than Python's.

The equivalence check, `test_a_batched_value_is_what_the_python_engine_draws_from_its_choices`:
for 38 strategies covering every node kind, the opaque fallbacks and
mixtures of the two, libhegel generates 60 cases each; for every case the
Python engine replays libhegel's recorded choice sequence through its own
`do_draw` code (`ConjectureData.for_choices`) and must arrive at the same
value, without a misaligned draw. It passes. Two of its failures along the
way were real: Hypothesis shuffles fixed-dictionary key order with a
Fisher-Yates pass that draws integers (issue 3906, now mirrored), and
forced draws never reached libhegel (now they do, through the new entry
point). It also pins the semantics in both directions: a reproduce blob
from either engine means the same test input to the other.

### Speed, round three

Whole loop on an idle machine, 2000 generated examples (`derandomize`,
`phases=[generate]`), find + shrink with 500. "Interactive" is the
round-two runner (`HYPOTHESIS_HEGEL_BATCH=0`), "batched" the interpreter,
both with the libhegel integer fix. Both libhegel columns are the same
runner, so the shrink call counts are identical; their time differs by
generation cost only.

| case | python | libhegel, interactive | libhegel, batched |
|---|---|---|---|
| generation, integers | 0.50 ms/ex | 0.27 | 0.27 |
| generation, text | 0.49 ms/ex | 0.31 | 0.32 |
| generation, lists of ints | 1.04 ms/ex | 0.57 | 0.41 |
| generation, dicts(text, floats) | 1.78 ms/ex | 1.14 | 0.90 |
| generation, tuples / one_of / builds | 1.29 ms/ex | 0.82 | 0.35 |
| generation, datetimes | 0.63 ms/ex | 0.49 | 0.43 |
| find + shrink `sum(list) > 1000` | 0.10 s, 50 calls | 1.82 s, 821 calls | 1.13 s, 821 calls |
| find + shrink text with 'a', len 10 | 0.26 s, 182 calls | 0.58 s, 374 calls | 0.61 s, 374 calls |
| find + shrink dict with 3 keys | 1.57 s, 429 calls | 4.87 s, 1438 calls | 3.89 s, 1438 calls |

On this mix 91% of strategy draws were batched (56672 against 5561
interactive, the latter the `datetimes` and `dictionaries` roots whose
inner draws are batched anyway). Three things to read from the table:

- **The round-two "libhegel is slower to generate" was libhegel's
  unbounded integer draw**, not the architecture: with the diffuse pool
  built lazily the interactive runner is already 1.5-2x faster than the
  Python engine per example, where round two had it 1.1-1.5x slower.
- **Batching pays in proportion to the draws per example**: nothing for
  one integer, 2.5x for the structured tuple, 3.7x against the Python
  engine there. Per-example overhead that remains is Hypothesis's own
  (`ConjectureData`, the build context, `deterministic_PRNG`, statistics;
  ~0.25 ms of the 0.27).
- **Shrinking is still libhegel's call count**: 2-16x more test calls
  than Hypothesis's shrinker on these three, as in round two. The
  interpreter makes each call cheaper, nothing more. A result cache by
  choice sequence and the missing passes are the engine's work.

Direct measurements of the engine, per case, through the extension
(`tests` with 3000 cases): an empty libhegel case costs 3.6 us, a bounded
integer draw 0.1-0.4 us on top, `integers()` through the interpreter
7.5 us, `text()` 7.5 us, `lists(integers())` 19 us. The engine is no
longer where a test's time goes.

### Suites, round three

| suite | round two | round three |
|---|---|---|
| `tests/cover` | 3938 passed, 46 failed | 3915 passed, 69 failed (3 workers, 6.5 minutes) |
| `tests/quality` | 240 passed, 30 failed | 242 passed, 30 failed |

The quality suite is unchanged in substance: the same 16 widening, 5
shrink-quality and 3 targeting cases, and in discovery the six
`large_factorial` cases (the 2**128 clamp); the duplicate-strings case
passes now. Shrink outcomes through the interpreter match the interactive
path's everywhere else, which is the other half of the equivalence claim:
the choice sequences are the same, so the shrinker's work is the same.

The cover suite is where the interpreter's price shows. The first run
hung, then failed 74 tests. Four bugs, now fixed:

- **No overrun on the batched path.** libhegel's first case is the
  all-simplest one and a nested `find_any` loops drawing from the outer
  case until its condition holds; the interactive path ran into
  `ConjectureData`'s length budget, the batched one appended nodes
  without checking it. `test_slices` ran for ever.
- **The choices query fails once libhegel has concluded the case** (its
  own budget ran out inside a draw); the runner now keeps Python's nodes
  for such a case.
- **Hypothesis's `sort_key` was choosing the "minimal" failure**, over
  nodes carrying permissive constraints, so a datetime shrunk by libhegel
  to the year 2000 lost to an earlier case with the year 1. libhegel's
  shrinker reports its minimal failure as a blob; the runner now keeps the
  executed case whose choices match it.
- Fixed-dictionary key-order shuffle and forced draws, above.

What remains (69) splits into round two's classes (database 11, flaky
semantics and overruns surfacing inside stateful steps 14, health-check,
seed and statistics wording 15, explain phase 6, engine internals 4,
discovery 3) and one new class, 16 tests: **a `do_draw` does more than draw**. The
built-in strategies' Python code also registers pretty-printers for the
values it builds (`builds`, `fixed_dictionaries`, `repr`-as-created),
labels arguments and draws for observability, records why a filter
rejected a value and where, adds notes to errors raised inside
`sampled_from` and `builds`, and warns about incompatible `shared`
bases. The interpreter produces the same values and the same choice
sequence, and none of those side effects; that is the whole of the
observability, custom-repr, `builds` error-message and
`sampled_from`-note failures (`shared` went back to Python, it is rare).
An interpreter that is to be a drop-in has to either reproduce them (the
pretty-printer registration and arg labels are mechanical, the error
notes need the Python frames) or hand values back through a thin Python
layer that does only that part. The second is the honest design: let the
engine generate, let `do_draw` decorate.

### What the interpreter does not cover yet

- Strategies with Python in their draw stay interactive: `composite`,
  `data`, `flatmap`, stateful rules, `recursive`, unique lists, dates and
  times (a `composite` over integers), regex, `from_type`. Their inner
  built-in draws are batched; the Python between them is not. Moving
  `recursive`, unique lists and the date strategies into the IR is
  mechanical; `composite` never moves.
- Optional fixed-dictionary keys and filtered `sampled_from` (both
  forced-draw heavy) are opaque; they could be nodes now that forced
  integers exist.
- Constant injection stays off on the engine path. With the forced
  integer in place it can be turned back on, passing constants down as
  forced draws, which is how Hypothesis's own replay treats them.
- `integers()` beyond int64 with weights, and forced integers beyond
  int64, fall back to the unweighted / unrecorded path (never seen in the
  suites, but a hole).
- The IR is a tuple protocol between two files with no schema; a real
  version gives each node a dataclass and a version tag, and checks the
  compiled tree against the strategy's `do_draw` source in the test suite
  when strategies change.

## What the engine-level flag taught

- **Faithful replay is the whole contract.** The first runner build shrank
  `floats() > 1.5` to 1e7 instead of 2.0. The cause was Hypothesis's constant
  injection in the provider: a replayed integer draw could come back as a
  constant instead of libhegel's recorded value, so libhegel saw the test as
  nondeterministic and fell into its bounded, confirmation-heavy handling.
  With constants off on this path libhegel shrinks to 2.0, as both engines'
  own tests expect. A real integration passes constants down as forced draws.
- **`shrink_towards` must reach the engine.** Without it datetimes shrank to
  year 6 instead of 2000. Now carried by the weighted entry point.
- **A choice-less completed case means "exhausted" to libhegel** and ends the
  run; the provider must not forward spans before a case has drawn.
- **libhegel's errors map cleanly**: its exhausted-space error is `core.py`'s
  `Unsatisfiable` path (exit reason finished, no valid case), its health
  checks are Hypothesis's `fail_health_check`, nondeterminism strictness
  `Error` gives Hypothesis's flaky semantics. That took the runner from 61 to
  46 cover failures.
- **Targeting works through the engine** (3 of 6 targeting-quality tests now
  pass; none did at the draw level) but libhegel's hill climb is weaker than
  Hypothesis's optimiser on the "threshold bug" cases.

## Speed

Whole loop, seconds and test-function calls, Python engine versus libhegel
runner (2000 examples; find + shrink with 500):

| case | python | libhegel runner |
|---|---|---|
| generation, integers | 0.63 ms/ex | 0.94 ms/ex |
| generation, text | 0.73 ms/ex | 0.74 ms/ex |
| generation, lists of ints | 1.36 ms/ex | 2.05 ms/ex |
| generation, dicts(text, floats) | 2.49 ms/ex | 2.84 ms/ex |
| find + shrink `sum(list) > 1000` | 0.08 s, 55 calls | 0.26 s, 194 calls |
| find + shrink text with 'a', len 10 | 0.21 s, 137 calls | 0.60 s, 702 calls |
| find + shrink dict with 3 keys | 0.18 s, 50 calls | 5.3 s, 1440 calls |

Generation is not faster because every draw still crosses into Python and
the test body dominates. Shrinking is slower by calls: Hypothesis's shrinker
caches results by choice sequence and orders its passes aggressively;
libhegel re-executes more candidates (the full cover suite takes 15 minutes
under the runner against 3.5 under the Python engine, almost all of it in
shrink-heavy tests). The remaining performance case for the migration is in
the engine's own work per call, which is small next to Python test execution.

## Remaining failures, engine level (46 in `tests/cover`)

- **Database** (11): libhegel's store is disabled and Hypothesis's reuse
  phase skipped, so saving, replaying and `.hypothesis/` layout tests fail.
  By design for this round; a real integration has to choose between
  teaching libhegel Hypothesis's database interface and keeping reuse in
  Python around the engine.
- **Flaky-test semantics** (12): Hypothesis distinguishes "failed once then
  passed", deadline flakiness and precondition flakiness with specific
  messages and exception types; libhegel reports one nondeterminism error.
- **Health-check wording and timing** (9): libhegel's slow-generation check
  fires on a different budget, and the seed / health-check report tests
  match Hypothesis's exact messages.
- **Explain phase** (6): comments such as "or any other generated value"
  come from the Python explain phase, which the runner does not run.
- **Engine-internal expectations** (5): shrink counts, the very-slow-shrinking
  warning, "runs the failing example twice", `booleans()` stopping after two
  examples.
- **Discovery** (3): two regex alphabet tests and
  `test_class_with_negative_category_and_positive_members` need characters
  libhegel's string distribution rarely picks within 100 examples.

Draw level (17): the same 13 as before (exhaustion the draw level cannot
see, alternative-backend verification semantics, one by design) plus four
distribution-flaky discovery tests.

## Remaining failures, engine level (30 in `tests/quality`)

| file | result | what fails |
|---|---|---|
| test_widening_shrinks | 3 passed, 16 failed | widening is a Hypothesis shrinker pass (rewrite a value into a nicer one from another alternative); libhegel's shrinker has no equivalent |
| test_shrink_quality | 85 passed, 5 failed | the three `lowering_together` cases (also fail at the draw level), `test_duplicate_containment`, `test_minimize_duplicated_characters_within_a_choice`: Hypothesis shrinker passes that libhegel lacks |
| test_targeting_quality | 3 passed, 3 failed | hill climbing reaches the moderate score but not the threshold bug with a large budget |
| test_discovery_ability | 84 passed, 6 failed | the four `large_factorial` cases (integers beyond 20! under the 2**128 clamp) and duplicate strings, unchanged from the draw level |
| poisoned lists and trees, float shrinking, widening-free files | 80 passed | shrink outcomes match where both shrinkers have the pass |

## Where the two engines differ

Vocabulary, closed by the hegel-rust patch: unbounded integers (still
clamped to 2**128 like HypothesisProvider), weights, `shrink_towards`, NaN
with finite bounds, arbitrary alphabets. Still open: strings cannot hold
lone surrogates in libhegel; `allow_infinity` has to be spelled out; the
engine ABI has no forced integer draw (needed to pass constants down).

Distribution, measured on 2000 examples: libhegel is far more biased to the
simplest values (6% zero integers versus 0.1%, 13% empty strings versus
0.1%, 846 distinct strings versus 2000); after the alphabet patch the `\W_`
case matches (1.9% versus 2.1% underscores). Neither is "right"; the quality
suite is where to argue it.

Shrinker: libhegel reaches the same minimum on the common cases and on the
poisoned corpora, lacks widening and the duplicate-aware passes, and spends
more calls.

## What a real integration still needs

1. Constant injection and `@example` replay through libhegel's choice
   sequence: the forced integer and boolean entry points now exist, the
   provider has to use them for constants (round three records only the
   draws Hypothesis itself forces).
2. A database story: either libhegel calls back into Hypothesis's
   `ExampleDatabase` (it is public API with third-party implementations),
   or reuse stays in Python with the blob codec above as the bridge.
3. The explain phase and the flaky / health-check reporting kept in Python
   around the engine, with libhegel's run errors mapped as the runner does.
4. The widening and duplicate-aware shrinker passes ported into libhegel if
   the quality suite is to stay green, and a result cache by choice sequence
   to close the call-count gap.
5. A delegating backend in libhegel for CrossHair (issue #4823), so the
   runner flag can apply to every `settings.backend`.
6. The interpreter as the engine's front door: the IR is what a shared
   engine would take from any frontend, and the two files here are a
   prototype of that boundary, not its final shape.
7. Governance: libhegel is MIT under an Antithesis-funded org; the license
   is fine, the dependency is a decision to make explicitly.
