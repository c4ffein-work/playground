# nnvp session notes — 2026-08-16

Handoff document: everything reviewed, found, and planned in this cloud
session, so a local session can pick up with full context. The only code
change shipped is the patch in `2026-08-16-readme-test-targets/`; everything
else below is design work, not yet implemented.

## What actually changed (the diff)

One commit, exported as a patch series against `master` @ `32515f8`:

```diff
--- a/README.md
+++ b/README.md
@@ -29,6 +29,8 @@ bun run build

 **Testing:**
 ```bash
+make test      # Run all tests (unit + e2e)
+make test-unit # Run unit tests headlessly
 make test-e2e  # Run Playwright e2e tests
 ```
```

Apply with:
`scripts/apply-patch.sh patches/c4ffein__nnvp/2026-08-16-readme-test-targets <your-nnvp-clone>`

Nothing else in nnvp was modified. In particular the bug fix below was
**discussed but not written** — it's the first thing to pick up.

## Bug found (unfixed): folder names with spaces corrupt the fold

`src/lib/Training/modelFolders.ts` — `foldFolders` keys live links as
`` `${path} ${workHash}` `` (line ~71) and destructures with
`key.split(' ')` (line ~78). Paths may contain spaces (`normalizePath`
allows them; the folder-name input in `ModelsWindow.vue` only trims), so a
folder like `/my models` splits wrong: the model vanishes from its folder
and a phantom folder appears. Fix options, either is sound:

- Minimal: split at `key.lastIndexOf(' ')` (safe: workHash is hex), or key
  by `JSON.stringify([path, workHash])`.
- Better (agreed direction): make `normalizePath` return `string[]`
  segments and use segments as the internal representation everywhere;
  keep the string form only in event payloads (they're unambiguous as
  stored — a discrete JSON field) and parse once at the fold boundary.
  Event schema stays untouched, so no log migration. Bonus: `/` in folder
  names becomes representable; `under()` becomes segment-prefix compare.
- Either way: regression test with a spaced folder name.

## Sync layer review — two known gaps (acceptable, but documented)

1. **Cloud purge isn't durable against other devices.** `localOnly` is
   device-private, so after device A purges a stream from the cloud,
   device B (holding unflagged copies) re-pushes it on next sync. If this
   should be fixed: a synced `stream.purged` control event that fold+sync
   respect ("never push members of this stream") closes it while staying
   merge-free.
2. **Sync pages the full remote uuid listing every run.** Fine now; when
   it bites, a per-device high-water cursor (deviceId, max seq) makes sync
   incremental without touching union semantics (chains are already
   totally ordered per device).

## Design: blob residency tiers (agreed, not built)

Model weight blobs get a residency policy of **two bits**:
`pinnedLocally` × `inCloud`, with `¬pinned ∧ ¬cloud` forbidden (nobody
holds the bytes = data loss as a policy; fail loudly if ever reached).
Legal states = the three user-facing tiers:

- **local-only** = `pinned ∧ ¬cloud`
- **local + cloud** = `pinned ∧ cloud`
- **cloud-only** = `¬pinned ∧ cloud` — local bytes may exist but are
  *cache*, not policy.

Key decisions:
- Single safety invariant: **evict iff `¬pinned`**. Enforce in one place.
- Blobs are content-addressed (workHash-adjacent), so cache entries can't
  be stale — eviction is always correct, LRU under a
  `navigator.storage.estimate()` budget is enough.
- Event log stays as-is and *references* blobs by hash; residency is
  metadata, bulk bytes move on a separate channel.
- **Scoping rule**: cloud tiers are account-scoped (policy rides the
  synced event log); local-only is inherently device-scoped — flag the
  whole stream `localOnly` (events AND blob) so other devices never see a
  model they can't materialize.
- Only cloud-only can hit "present but not materialized" (cache miss
  offline); pinned tiers never need that UI path.
- Tier transitions never pass through a zero-holders state; ordering
  favors duplication over loss (same principle as `applyPlan`).
- Monetization stays on the honest side: price cloud storage/sync/remote
  compute; never cap local capability (`.nnvp` export exists anyway; the
  platform's own IndexedDB evictability is the truthful argument for
  cloud backup — surface it when a user picks local-only, and call
  `navigator.storage.persist()`).
- Login flow: default residency policy setting (revisitable) + bulk triage
  of existing blobs applying that default — not a per-blob wizard.
  Event-log sync stays decoupled from (separately consented) blob upload.

## Testing strategy (agreed)

Same recipe as `modelFolders.ts`/`sync.ts`, extended:

1. **Pure planner**: `planResidency(state, intent) → plan` of primitives
   (upload/download/evict/flagPin/cloudDelete); executor is dumb. Most
   tests are synchronous tables over plain data.
2. **Property/model-based tests** (fast-check under bun): random op
   sequences incl. crash (truncate plan mid-apply) and duplicate delivery;
   invariants as oracle: no blob held by nobody, no eviction of pinned,
   forbidden state never reached.
3. **Cache chaos**: randomly delete cache entries mid-scenario — a faithful
   simulation of browser eviction. Pinned/local-only never affected;
   cloud-only degrades to "not materialized", never crashes.
4. **Two-device rig**: two in-memory app cores + one MemoryCloud; scripted
   interleavings; assert convergence + invariants; regression: local-only
   stream on A produces zero observables on B.
5. **Contract tests**: one blob-endpoint suite (idempotent put by hash,
   get-after-put round-trip, delete→404, pagination) run against BOTH
   MemoryCloud and the real backend — keeps the fake honest. Wrap the fake
   in a seeded FlakyCloud (fail/delay/duplicate) to assert retry safety.
6. **Scenario layer** ("user toggles the save of this model"): scripts of
   user intents + assertions on observable state, running on the headless
   two-device rig — full core, no browser. Parameterize the suite over the
   cloud implementation: MemoryCloud every CI run; real backend when
   available.
7. **Determinism**: inject clock + access order (LRU takes ordering as
   data); no Date.now()/Math.random() in core; every failing seed replays.
8. Playwright stays thin: sign-in, pin/unpin toggle, eviction recovery.

**Backend note**: `c4ffein/nnvp-backend` is private and cross-owner, so
cloud sessions scoped to this playground cannot clone it — and per
drop-box rules no patch against it may be stored here regardless.
Scenario-runs against the real backend therefore happen locally (or in a
session started with nnvp-backend as a source): client repo ships the
fake + contract suite as the spec artifact; the backend runs the same
contract suite from its side.

## Suggested pickup order (local)

1. Fix the spaced-folder-name bug (+ regression test) — smallest, real,
   user-facing.
2. Decide string-vs-segments internal representation while touching that
   code.
3. Optionally: `stream.purged` control event for durable cloud purge.
4. Then the residency work, planner-first, tests before executor.
