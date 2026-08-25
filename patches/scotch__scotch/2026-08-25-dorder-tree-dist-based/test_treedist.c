/* Sanity test: SCOTCH_dgraphOrderTreeDist must return a treetab whose
   values are based (start at baseval), root father staying -1.
   Ring graph of 16 vertices split evenly over the processes.

   Build (against a PT-Scotch build tree <B>, e.g. cmake -DBUILD_PTSCOTCH=ON):
     mpicc -O2 -I<B>/src/include test_treedist.c -o test_treedist \
       <B>/lib/libptscotch.a <B>/lib/libscotch.a \
       <B>/lib/libptscotcherrexit.a -lm -lz -lpthread
   Add -DBASEVAL=0 to check the 0-based case (default is 1).

   Run (process count must divide GLBNBR):
     mpirun -np 2 ./test_treedist
     mpirun -np 4 ./test_treedist

   Expected: "OK: treetab is based ..." and exit code 0; e.g. on 2 ranks
   with baseval 1: treetab = [-1, 1, 1, 1] (was [-1, 0, 0, 0] pre-fix). */

#include <stdio.h>
#include <stdlib.h>
#include <mpi.h>
#include <stdint.h>
#include "ptscotch.h"

#define GLBNBR 16
#ifndef BASEVAL
#define BASEVAL 1
#endif

int
main (int argc, char *argv[])
{
  SCOTCH_Dgraph       grafdat;
  SCOTCH_Dordering    ordedat;
  SCOTCH_Strat        stradat;
  SCOTCH_Num          vertloctab[GLBNBR + 1];
  SCOTCH_Num          edgeloctab[2 * GLBNBR];    /* degree 2 each */
  SCOTCH_Num          vertlocnbr;
  SCOTCH_Num          cblkglbnbr;
  SCOTCH_Num *        treeglbtab;
  SCOTCH_Num *        sizeglbtab;
  int                 proclocnum;
  int                 procglbnbr;
  int                 o = 0;

  MPI_Init (&argc, &argv);
  MPI_Comm_rank (MPI_COMM_WORLD, &proclocnum);
  MPI_Comm_size (MPI_COMM_WORLD, &procglbnbr);

  vertlocnbr = GLBNBR / procglbnbr;
  for (SCOTCH_Num i = 0; i <= vertlocnbr; i ++)
    vertloctab[i] = BASEVAL + 2 * i;
  for (SCOTCH_Num i = 0; i < vertlocnbr; i ++) {
    SCOTCH_Num glb = proclocnum * vertlocnbr + i; /* un-based global index */
    edgeloctab[2 * i]     = BASEVAL + (glb + GLBNBR - 1) % GLBNBR;
    edgeloctab[2 * i + 1] = BASEVAL + (glb + 1) % GLBNBR;
  }

  if (SCOTCH_dgraphInit  (&grafdat, MPI_COMM_WORLD) != 0) return (1);
  if (SCOTCH_dgraphBuild (&grafdat, BASEVAL, vertlocnbr, vertlocnbr,
                          vertloctab, vertloctab + 1, NULL, NULL,
                          2 * vertlocnbr, 2 * vertlocnbr,
                          edgeloctab, NULL, NULL) != 0) return (1);
  if (SCOTCH_dgraphCheck (&grafdat) != 0) return (1);
  SCOTCH_stratInit (&stradat);
  if (SCOTCH_dgraphOrderInit    (&grafdat, &ordedat) != 0) return (1);
  if (SCOTCH_dgraphOrderCompute (&grafdat, &ordedat, &stradat) != 0) return (1);

  if ((cblkglbnbr = SCOTCH_dgraphOrderCblkDist (&grafdat, &ordedat)) < 0) return (1);
  treeglbtab = malloc (cblkglbnbr * sizeof (SCOTCH_Num));
  sizeglbtab = malloc (cblkglbnbr * sizeof (SCOTCH_Num));
  if (SCOTCH_dgraphOrderTreeDist (&grafdat, &ordedat, treeglbtab, sizeglbtab) != 0) return (1);

  if (proclocnum == 0) {
    SCOTCH_Num          rootnbr = 0;
    SCOTCH_Num          sizsum  = 0;

    printf ("cblkglbnbr = %ld\ntreetab:", (long) cblkglbnbr);
    for (SCOTCH_Num i = 0; i < cblkglbnbr; i ++)
      printf (" %ld", (long) treeglbtab[i]);
    printf ("\nsizetab:");
    for (SCOTCH_Num i = 0; i < cblkglbnbr; i ++)
      printf (" %ld", (long) sizeglbtab[i]);
    printf ("\n");

    for (SCOTCH_Num i = 0; i < cblkglbnbr; i ++) {
      if (treeglbtab[i] == -1) {
        rootnbr ++;
        if (sizeglbtab[i] != GLBNBR) {            /* root subtree holds whole graph */
          printf ("KO: root size %ld != %d\n", (long) sizeglbtab[i], GLBNBR);
          o = 1;
        }
      }
      else if ((treeglbtab[i] < BASEVAL) || (treeglbtab[i] >= cblkglbnbr + BASEVAL)) {
        printf ("KO: treetab[%ld] = %ld out of based range [%d, %ld]\n",
                (long) i, (long) treeglbtab[i], BASEVAL, (long) (cblkglbnbr + BASEVAL - 1));
        o = 1;
      }
    }
    if (rootnbr != 1) {
      printf ("KO: %ld roots\n", (long) rootnbr);
      o = 1;
    }
    if (o == 0)
      printf ("OK: treetab is based (baseval = %d), single root = -1\n", BASEVAL);
  }

  SCOTCH_dgraphOrderExit (&grafdat, &ordedat);
  SCOTCH_stratExit       (&stradat);
  SCOTCH_dgraphExit      (&grafdat);
  MPI_Finalize ();
  return (o);
}
