/* Full dgraphFold() on a real 72-process graph with the DGF1 vertex distribution.
** Graph: N vertices, vertex g adjacent to g-1, g+1, g-K, g+K (mod N); vertex load = (g % 1000) + 1.
** Usage: mpirun -np 72 test_fold2 <partval> [baseval]
*/
#include <mpi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "common.h"
#include "dgraph.h"
#include "ptscotch.h"

#define PROCNBR 72
#define KCHORD  1000
static Gnum cnttab[PROCNBR] = { 60543, 131855, 134017, 134016, 134049, 133099, 134048, 123444, 133725, 130856, 70884, 73911, 533, 0, 126, 1597, 133610, 89898, 76594, 112721, 131728, 24227, 134049, 291, 93588, 133629, 7940, 14213, 8049, 46645, 113263, 131490, 65825, 132591, 129222, 17134, 3415, 126638, 96, 996, 133599, 132015, 132499, 1318, 0, 4377, 74274, 109787, 134049, 12681, 8641, 132625, 6124, 130498, 120501, 0, 1, 0, 24006, 1140, 51558, 463, 92345, 63224, 73438, 66990, 3044, 84566, 141, 27230, 77611, 536 };

static Gnum veloOf (Gnum g) { return ((g % 1000) + 1); }

int main (int argc, char ** argv)
{
  int rank, size, partval, i, o, oglb, isrcv, procfldnbr;
  Gnum baseval = 0, N, vertlocnbr, vertglbbas, g, e;
  Gnum * vertloctab, * veloloctab, * edgeloctab;
  SCOTCH_Dgraph orggrafdat;
  Dgraph fldgrafdat;
  Dgraph * orggrafptr = (Dgraph *) &orggrafdat;

  MPI_Init (&argc, &argv);
  MPI_Comm_rank (MPI_COMM_WORLD, &rank);
  MPI_Comm_size (MPI_COMM_WORLD, &size);
  if (size != PROCNBR) { if (rank == 0) fprintf (stderr, "need %d ranks\n", PROCNBR); MPI_Abort (MPI_COMM_WORLD, 1); }
  partval = (argc > 1) ? atoi (argv[1]) : 0;
  if (argc > 2) baseval = atoi (argv[2]);
  if (argc > 3) { int ii; long vv; if (sscanf (argv[3], "%d=%ld", &ii, &vv) == 2) cnttab[ii] = vv; }

  for (i = 0, N = 0, vertglbbas = 0; i < PROCNBR; i ++) { if (i < rank) vertglbbas += cnttab[i]; N += cnttab[i]; }
  vertlocnbr = cnttab[rank];
  vertloctab = malloc ((vertlocnbr + 1) * sizeof (Gnum));
  veloloctab = malloc ((vertlocnbr + 1) * sizeof (Gnum));
  edgeloctab = malloc ((4 * vertlocnbr + 1) * sizeof (Gnum));
  for (i = 0, e = 0; i < vertlocnbr; i ++) {
    g = vertglbbas + i;                           /* Un-based global number */
    vertloctab[i] = e + baseval;
    veloloctab[i] = veloOf (g);
    edgeloctab[e ++] = (g + N - 1) % N + baseval;
    edgeloctab[e ++] = (g + 1) % N + baseval;
    edgeloctab[e ++] = (g + N - KCHORD) % N + baseval;
    edgeloctab[e ++] = (g + KCHORD) % N + baseval;
  }
  vertloctab[vertlocnbr] = e + baseval;

  SCOTCH_dgraphInit (&orggrafdat, MPI_COMM_WORLD);
  if (SCOTCH_dgraphBuild (&orggrafdat, baseval, vertlocnbr, vertlocnbr, vertloctab, NULL, veloloctab, NULL, e, e, edgeloctab, NULL, NULL) != 0) { fprintf (stderr, "build failed\n"); MPI_Abort (MPI_COMM_WORLD, 1); }
  if (SCOTCH_dgraphCheck (&orggrafdat) != 0) { fprintf (stderr, "orig check failed\n"); MPI_Abort (MPI_COMM_WORLD, 1); }

  procfldnbr = (PROCNBR + 1) / 2;
  isrcv = (partval == 0) ? (rank < procfldnbr) : (rank >= procfldnbr);

  memset (&fldgrafdat, 0, sizeof (Dgraph));
  o = dgraphFold (orggrafptr, partval, &fldgrafdat, NULL, NULL, MPI_INT);
  MPI_Allreduce (&o, &oglb, 1, MPI_INT, MPI_MAX, MPI_COMM_WORLD);
  if (rank == 0) printf ("partval=%d baseval=%ld N=%ld : dgraphFold returned %d (max over ranks)\n", partval, (long) baseval, (long) N, oglb);
  if (oglb != 0) { MPI_Finalize (); return (1); }

  if (isrcv) {                                    /* Validate folded graph on receivers */
    Gnum   loc[6], glb[6];                        /* vert, edge, velosum, vnumsum, vnumsqsum(mod), velo-mismatch */
    Gnum   v, bad = 0, badvnum = 0, baddeg = 0;
    int    chk = dgraphCheck (&fldgrafdat), chkglb;
    MPI_Allreduce (&chk, &chkglb, 1, MPI_INT, MPI_MAX, fldgrafdat.proccomm);
    loc[0] = fldgrafdat.vertlocnbr; loc[1] = fldgrafdat.edgelocnbr; loc[2] = 0; loc[3] = 0; loc[4] = 0;
    for (v = fldgrafdat.baseval; v < fldgrafdat.vertlocnnd; v ++) {
      Gnum vn = fldgrafdat.vnumloctax[v] - baseval;
      if ((vn < 0) || (vn >= N)) badvnum ++;
      if (fldgrafdat.veloloctax[v] != veloOf (vn)) bad ++;
      if ((fldgrafdat.vendloctax[v] - fldgrafdat.vertloctax[v]) != 4) baddeg ++;
      loc[2] += fldgrafdat.veloloctax[v]; loc[3] += vn; loc[4] = (loc[4] + (vn * vn) % 1000000007) % 1000000007;
    }
    loc[5] = bad + badvnum + baddeg;
    MPI_Allreduce (loc, glb, 6, GNUM_MPI, MPI_SUM, fldgrafdat.proccomm);
    { Gnum veloexp = 0, sqexp = 0; for (g = 0; g < N; g ++) { veloexp += veloOf (g); sqexp = (sqexp + (g * g) % 1000000007) % 1000000007; }
      if (fldgrafdat.proclocnum == 0) {
        printf ("folded: procglbnbr=%d vertglbnbr=%ld (hdr %ld) edgeglbnbr=%ld (hdr %ld) dgraphCheck=%d\n", fldgrafdat.procglbnbr, (long) glb[0], (long) fldgrafdat.vertglbnbr, (long) glb[1], (long) fldgrafdat.edgeglbnbr, chkglb);
        printf ("checks: vert %s, edge %s, velosum %s, vnum-sum %s, vnum-sqsum %s, per-vertex mismatches %ld\n",
                glb[0] == N ? "OK" : "BAD", glb[1] == 4 * N ? "OK" : "BAD", glb[2] == veloexp ? "OK" : "BAD",
                glb[3] == (N * (N - 1)) / 2 ? "OK" : "BAD", (glb[4] % 1000000007) == sqexp ? "OK" : "BAD", (long) glb[5]);
        printf ("folded proccnttab:"); for (i = 0; i < fldgrafdat.procglbnbr; i ++) printf (" %ld", (long) fldgrafdat.proccnttab[i]); printf ("\n");
      }
    }
    dgraphExit (&fldgrafdat);
  }
  SCOTCH_dgraphExit (&orggrafdat);
  MPI_Finalize ();
  return (0);
}
