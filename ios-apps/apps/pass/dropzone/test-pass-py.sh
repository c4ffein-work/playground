#!/bin/sh
# The computer's side, for real: pass.py's `sync` against a drop zone started from entrypoint.sh,
# over sftp, with a phone played by sftp + gpg + git as SYNC.md describes it.
# Needs what test.sh needs, plus gpg, git, and PASS_PY: the path of a pass.py that has `sync`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
[ "$(id -u)" = 0 ] || { echo "test-pass-py.sh needs root" >&2; exit 1; }
[ -f "${PASS_PY:-}" ] || { echo "set PASS_PY to the path of pass.py" >&2; exit 1; }

work=$(mktemp -d /var/lib/dropzone-test.XXXXXX)
chmod 755 "$work"
tmp=$(mktemp -d)
server=
cleanup() {
    [ -z "$server" ] || kill "$server" 2>/dev/null || true
    gpgconf --homedir "$tmp/gnupg" --kill all 2>/dev/null || true
    rm -rf "$work" "$tmp"
}
trap cleanup EXIT
fail() {
    echo "FAILED: $*" >&2
    exit 1
}

# Nothing of this machine's own git or gpg setup takes part.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1

# Two devices: an OpenPGP key and an SSH key each.
export GNUPGHOME="$tmp/gnupg"
mkdir -m 700 "$GNUPGHOME"
for name in pc phone; do
    gpg --batch --quiet --passphrase '' --quick-generate-key "$name <$name@example.invalid>" ed25519 sign never
    fpr=$(gpg --batch --with-colons --list-keys "$name@example.invalid" | awk -F: '$1 == "fpr" { print $10; exit }')
    gpg --batch --quiet --passphrase '' --quick-add-key "$fpr" cv25519 encr never
    eval "${name}_key=$fpr"
    ssh-keygen -q -t ed25519 -N '' -C "$name" -f "$tmp/$name.ssh"
done

port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
DROPZONE_DATA="$work/data" DROPZONE_PORT="$port" DROPZONE_KEY_PC="$(cat "$tmp/pc.ssh.pub")" \
DROPZONE_KEY_PHONE="$(cat "$tmp/phone.ssh.pub")" "$here/entrypoint.sh" >"$tmp/server.log" 2>&1 &
server=$!
for _ in $(seq 50); do
    python3 -c "import socket; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null && break
    sleep 0.1
done
ssh-keyscan -p "$port" 127.0.0.1 2>/dev/null >"$tmp/known_hosts"
ssh_options="-o UserKnownHostsFile=$tmp/known_hosts -o StrictHostKeyChecking=yes -o IdentitiesOnly=yes -o BatchMode=yes"

# The computer: a store made and managed by pass.py.
export PASSWORD_STORE_DIR="$tmp/store"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
pass() {
    python3 "$PASS_PY" "$@"
}
mkdir "$PASSWORD_STORE_DIR"
pass init "$pc_key" >/dev/null
pass git init
printf 'from the computer\n' | pass insert -m -f first >/dev/null
pass sync setup pc "sftp://drop@127.0.0.1:$port" "$pc_key" >/dev/null
git -C "$PASSWORD_STORE_DIR" config pass-sync.sftp-options "-i $tmp/pc.ssh $ssh_options"
pass sync peer phone "$phone_key" >/dev/null
pass sync push >/dev/null || fail "pass sync push over sftp"

# The phone: takes the computer's file, adds an entry, publishes.
phone_sftp() {
    printf '%s\n' "$@" | sftp -q -b - -P "$port" -i "$tmp/phone.ssh" $ssh_options drop@127.0.0.1 >/dev/null
}
phone_sftp "get pc.bundle.gpg $tmp/pc.message" || fail "the phone cannot read what pass.py published"
gpg --batch --quiet --status-fd 3 --output "$tmp/pc.bundle" --decrypt "$tmp/pc.message" 3>"$tmp/status"
grep -q "VALIDSIG .* $pc_key\$" "$tmp/status" || fail "what pass.py published is not signed by the computer's key"
git init --quiet "$tmp/phone"
git -C "$tmp/phone" fetch --quiet "$tmp/pc.bundle" refs/pass-sync/pc:refs/pass-sync/pc
git -C "$tmp/phone" checkout --quiet -b work refs/pass-sync/pc
printf 'typed on the phone\n' | gpg --batch --quiet --trust-model always --encrypt --recipient "$pc_key" \
    --output "$tmp/phone/from-phone.gpg"
git -C "$tmp/phone" add from-phone.gpg
git -C "$tmp/phone" -c user.signingkey="$phone_key" commit --quiet -S -m "Add given password for from-phone to store."
git -C "$tmp/phone" update-ref refs/pass-sync/phone HEAD
git -C "$tmp/phone" bundle create --quiet --version=2 "$tmp/phone.bundle" refs/pass-sync/phone
gpg --batch --quiet --trust-model always --compress-algo none --local-user "$phone_key" --sign --encrypt \
    --recipient "$pc_key" --recipient "$phone_key" --output "$tmp/phone.message" "$tmp/phone.bundle"
phone_sftp "put $tmp/phone.message phone.bundle.gpg.part" "rename phone.bundle.gpg.part phone.bundle.gpg" ||
    fail "the phone cannot publish"

# The computer takes it, and what it then publishes is the merged state.
pass sync fetch | grep -q "phone: 1 commit(s) to take" || fail "pass sync fetch does not see the phone's commit"
pass sync log phone | grep -q "signed by phone" || fail "pass sync log does not name the signer"
pass sync merge phone >/dev/null || fail "pass sync merge"
[ "$(pass show from-phone)" = "typed on the phone" ] || fail "the entry from the phone does not decrypt on the computer"
phone_sftp "get pc.bundle.gpg $tmp/pc.message"
gpg --batch --yes --quiet --output "$tmp/pc.bundle" --decrypt "$tmp/pc.message"
[ "$(git bundle list-heads "$tmp/pc.bundle" | cut -d' ' -f1)" = "$(git -C "$tmp/phone" rev-parse HEAD)" ] ||
    fail "after the merge, the computer did not publish the phone's tip"

# A drop zone that is not there is an error, not a silence.
git -C "$PASSWORD_STORE_DIR" config pass-sync.url "sftp://drop@127.0.0.1:1"
if pass sync fetch >/dev/null 2>&1; then fail "fetch from a dead drop zone succeeded"; fi
if pass sync push >/dev/null 2>&1; then fail "push to a dead drop zone succeeded"; fi

echo "pass.py sync over sftp: all checks passed"
