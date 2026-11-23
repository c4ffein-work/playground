#!/bin/sh
# Runs the tests of the bank Swift package against a mock of the Qonto API on this machine:
# Swift -> UniFFI -> Rust -> crates/bank-core/tests/mock (the same mock as the Rust tests).
# Starts the mock on a free port and stops it after. Never the real API: there is nothing to configure.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
package="$root/apps/bank/Package"

cd "$root"
cargo build -q -p bank-core --example mock_qonto

state=$(mktemp -d)
# Two transactions per page, so that the tests go through several pages.
"$root/target/debug/examples/mock_qonto" "$state/url" 2 >"$state/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -rf "$state"' EXIT

tries=0
until [ -f "$state/url" ]; do
    tries=$((tries + 1))
    if [ "$tries" -gt 100 ] || ! kill -0 "$server" 2>/dev/null; then
        cat "$state/server.log" >&2
        echo "the mock of the Qonto API did not start" >&2
        exit 1
    fi
    sleep 0.1
done

# The credentials the mock accepts (mock::SLUG, mock::SECRET): made up, and of no use anywhere else.
BANK_E2E_URL=$(cat "$state/url")
export BANK_E2E_URL BANK_E2E_SLUG=test-org-slug BANK_E2E_SECRET=test-secret-key

cd "$package" && "$root/scripts/swift.sh" test
