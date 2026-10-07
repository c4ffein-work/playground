/* Replay dgraphFoldComm() on the DGF1 vertex distribution reported for 72 processes.
** Usage: mpirun -np 72 test_foldcomm <partval> [baseval]
*/
#include <mpi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "common.h"
#include "dgraph.h"
#include "dgraph_fold_comm.h"

#define PROCNBR 72
static Gnum cnttab[PROCNBR] = { 60543, 131855, 134017, 134016, 134049, 133099, 134048, 123444, 133725, 130856, 70884, 73911, 533, 0, 126, 1597, 133610, 89898, 76594, 112721, 131728, 24227, 134049, 291, 93588, 133629, 7940, 14213, 8049, 46645, 113263, 131490, 65825, 132591, 129222, 17134, 3415, 126638, 96, 996, 133599, 132015, 132499, 1318, 0, 4377, 74274, 109787, 134049, 12681, 8641, 132625, 6124, 130498, 120501, 0, 1, 0, 24006, 1140, 51558, 463, 92345, 63224, 73438, 66990, 3044, 84566, 141, 27230, 77611, 536 };

int main (int argc, char ** argv)
{
  int rank, size, partval, procfldnbr, procrcvbas, procrcvnnd, isrcv, ret, retglb, i;
  Gnum baseval = 0;
  Dgraph grafdat;
  Gnum procvrttab[PROCNBR + 1], proccnttab[PROCNBR], fldproccnttab[PROCNBR];
  int commmax = -1, commtypval = -1, vertadjnbr = -1;
  DgraphFoldCommData * commdattab = NULL;
  Gnum * commvrttab = NULL, * vertadjtab = NULL, * vertdlttab = NULL;

  MPI_Init (&argc, &argv);
  MPI_Comm_rank (MPI_COMM_WORLD, &rank);
  MPI_Comm_size (MPI_COMM_WORLD, &size);
  if (size != PROCNBR) { if (rank == 0) fprintf (stderr, "need %d ranks\n", PROCNBR); MPI_Abort (MPI_COMM_WORLD, 1); }
  partval = (argc > 1) ? atoi (argv[1]) : 0;
  if (argc > 2) baseval = atoi (argv[2]);
  if (argc > 3) { int ii; long vv; if (sscanf (argv[3], "%d=%ld", &ii, &vv) == 2) cnttab[ii] = vv; }

  memset (&grafdat, 0, sizeof (Dgraph));
  grafdat.baseval = baseval;
  grafdat.proccomm = MPI_COMM_WORLD;
  grafdat.procglbnbr = PROCNBR;
  grafdat.proclocnum = rank;
  procvrttab[0] = baseval;
  for (i = 0; i < PROCNBR; i ++) { proccnttab[i] = cnttab[i]; procvrttab[i + 1] = procvrttab[i] + cnttab[i]; }
  grafdat.procvrttab = procvrttab;
  grafdat.proccnttab = proccnttab;
  grafdat.procdsptab = procvrttab;
  grafdat.vertglbnbr = procvrttab[PROCNBR] - baseval;
  grafdat.vertlocnbr = cnttab[rank];
  grafdat.vertlocnnd = baseval + cnttab[rank];

  procfldnbr = (PROCNBR + 1) / 2;
  if (partval == 0) { procrcvbas = 0; procrcvnnd = procfldnbr; } else { procrcvbas = procfldnbr; procrcvnnd = PROCNBR; }
  isrcv = (rank >= procrcvbas) && (rank < procrcvnnd);

  ret = dgraphFoldComm (&grafdat, partval, &commmax, &commtypval, &commdattab, &commvrttab,
                        isrcv ? fldproccnttab : NULL, isrcv ? &vertadjnbr : NULL,
                        isrcv ? &vertadjtab : NULL, isrcv ? &vertdlttab : NULL);
  MPI_Allreduce (&ret, &retglb, 1, MPI_INT, MPI_MAX, MPI_COMM_WORLD);
  if (rank == 0)
    printf ("partval=%d baseval=%ld vertglbnbr=%ld : dgraphFoldComm returned %d (max over ranks), commmax=%d\n", partval, (long) baseval, (long) grafdat.vertglbnbr, retglb, commmax);
  if (retglb == 0) {
    for (i = 0; i < PROCNBR; i ++) {
      MPI_Barrier (MPI_COMM_WORLD);
      if (i != rank) continue;
      printf ("rank %2d type=%d vertlocnbr=%7ld :", rank, commtypval, (long) grafdat.vertlocnbr);
      { int c; Gnum tot = 0; for (c = 0; c < commmax && commdattab[c].procnum != -1; c ++) { printf (" [%s%ld:%ld@%ld]", (commtypval & DGRAPHFOLDCOMMSEND) ? "->" : "<-", (long) commdattab[c].procnum, (long) commdattab[c].vertnbr, (long) commvrttab[c]); tot += commdattab[c].vertnbr; }
        printf (" total=%ld", (long) tot); }
      if (isrcv && rank == procrcvbas) { int p; printf ("\n   fldproccnttab:"); for (p = 0; p < procrcvnnd - procrcvbas; p ++) printf (" %ld", (long) fldproccnttab[p]); }
      printf ("\n"); fflush (stdout);
    }
  }
  MPI_Finalize ();
  return (retglb != 0);
}
