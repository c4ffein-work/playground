# Report: basing the tree array returned by `dorderTreeDist()`

Target repo: [gitlab.inria.fr/scotch/scotch](https://gitlab.inria.fr/scotch/scotch),
cut against `master` at v7.0.13 (see `manifest.json` for the exact base SHA).
Series: `0001-Base-the-tree-array-returned-by-dorderTreeDist-as-do.patch`.

## What was asked

François's request (originally in French, paraphrased): in
`src/libscotch/dorder_tree_dist.c`, the routine `dorderTreeDist()` computes a
`treetab` array whose values are not *based* — they always start at 0 instead
of at `baseval`, the base-index field of the `grafptr` distributed graph
passed as a parameter, which the routine did not use at all. Modify
`dorderTreeDist()` (and its sub-routines if needed) so that the returned
array is based.

## Understanding the code

`dorderTreeDist()` is the internal implementation of the public
`SCOTCH_dgraphOrderTreeDist()` routine. It gathers, on every MPI process, the
distributed part of the elimination tree of a distributed ordering: for each
distributed column block *i*, `treeglbtab[i]` receives the index of its
father in the tree (or −1 for the root) and `sizeglbtab[i]` the number of
vertices in its subtree.

The routine works in stages:

1. Each process contributes a 4-tuple per locally-rooted column block
   (global block index, inverse-permutation start index, father's global
   block index, subtree vertex count), all-gathered into `dataglbtab`.
   The root's father slot holds −1.
2. Blocks are sorted by ascending start index to derive the *permuted*
   column block numbering (`srt1glbtab[2k+1] = dblkglbnum`) — this is where
   the un-based 0-, 1-, 2-… numbering was born.
3. Father fields in `dataglbtab` are rewritten from original block indices
   to the permuted numbering. The root entry is never rewritten (the
   rewrite loop starts at 1, skipping the lone −1), so it keeps −1.
4. Results are scattered into `treeglbtab` / `sizeglbtab` at the slot given
   by each block's permuted index.

Three observations settled what "based" must mean here:

- The **user manual** (`doc/src/ptscotch/p_l.tex`, section
  `SCOTCH_dgraphOrderTreeDist`) already documents the intended behavior:
  "`treeglbtab[i]` holds the index of the father of node *i* in the
  elimination tree, or −1 if *i* is the root of the tree. All node indices
  start from `baseval`." So the fix makes the code match its documentation;
  the root sentinel stays −1 regardless of base.
- The **sequential counterpart** `orderTree()` (`src/libscotch/order.c`)
  already numbers column blocks from `baseval` and accesses `treetab`
  through a based pointer (`treetab - baseval`), with −1 as the root's
  father. The distributed routine should follow the same convention.
- The **ParMeTiS compatibility wrapper**
  (`src/libscotchmetis/parmetis_dgraph_order.c`) already compensates:
  `fathnum = treeglbtab[cblknum] - baseval; /* Use un-based indices */` and
  treats `fathnum < 0` as the root. It was written for the based
  convention, and only worked so far because ParMeTiS callers mostly use
  `numflag = 0`. With the fix it becomes correct for `numflag = 1` too, and
  stays correct for 0.

## The change

All contained in `dorderTreeDist()`; no sub-routine needed modification.

- Read `baseval` from the hitherto-unused `grafptr` parameter.
- In stage 2, assign based permuted indices:
  `srt1glbtab[2 * dblkglbnum + 1] = dblkglbnum + baseval;`. The father
  rewrite of stage 3 then propagates based values into `dataglbtab`
  automatically; the root keeps −1.
- In stage 4, write through based pointers
  (`treeglbtax = treeglbtab - baseval;`, likewise `sizeglbtax`), the usual
  Scotch "tax" idiom, so the entry for block `baseval + k` still lands at
  array slot `k`. Without this, the now-based slot indices would have
  written one cell past the end of both user arrays for `baseval = 1`.
- Updated the routine's header comment (and the file's version-date block)
  to state the convention explicitly.

Nothing else consumes these values internally — the only callers are the
public library wrapper and the ParMeTiS wrapper discussed above — so no
other code required adjustment.

## QA

No MPI toolchain was present in the session container, so OpenMPI and flex
were installed first. Verification steps:

1. **Build**: configured with CMake (`BUILD_PTSCOTCH=ON`, Release) and built
   `libptscotch`; the modified file compiles with no new warnings.
2. **Behavioral test**: wrote a small MPI program
   (`test_treedist.c`, in this directory, with build/run instructions in
   its header comment) that builds a distributed
   ring graph of 16 vertices with `SCOTCH_dgraphBuild`, computes an ordering
   with the default strategy, then calls `SCOTCH_dgraphOrderCblkDist()` and
   `SCOTCH_dgraphOrderTreeDist()` and checks the invariants: every value of
   `treeglbtab` is either −1 (exactly once) or lies in
   `[baseval, cblkglbnbr + baseval − 1]`, and the root's `sizeglbtab` entry
   equals the total vertex count.
3. **Results**:
   - 2 ranks, `baseval = 1`: `cblkglbnbr = 4`,
     `treetab = [-1, 1, 1, 1]`, `sizetab = [16, 7, 7, 2]` — based values,
     single root at −1. (Pre-fix, the fathers would have read 0.)
   - 2 ranks, `baseval = 0`: `treetab = [-1, 0, 0, 0]` — bit-identical to
     the pre-change behavior, confirming no regression for the 0-based case.
   - 4 ranks, `baseval = 1`: `cblkglbnbr = 10`,
     `treetab = [-1, 1, 2, 2, 2, 1, 6, 6, 6, 1]`,
     `sizetab = [16, 7, 3, 3, 1, 7, 3, 3, 1, 2]` — a two-level nested
     dissection tree, all father indices within the based range, subtree
     sizes consistent.

The debug-mode internal checks (`SCOTCH_DEBUG_DORDER2`) were reviewed by
hand: they operate on the pre-permutation, un-based block indices (or on the
raw −1 root sentinel before rewriting), so none of them is affected by the
basing.

## Applying

```sh
scripts/apply-patch.sh patches/scotch__scotch/2026-08-25-dorder-tree-dist-based /path/to/scotch-clone
```

This recreates the commit (message and authorship included) on a
`patch/2026-08-25-dorder-tree-dist-based` branch via `git am --3way`.
