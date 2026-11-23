#!/bin/sh
# Runs `swift <args>` in the current directory: with the local toolchain, or in Docker when there is none.
#   SWIFT_IMAGE   the image to use (default: swift:6.1-noble)
# In Docker, the container shares the host network (a test server on 127.0.0.1 is reachable)
# and gets every environment variable whose name contains _E2E_.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
if command -v swift >/dev/null 2>&1; then
    exec swift "$@"
fi
: "${SWIFT_IMAGE:=swift:6.1-noble}"
set -- "$SWIFT_IMAGE" swift "$@"
for name in $(env | sed -n 's/^\([A-Za-z0-9_]*_E2E_[A-Za-z0-9_]*\)=.*/\1/p'); do
    set -- -e "$name" "$@"
done
exec docker run --rm --network host -v "$root":"$root" -w "$PWD" "$@"
