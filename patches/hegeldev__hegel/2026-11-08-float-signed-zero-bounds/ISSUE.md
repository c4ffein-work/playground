**Title:** libhegel: `hegel_generate_float` with `min_value=0.0, max_value=-0.0` fails with an internal error

A float draw with bounds `min_value = 0.0, max_value = -0.0` returns `HEGEL_E_INTERNAL` instead of `HEGEL_E_INVALID_ARG`:

```
Internal error in hegel at libhegel/src/native/core/choices.rs:707:9: FloatChoice::simplest: no valid float for this choice. This is a bug in hegel itself; please report it at https://github.com/hegeldev/hegel/issues
```

Seen on `libhegel/0.50.0` and on `main` (b2f2bb2e9). The C++, TypeScript, Go and OCaml frontends pass these bounds through unchanged, so their users get the same error. Rust, Python and Java reject the bounds before calling the engine.

**Cause.** `generate_float` (`libhegel/src/native/draws/mod.rs:135`) rejects bad bounds with `min_value > max_value`, which is false for `0.0` and `-0.0`, so the draw goes ahead. `FloatChoice::validate` compares bounds sign-aware (`-0.0 < 0.0`), so no float is in `[0.0, -0.0]`. `simplest()` falls through to `simplest_nonzero()`, which ends at `hegel_internal_error!`.

**Fix.** Compare the bounds with `sign_aware_lte`, as the Rust frontend does. I have a patch with a regression test and can open a PR.

<details>
<summary>Reproducer: C++ frontend, a realistic test</summary>

A test that draws an interval, then a point inside it. Ordering the bounds with `if (hi < lo) std::swap(lo, hi)` leaves `lo = 0.0, hi = -0.0` unswapped. Without a fixed seed it fails about one run in 40 (51 of 2000 seeds, database off); seed 71 fails on every run.

```cpp
#include <hegel/hegel.h>
#include <stdexcept>
#include <utility>

namespace gs = hegel::generators;

struct Interval {
    double lo, hi;
    bool contains(double x) const { return lo <= x && x <= hi; }
};

HEGEL_TEST(point_drawn_in_interval_is_contained,
           {.seed = 71, .database = hegel::Database::disabled()})(hegel::TestCase& tc) {
    auto bound = gs::floats<double>({.min_value = -1e6, .max_value = 1e6});
    double lo = tc.draw("lo", bound);
    double hi = tc.draw("hi", bound);
    if (hi < lo) std::swap(lo, hi);
    Interval iv{lo, hi};

    double x = tc.draw("x", gs::floats<double>({.min_value = iv.lo, .max_value = iv.hi}));
    if (!iv.contains(x)) throw std::runtime_error("point outside its interval");
}

int main() { point_drawn_in_interval_is_contained(); }
```

On `main`:

```
Falsified after 9 test cases (0 discarded):

  auto lo = 0.0;
  auto hi = -0.0;

Exception: std::runtime_error: hegel_generate_float failed (internal error): Internal error in hegel at libhegel/src/native/core/choices.rs:707:9: FloatChoice::simplest: no valid float for this choice. ...
note: nondeterministic failure, confirmed: failed 19 of 20 replays at confirmation and 1 of 1 at report time
```

With the fix, the test still fails, since it asks for an empty range, but with an error that names the bounds (and without the spurious "nondeterministic" note):

```
Falsified after 9 test cases (0 discarded):

  auto lo = 0.0;
  auto hi = -0.0;

Exception: std::invalid_argument: min_value (0) must be <= max_value (-0) after exclusive-bound adjustment
```

The minimal form is a single draw, which fails on every seed:

```cpp
tc.draw("x", gs::floats<double>({.min_value = 0.0, .max_value = -0.0}));
```

</details>

<details>
<summary>Reproducer: C ABI, as a libhegel test (part of the patch)</summary>

Added to `float_draws_respect_spec_and_validate_arguments` in `libhegel/tests/c_abi_generate.rs`, next to the existing `2.0, 1.0` case:

```rust
assert_eq!(
    hegel_generate_float(
        ctx,
        tc,
        64,
        0.0,
        -0.0,
        false,
        false,
        false,
        false,
        f64::from_bits(1),
        &mut out,
    ),
    HEGEL_E_INVALID_ARG
);
assert!(last_error(ctx).contains("min_value"));
```

On `main`, every run:

```
assertion `left == right` failed
  left: HEGEL_E_INTERNAL
 right: HEGEL_E_INVALID_ARG
```

</details>

<details>
<summary>Proposed fix (<code>libhegel/src/native/draws/mod.rs</code>)</summary>

```diff
@@ -12,6 +12,7 @@ use crate::native::core::{
     EngineError, FloatChoice, FloatWidth, ManyState, NativeTestCase, RecursionState, Status,
     float_restrict_and_redraw,
 };
+use crate::native::floats::sign_aware_lte;
 use crate::native::intervalsets::IntervalSet;
 use crate::unicodedata::UnicodeVersion;
 use alloc::boxed::Box;
@@ -132,7 +133,7 @@ pub fn generate_float(ntc: &mut NativeTestCase, spec: &FloatSpec) -> Result<f64,
         min_value = min_value.max(f64::from(f32::MIN));
         max_value = max_value.min(f64::from(f32::MAX));
     }
-    if min_value > max_value {
+    if !sign_aware_lte(min_value, max_value) {
         return Err(EngineError::InvalidArgument(format!(
             "min_value ({min_value}) must be <= max_value ({max_value}) \
              after exclusive-bound adjustment"
```

With it, `cargo test --release -p hegeltest-c` passes, and `cargo fmt --check` and clippy are clean. The other frontends' suites were not run.

</details>

<details>
<summary>Which frontends are affected</summary>

How each frontend on `main` checks float bounds before calling `hegel_generate_float`:

| Frontend | Check | Affected |
|---|---|---|
| Rust | `sign_aware_lte` (`frontends/rust/src/generators/numeric.rs:302`) | no |
| Python | `_sign_aware_lte` (`frontends/python/python/hegel/generators/numeric.py:213`) | no |
| Java | explicit `0.0` / `-0.0` check (`frontends/java/shared/src/main/java/dev/hegel/generators/Floats.java:45`) | no |
| C++ | `min > max` (`frontends/cpp/include/hegel/generators/numeric.h:97`) | yes (reproduced above) |
| TypeScript | `min > max` (`frontends/typescript/src/generators/numeric.ts:76`) | yes (read from source) |
| Go | none (`frontends/go/primitives.go:132`) | yes (read from source) |
| OCaml | none (`frontends/ocaml/lib/generators_primitives.ml.in:45`) | yes (read from source) |

With the engine fix, each of these gets its usual invalid-argument error, so none of them needs a change.

</details>

<details>
<summary>How it was found</summary>

In [hegel-c](https://github.com/c4ffein/hegel-c), a C binding over libhegel, a port of hegel-rust's `floats::f64_tests::with_min_and_max` failed about one run in thirty:

```c
double a = hegel_draw_double(tc, -1e15, 1e15);
double b = hegel_draw_double(tc, -1e15, 1e15);
double lo = a < b ? a : b;
double hi = a < b ? b : a;
double n  = hegel_draw_double(tc, lo, hi);
```

With `a = -0.0, b = 0.0`, `a < b` is false, so the third draw asks for `[0.0, -0.0]`. The engine draws signed zeros often enough to hit this pair (`HEGEL_SEED=40` does). The test needs fixing on hegel-c's side too, but the engine should report invalid bounds, not an internal error.

</details>
