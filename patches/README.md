# Patch drop-box

Cloud sessions can only push to repos they're scoped to — but they can *read*
any public repo. This directory closes the gap: a session clones the target
repo, commits changes locally, and exports them here as a `git format-patch`
series. You then pull this repo and apply the series onto your own clone of
the target, where you have real push access.

**Public repos only.** Patches contain full code context (changed hunks plus
surrounding lines). Never store diffs cut against a private repo here.

## Layout

```
patches/
  <owner>__<repo>/                  # target repo, / replaced by __
    <YYYY-MM-DD>-<slug>/            # one change = one directory
      manifest.json                 # repo, remote, base_branch, base_sha, title
      0001-<subject>.patch          # git format-patch output, binary-safe
      0002-...
```

`manifest.json` records the exact commit (`base_sha`) the series was cut
against. If a series stops applying cleanly, that SHA tells you how far the
target has drifted since.

## Generating a series (session side)

```sh
git clone https://github.com/<owner>/<repo> /path/to/clone
# ...make changes, commit with good messages...
scripts/new-patch.sh /path/to/clone <slug>
git add patches/ && git commit && git push
```

An optional third argument overrides the base ref (default: origin's default
branch).

## Applying a series (your side)

```sh
scripts/apply-patch.sh patches/<owner>__<repo>/<date>-<slug> /path/to/your/clone
```

This creates branch `patch/<date>-<slug>` in your clone (from the recorded
base commit when available, otherwise from `origin/<base_branch>`) and runs
`git am --3way`, which reconstructs the original commits — messages and
authorship included. From there: review, rebase, push, open a PR, whatever
you like.

If `git am` stops on a conflict, resolve it and `git am --continue`, or bail
out with `git am --abort`.

## Lifecycle

Delete a series' directory once it has landed in the target repo (or is
abandoned) so this directory only ever holds pending work.
