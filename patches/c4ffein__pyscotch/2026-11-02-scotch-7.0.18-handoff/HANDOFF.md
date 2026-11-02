# Handoff: Scotch 7.0.18 for PyScotch, for the next Claude session

Read this file first. It is written for a Claude session on the maintainer's own machine, with a clone of
`c4ffein/pyscotch` and of this playground repo, picking up where a cloud session stopped on 2026-11-02. You should
not need to re-derive anything. The 7.0.17 round's handoff (`HANDOFF.md` in the archive linked from
`upstream/README.md` in PyScotch) explains the method; this file covers what the 7.0.18 round did and what is left.

## 0. Where things are

Nothing is in PyScotch yet. Everything is in three patch series in this playground repo, next to this folder:

| Series (`patches/c4ffein__pyscotch/`) | Commits | What |
|---|---|---|
| `2026-11-02-scotch-7.0.18-bump/` | 1 | Scotch 7.0.18 catalogued (sha256 pinned), put in `_SKIPPED_VERSIONS`, determinism patch carried over as `scotch-7.0.18-bgraph-bipart-gg-determinism.patch`, CHANGELOG, QUESTIONS status |
| `2026-11-02-upstream-report-carry/` | 1 | `gen_upstream_report.py carry`, `location_<version>` keys, `upstream_fix_review`, repro kits updated in place |
| `2026-11-02-scotch-7.0.18-report/` | 2 | report tooling fixes (cited patches stay in their group; templates name the reviewed version), then `upstream/reports/7.0.18/` + updated `upstream/repro/` |

They apply in that order on PyScotch `main` at `8790918` ("include analysis process archives"). The base of each
later series is the previous series' last commit; the apply script recreates them:

```sh
cd <playground>
scripts/apply-patch.sh patches/c4ffein__pyscotch/2026-11-02-scotch-7.0.18-bump        <pyscotch-clone>
scripts/apply-patch.sh patches/c4ffein__pyscotch/2026-11-02-upstream-report-carry     <pyscotch-clone>
scripts/apply-patch.sh patches/c4ffein__pyscotch/2026-11-02-scotch-7.0.18-report      <pyscotch-clone>
```

Each call makes its own `patch/<date>-<slug>` branch from the recorded base. The 2nd and 3rd bases are commit ids
from the cloud clone (`5c013d5`, `b4e1a03`), which your clone will not have. Simplest is to stack them yourself
on one branch:

```sh
git -C <pyscotch> checkout -b scotch-7.0.18 8790918
for s in scotch-7.0.18-bump upstream-report-carry scotch-7.0.18-report; do
  git -C <pyscotch> am -3 <playground>/patches/c4ffein__pyscotch/2026-11-02-$s/*.patch
done
```

Verified in the cloud: this reproduces the cloud tree exactly. `git am` prints thousands of "trailing whitespace"
warnings; they come from the Scotch `.patch` files stored inside the report and are harmless. Commit authors are
`c4ffein <c4ffein.work@gmail.com>`; change them if you prefer your usual address.

**The process archive** is `pyscotch-scotch-7.0.18-handoff.tar.xz` in this folder (sha256 in `SHA256SUMS`). It
holds what is NOT in the PyScotch repo; see section 2.

## 1. What was done

**A: the bump.** `make bump SCOTCH=7.0.18` downloads and compiles 7.0.18, then stops at the reproducibility gate:
the `h` strategy at 4 threads gave 5 distinct partitions over 12 fresh runs, and 3 of 5 gate tests failed.
`bgraph_bipart_gg.c`, `bgraph_bipart_gg.h` and `gain.c` are byte-identical to 7.0.17 (SEP-13 untouched). The
7.0.17 behavioral patch applies unchanged; with it the gate passes 3 x 30 fresh runs. `library.h` has no new
public C function since 7.0.15 (the only new exported thing is the `graphpartovlview` Fortran wrapper); the
API-completeness test passes. The shipped version stays 7.0.15. Stage 4 of `bump_scotch.py` (exported-symbol drift)
was not run by the script because stage 3 stopped it; it was checked by hand as above.

**B: every finding and every upstream fix re-checked**, following the 7.0.17 handoff's section 4:

1. `git cherry v7.0.18 combined` against the 7.0.17 bundle: upstream took ONE of our 240 commits verbatim
   (`dorderCblkDist` negative return). 84 of our patches no longer applied.
