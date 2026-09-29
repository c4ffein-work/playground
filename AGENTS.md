# AGENTS.md

This is Claude's playground repo (Claude goes by **cl4ude** here). Claude can
commit and push freely without asking for confirmation.

## Layout

- `README.md` + `hello.svg` — the animated intro for people discovering the repo.
- `patches/` — the patch drop-box (see below).
- `scripts/new-patch.sh`, `scripts/apply-patch.sh` — export / apply a series.
- `clones/` — **gitignored**. Put local clones of patch-target repos here
  (`clones/<repo>`), not in the repo root and not in `/tmp`. Anything under
  it is scratch: delete freely, never commit.

## Patch drop-box workflow

This repo doubles as a drop-box for diffs targeting repos this session cannot
push to. When asked to make a change to another repo, follow the convention in
`patches/README.md`:

1. Clone the target repo (read-only) into `clones/<repo>`.
2. Make the changes there and commit them with clear messages — these commit
   messages survive into the final commits via `git am`.
3. Run `scripts/new-patch.sh clones/<repo> <slug>` to export the series into
   `patches/`.
4. Commit the new `patches/` directory here and push — **on `main`, directly**,
   even when the session was given a feature branch: this repo is a drop-box,
   a series on a side branch is invisible to the next session and to the
   person applying it. History here is disposable (force-push to clean up).

Rules:
- **Public target repos only.** Never commit a diff cut against a private
  repo — patches leak surrounding code context.
- One change = one `patches/<owner>__<repo>/<date>-<slug>/` directory.
- Don't edit `.patch` files by hand; re-cut the series from the clone instead.
- Delete a series once it has landed upstream (check the target's history:
  the patch reverse-applies cleanly, or its subject is in `git log`). Notes
  written next to a series (`*.md` handoffs) may outlive it if they still
  describe unimplemented work.
