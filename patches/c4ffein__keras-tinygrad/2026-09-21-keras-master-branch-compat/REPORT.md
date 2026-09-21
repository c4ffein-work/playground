# keras-tinygrad vs Keras master / `pluggable_backend` / keras-openvino — 2026-09-21

Report for whoever picks this up (human or a fresh session). The patch
series in this directory targets `c4ffein/keras-tinygrad` (`main` at
`cf75be8`, see `manifest.json`); apply it with
`scripts/apply-patch.sh patches/c4ffein__keras-tinygrad/2026-09-21-keras-master-branch-compat <clone>`.

## The question

"Check the OpenVINO version of Keras — are we missing things?"

The answer has two halves, because "the OpenVINO backend" now means two
different things:

- On Keras **master** (3.16.0-dev, `c6a3b948`, 2026-09-19) it is still the
  in-tree `keras/src/backend/openvino`.
- On the **`pluggable_backend`** branch (`60be5d35`, 2026-09-17, 80 commits
  ahead of master) it has been moved OUT of the tree into
  `keras-team/keras-openvino` (`c00c9f92`), next to `keras-team/keras-mlx`
  and `keras-team/keras-paddle`. That package is the de facto template for
  an out-of-tree backend — the thing keras-tinygrad wants to be.

Everything below was checked statically against clones of those four
trees (no pilot re-run on the branch). The v3.15.1 tag was fetched for the
"since the pin" diffs.

## Findings

### Op surface (vs the numpy backend, the semantic reference)

| | |
|---|---|
| At v3.15.1 | name-complete, incl. the aliases (`abs`, `amax`, `amin`, `conj`, `true_divide`) and `fmax`/`fmin`; only `unique` / `vectorize` absent (documented decision items) |
| Added on master since the pin, missing here | `copysign`, `float_power`, `cov` (numpy); `gammainc`, `lgamma` (math) — **landed in this series** |
| Added on master, already here | `column_stack`, `matrix_power` (the first-pass chat summary wrongly listed `matrix_power` as missing) |
| Optional fused hooks on master | `backend.ops.nn.rms_normalization` / `layer_normalization`, probed with `hasattr` (torch only in-tree). Perf lever, not a gap |

### Keras master — what will break the import-hook path when 3.16 ships

