# dgraphFoldComm: "internal error (2)" on the second DGF1 distribution

Analysis of the second `dgraphFoldComm()` failure reported against
v7.0.17rc2, reproduced and root-caused on v7.0.16 (`ea5d156`) with the
rc2 fix (sender/receiver flag reset on retry) applied.

```
DGF1 72 4823836 { 60543, 131855, 134017, ... , 77611, 536 }
ERROR: dgraphFoldComm: internal error (2)
ERROR: dgraphFold2: cannot compute folding communications (2)
```

The string is the per-process vertex count array of a 72-process graph
(the 72 values sum to exactly 4823836). The failing fold is the one
toward part 0 (`partval = 0`: processes 0–35 receive, 36–71 send); the
fold toward part 1 succeeds.

## Verdict

* The rc2 fix is correct but addresses a different bug. This input still
  fails with master + rc2 fix.
* The fix is the **second** commit of the `skoloCFD/scotch`
  `fix/dgraph-fold-comm-retry` branch, `56f87ba` "Bugfix: skip fold
  sender-receiver that keeps all its vertices", which rc2 did not take.
  It is two lines. `0001-*.patch` is that commit cherry-picked onto
  v7.0.16 master (it applies equally on rc2; the hunk is independent of
  the flag-reset hunk).
* With it, `dgraphFoldComm()` succeeds at `commmax = 4` with no retry, and a
  full `dgraphFold()` of a 4.8M-vertex graph with this distribution passes
  `dgraphCheck()` plus vertex/edge/load conservation checks, for both fold
  directions and bases 0 and 1 (see "Verification").

## Root cause

The end game of the greedy loop, traced on rank 0 (all ranks run the same
deterministic computation). Receiver targets are 133995/133996 vertices.

Round `commmax = 4` (abridged):

```
SEL SND proc=38 sndnbr=96   dlt=0 rcvnum=29(proc 25,exc -26, commrcv 2) rcvnnd=35
  XFER 26 -> rcv proc 25 (slot 2) full=1 remain=70
  XFER 70 -> rcv proc 8  (slot 0) full=0 remain=0
SEL S/R proc=22 sndnbr=54   dlt=0 rcvnum=30(proc 8, exc -201, commrcv 1) rcvnnd=34
  XFER 54 -> rcv proc 8  (slot 1)
SEL S/R proc=4  sndnbr=53   ...  XFER 53 -> rcv proc 8 (slot 2)
SEL S/R proc=6  sndnbr=52   ...  XFER 52 -> rcv proc 8 (slot 3)      <- 4th message: proc 8 dropped, 42 vertices of capacity unused
SEL S/R proc=2  sndnbr=21   dlt=0 rcvnum=31(proc 3, exc 20, commrcv 0) rcvnnd=31
CAP commnbr=1 cap=-20 -> sndnbr=0 dlt=21                              <- overload absorbs the whole excess
  XFER 0 -> rcv proc 3 (slot 0)                                       <- zero-vertex send recorded anyway
REDO                                                                  <- pure sender proc 56 (1 vertex) still unplaced
```

Round `commmax = 5`: proc 8 now takes five messages (70, 54, 53, 52, 21)
and is dropped again with 21 vertices of capacity unused. Then:

```
SEL S/R proc=3 sndnbr=20 dlt=0 rcvnum=31(proc 3, exc 20, commrcv 0) rcvnnd=30   <- receiver pool is empty
CAP commnbr=0 cap=0 -> sndnbr=0 dlt=20
ERROR: dgraphFoldComm: internal error (2)      (sortrcvnum 31 > sortrcvnnd 30)
```

Two things combine:

1. A receiver that reaches `commmax` messages is dropped from the pool even
   though it still has capacity (the message cap, not fullness, ends it).
   That capacity is lost for the round; the leftover is pushed onto the
   remaining sender-receivers (procs 2, 3: excess 21, 20) and the last pure
   sender (proc 56: 1 vertex). This is by design and the retry loop is the
   intended remedy.
2. When a sender-receiver is selected and the remaining receivers cannot
   take its excess, the overload computation raises `vertglbdlt` and
   reduces `vertsndnbr`, possibly to 0: the process should simply keep all
   its vertices. But the `do { ... } while (vertsndnbr > 0)` loop still
   executes once. It records a zero-vertex send and, when the pool is
   already empty, reads `procsrttab[sortrcvnum]` one past the pool, which
   is the sender-receiver's own (just removed) entry: a zero-vertex
   self-send. `SCOTCH_DEBUG_DGRAPH2` catches it as internal error (2).

