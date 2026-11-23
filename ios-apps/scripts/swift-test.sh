#!/bin/sh
# Runs the tests of the companion Swift package against a real companion server:
# Swift -> UniFFI -> Rust -> companion.py. Starts the server on a free port and stops it after.
#   COMPANION_PY   path to companion.py (default: ../clones/companion/src/companion.py)
#   SWIFT_IMAGE    Docker image used when there is no `swift` here (default: swift:6.1-noble)
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
package="$root/apps/companion/Package"
: "${COMPANION_PY:=$root/../clones/companion/src/companion.py}"
: "${SWIFT_IMAGE:=swift:6.1-noble}"
[ -f "$COMPANION_PY" ] || { echo "no companion.py at $COMPANION_PY: set COMPANION_PY" >&2; exit 1; }

state=$(mktemp -d)
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
export COMPANION_E2E_URL="http://127.0.0.1:$port"
export COMPANION_E2E_ID=swift-e2e
export COMPANION_E2E_SECRET=0123456789abcdef0123456789abcdef

COMPANION_CONFIG="$state/no-config.json" COMPANION_STATE_DIR="$state/state" COMPANION_RATE_LIMIT_MAX=1000 \
COMPANION_BOOTSTRAP_ADMIN="$COMPANION_E2E_ID:$COMPANION_E2E_SECRET" \
    python3 "$root/crates/companion-core/tests/run_companion.py" "$COMPANION_PY" server --port "$port" \
    >"$state/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -rf "$state"' EXIT

python3 - "$COMPANION_E2E_URL" <<'PY' || { cat "$state/server.log" >&2; exit 1; }
import sys, time, urllib.request
for _ in range(100):
    try:
        urllib.request.urlopen(sys.argv[1], timeout=1)
        sys.exit(0)
    except OSError:
        time.sleep(0.1)
sys.exit("the companion server did not start")
PY

if command -v swift >/dev/null 2>&1; then
    (cd "$package" && swift test)
else
    docker run --rm --network host -v "$root":"$root" -w "$package" \
        -e COMPANION_E2E_URL -e COMPANION_E2E_ID -e COMPANION_E2E_SECRET "$SWIFT_IMAGE" swift test
fi
