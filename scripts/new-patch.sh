#!/usr/bin/env bash
# Cut a patch series from a local clone and store it under patches/.
#
# Usage: scripts/new-patch.sh <clone-dir> <slug> [base-ref]
#
#   <clone-dir>  local clone of the target repo, with your commits on HEAD
#   <slug>       short kebab-case name for the change (used in the dir name)
#   [base-ref]   ref the series is based on (default: origin's default branch)
#
# Output: patches/<owner>__<repo>/<YYYY-MM-DD>-<slug>/ containing
# NNNN-*.patch files (git format-patch, binary-safe) and a manifest.json
# recording the exact base commit the series applies onto.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ $# -lt 2 ]; then
  grep '^# ' "$0" | sed 's/^# \{0,1\}//'
  exit 1
fi

CLONE="$1"
SLUG="$2"
BASE_REF="${3:-}"

cd "$CLONE"

REMOTE_URL="$(git remote get-url origin)"
# owner/repo from https://github.com/owner/repo(.git) or git@github.com:owner/repo(.git)
OWNER_REPO="$(printf '%s' "$REMOTE_URL" | sed -E 's#^(https://[^/]+/|git@[^:]+:)##; s#\.git$##')"
OWNER="${OWNER_REPO%%/*}"
REPO="${OWNER_REPO#*/}"

if [ -z "$BASE_REF" ]; then
  BASE_REF="$(git symbolic-ref --quiet refs/remotes/origin/HEAD || true)"
  if [ -z "$BASE_REF" ]; then
    git remote set-head origin --auto >/dev/null
    BASE_REF="$(git symbolic-ref refs/remotes/origin/HEAD)"
  fi
fi
BASE_BRANCH="${BASE_REF##*/}"
BASE_SHA="$(git merge-base "$BASE_REF" HEAD)"

if [ "$BASE_SHA" = "$(git rev-parse HEAD)" ]; then
  echo "error: no commits on HEAD beyond $BASE_REF — nothing to export" >&2
  exit 1
fi

TITLE="$(git log --format=%s "${BASE_SHA}..HEAD" | tail -1)"
COUNT="$(git rev-list --count "${BASE_SHA}..HEAD")"
DATE="$(date +%F)"

OUT_DIR="$REPO_ROOT/patches/${OWNER}__${REPO}/${DATE}-${SLUG}"
mkdir -p "$OUT_DIR"

git format-patch --binary --output-directory "$OUT_DIR" "${BASE_SHA}..HEAD" >/dev/null

OWNER_REPO="$OWNER_REPO" REMOTE_URL="$REMOTE_URL" BASE_BRANCH="$BASE_BRANCH" \
BASE_SHA="$BASE_SHA" DATE="$DATE" COUNT="$COUNT" TITLE="$TITLE" \
python3 - "$OUT_DIR/manifest.json" <<'PY'
import json, os, sys
with open(sys.argv[1], "w") as f:
    json.dump({
        "repo": os.environ["OWNER_REPO"],
        "remote": os.environ["REMOTE_URL"],
        "base_branch": os.environ["BASE_BRANCH"],
        "base_sha": os.environ["BASE_SHA"],
        "created": os.environ["DATE"],
        "patches": int(os.environ["COUNT"]),
        "title": os.environ["TITLE"],
    }, f, indent=2)
    f.write("\n")
PY

echo "wrote $(ls "$OUT_DIR" | grep -c '\.patch$') patch(es) to ${OUT_DIR#"$REPO_ROOT"/}"