The fix (`56f87ba`) is to `continue` the outer loop when `vertsndnbr <= 0`
after the overload adjustment: the sender-receiver has already been
removed from the receiver pool, its slot 0 keeps its full local count, and
`vertglbdlt` has been raised to cover its excess, so nothing else needs to
change. In the traced case proc 2 then keeps everything (dlt becomes 21),
and proc 56's single vertex fits in proc 3 (capacity 21 − 20 = 1): the
round completes at `commmax = 4`.

## What happens in a non-debug build

Without `SCOTCH_DEBUG_DGRAPH2` no check fires. On this exact input the
bogus rounds are discarded by the retry (proc 56 forces REDO), and the loop
converges at `commmax = 7` with perfect balance, so a release build does
not fail here; the failing user must be running with the DGRAPH2 checks
on, or rc2 enables them.

If the one-vertex pure sender were absent (`cnttab[56] = 0`), master + rc2
ends round 4 with the zero-vertex send proc 2 → proc 3 in the *final*
plan: proc 2 is flagged sender-receiver and sends a zero-length message.
`dgraphFold()` still completes correctly (the matching zero-length receive
is posted), so it is wasteful rather than fatal in that variant. The
empty-pool self-send ending in the final plan was not reproduced; it
would need no pure sender left at that point.

## Balance after the fix

Final `proccnttab` of the folded graph (36 processes), fixed code,
`commmax = 4`:

```
133996 133996 134017 134017 133996 ... 133954 ... 133995 ... 133995
```

Procs 2 and 3 keep their 21/22 surplus vertices, proc 8 ends 42 under
target. Max deviation 42 vertices on ~134k (0.03 %), against three extra
retry rounds and seven messages per process for perfect balance.

## Verification

Environment: OpenMPI, 72 oversubscribed ranks, `INTSIZE=64`,
`-DSCOTCH_DEBUG_DGRAPH2` (also run without, where noted). The two
programs here are linked against `libptscotch.a` and call the internal
API directly.

`test_foldcomm.c` builds only the process arrays of a `Dgraph` and calls
`dgraphFoldComm()`; `test_fold2.c` builds a real 4,823,836-vertex graph
(vertex g adjacent to g±1 and g±1000 mod N, vertex load (g mod 1000)+1),
calls `dgraphFold()` and checks the folded graph with `dgraphCheck()`,
vertex/edge/load totals, the `vnumloctax` permutation (sum and sum of
squares) and per-vertex load against `vnum`. Usage: `<partval> [baseval]
[rank=count]` where the third argument overrides one entry of the
distribution.

| code                                   | `dgraphFoldComm` partval 0 | partval 1 | `dgraphFold` + checks |
|----------------------------------------|----------------------------|-----------|------------------------|
| v7.0.16 master                         | internal error (2), round 5 | OK, commmax 4 | fails            |
| master + rc2 flag reset (`e4c47a5`)    | internal error (2), round 5 | OK        | fails ("cannot compute folding communications (1)/(2)") |
| master + rc2 + `56f87ba` (this series) | OK, commmax 4, no retry    | OK        | OK for partval 0/1, baseval 0/1 |
| skoloCFD `fix/dgraph-fold-sender-receiver` (`929b0db`) | internal error (2) | – | – |
| master + rc2, no DGRAPH2 checks        | OK, commmax 7              | OK        | OK |

Build line used for the tests (adjust paths):

```sh
mpicc -O2 -std=gnu99 -DSCOTCH_PTSCOTCH -DSCOTCH_DEBUG_DGRAPH2 -DINTSIZE64 -DSCOTCH_RENAME \
  -DSCOTCH_VERSION_NUM=7 -DSCOTCH_RELEASE_NUM=0 -DSCOTCH_PATCHLEVEL_NUM=16 -Drestrict=__restrict \
  -I src/libscotch -I build/src/libscotch -I build/src/include \
  test_fold2.c -o test_fold2 build/lib/libptscotch.a build/lib/libscotch.a build/lib/libptscotcherr.a -lm
mpirun --oversubscribe -np 72 ./test_fold2 0 0
```

## Note on the larger fork branch

`fix/dgraph-fold-sender-receiver` (`929b0db`) rewrites `dgraphFold2()` on
the premise that a sender-receiver may also receive. Within one round of
`dgraphFoldComm()` that cannot happen: sender-receivers are taken from the
top of the receiver sort array and removed, while receives are consumed
from the bottom, so a process is either a sender-receiver or a receiver.
That branch does not fix this input and is not recommended.
