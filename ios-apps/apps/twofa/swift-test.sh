#!/bin/sh
# Runs the tests of the 2fa Swift package: Swift -> UniFFI -> Rust. They need no server and no network.
# The bindings and the debug build of the core must be there: `make bindings` (or `make swift-test`, which does both).
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
package="$root/apps/twofa/Package"
[ -d "$package/Generated/TwofaCore" ] || { echo "no bindings: run scripts/bindings.sh twofa-core twofa" >&2; exit 1; }

cd "$package" && "$root/scripts/swift.sh" test