2. Pristine 7.0.18 ASan+UBSan builds, 32- and 64-bit `Gnum` (`infra/makefile-inc/` has their `Makefile.inc`).
3. 8 agents, 2-5 groups each, brief `infra/BRIEF18.md` (the 7.0.17 brief adapted: base v7.0.18, review each upstream
   fix, rebase our patches, new defects as `PS3-<Gnn>-<n>`). Prompts verbatim in `prompts18.md`.
4. Merge: `gen_upstream_report.py carry` then `merge-audit --check-apply --order --repro-root`. 12 conflicts at first;
   resolved by hand and written back (`merge/RESOLUTIONS.md`), 5 duplicate patches dropped.
5. QA, one tree at a time under `flock`, `make -k`: see `qa.json` in the report and `qa/` in the archive.

**Numbers** (`upstream/reports/7.0.18/findings.json`):

- 350 findings + 25 new (PS3-*). Statuses in 7.0.18: 279 still present, 46 fixed, 41 partly fixed, 6 not a bug,
  3 inconclusive.
- Of the 192 review leads: 45 fixed, 37 partly, 108 still present, 2 not bugs.
- Verified by running: 304; partly run: 26; source read only: 45.
- 71 upstream 7.0.18 commits reviewed against our findings (`upstream_fix_review` on each finding): 32 correct,
  32 incomplete, 7 wrong.
- The 7 wrong ones: 18911e4f (tleaf double free, UB overflow test), ecaf094e (sub-arch double free), 22c9c42e
  (`SCOTCH_graphMapFixed` infinite loop, PS3-G12-1, high), 86559b85 (failed thread start still hangs, context
  freed under a running thread), 27f4193c (`dgmap -v` CommDelta=2 for every unweighted graph), 858ea2bc
  (dummysizes, one-character file names taken as stdin/stdout), 59c9a31c (uninitialised pointer on non-root
  processes). Also 52dcf602 fixes COR-24 but makes core number -1 pin threads to CPU 0.
- 232 commits on v7.0.18, one series, plain `git am` OK.
- QA:
  - 0 warnings on both sizes.
  - Patched tree: `check`, `ptcheck` and `escheck` pass with 0 sanitizer reports.
  - Pristine 7.0.18 fails `check_scotch_arch_build` (a leak). It shows 23 (32-bit) or 25 (64-bit) UBSan reports in
    `check` and 7 in `ptcheck`: `bgraph_bipart_ml.c:195`, `kgraph_map_ml.c:239`, and on 64-bit two shifts in
    `mesh_induce_sepa.c:160-161`.
- 6 items that were "fixed", "partially-fixed" or "inconclusive" in 7.0.17 read "still-present" in 7.0.18. These are NOT upstream regressions: in
  7.0.17 they were verified on top of other groups' patches. The patches that fix them are in the series and the
  findings point at them (DMO-08, DMO-16, PS-DMO-3, PS-DMO-4, COR-17, PS2-G22-5).

## 2. What is where

**In PyScotch (after applying the series):**

| Path | What |
|---|---|
| `upstream/reports/7.0.18/findings.json` | every finding with `history` {7.0.17, 7.0.18}, status, evidence, location, fix options, `upstream_fix_review` |
| `upstream/reports/7.0.18/patches/<group>/`, `series/` | per-group commits; the combined 232-commit series |
| `upstream/reports/7.0.18/groups/<group>/` | each group's `SUMMARY.md`, raw `audit.json`, `extra.json` |
| `upstream/reports/7.0.18/{meta,blocking,qa}.json`, `order.txt` | highlights for the maintainer report, the release blocker (SEP-13), QA matrix, group order |
| `upstream/repro/<group>/` | reproducer kits, updated in place (7.0.18 adaptations added next to the 7.0.17 files) |

