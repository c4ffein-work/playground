# NNVP — review & improvements

Source reviewed: `c4ffein/nnvp` @ `4e26758` ("clearer TODOs").
This folder is a landing spot because this session is scoped to `c4ffein-work/playground`
and cannot push to `c4ffein/nnvp` (see **Why it's here** at the bottom).

- `../nnvp/` — full self-contained copy of the project with the changes below applied (buildable).
- `0001-*.patch`, `0002-*.patch` — the changes as a commit series that `git am`s cleanly onto
  `c4ffein/nnvp` master (see **Opening the PR** below).

> This is an ongoing autonomous loop working through `nnvp/docs/tasks.md`; the patch series and
> this doc grow as items land.

---

## WDYT — the honest take

A browser-based visual editor for Keras models: wire up layer nodes on a D3 canvas, generate
Python **or** JS (TF.js) code, and train in-browser via TensorFlow.js. Vue 3 + Vite 6, ~11k LOC
source, ~3.4k LOC Playwright e2e. Live at nnvp.io.

**Strong points**
- Modern, current toolchain (Vue 3.5, Vite 6, oxlint, Playwright 1.56); the Draw.io→Vue/D3
  rewrite paid down real debt.
- Above-average e2e coverage for a project this size.
- Sensible codegen design: `KerasGenerator` topologically sorts layers, detects Sequential vs
  functional, and delegates to Python/JS helpers via composition (with a self-aware comment on why).
- Layer catalog is **auto-generated** from Keras 3 introspection, not hand-maintained.
- Refreshingly honest docs (`history.md`, `tasks.md`).

**What I pushed back on (and fixed — see below)**
- `ParamsBlock.vue` was 2074 lines, ~1840 of them a single `getLayerHelp()` object literal of
  hardcoded HTML — the biggest smell in the tree.
- Zero unit tests: the pure, deterministic codegen was only covered indirectly through slow e2e.
- Three latent bugs in the generators (cycle recursion, input mutation, debug logging).

**Left deliberately untouched** (owner's call, or unverifiable here)
- `@tensorflow/tfjs` pinned to `^3.3.0` (early 2021) — worth a bump but a real migration risk;
  in-browser training can't be verified in this sandbox (the MNIST fetch is network-blocked here).
- `legacy-backend/` is dead (backend was shut down per `history.md`) — I'd archive/remove it, but
  that's a judgment call I left to you.

---

## Changes in this diff

### 1. Extract layer help into a data module
`ParamsBlock.vue`: **2075 → 241 lines**. All **97** help entries moved byte-for-byte into
`src/lib/KerasInterface/layerHelp.js`; `getLayerHelp()` is now a thin lookup that preserves the
original unknown-layer fallback. No behavior change (verified against `layer-help-modals.spec.js`).

### 2. Unit-test suite for the code generators
- Added Vitest + `npm run test:unit`, with `vitest.config.js` scoped to `tests/unit/` so it never
  picks up the Playwright specs.
- `tests/unit/KerasGenerator.test.js` — **25 tests**: `jsonToGraph`/composite flattening,
  `createTreatmentList` topological ordering (linear / branching / diamond / cycles), `isSequential`,
  and Python + JS generation asserting exact output strings.

### 6. Theme v0.1 — softer panel elevation + dead-scaffold removal
- Replaced the hard pure-black `1px` floating-panel hairline with a subtle border (`#e5e7eb`) + a
  soft drop shadow (new `--panel-border` / `--panel-shadow` tokens) for a lighter, modern look.
- Deleted `src/style.css` (default Vite scaffold — `#646cff` links, centered 1280px card, dark
  boilerplate button) and its `main.js` import; it conflicted with the real design system and shipped
  unused. Verified via layout e2e (5/5) + a screenshot.

### 5. Fix double-slash dataset URLs (FashionMNIST/CIFAR10) + datasets-sources tests
- `datasets-sources.js` built FashionMNIST/CIFAR10 paths as `cdnDir+"/fashion_mnist/…"` while the
  default `cdnDir` ends in a slash — producing `datasets//fashion_mnist/…` (MNIST correctly used no
  leading slash). Strict CDNs/object stores can 404 on `//`; risky given the planned Netlify→OVH move.
  Removed the leading slashes.
- Added `tests/unit/datasets-sources.test.js` (3 tests) asserting well-formed URLs (no `//` after the
  scheme — fails before the fix), per-dataset directories, and config/description presence.

### 4. Deselect → empty right panel (roadmap "broken features" item) + notify-guard fix
- Added a core-features e2e that selects a layer (params shown, overview hidden), clicks empty
  canvas to deselect, and asserts the right panel returns to the empty **Network Overview** state
  with the node's `selected` class removed. Verified it fails when selection notification is disabled.
- Fixed `D3GraphEditor.notifySelectionChanged`: it invoked the callback inside the `if` condition
  (double-invoke on truthy return; would throw if unregistered) instead of guarding on existence —
  now consistent with `notifyGraphChanged`.
- Checked the item off in `docs/tasks.md`.

### 3. Three bug fixes (each with a test that fails without the fix)
- **Cycle guard** in `createTreatmentList` — a cyclic graph previously infinite-recursed
  (`RangeError: Maximum call stack size exceeded`); the guard also stops a node reachable via two
  paths from being emitted twice in converging DAGs.
- **Input mutation** in `KerasGeneratorPythonHelper.generateModelFunction` — used destructive
  `this.inputs.splice(-1)` (mutated state during generation) where outputs correctly used `slice`.
- **Debug logging** — removed two leftover `console.log`s in the JS helper.

### Verification
`lint` 0/0 · `test:unit` 25/25 · `build` exit 0 · help-modal + code-generation e2e pass.
The only failing e2e are 3 dataset-training tests that fail **identically on pristine master**
because the MNIST download is blocked by this environment's network policy — not a regression.

---

## Opening the PR on the real repo

```bash
git clone https://github.com/c4ffein/nnvp.git && cd nnvp
git checkout -b claude/nnvp-improvements
git am /path/to/0001-Extract-layer-help-into-data-module-add-KerasGenerat.patch
git push -u origin claude/nnvp-improvements
# then open the PR from that branch
```

## Why it's here and not a PR on nnvp

This session's git + GitHub access is scoped to `c4ffein-work/playground`. Pushing to
`c4ffein/nnvp` returns 403 and the GitHub API returns "repository not configured for this session";
`add_repo` refuses it as a cross-owner add. To open the PR directly, either start a Claude Code
session with `c4ffein/nnvp` as the initial source, or grant this session access — then I can push
the branch and open the PR in one step.
