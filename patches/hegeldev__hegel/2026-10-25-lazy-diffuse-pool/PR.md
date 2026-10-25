# PR: Build the diffuse integer pool only when it is selected

Target: hegeldev/hegel `main` (36f65de4). One commit, `libhegel/` only, with
`libhegel/RELEASE.md` (patch).

## Title

Build the diffuse integer pool only when it is selected

## Body

`biguint_sample_in_range` (the sampler for integer ranges wider than
`i128`, which is what a frontend's unbounded `integers()` becomes once it
is clamped to `[-2**128, 2**128]`) built its "diffuse" pool of candidates,
up to 256 big-integer powers of two, sorted and deduplicated, on every draw,
although that category is selected only a small fraction of the time. The
"interesting" pool next to it is already built only when selected. This
does the same for the diffuse pool.

The RNG is consumed identically (the pool never drew from it until a value
was picked from it), so seeded runs generate exactly the same values.
Verified with a scratch C ABI test drawing 20000 integers in
`[-2**128, 2**128]` from seed 12345 across 400 cases and hashing every
result: the fingerprint is the same before and after.

Timing from the same test, per draw: 5.7 us to 1.3 us at the workspace dev
profile, 3.0 us to 1.2 us at release.

No test added: the change has no observable behaviour other than time, and
the existing integer tests (`c_abi_generate`, the embedded state tests)
cover the sampler. `cargo clippy --all-targets`, `cargo fmt` and the
libhegel test suite pass (the `smoke` binary needs a built shared library
and fails identically on an untouched checkout here).

## Checklist for whoever opens it

- `git am patches/hegeldev__hegel/2026-10-25-lazy-diffuse-pool/0001-*.patch` on a
  fresh checkout of `main`, or apply `scripts/apply-patch.sh`.
- `just libhegel::check` (the full definition of done; not run here, only
  its cargo parts).
