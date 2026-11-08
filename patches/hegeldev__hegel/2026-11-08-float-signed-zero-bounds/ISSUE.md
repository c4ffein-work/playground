**Title:** libhegel: `hegel_generate_float` with `min_value=0.0, max_value=-0.0` fails with an internal error

A float draw with bounds `min_value = 0.0, max_value = -0.0` returns `HEGEL_E_INTERNAL` instead of `HEGEL_E_INVALID_ARG`:

```
Internal error in hegel at libhegel/src/native/core/choices.rs:707:9: FloatChoice::simplest: no valid float for this choice. This is a bug in hegel itself; please report it at https://github.com/hegeldev/hegel/issues
```

Seen on `libhegel/0.50.0` and on `main` (b2f2bb2e9). The C++, TypeScript, Go and OCaml frontends pass these bounds through unchanged, so their users get the same error. Rust, Python and Java reject the bounds before calling the engine.

A test reaches these bounds by sorting two drawn floats to use as a range, e.g. `[a, b].sort((x, y) => x - y)` in TypeScript, the comparator from the TypeScript README. Sorts treat `0` and `-0` as equal, so `[0, -0]` stays in that order.

**Cause.** `generate_float` (`libhegel/src/native/draws/mod.rs:135`) rejects bad bounds with `min_value > max_value`, which is false for `0.0` and `-0.0`, so the draw goes ahead. `FloatChoice::validate` compares bounds sign-aware (`-0.0 < 0.0`), so no float is in `[0.0, -0.0]`. `simplest()` falls through to `simplest_nonzero()`, which ends at `hegel_internal_error!`.

**Fix.** Compare the bounds with `sign_aware_lte`, as the Rust frontend does. I have a patch with a regression test and can open a PR.

<details>
<summary>Reproducer: TypeScript frontend</summary>

Draw an interval, then a point inside it. Without a seed this fails about one run in 40 (28 of 1000 seeds, database off); seed 71 fails every time.

```js
import * as hegel from "@hegeldev/hegel";
import * as gs from "@hegeldev/hegel/generators";

hegel.test((tc) => {
  const bound = gs.floats({ minValue: -1e6, maxValue: 1e6 });
  const [lo, hi] = [tc.draw(bound), tc.draw(bound)].sort((a, b) => a - b);
  const x = tc.draw(gs.floats({ minValue: lo, maxValue: hi }));
  if (x < lo || x > hi) throw new Error("point outside its interval");
}, { seed: 71, database: hegel.Database.disabled });
```

The direct form fails on every run:

```js
hegel.test((tc) => {
  tc.draw(gs.floats({ minValue: 0, maxValue: -0 }));
});
```

Both, on `main`:

```
LibhegelError: hegel_generate_float failed: Internal error in hegel at libhegel/src/native/core/choices.rs:707:9: FloatChoice::simplest: no valid float for this choice. ...
  code: -8
```

With the fix, the test still fails, since it asks for an empty range, but the error names the bounds:

```
LibhegelError: hegel_generate_float failed: min_value (0) must be <= max_value (-0) after exclusive-bound adjustment
  code: -5
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
<summary>Which frontends are affected, and which idioms reach it</summary>

How each frontend on `main` checks float bounds before calling `hegel_generate_float`:

| Frontend | Check | Affected |
|---|---|---|
| Rust | `sign_aware_lte` (`frontends/rust/src/generators/numeric.rs:302`) | no |
| Python | `_sign_aware_lte` (`frontends/python/python/hegel/generators/numeric.py:213`) | no |
| Java | explicit `0.0` / `-0.0` check (`frontends/java/shared/src/main/java/dev/hegel/generators/Floats.java:45`) | no |
| C++ | `min > max` (`frontends/cpp/include/hegel/generators/numeric.h:97`) | yes (reproduced) |
| TypeScript | `min > max` (`frontends/typescript/src/generators/numeric.ts:76`) | yes (reproduced above) |
| Go | none (`frontends/go/primitives.go:132`) | yes (read from source) |
| OCaml | none (`frontends/ocaml/lib/generators_primitives.ml.in:45`) | yes (read from source) |

With the engine fix, each of these gets its usual invalid-argument error, so none of them needs a change.

How each language's usual ways of ordering two floats `a, b` treat `0` and `-0`, checked by running each one on both orders of the zeros:

| Idiom | C++ | TypeScript | Go | OCaml |
|---|---|---|---|---|
| min / max | `std::min` / `std::max`: safe | `Math.min` / `Math.max`: safe | `min` / `max`, `math.Min` / `math.Max`: safe | `Float.min` / `Float.max`, `Stdlib.min` / `max`: safe |
| sort the pair | `std::minmax`: **`[0, -0]`** | `.sort((x, y) => x - y)`: **`[0, -0]`** | `slices.Sort`: **`[0, -0]`** | `List.sort compare`: **`[0, -0]`** |
| `if (hi < lo) swap` | **`[0, -0]`** | **`[0, -0]`** | **`[0, -0]`** | **`[0, -0]`** |

The min/max forms are likely why hegel-rust's `with_min_and_max` test, which uses `a.min(b)` / `a.max(b)`, never hit this.

</details>

<details>
<summary>How it was found</summary>

In [hegel-c](https://github.com/c4ffein/hegel-c), a C binding over libhegel, a port of hegel-rust's `floats::f64_tests::with_min_and_max` failed about one run in thirty. The port ordered the bounds with `a < b ? a : b`, which with `a = -0.0, b = 0.0` gives `[0.0, -0.0]`. The test needs fixing on hegel-c's side too, but the engine should report invalid bounds, not an internal error.

</details>
