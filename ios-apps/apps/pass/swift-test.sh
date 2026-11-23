#!/bin/sh
# Runs the tests of the pass Swift package against a real store and a real relay:
# Swift -> UniFFI -> Rust -> `git http-backend`, on a store made here with gpg and pass.py.
# Then checks, with git and gpg, what the Swift side pushed to its branch of the relay.
#   PASS_PY   path to pass.py (default: ../clones/pass/pass.py)
# gpg, git and python3 run here, on the host; only `swift test` runs in Docker when there
# is no swift. What the tests read (the key to import) is under target/, which the
# container sees at the same path; the relay is reached over 127.0.0.1.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
package="$root/apps/pass/Package"
: "${PASS_PY:=$root/../clones/pass/pass.py}"
[ -f "$PASS_PY" ] || { echo "no pass.py at $PASS_PY: set PASS_PY" >&2; exit 1; }

# A world of its own: nothing of this machine's gpg or git setup, and a short path for gpg-agent's socket.
work=$(mktemp -d)
fixtures="$root/target/pass-swift-e2e"
rm -rf "$fixtures"
mkdir -p "$fixtures" "$work/gnupg" "$work/home" "$work/relay"
chmod 700 "$work/gnupg"
export GNUPGHOME="$work/gnupg" HOME="$work/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$work/gitconfig"
export PASSWORD_STORE_DIR="$work/laptop"
passphrase=correct-horse-battery-staple
export PASSWORD_STORE_GPG_OPTS="--pinentry-mode loopback --passphrase $passphrase"
printf '[user]\n\tname = Laptop\n\temail = laptop@example.org\n[init]\n\tdefaultBranch = main\n' >"$GIT_CONFIG_GLOBAL"
gpg="gpg --batch --quiet --pinentry-mode loopback --passphrase $passphrase"

server=
cleanup() {
    [ -z "$server" ] || kill "$server" 2>/dev/null || true
    gpgconf --kill all 2>/dev/null || true
    rm -rf "$work" "$fixtures"
}
trap cleanup EXIT

# The owner's key, as people have them: a primary key that signs, a subkey that decrypts, a passphrase.
$gpg --gen-key <<EOF
Key-Type: EDDSA
Key-Curve: ed25519
Key-Usage: sign,cert
Subkey-Type: ECDH
Subkey-Curve: cv25519
Subkey-Usage: encrypt
Name-Real: Swift E2E
Name-Email: swift-e2e@example.org
Expire-Date: 0
Passphrase: $passphrase
%commit
EOF
fingerprint=$($gpg --with-colons --list-keys swift-e2e@example.org | sed -n 's/^fpr:*\([0-9A-F]*\):*$/\1/p' | head -n 1)
$gpg --armor --export-secret-keys "$fingerprint" >"$fixtures/secret.asc"
printf '%s\n' "$fingerprint" >"$fixtures/fingerprint"

# The laptop's store, and the relay at two moments: `older.git` is `store.git` two changes ago.
pass() { python3 "$PASS_PY" "$@"; }
pass init "$fingerprint" >/dev/null
pass git init
printf 'hunter2\nlogin: alice\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n' | pass insert -m bank >/dev/null
laptop="$PASSWORD_STORE_DIR"
git init --quiet --bare --initial-branch=main "$work/relay/store.git"
git -C "$laptop" remote add relay "$work/relay/store.git"
git -C "$laptop" push --quiet relay main
git clone --quiet --bare "$work/relay/store.git" "$work/relay/older.git"
printf 's3cret\n' | pass insert -m Email/work >/dev/null
printf 'from-laptop\n' | pass insert -m wifi >/dev/null
git -C "$laptop" push --quiet relay main

port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 "$root/crates/pass-core/tests/git_http_server.py" --root "$work/relay" --port "$port" >"$work/server.log" 2>&1 &
server=$!
python3 - "$port" <<'PY' || { cat "$work/server.log" >&2; exit 1; }
import socket, sys, time
for _ in range(100):
    try:
        socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1).close()
        sys.exit(0)
    except OSError:
        time.sleep(0.1)
sys.exit("the git http server did not start")
PY

export PASS_E2E_FIXTURES="$fixtures"
export PASS_E2E_URL="http://127.0.0.1:$port/store.git"
export PASS_E2E_OLDER_URL="http://127.0.0.1:$port/older.git"
export PASS_E2E_PASSPHRASE="$passphrase"
(cd "$package" && "$root/scripts/swift.sh" test)

# What the Swift tests left on the relay, as the owner would look at it from the laptop.
fail() { echo "swift-test.sh: $1" >&2; exit 1; }
git -C "$laptop" fetch --quiet relay
branch=relay/devices/swift
subjects=$(git -C "$laptop" log --format=%s "main..$branch")
expected='Remove Email/work from store.
Rename wifi to home/wifi.
Edit password for bank using pass-ios.
Add generated password for swift/generated.
Add given password for swift/added to store.'
[ "$subjects" = "$expected" ] || fail "unexpected commits on $branch:
$subjects"
[ "$(git -C "$laptop" log --format='%G? %GF' "main..$branch" | sort -u)" = "G $fingerprint" ] \
    || fail "the commits on $branch are not all signed by the store's key"
content=$(git -C "$laptop" show "$branch:swift/added.gpg" | $gpg --decrypt)
[ "$content" = "from-swift
login: swift" ] || fail "gpg does not read swift/added as it was written: $content"
git -C "$laptop" merge --quiet --verify-signatures --no-edit "$branch"
[ "$(pass show swift/added | head -n 1)" = from-swift ] || fail "pass does not show swift/added after the merge"
[ "$(git -C "$laptop" rev-parse relay/devices/diverged)" = "$(git -C "$laptop" rev-parse relay/main)" ] \
    || fail "the discarded device branch was not moved back to main"
echo "swift-test.sh: the relay holds what the Swift tests pushed, signed, and gpg and pass read it"
