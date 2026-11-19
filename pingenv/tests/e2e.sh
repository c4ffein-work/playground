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
for name in app.pingenv.test lb.pingenv.test sub.wild.pingenv.test other.test; do
    RESOLVE+=(--resolve "$name:$HTTP:127.0.0.1" --resolve "$name:$HTTPS:127.0.0.1")
done
get() { curl -sS --max-time 10 "${RESOLVE[@]}" "$@"; }

python3 tests/echo_upstream.py A 18001 & PIDS+=($!)
python3 tests/echo_upstream.py B 18002 & PIDS+=($!)
wait_for_port 18001; wait_for_port 18002

echo "== configuration errors"
set +e
OUT="$(env -i ROUTE_A_HOST=a.test ROUTE_B_UPSTREAM=b TLS_MODE=nope "$BIN" 2>&1)"; CODE=$?
set -e
check "invalid configuration exits with 2" "$CODE" 2
check "reports all problems" "$(grep -c '^  - ' <<<"$OUT")" 4

echo "== self-signed mode"
env -i \
    HTTP_ADDR=127.0.0.1:$HTTP HTTPS_ADDR=127.0.0.1:$HTTPS HTTPS_REDIRECT_PORT=$HTTPS \
    TLS_MODE=selfsigned \
    ROUTE_APP_HOST=app.pingenv.test ROUTE_APP_UPSTREAM=127.0.0.1:18001 \
    ROUTE_API_HOST=app.pingenv.test ROUTE_API_PATH=/api ROUTE_API_STRIP_PREFIX=true \
    ROUTE_API_UPSTREAM=localhost:18002 ROUTE_API_SET_HOST=backend.internal \
    ROUTE_LB_HOST=lb.pingenv.test ROUTE_LB_UPSTREAM=127.0.0.1:18001,127.0.0.1:18009,127.0.0.1:18002 \
    ROUTE_WILD_HOST='*.wild.pingenv.test' ROUTE_WILD_UPSTREAM=127.0.0.1:18001 \
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

kill -TERM $GATE
wait_for_exit $GATE
check "exits on SIGTERM within 10s" "$(kill -0 $GATE 2>/dev/null && echo running || echo stopped)" stopped

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
        env -i HTTP_ADDR=127.0.0.1:5002 HTTPS_ADDR=127.0.0.1:$HTTPS HTTPS_REDIRECT=false \
            TLS_MODE=acme ACME_DIRECTORY_URL=https://localhost:14000/dir \
            ACME_CA_FILE="$MINICA" ACME_EMAIL=ops@pingenv.test DATA_DIR="$WORK/data" \
            ROUTE_APP_HOST=app.pingenv.test ROUTE_APP_UPSTREAM=127.0.0.1:18001 \
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
