#!/bin/sh
# Runs the tests of the feed Swift package: Swift -> UniFFI -> Rust -> feeds served on this machine.
# Starts the fixture server (Package/Tests/fixtures/server.py) on a free port and stops it after.
# Nothing is fetched from the internet.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
package="$root/apps/feed/Package"

log=$(mktemp)
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
export FEED_E2E_URL="http://127.0.0.1:$port"

python3 "$package/Tests/fixtures/server.py" "$port" >"$log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -f "$log"' EXIT

python3 - "$FEED_E2E_URL/atom.xml" <<'PY' || { cat "$log" >&2; exit 1; }
import sys, time, urllib.request
for _ in range(100):
    try:
        urllib.request.urlopen(sys.argv[1], timeout=1)
        sys.exit(0)
    except OSError:
        time.sleep(0.1)
sys.exit("the fixture server did not start")
PY

cd "$package" && "$root/scripts/swift.sh" test