`python docs/site/gen_analysis.py` renders the tracker (newest version automatically; 375 finding pages, history
column). `python scripts/gen_upstream_report.py build --data upstream/reports/7.0.18 [--pdf]` renders the report and
`out/email.txt`, an UNSENT draft; `meta.json` still has `"sender": "[your name]"`. The PDF needs Playwright's Chromium
(see the 7.0.17 handoff's lesson on screenshots in a root-less container).

**In the archive `pyscotch-scotch-7.0.18-handoff.tar.xz` (not in PyScotch):**

| Path | What |
|---|---|
| `prompts18.md` | the 8 agent launches and 2 forwarded messages, verbatim |
| `DECISIONS.md` | every group's "user must decide" section, verbatim |
| `infra/BRIEF18.md`, `STYLE.md`, `stylecheck.py` | the brief, patch checklist and checker the agents followed |
| `infra/groups/<G>.json` | per-group digests generated from the 7.0.17 `findings.json` (+ upstream commits touching each group) |
| `infra/upstream_7.0.17_to_7.0.18.patch` | `git log -p v7.0.17..v7.0.18` |
| `infra/makefile-inc/` | the ASan+UBSan `Makefile.inc` for the 32- and 64-bit pristine trees |
| `merge/` | `RESOLUTIONS.md`, `apply-sequence.txt`, `order.txt`, `replay.sh` / `cont.sh` (re-apply with recorded resolutions) |
| `dropped/<group>/` | the 5 duplicate patches removed at merge |
| `pre-merge/<group>/` | the agents' originals of the 28 patches rewritten at merge |
| `qa/` | `run.sh`, `summary.txt` (first run), `summary2.txt` (after the checkMap fix), all 14 suite logs |
| `git/scotch-7.0.18-combined-232.bundle` | the 232 commits; prerequisite tag v7.0.18. `git fetch <bundle> comb:combined18` |

The scripts in `merge/` and `qa/` contain the cloud session's absolute scratch paths; adjust `SP=` at their top.
Agent transcripts are not included: they stayed in the cloud session.

## 3. What is left (in order)

1. **Apply the three series** (section 0). Run `make lint-check format-check`, then `pytest
   tests/pyscotch_base/test_scotch_build.py tests/pyscotch_base/test_upstream_report.py`. The second file has 20
   tests; 2 are new this round. Push.
2. **`docs/site/api_data.json`**: not regenerated. No public API changed in PyScotch, and the shipped version is still
   7.0.15, so it should not be stale. If the "Verify docs API data" CI job fails anyway, run `make docs-api`
   (Python >= 3.14, fresh 64-bit build, `PYSCOTCH_PARALLEL=1`); never hand-edit it.
3. **Settle the decisions** in section 5 (and `DECISIONS.md`). The series implements our recommendation for each, so
   a "no" means dropping or swapping a commit (alternatives are in `upstream/reports/7.0.18/alt/`). Then re-run
   `merge-audit` (section 4).
4. **Report urgent regressions upstream**, independently of the full report: PS3-G12-1 (`SCOTCH_graphMapFixed` /
   `PartFixed` hang, 22c9c42e) and PS3-G01-1 / PS3-G02-1 (double frees on malformed architecture files). PS3-G16-1
   (CommDelta=2) changes printed output for every distributed mapping.
5. **Send the report**: fill `sender` in `meta.json`, build with `--pdf`, review `out/email.txt` (the credit line
   cites Robert Perry's review of 7.0.16). Tell the maintainer that the BLD-03 Sonar token archived in his CI
   artifacts needs rotating; the report gives only the variable names, not the value.
6. **Publish this archive** next to the 7.0.17 one in `c4ffein/static-files` and link it from
   `upstream/README.md` (as done for 7.0.17), if you want the process public. It carries no third-party review text:
   the group digests contain only our own titles and evidence.
7. **Not done, needs tools the cloud lacked:** regenerate the `.1` man pages (`txt2man`); compile the LaTeX doc fixes
   (PS3-G16-2 `\ref` fix, PS3-G28-1 dgscat synopsis); CMake older than 3.18; FreeBSD/MinGW/Haiku.
8. **When 7.0.19 (or 7.0.18rc*) appears**, repeat with section 4.
9. **When upstream fixes SEP-13**: `make bump SCOTCH=<that version> BUMP_FLAGS=--apply`, retire the
   `_SKIPPED_VERSIONS` / `_BEHAVIORAL_PATCHES` entries, and update the 7 PyScotch edge-load-sum assertions (next
   item).
10. **PyScotch's own tests change when the shipped version moves past 7.0.15.** Upstream adopted our edge-load-sum
    convention (5a711ca5: each edge once), so for weighted graphs the sum halves:
    - `test_interop.py` lines 65 (14->7), 181 (4->2), 246 (4->2), 314 (10->5) and 357 (8->4);
    - `test_graph.py:144` ((1,3,12)->(1,3,6));
    - `hypothesis/test_graph_properties.py:356` (`2*sum` -> `sum`).

    Per this repo's rule these are expected values that change with the library, not tests "made to pass". Say so in
    the commit.

## 4. Doing the next round

Same as the 7.0.17 handoff's section 4, plus what this round changed:

```sh
make bump SCOTCH=7.0.19                                   # gated; needs OMPI_ALLOW_RUN_AS_ROOT=1 ..._CONFIRM=1 if root
python3 scripts/gen_upstream_report.py carry --version 7.0.19 --from upstream/reports/7.0.18 --to upstream/reports/7.0.19
# ... agents write WORKROOT/<G>/{audit.json,extra.json,patches/,alt/,repro/,SUMMARY.md}; audit locations as location_7_0_19
cp upstream/reports/7.0.18/{order.txt,meta.json,blocking.json} upstream/reports/7.0.19/   # then edit meta/blocking
python3 scripts/gen_upstream_report.py merge-audit --from WORKROOT --to upstream/reports/7.0.19 \
    --check-apply <scotch.git> --order upstream/reports/7.0.19/order.txt --repro-root upstream/repro
```

- Start with `git cherry v7.0.19 combined18` against this round's bundle to see what upstream merged.
- Regenerate the group digests from `upstream/reports/7.0.18/findings.json` (the generator is inline in this
  session's history; it is ~30 lines: per group, the items, the files the group's patches touch, and the upstream
  commits touching them).
- Groups with no upstream change and patches that still apply (this round: G08-G11, G15, G27) need only a kit
  re-run and a rebase. Give them to an agent that also has a heavy group.
- One agent per 2-5 groups (8 agents) worked. Total about 4M agent tokens; each agent took 55-95 minutes.

## 5. Decisions still open

Cross-group ones first; the full list is `DECISIONS.md`.

- **Unconditional release-build MPI agreements**, in G18 0003/0004/0006, G19 0001/0002 and G21 0002/0008. In
  7.0.18 the maintainer fixed one such hang by piggybacking on an existing collective (484adec3) instead of adding
  one. Send ours, or ask him for his preferred idiom first?
- **Behaviour changes in our patches.** These now become errors: `dist` with multiplier 0 (G01 0003),
  `SCOTCH_archHcub(0)` (G02 0006), `SCOTCH_archTorus*()` sizes < 1 (G03), `ubvec[0] == 1.0` (G25 0003, which 7.0.18
  takes as zero tolerance), duplicate labels in `graphCheck` (G14), HB fields wider than 80 characters (G14 0009),
  and user dgraphs with edge loads <= 0 or out-of-range ends (`dgraphCheck`, G19 0006).
- **Visible output changes.** dgmap's label `Cmlosum` becomes `CommLoad` (G16 0007, breaks greps). PartDiam for
  disconnected parts (G16: ours links components by one edge; alt sums diameters).
- **DMO-03** (G22): keep his fatal error when the sequential method fails on some ranks, or match the vdgraph twin
  (alt). The two twins now disagree (PS3-G22-1).
- **PS2-G06-1**: the fixed-seed context option is never read. Reseed per context or once per process? No commit
  until decided.
- **G29 0007** re-adds two custom targets of the kind 635741dd removed (needed to stop `metis.h` / `esmumps.h`
  being generated twice under `make -j`).
- **G28 0004** makes `errorPrint` in `libptscotcherrexit` call `MPI_Abort`.
- **G17 0006/0007** add Fortran wrappers for undocumented C routines.

## 6. Lessons from this round

- **Semantic conflicts pass `git am`.** G28 changed a test helper's signature while G12 added calls with the old
  one. Both applied cleanly, and `check_scotch_graph_map` segfaulted. After combining, grep the `check` build logs
  for warnings: test programs are compiled during `make check`, outside the warning-free library build.
- **Three groups fixed the same new regression** (22c9c42e) because it lives in one group's file and shows up in
  two others' tests. When an agent reports a defect in another group's file, forward it at once (SendMessage) and
  decide who owns the commit before merge.
- **"Fixed" in the previous round may mean "fixed by our own other patch"**: an agent auditing on bare v7.0.N+1 sees it
  again. Not a regression; check `notes` before reporting one.
- **A shared, read-only INFRA tree got written to** (`v7.0.18/src/check/data/m4x4.grf` overwritten with a deco file
  at 08:46). Restore from `git show v7.0.18:<path>` and verify INFRA before QA: `git ls-tree` + `cmp` loop. Make it
  read-only (`chmod -R a-w`) next time.
- **Root containers**: `mpirun` refuses to run as root unless `OMPI_ALLOW_RUN_AS_ROOT=1 OMPI_ALLOW_RUN_AS_ROOT_CONFIRM=1`.
  The bump script's gate fails with a misleading "nondeterminism" message otherwise.
- `make` inside the ASan trees needs `CCD = mpicc` in `Makefile.inc` for `ptdummysizes`.
- `merge-audit` used to copy a patch cited by another group into the citing group's folder (fixed this round). It also
  used to delete a group's whole repro folder (fixed: kits are updated in place).
