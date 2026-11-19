#!/usr/bin/env bash
# End-to-end test: runs the real binary against local upstreams.
#
#   tests/e2e.sh                      routing, redirects, self-signed TLS
#   tests/e2e.sh --acme <pebble-dir>  also issues a certificate from Pebble,
#                                     Let's Encrypt's ACME test server
#
# The ACME part needs a Pebble checkout with both binaries built:
#   go build -o pebble-bin ./cmd/pebble
#   go build -o challtestsrv-bin ./cmd/pebble-challtestsrv
set -euo pipefail
cd "$(dirname "$0")/.."

PEBBLE_DIR=""
[[ "${1:-}" == "--acme" ]] && PEBBLE_DIR="$(cd "$2" && pwd)"

# Everything here is on localhost; an outbound proxy would only get in the way.
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy

cargo build --quiet
BIN="$PWD/target/debug/pingenv"
WORK="$(mktemp -d)"
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

PASS=0
FAIL=0
check() { # check <description> <actual> <expected>
    if [[ "$2" == "$3" ]]; then
        PASS=$((PASS + 1)); echo "  ok    $1"
    else
        FAIL=$((FAIL + 1)); echo "  FAIL  $1: got '$2', expected '$3'"
    fi
}
wait_for_port() {
    for _ in $(seq 100); do
        (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && return 0
        sleep 0.1
    done
    echo "port $1 never opened"; return 1
}
wait_for_exit() {
    for _ in $(seq 100); do kill -0 "$1" 2>/dev/null || return 0; sleep 0.1; done
}
field() { python3 -c 'import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {"d": d}))' "$1"; }

HTTP=18080
HTTPS=18443
RESOLVE=()
for name in app.pingenv.test lb.pingenv.test sub.wild.pingenv.test site.pingenv.test live.pingenv.test split.pingenv.test other.test; do
    RESOLVE+=(--resolve "$name:$HTTP:127.0.0.1" --resolve "$name:$HTTPS:127.0.0.1")
done
get() { curl -sS --max-time 10 "${RESOLVE[@]}" "$@"; }

python3 tests/echo_upstream.py A 18001 & PIDS+=($!)
python3 tests/echo_upstream.py B 18002 & PIDS+=($!)
wait_for_port 18001; wait_for_port 18002

echo "== configuration errors"
set +e
OUT="$(env -i PINGENV_ROUTE_A_HOST=a.test PINGENV_ROUTE_B_UPSTREAM=b PINGENV_TLS_MODE=nope "$BIN" 2>&1)"; CODE=$?
set -e
check "invalid configuration exits with 2" "$CODE" 2
check "reports all problems" "$(grep -c '^  - ' <<<"$OUT")" 4
set +e
OUT="$(env -i PINGENV_ROUTE_A_HOST=a.test PINGENV_ROUTE_A_UPSTREAM=a:1 PINGENV_TLS_MOD=acme TLS_MODE=bogus "$BIN" 2>&1)"; CODE=$?
set -e
check "unknown PINGENV_ variable is an error, unprefixed ones are ignored" \
    "$CODE $(grep -c '^  - ' <<<"$OUT") $(grep -c 'PINGENV_TLS_MOD: unknown setting' <<<"$OUT")" "2 1 1"

# A site to serve: small files, one file above the in-memory limit, things
# that must never be served, and a symlink pointing out of the root.
SITE="$WORK/site"
mkdir -p "$SITE/css" "$SITE/docs" "$SITE/.git" "$SITE/.well-known" "$SITE/empty"
echo '<h1>home</h1>' >"$SITE/index.html"
echo 'body{}' >"$SITE/css/site.css"
echo '<h1>docs</h1>' >"$SITE/docs/index.html"
echo 'SECRET=1' >"$SITE/.env"
echo 'x' >"$SITE/.git/config"
echo 'Contact: mailto:x@pingenv.test' >"$SITE/.well-known/security.txt"
echo 'outside' >"$WORK/outside.txt"
ln -s "$WORK/outside.txt" "$SITE/escape.txt"
ln -s "$SITE/css/site.css" "$SITE/inside.css"
head -c 300000 /dev/urandom >"$SITE/big.bin"
BIG_SUM="$(sha256sum <"$SITE/big.bin")"

echo "== self-signed mode"
env -i \
    PINGENV_HTTP_ADDR=127.0.0.1:$HTTP PINGENV_HTTPS_ADDR=127.0.0.1:$HTTPS PINGENV_HTTPS_REDIRECT_PORT=$HTTPS \
    PINGENV_TLS_MODE=selfsigned \
    PINGENV_ROUTE_APP_HOST=app.pingenv.test PINGENV_ROUTE_APP_UPSTREAM=127.0.0.1:18001 \
    PINGENV_ROUTE_API_HOST=app.pingenv.test PINGENV_ROUTE_API_PATH=/api PINGENV_ROUTE_API_STRIP_PREFIX=true \
    PINGENV_ROUTE_API_UPSTREAM=localhost:18002 PINGENV_ROUTE_API_SET_HOST=backend.internal \
    PINGENV_ROUTE_LB_HOST=lb.pingenv.test PINGENV_ROUTE_LB_UPSTREAM=127.0.0.1:18001,127.0.0.1:18009,127.0.0.1:18002 \
    PINGENV_ROUTE_SITE_HOST=site.pingenv.test PINGENV_ROUTE_SITE_ROOT="$SITE" \
    PINGENV_ROUTE_SITE_MEMORY_MAX_FILE=64KiB PINGENV_ROUTE_SITE_FALLBACK=/index.html \
    PINGENV_ROUTE_ASSETS_HOST=app.pingenv.test PINGENV_ROUTE_ASSETS_PATH=/static \
    PINGENV_ROUTE_ASSETS_ROOT="$SITE" PINGENV_ROUTE_ASSETS_CACHE_CONTROL='public, max-age=60' \
    PINGENV_ROUTE_LIVE_HOST=live.pingenv.test PINGENV_ROUTE_LIVE_ROOT="$SITE" \
    PINGENV_ROUTE_LIVE_MEMORY_MAX_FILE=0 \
    PINGENV_ROUTE_SPLIT_HOST=split.pingenv.test PINGENV_ROUTE_SPLIT_UPSTREAM=127.0.0.1:18090 \
    PINGENV_ROUTE_WILD_HOST='*.wild.pingenv.test' PINGENV_ROUTE_WILD_UPSTREAM=127.0.0.1:18001 \
    "$BIN" >"$WORK/selfsigned.log" 2>&1 &
GATE=$!; PIDS+=($GATE)
wait_for_port $HTTP; wait_for_port $HTTPS

check "http redirects to https" \
    "$(get -o /dev/null -w '%{http_code} %{redirect_url}' "http://app.pingenv.test:$HTTP/x?y=1")" \
    "308 https://app.pingenv.test:$HTTPS/x?y=1"
check "unknown host is not redirected" \
    "$(get -o /dev/null -w '%{http_code}' "http://other.test:$HTTP/")" 404
check "unknown acme challenge is a 404, not a redirect" \
    "$(get -o /dev/null -w '%{http_code}' "http://app.pingenv.test:$HTTP/.well-known/acme-challenge/nope")" 404
check "unknown SNI fails the handshake" \
    "$(get -k -o /dev/null -w '%{http_code}' "https://other.test:$HTTPS/" 2>/dev/null || true)" 000

BODY="$(get -k -H 'X-Forwarded-For: 6.6.6.6' "https://app.pingenv.test:$HTTPS/hello?q=1")"
check "routes to upstream A" "$(field 'd["upstream"]' <<<"$BODY")" A
check "path and query are kept" "$(field 'd["path"]' <<<"$BODY")" "/hello?q=1"
check "client Host is kept" "$(field 'd["headers"]["host"]' <<<"$BODY")" "app.pingenv.test:$HTTPS"
check "X-Forwarded-Proto" "$(field 'd["headers"]["x-forwarded-proto"]' <<<"$BODY")" https
check "spoofed X-Forwarded-For is replaced" "$(field 'd["headers"]["x-forwarded-for"]' <<<"$BODY")" 127.0.0.1
check "X-Forwarded-Host" "$(field 'd["headers"]["x-forwarded-host"]' <<<"$BODY")" app.pingenv.test

check "negotiates HTTP/2" \
    "$(get -k --http2 -o /dev/null -w '%{http_version}' "https://app.pingenv.test:$HTTPS/")" 2
BODY="$(get -k --http2 "https://app.pingenv.test:$HTTPS/h2")"
check "HTTP/2 request reaches upstream with a Host" \
    "$(field 'd["headers"]["host"]' <<<"$BODY")" "app.pingenv.test:$HTTPS"

BODY="$(get -k "https://app.pingenv.test:$HTTPS/api/users?id=7")"
check "longest path prefix wins" "$(field 'd["upstream"]' <<<"$BODY")" B
check "prefix is stripped" "$(field 'd["path"]' <<<"$BODY")" "/users?id=7"
check "SET_HOST overrides Host" "$(field 'd["headers"]["host"]' <<<"$BODY")" backend.internal
check "/apix does not match /api" \
    "$(get -k "https://app.pingenv.test:$HTTPS/apix" | field 'd["upstream"]')" A

SEEN=""
for _ in 1 2 3 4 5 6; do
    SEEN+="$(get -k "https://lb.pingenv.test:$HTTPS/" | field 'd["upstream"]')"
done
check "round robin skips the dead upstream" "$(grep -o . <<<"$SEEN" | sort -u | tr -d '\n') ${#SEEN}" "AB 6"
check "wildcard host" \
    "$(get -k "https://sub.wild.pingenv.test:$HTTPS/" | field 'd["upstream"]')" A
check "access log has one line per request" \
    "$(grep -c 'route=api upstream=localhost:18002' "$WORK/selfsigned.log")" 1

echo "== static files"
S="https://site.pingenv.test:$HTTPS"
code() { get -k -o /dev/null -w '%{http_code}' "$@"; }
hdr() { get -k -o /dev/null -D - "${@:2}" | tr -d '\r' | awk -F': ' -v h="$1" 'tolower($1)==tolower(h){print $2}'; }
src() { sleep 0.2; grep "$1 " "$WORK/selfsigned.log" | tail -1 | grep -o 'upstream=[a-z-]*'; }

check "serves the index for /" "$(get -k "$S/")" '<h1>home</h1>'
check "content type" "$(hdr content-type "$S/css/site.css")" 'text/css; charset=utf-8'
check "small file comes from memory" "$(src 'site.pingenv.test/css/site.css')" upstream=memory
check "default Cache-Control" "$(hdr cache-control "$S/css/site.css")" no-cache
check "large file is intact" "$(get -k "$S/big.bin" | sha256sum)" "$BIG_SUM"
check "large file is streamed from disk" "$(src 'site.pingenv.test/big.bin')" upstream=disk
check "large file over HTTP/2 is intact" "$(get -k --http2 "$S/big.bin" | sha256sum)" "$BIG_SUM"
check "HEAD has a length and no body" \
    "$(get -k -I "$S/big.bin" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-length"{print $2}') $(get -k -I -o /dev/null -w '%{size_download}' "$S/big.bin")" "300000 0"

check "range on a disk file" \
    "$(get -k -r 100000-100099 "$S/big.bin" | sha256sum)" "$(tail -c +100001 "$SITE/big.bin" | head -c 100 | sha256sum)"
check "range status and Content-Range" \
    "$(code -r 100000-100099 "$S/big.bin") $(hdr content-range -r 100000-100099 "$S/big.bin")" "206 bytes 100000-100099/300000"
check "suffix range on a memory file" "$(get -k -r -3 "$S/css/site.css")" "{}"
check "range past the end is 416" "$(code -r 999999- "$S/big.bin")" 416

ETAG="$(hdr etag "$S/css/site.css")"
check "If-None-Match gives 304" "$(code -H "If-None-Match: $ETAG" "$S/css/site.css")" 304
check "If-None-Match with another tag gives 200" "$(code -H 'If-None-Match: "nope"' "$S/css/site.css")" 200
check "If-Modified-Since gives 304" \
    "$(code -H "If-Modified-Since: $(hdr last-modified "$S/big.bin")" "$S/big.bin")" 304
check "If-Range with a stale tag sends the whole file" \
    "$(code -r 0-9 -H 'If-Range: "stale"' "$S/big.bin")" 200

check "directory without slash redirects" \
    "$(get -k -o /dev/null -w '%{http_code} %{redirect_url}' "$S/docs?x=1")" "308 $S/docs/?x=1"
check "directory index" "$(get -k "$S/docs/")" '<h1>docs</h1>'
check "directory without index is not listed" "$(code "https://live.pingenv.test:$HTTPS/empty/")" 404
check "dotfile is refused" "$(code "$S/.env")" 404
check "dot directory is refused" "$(code "$S/.git/config")" 404
check ".well-known is served" "$(code "$S/.well-known/security.txt")" 200
check "traversal is refused" "$(code --path-as-is "$S/css/../../outside.txt")" 404
check "encoded traversal is refused" "$(code --path-as-is "$S/%2e%2e/outside.txt")" 404
check "encoded slash traversal is refused" "$(code --path-as-is "$S/css%2f..%2f..%2foutside.txt")" 404
check "symlink out of the root is refused" "$(code "$S/escape.txt")" 404
check "symlink inside the root is served" "$(get -k "$S/inside.css")" 'body{}'
check "fallback for application routes" "$(get -k "$S/users/42")" '<h1>home</h1>'
check "no fallback for missing assets" "$(code "$S/missing.js")" 404
check "POST is refused" "$(code -X POST "$S/")" 405

A="https://app.pingenv.test:$HTTPS/static"
check "files under a path prefix" "$(get -k "$A/css/site.css")" 'body{}'
check "prefix root serves the index" "$(get -k "$A/")" '<h1>home</h1>'
check "prefix without slash redirects" \
    "$(get -k -o /dev/null -w '%{http_code} %{redirect_url}' "$A")" "308 $A/"
check "custom Cache-Control" "$(hdr cache-control "$A/css/site.css")" 'public, max-age=60'
check "no fallback unless configured" "$(code "$A/users/42")" 404
check "other paths still reach the upstream" \
    "$(get -k "https://app.pingenv.test:$HTTPS/statics" | field 'd["upstream"]')" A

L="https://live.pingenv.test:$HTTPS"
check "limit 0 serves small files from disk" "$(get -k "$L/css/site.css") $(src 'live.pingenv.test/css/site.css')" 'body{} upstream=disk'
echo 'body{color:red}' >"$SITE/css/site.css"
echo 'new' >"$SITE/new.txt"
check "memory copy is a startup snapshot" "$(get -k "$S/css/site.css")" 'body{}'
check "disk route sees the change" "$(get -k "$L/css/site.css")" 'body{color:red}'
check "files added later are served from disk" "$(get -k "$S/new.txt") $(src 'site.pingenv.test/new.txt')" 'new upstream=disk'

echo "== split layout: one pingenv as the edge, another as the file server"
env -i PINGENV_HTTP_ADDR=127.0.0.1:18090 \
    PINGENV_ROUTE_FILES_HOST=split.pingenv.test PINGENV_ROUTE_FILES_ROOT="$SITE" \
    "$BIN" >"$WORK/inner.log" 2>&1 &
INNER=$!; PIDS+=($INNER)
wait_for_port 18090
P="https://split.pingenv.test:$HTTPS"
check "file served through both" "$(get -k "$P/docs/")" '<h1>docs</h1>'
check "edge proxied it, file server answered from memory" \
    "$(src 'split.pingenv.test/docs/') $(sleep 0.2; grep -c 'split.pingenv.test/docs/ 200 route=files upstream=memory' "$WORK/inner.log")" \
    "upstream= 1"
check "large file through both is intact" "$(get -k "$P/big.bin" | sha256sum)" "$BIG_SUM"
check "range through both" "$(code -r 0-99 "$P/big.bin") $(hdr content-range -r 0-99 "$P/big.bin")" "206 bytes 0-99/300000"
check "304 through both" "$(code -H "If-None-Match: $(hdr etag "$P/big.bin")" "$P/big.bin")" 304
kill -TERM $INNER
wait_for_exit $INNER
check "file server down gives 502 on its route" "$(code "$P/docs/")" 502
check "other routes are unaffected" "$(get -k "https://app.pingenv.test:$HTTPS/" | field 'd["upstream"]')" A

kill -TERM $GATE
wait_for_exit $GATE
check "exits on SIGTERM within 10s" "$(kill -0 $GATE 2>/dev/null && echo running || echo stopped)" stopped

set +e
OUT="$(env -i PINGENV_ROUTE_S_HOST=s.test PINGENV_ROUTE_S_ROOT="$SITE" PINGENV_MEMORY_MAX_TOTAL=10 \
    PINGENV_HTTP_ADDR=127.0.0.1:$HTTP "$BIN" 2>&1)"; CODE=$?
set -e
check "exceeding the memory cap fails at startup" \
    "$CODE $(grep -c 'exceed PINGENV_MEMORY_MAX_TOTAL' <<<"$OUT")" "1 1"

if [[ -n "$PEBBLE_DIR" ]]; then
    echo "== acme mode (Pebble)"
    MINICA="$PEBBLE_DIR/test/certs/pebble.minica.pem"
    # Mock DNS: every name resolves to 127.0.0.1, so Pebble validates us.
    "$PEBBLE_DIR/challtestsrv-bin" -defaultIPv4 127.0.0.1 -defaultIPv6 "" \
        -http01 "" -https01 "" -tlsalpn01 "" -doh "" >"$WORK/dns.log" 2>&1 &
    PIDS+=($!)
    (cd "$PEBBLE_DIR" && PEBBLE_VA_NOSLEEP=1 PEBBLE_WFE_NONCEREJECT=0 \
        exec ./pebble-bin -config test/config/pebble-config.json -dnsserver 127.0.0.1:8053 \
        >"$WORK/pebble.log" 2>&1) &
    PIDS+=($!)
    wait_for_port 14000; wait_for_port 15000

    start_acme() {
        # Pebble validates on port 5002 (httpPort in its config).
        env -i PINGENV_HTTP_ADDR=127.0.0.1:5002 PINGENV_HTTPS_ADDR=127.0.0.1:$HTTPS PINGENV_HTTPS_REDIRECT=false \
            PINGENV_TLS_MODE=acme PINGENV_ACME_DIRECTORY_URL=https://localhost:14000/dir \
            PINGENV_ACME_CA_FILE="$MINICA" PINGENV_ACME_EMAIL=ops@pingenv.test PINGENV_DATA_DIR="$WORK/data" \
            PINGENV_ROUTE_APP_HOST=app.pingenv.test PINGENV_ROUTE_APP_UPSTREAM=127.0.0.1:18001 \
            "$BIN" >"$1" 2>&1 &
        GATE=$!; PIDS+=($GATE)
        wait_for_port $HTTPS
    }
    start_acme "$WORK/acme1.log"
    check "answers HTTPS while the certificate is still pending" \
        "$(get -k -o /dev/null -w '%{http_code}' "https://app.pingenv.test:$HTTPS/")" 200

    CERT="$WORK/data/acme/localhost_14000/certs/app.pingenv.test/fullchain.pem"
    for _ in $(seq 300); do grep -q 'certificate issued' "$WORK/acme1.log" && break; sleep 0.1; done
    check "certificate is issued" "$(grep -c 'app.pingenv.test: certificate issued' "$WORK/acme1.log")" 1
    check "certificate is stored" "$([[ -s "$CERT" ]] && echo yes || echo no)" yes
    check "private key is owner-only" "$(stat -c %a "$(dirname "$CERT")/privkey.pem")" 600

    curl -sS --cacert "$MINICA" https://localhost:15000/roots/0 >"$WORK/root.pem"
    # No -k: this only passes if the issued certificate was swapped in live.
    check "issued certificate is served and verifies against the CA root" \
        "$(get --cacert "$WORK/root.pem" -o /dev/null -w '%{http_code}' "https://app.pingenv.test:$HTTPS/")" 200

    kill -TERM $GATE
    wait_for_exit $GATE
    start_acme "$WORK/acme2.log"
    check "restart reuses the stored certificate" \
        "$(get --cacert "$WORK/root.pem" -o /dev/null -w '%{http_code}' "https://app.pingenv.test:$HTTPS/")" 200
    sleep 2
    check "restart does not request a new certificate" \
        "$(grep -c 'requesting certificate' "$WORK/acme2.log" || true)" 0
fi

echo
echo "$PASS passed, $FAIL failed"
if [[ $FAIL -gt 0 ]]; then
    for log in "$WORK"/*.log; do echo "--- $log"; tail -20 "$log"; done
    exit 1
fi
