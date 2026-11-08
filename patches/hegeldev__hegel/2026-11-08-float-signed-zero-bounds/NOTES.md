# libhegel: `hegel_generate_float(min=0.0, max=-0.0)` → internal error

Tracked in c4ffein/bugs as `bugs:8c229d24-4b71-4e69-b782-ddcf94bbf1fd`
(libhegel-float-simplest-nonzero). Base: hegeldev/hegel `main` (see
manifest.json); also reproduced on `libhegel/0.50.0`. Not fixed upstream
as of 2026-11-08.

## Root cause

`generate_float` (`libhegel/src/native/draws/mod.rs`) rejects bad bounds
with `min_value > max_value`. For `0.0` and `-0.0` that is false, so the
draw goes ahead. `FloatChoice::validate` compares bounds sign-aware
(`-0.0 < 0.0`), so `[0.0, -0.0]` holds no float: `simplest()` falls
through to `simplest_nonzero()`, which ends at
`hegel_internal_error!("FloatChoice::simplest: no valid float for this choice")`
(`core/choices.rs:707`) → `HEGEL_E_INTERNAL`.

The Rust frontend rejects these bounds itself (`frontends/rust/src/generators/numeric.rs`,
`sign_aware_lte`), so only C-ABI callers hit it.

## Why hegel-c sees it ~1 run in 30

`tests/from-hegel-rust/test_floats_f64_bounds.c` draws `a`, `b`, then
`lo = a < b ? a : b; hi = a < b ? b : a;`. With `a = -0.0, b = 0.0`,
`a < b` is false, so it asks for `[0.0, -0.0]`. The engine draws signed
zeros often, and `HEGEL_SEED=40` gets that pair.

With the fix the engine returns `HEGEL_E_INVALID_ARG`, which hegel-c reports
as a health-check failure, so that test stays red at seed 40 until hegel-c
orders the bounds sign-aware too (e.g. `lo = (a < b || (a == b && signbit(a))) ? a : b`).
That's a separate hegel-c change, not in this series.

## Reproducer (Rust, in the patch)

Added to `libhegel/tests/c_abi_generate.rs`,
`float_draws_respect_spec_and_validate_arguments`:

```rust
assert_eq!(
    hegel_generate_float(ctx, tc, 64, 0.0, -0.0, false, false, false, false,
                         f64::from_bits(1), &mut out),
    HEGEL_E_INVALID_ARG
);
```

Without the fix: `left: HEGEL_E_INTERNAL, right: HEGEL_E_INVALID_ARG`, every
run. With it: `cargo test --release -p hegeltest-c` passes (2690 tests),
`cargo fmt --check` and clippy are clean. Not run: `just check` (the other
frontends).

## Issue text

`ISSUE.md` is ready to paste into a hegeldev/hegel issue: summary up top,
then collapsed sections (C++ repro, C ABI test, fix diff, affected
frontends, how it was found). C++, TypeScript, Go and OCaml pass these
bounds through unchanged; C++ was reproduced, the other three read from
source.

`cpp-repro/` holds two C++ reproducers: `repro.cpp`, a single draw with
`[0.0, -0.0]` (fails every run), and `interval.cpp`, a realistic test that
draws an interval and a point in it (about 1 run in 40; pinned to seed 71
with the database off, since a `.hegel` database replays a stored failure
and makes every later seed look like a hit). Build against an existing
libhegel:

    cmake -S cpp-repro -B build -DHEGEL_SRC=<hegel checkout> \
          -DHEGEL_LIBHEGEL_LIBRARY=<hegel checkout>/target/release/libhegel_c.so
    cmake --build build && ./build/repro && ./build/interval