1. **`ops/` subpackage layout** (#23642): every backend's
   `core/image/linalg/math/nn/numpy` moved under `backend/<name>/ops/`,
   exported as `ops`; keras calls `backend.ops.numpy.x`,
   `backend.ops.convert_to_tensor`, … (`backend.numpy.x` is gone). The
   hook serves the flat layout. `keras_tinygrad.src.ops` already has the
   right shape for the branch; the hook path does not use it. NOT done in
   this series — 3.16 is not on PyPI and the pin is `<3.16`.
2. **One loader anchor drifted**: the `DynamicBackend.__getattr__` block in
   `keras/src/utils/backend_utils.py` (the per-backend `return`s collapsed,
   numpy no longer special-cased). The other five anchors still match
   master exactly once. NOT done (same reason).
3. **`hasattr` probes on the backend**: master decides between a backend op
   and its backend-agnostic fallback with `hasattr(backend.ops.numpy,
   "copysign")` etc. `hasattr` only swallows `AttributeError`; the PEP 562
   loud stubs raised a plain `NotImplementedError`, which would escape the
   probe and fail even ops keras can compute itself. **Fixed** (patch 1).
4. Trainer base gained a `state_sync()` no-op hook (#23620); numpy
   `Trainer.predict` reworked. Nothing to do.

### `pluggable_backend` branch — current protocol (moved since the 08-27 pilot)

1. Discovery = hard-coded `config._PLUGGABLE_BACKENDS = {"mlx", "openvino",
   "paddle"}` + the `keras_<name>.src` naming convention. **No entry
   points.** `tinygrad` is not in the set, so plain `KERAS_BACKEND=tinygrad`
   raises "Unsupported backend" on the branch head; the pilot's
   generic-`else` patch is still the only way in. Keras-side; nothing a
   package patch can do.
2. Reference layout (keras-openvino / keras-mlx): `keras_<name>/src/{ops/,
   random.py, rnn.py, trainer.py, variable.py, version.py}`, `Variable`
   split out of `ops/core.py`, `src/__init__.py` star-exports `ops`,
   `random`, `rnn`, `compute_output_spec`, `device_scope`, `Variable`,
   `name_scope`, `SUPPORTS_*`, `IS_THREAD_SAFE`, `distribution_lib = None`.
3. `layer.py` / `export.py` are OPTIONAL (`find_spec` + fallback), and when
   present the attribute names are **`BackendLayer`** and
   **`SavedModelExportArchive`**. The shim exported `Layer`/`TinygradLayer`
   and `ExportArchive`/`TinygradExportArchive` → both `getattr`s would fail
   at `import keras`. **Fixed** (patch 3): `BackendLayer` added,
   `src/export.py` removed (neither reference package ships one; the base
   fallback raises the same loud error).
4. Test exclusions: per-package `excluded_tests.txt` of exact pytest node
   ids at the repo root, read by keras' conftest only for a checked-out
   backend (#23671). Same idea as `scripts/referee-baseline.txt`, opposite
   polarity. `integration_tests/import_test.py` reads
   `KERAS_BACKEND_PACKAGES`.
5. Reference CI: check out `keras@pluggable_backend` beside the backend,
   `pip install -e .`, `KERAS_HOME` → a `keras.json` selecting the
   backend, `pytest keras -n auto --dist loadfile --ignore
   keras/src/applications --ignore keras/src/wrappers` from the keras
   root. Ruff line-length 80, pre-commit, Apache-2.0.
6. Still open from the pilot: `standardize_dtype` (`.name` first, `"mlx"`
   in the string heuristic — the `standardize_dtype_hook` is still a local
   patch); the float8 `train_one_step` test-side assumption.
7. RFC #23523 is open, no decision recorded.

## The series

| # | Patch | What |
|---|---|---|
| 1 | `0001-Raise-MissingOpError-from-the-loud-stubs…` | `core.MissingOpError(NotImplementedError, AttributeError)`, raised by the five PEP 562 stubs. Direct call: still loud. `hasattr` / `getattr(default)`: absence. |
| 2 | `0002-Add-copysign-float_power-cov-lgamma-gammainc…` | The five master ops + `tests/test_ops_beyond_pin.py` (keras master's own test cases, gradient receipts, the MissingOpError contract). |
| 3 | `0003-Shim-the-pluggable_backend-branch-s-current-plugin-names` | `BackendLayer`; `src/export.py` deleted. |
| 4 | `0004-docs-keras-master-3.16-dev-and-pluggable_backend-status…` | `docs/upstream/keras-master-and-branch-status-2026-09-21.md`, HANDOFF remainder 9, a pointer in the pilot report. |

Implementation notes worth knowing before touching the ops again (all in
the commit messages and code comments too):

- `copysign`: bit-level on both sides — tinygrad's `abs` keeps `-0.0`, so
  the magnitude is masked through a bitcast; works on all four float widths.
- `lgamma`: keras' own Lanczos constants. Two gradient traps fixed: the
  unselected reflection branch hit `log(sin(0))` at every positive
  integer and poisoned the `where` gradient (0 × inf); the backend's
  compensated `log1p` has a 0/0 gradient at 0 (x = 1). `lgamma'(n) ==
  digamma(n)` is asserted by the test.
- `gammainc`: Numerical Recipes series + Lentz continued fraction at fixed
  term counts (200 / 60 — float32-exact to a ≈ 1000, measured against
  scipy). Each branch is clamped into its own convergence region so the
  discarded branch is finite. The unrolled recurrences are cut every 20
  iterations by STACKING the carried values into one
  `contiguous().contiguous_backward()` buffer — see `_cut_all`'s docstring
  for why not one-by-one (75 distinct compiles) and why not `realize()`
  (the gradient silently stops at the last segment in tinygrad 0.13; the
  linalg docstring's claim that realize is gradient-safe looks wrong for
  differentiable values). Forward: 5 kernels per call shape (~3 s first
  use, ~0.4 s after). Gradient: ~25 kernels, ~30 s of clang on first use.
  Float32 cancellation in the log prefactor costs ~1e-5 relative from
  a ≈ 50 up (the test tolerates 5e-5 in that sweep).
- numpy float64 inputs compute in float32 per the backend's documented
  promotion policy (`docs/float64-promotion.md`); the new ops follow it.

## Verification (2026-09-21, python 3.11.15, clang, `uv sync` in the clone)

- `make verify`: ruff check + format clean; **48 passed in 74 s** (was 22
  in 37 s — the new module is ~48 s, dominated by the gammainc gradient
  compile).
- `make smoke`: SMOKE OK. `make vendor-check`: all six anchors match the
  installed keras 3.15.1 exactly once.
- Not run: `make referee` (~25 min, needs tensorflow for collection),
  `make tutorial`, `make fuzz`. The new ops are not on any layer's path,
  and the MissingOpError change only alters the exception type of a
  never-succeeding call, so the referee tally should be unchanged — but
  run it before merging, per the repo's rule.

## Not done, deliberately

- The 3.16 hook work (`ops/` layout + the `backend_utils` anchor): wait for
  the release; it is the same restructure `keras_tinygrad/src/__init__.py`
  calls "the planned full restructure".
- Anything keras-side on the branch (`_PLUGGABLE_BACKENDS`,
  `standardize_dtype`): issue material, not package patches.
- `unique` / `vectorize`: still the owner's decision items; master calls
  them directly (no agnostic fallback) so they stay loud either way.
- Fused `rms_normalization` / `layer_normalization` hooks: optional perf.

## Where things are

- Clones used (session scratchpad, ephemeral): keras master + v3.15.1 tag
  + `pluggable_backend` (fetched as FETCH_HEAD), keras-openvino, keras-mlx,
  keras-tinygrad with the four commits on top of `cf75be8`.
- Re-derive any number here from the clones rather than trusting it: the
  anchor check is `python -c` over `_loader._PATCHES` against a keras tree;
  the op-name diffs are an `ast` walk of each backend's modules.
