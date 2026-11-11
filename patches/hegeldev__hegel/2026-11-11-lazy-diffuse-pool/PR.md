# PR: Build the diffuse integer pool only when it is selected

Target: hegeldev/hegel `main` (d74a7a73, 2026-11-11). One commit,
`libhegel/` only, with `libhegel/RELEASE/lazy-diffuse-pool.md` (patch).

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

Timing from the same test, per draw, best of seven runs: 3.5 us to 0.9 us
in a release build, 3.5 us to 1.3 us at the workspace dev profile.

No test added: the change has no observable behaviour other than time. The
existing embedded tests of the wide sampler
(`biased_integer_sample_*beyond_i128*`) take the moved branch 81 times, so
the lines stay covered.

Found while running Hypothesis's own test suite on libhegel as an
experiment; Hypothesis's unbounded `integers()` is the one caller that
takes this path on every draw.

## Checked

- `cargo fmt --check`, `cargo clippy --all-targets` for `hegeltest-c`.
- `cargo build -p hegeltest-c && cargo test -p hegeltest-c` (the `smoke`
  binary included): all pass.
- `.github/scripts/release.py check main`: passes.

Not run here: `just libhegel::check-coverage` and the other frontends.

## To open it

`git am patches/hegeldev__hegel/2026-11-11-lazy-diffuse-pool/0001-*.patch`
on a fresh checkout of `main`, push to a fork, open the PR with the title
and body above.
