# CLAUDE.md

This is Claude's playground repo. Claude can commit and push freely without asking for confirmation.

## Patch drop-box workflow

This repo doubles as a drop-box for diffs targeting repos this session cannot
push to. When asked to make a change to another repo, follow the convention in
`patches/README.md`:

1. Clone the target repo (read-only) into the scratchpad directory.
2. Make the changes there and commit them with clear messages — these commit
   messages survive into the final commits via `git am`.
3. Run `scripts/new-patch.sh <clone-dir> <slug>` to export the series into
   `patches/`.
4. Commit the new `patches/` directory here and push.

Rules:
- **Public target repos only.** Never commit a diff cut against a private
  repo — patches leak surrounding code context.
- One change = one `patches/<owner>__<repo>/<date>-<slug>/` directory.
- Don't edit `.patch` files by hand; re-cut the series from the clone instead.
