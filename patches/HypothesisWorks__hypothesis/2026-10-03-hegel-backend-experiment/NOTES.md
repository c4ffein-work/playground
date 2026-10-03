# Hypothesis on libhegel: from a draw-level backend to a runner-level engine

Session notes for two patch series: this one against Hypothesis master
(44b82b24, v6.168.3) and `patches/hegeldev__hegel-rust/2026-10-01-hypothesis-needs`
against hegel-rust main (ebfd9d53, hegeltest 0.48.1 / libhegel 0.44.1).
The question was whether Hypothesis's Python-to-Rust engine migration
(HypothesisWorks/hypothesis issue #4740) can be solved by consuming Hegel's
native engine, and how to check equivalence. Two rounds:

1. **Draw level** (2026-09-29): libhegel linked into `hypothesis._native`
   and exposed as `settings(backend="hegel")`; Hypothesis keeps the loop.
2. **Engine level** (2026-10-01): three additions to libhegel itself, then
   `HYPOTHESIS_ENGINE=hegel` hands libhegel the whole run loop (generation,
   shrinking, targeting, the choice budget) while Python keeps
   `ConjectureData`, test execution and reporting.

## The patches

hegel-rust, one commit:

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

Hypothesis, three commits:

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

1. A forced-draw entry point in the C ABI, so Hypothesis's constant
   injection and `@example` replay can go through libhegel's choice
   sequence rather than around it.
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
6. Governance: libhegel is MIT under an Antithesis-funded org; the license
   is fine, the dependency is a decision to make explicitly.
