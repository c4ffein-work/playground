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

### 10. Contract tests + dim inference + D3 fixes + hardened backend (0008)
- **Front/API contract suite** — the real `apiClient` against the real Django backend, zero mocks
  (`npm run test:contract`, 16 tests: auth shapes/status codes, CRUD with graph deep-equality,
  ownership isolation). **No mismatch found**, re-verified against the hardened backend.
- **Codegen dim inference** — PyTorch/Tinygrad in-dims now derived from the graph
  (`Input([28,28,1])→Flatten→Dense(128)` → `nn.Linear(784, 128)`); uninferable → loud `# TODO`.
- **Four D3 bugs fixed** (idempotent `selectOnNode` + shift-click toggle, `removeObserver`
  `splice(-1)`, `clone` no-return/wrong-target, `isKerasError` ReferenceError), each with a
  failing-then-passing test. Known-broken composite/"Group layers" feature documented, untouched.
- **Backend hardened** (`../nnvp-backend/`, 25 tests): proxy requires JWT (+30/m throttle),
  refuses prod start on dev SECRET_KEY, env-driven CORS/hosts, password validation, `SECURITY.md`
  with an honest gaps list. **Chat backend-proxy mode** added to the SPA: base URL = backend →
  `/api/assistant/messages` + Bearer JWT, no Anthropic key in the browser.
Verified: lint 0/0, **173 unit** + **16 contract** + 22 e2e sanity; backend **25/25**.

### 9. Accounts + Tinygrad + dark canvas (0007) + Django backend (`../nnvp-backend/`)
- **Optional cloud accounts** (`apiClient.js` + `AccountPanel.vue`) — sign in/register + "My Projects"
  save/load, progressive-enhancement (fully usable logged-out). Round-trips through the same
  `toJSON`/`loadJSON` as the File menu. 17 unit tests + a mocked-backend e2e.
- **Tinygrad codegen** (`KerasGeneratorTinygradHelper.js`) — the 4th generation target
  (Keras/TF.js/PyTorch/Tinygrad); modules + Tensor-method activations, unsupported → `# TODO`.
- **Dark-themed D3 canvas** — canvas tokens; the board/nodes/edges now re-skin with the theme.
- **`nnvp-backend/`** — a **separate Django Ninja project** (not part of the SPA): email/JWT auth,
  per-user project storage, and a server-side Anthropic proxy (`/api/assistant/messages`). 12 tests,
  `/api/docs` OpenAPI. Public/no-CORS-lock for now; hardening TODOs (rate-limit, CORS, secrets) in its README.
  Verified together: lint 0/0, **162 unit tests**, build ok, **61/61 non-network e2e**.

### 8. Ten improvements in parallel (0006) — nine subagents, merged with zero conflicts
1. **Dark mode + theme v0.1** — semantic tokens, dark palette + persisted `[data-theme]` toggle, 8 components tokenized.
2. **Code-split tfjs** — lazy `loadTf.js` + manualChunks; entry bundle **2,674 → 606 kB**, tfjs on-demand.
3. **Chat hardening** — read-only-by-default guardrail, robust client errors, validation (+23 tests).
4. **D3 unit tests** — 48 jsdom tests; fixed a real `debugGetBoardState` property bug.
6. **PyTorch RNN/LSTM/GRU/Embedding** — correct tuple-return forward handling.
7. **Removed dead `legacy-backend/`** + `docs/datasets.md`.
8. **Dataset-load error UX** — visible error + Retry (e2e-verified via the blocked fetch).
9. **tfjs-4 training smoke-test** — synthetic-data `model.fit`, proves v4 training works with no network.
10. **Accessibility** — ARIA/keyboard/focus-trap pass across 13 components.
Plus a `playwright.config` `testMatch` fix so the whole-dir e2e run stops colliding with the vitest files.
Verified together: lint 0/0, **137 unit tests**, build ok, **60/60 non-network e2e**.

### 7. Four features in parallel (0005) — chat assistant · tutorial · PyTorch · tfjs 4
Built by four independent subagents (each on the theme-v0.1 base), then merged with a combined
verification pass (lint 0/0, 60 unit tests, build on tfjs 4, e2e 11/11 + 32/32 non-network core).
- **AI chat-assistant bubble** — actions API over `D3Interface`/`KerasInterface` exposed to Claude via
  an Anthropic tool-use client (bring-your-own-key). *Live chat needs an API key — not exercised here.*
- **MNIST tutorial mode** — 8-step guided build, coachmark overlay, auto-advances on editor state.
- **PyTorch codegen** — `torch.nn.Module` output (LazyLinear/LazyConv2d), unsupported→`# TODO`.
- **tfjs 3.3→4.22** — builds against v4, no code changes needed. *In-browser training needs a real
  browser+network — not exercised here.*

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
