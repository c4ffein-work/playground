# Review guide — start here

Everything from the session lives in this repo so it can be reviewed from any machine.
Branch: `claude/c4ffein-nnvp-review-4llfqs` (PR #1 on `c4ffein-work/playground`).

## What's where

| Path | What it is |
|---|---|
| `nnvp/` | The full updated SPA source (upstream `c4ffein/nnvp` @ `4e26758` + all changes applied) |
| `nnvp-backend/` | Standalone Django Ninja backend (accounts/JWT, projects storage, LLM proxy) — copy out to its own repo when ready |
| `nnvp-review/REVIEW.md` | The running writeup: every change group, newest first |
| `nnvp-review/0001–0008-*.patch` | The SPA changes as a commit series; verified to `git am` cleanly onto upstream `c4ffein/nnvp` master (`4e26758`) |
| `nnvp-review/screenshots/` | Visual evidence (index below) |

## Re-running every verification yourself

SPA (from `nnvp/nnvp-client-vue/`):
```bash
npm ci
npm run lint          # oxlint — expect 0/0
npm run test:unit     # Vitest — expect 173 passed (13 files)
npm run build         # expect exit 0; tfjs is a separate lazy chunk
npm run test:e2e      # Playwright; see caveat below about network-blocked tests
NNVP_BACKEND_DIR=../../nnvp-backend bash scripts/test-contract.sh   # 16/16, boots the real backend
```

Backend (from `nnvp-backend/`):
```bash
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python manage.py migrate
.venv/bin/python manage.py test   # expect 25 passed
.venv/bin/python manage.py runserver  # /api/docs = OpenAPI UI
```

Applying to real nnvp:
```bash
git clone https://github.com/c4ffein/nnvp.git && cd nnvp
git am /path/to/nnvp-review/*.patch
```

## What was verified vs. what was NOT

Verified in-session (all green at final push):
- SPA: lint 0/0 · 173 unit tests · build · non-network e2e (app, interactions, help modals,
  tutorial, a11y, dataset-error, cloud-with-mocked-backend, core-features codegen).
- Front↔API **contract**: 16 real round-trip tests (no mocks) against the **hardened** backend.
- Backend: 25 tests, migrations clean, curl smoke (auth flow, proxy 401/503, weak-password 422).
- tfjs 4 training: synthetic-data `model.fit` on CPU (sequential, functional, and the generated
  flatten→dense→dense architecture) — losses finite and decreasing.

NOT verifiable in the sandbox — **check these manually first**:
1. **Real cloud round-trip through the UI**: run backend + SPA together, sign in, Save to cloud,
   reload, Open from cloud. (apiClient↔API is contract-tested; the UI path used a mocked backend.)
2. **Live AI chat**: needs an Anthropic key — either in the chat settings (direct), or sign in and
   set the chat base URL to the backend URL (proxy mode, key stays server-side).
3. **Real-GPU in-browser training** (WebGL) — the MNIST dataset fetch was network-blocked in the
   sandbox, so the ~4 dataset/training Playwright tests fail there (verified identical on pristine
   upstream = environmental, not regressions). On a normal box they should pass.
4. Dark mode by eye — tokens were checked for contrast, not by a designer.

## Known issues & honest caveats (deliberately left)

- **"Group layers" / composite feature is broken upstream** (`D3Model.createComposite` calls
  editor-only methods → throws; more bugs behind it: `D3LayerComposite.drawLayer` `taget` typos,
  `D3Layer.getModel` precedence bug). Documented, NOT fixed — reviving the feature is a scope
  decision, and its downstream code needs its own pass.
- **Backend gaps** (see `nnvp-backend/SECURITY.md`): no refresh tokens/revocation, no email
  verification, no login rate limiting, throttle is per-process, proxy has no model/max_tokens
  allowlist. Deploy checklist is in the README.
- **Codegen dims**: inference covers feature-dim chains (Dense/RNN/Embedding/Conv channels,
  Flatten of fully-known shapes). Post-Conv spatial arithmetic is NOT attempted — those lines keep
  a loud `# TODO: could not infer from graph`.
- **Commit trailers are inconsistent** across the branch history (earlier commits `Claude Opus 4.8`,
  later `Claude Fable 5`) — cosmetic, an instruction miss caught in the mid-session self-review.
- The `nnvp/.github/` and `nnvp/CLAUDE.md` files are copies from upstream; workflows in a
  subdirectory don't run, and the nested CLAUDE.md will apply to future Claude sessions in `nnvp/`.

## Screenshot index (`screenshots/`)

| File | Shows |
|---|---|
| `01-theme-v01-light.png` | Theme v0.1: soft panel borders/shadows replacing black hairlines |
| `02-light-mode-final.png` / `03-dark-mode-ui.png` | Final tokenized light / dark UI chrome |
| `04-dark-mode-menu-tutorial-chat.png` | Dark mode with Tutorial menu, theme toggle, chat bubble |
| `05-canvas-light.png` / `06-canvas-dark-with-node.png` | D3 canvas theming (light unchanged / dark board with readable node) |
| `07-chat-panel-v1.png` | AI chat panel, first version (settings, empty-key state) |
| `08-chat-hardened-modes-error.png` | Chat guardrails: Read-only/Allow-edits toggle + error bubble |
| `09-tutorial-overlay.png` | MNIST tutorial overlay (Step 1/8, coachmark) |
| `10-dataset-error-retry.png` | Dataset-load failure UX: visible error + Retry |
| `11-a11y-focus-ring.png` | Keyboard focus ring on a catalog item |
| `12-cloud-accounts-panel.png` | Cloud accounts panel: signed in, saved project, reopened model |
