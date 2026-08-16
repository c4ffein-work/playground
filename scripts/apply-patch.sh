#!/usr/bin/env bash
# Apply a stored patch series onto a local clone of its target repo.
#
# Usage: scripts/apply-patch.sh <patch-dir> <clone-dir>
#
#   <patch-dir>  a patches/<owner>__<repo>/<date>-<slug>/ directory
#   <clone-dir>  your local clone of the target repo
#
# Creates branch patch/<date>-<slug> in the clone — from the recorded base
# commit when the clone has it, otherwise from origin/<base_branch> — then
# applies the series with `git am --3way`.
set -euo pipefail

if [ $# -ne 2 ]; then
  grep '^# ' "$0" | sed 's/^# \{0,1\}//'
  exit 1
fi

PATCH_DIR="$(cd "$1" && pwd)"
CLONE="$2"
MANIFEST="$PATCH_DIR/manifest.json"

[ -f "$MANIFEST" ] || { echo "error: no manifest.json in $PATCH_DIR" >&2; exit 1; }

read -r TARGET_REPO BASE_BRANCH BASE_SHA < <(python3 -c '
import json, sys
m = json.load(open(sys.argv[1]))
print(m["repo"], m["base_branch"], m["base_sha"])
' "$MANIFEST")

cd "$CLONE"

ACTUAL_REPO="$(git remote get-url origin | sed -E 's#^(https://[^/]+/|git@[^:]+:)##; s#\.git$##')"
if [ "$ACTUAL_REPO" != "$TARGET_REPO" ]; then
  echo "error: patch targets $TARGET_REPO but clone's origin is $ACTUAL_REPO" >&2
  exit 1
fi

git fetch origin "$BASE_BRANCH"

BRANCH="patch/$(basename "$PATCH_DIR")"
if git cat-file -e "${BASE_SHA}^{commit}" 2>/dev/null; then
  START="$BASE_SHA"
else
  echo "note: base commit $BASE_SHA not found locally; branching from origin/$BASE_BRANCH (3-way merge will reconcile)"
  START="origin/$BASE_BRANCH"
fi

git switch --create "$BRANCH" "$START"
git am --3way "$PATCH_DIR"/*.patch

echo "applied $(git rev-list --count "${START}..HEAD") commit(s) on branch $BRANCH"
