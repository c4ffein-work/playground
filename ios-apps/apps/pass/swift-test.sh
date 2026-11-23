#!/bin/sh
# Runs the tests of the pass Swift package for real: Swift -> UniFFI -> Rust -> a drop zone
# (dropzone/entrypoint.sh: OpenSSH), with another device that is pass.py: a store made and published by it
# (`pass sync`). Then has pass.py look at what the Swift side published, and take it.
#   PASS_PY   path to a pass.py that has `sync` (default: ../clones/pass/pass.py)
# Needs root and an account named `drop` (what the drop zone needs), sshd, sftp, gpg, git, python3.
# All of those run here, on the host; only `swift test` runs in Docker when there is no swift.
# What the tests read (the phone's keys) is under target/, which the container sees at the
# same path; the drop zones are reached over 127.0.0.1.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
package="$root/apps/pass/Package"
: "${PASS_PY:=$root/../clones/pass/pass.py}"
[ -f "$PASS_PY" ] || { echo "no pass.py at $PASS_PY: set PASS_PY" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "swift-test.sh needs root: the drop zone's sshd does" >&2; exit 1; }

# A world of its own: nothing of this machine's gpg or git setup, and a short path for gpg-agent's socket.
work=$(mktemp -d)
# Not under /tmp: sshd refuses to chroot below a directory others can write.
zones=$(mktemp -d /var/lib/pass-swift-e2e.XXXXXX)
chmod 755 "$zones"
fixtures="$root/target/pass-swift-e2e"
rm -rf "$fixtures"
mkdir -p "$fixtures" "$work/gnupg" "$work/home"
chmod 700 "$work/gnupg"
export GNUPGHOME="$work/gnupg" HOME="$work/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$work/gitconfig"
export PASSWORD_STORE_DIR="$work/computer"
printf '[user]\n\tname = Computer\n\temail = computer@example.org\n[init]\n\tdefaultBranch = main\n' >"$GIT_CONFIG_GLOBAL"
passphrase=correct-horse-battery-staple

servers=
cleanup() {
    for server in $servers; do kill "$server" 2>/dev/null || true; done
    gpgconf --kill all 2>/dev/null || true
    rm -rf "$work" "$zones" "$fixtures"
}
trap cleanup EXIT
fail() { echo "swift-test.sh: $1" >&2; exit 1; }

# Two devices. The computer's OpenPGP key has no passphrase (pass.py asks gpg, gpg would ask a
# terminal); the phone's has one, and is exported for the phone to import. An SSH key each.
gpg --batch --quiet --passphrase '' --quick-generate-key 'pc <pc@example.org>' ed25519 sign never
pc=$(gpg --batch --with-colons --list-keys pc@example.org | awk -F: '$1 == "fpr" { print $10; exit }')
gpg --batch --quiet --passphrase '' --quick-add-key "$pc" cv25519 encr never
gpg --batch --quiet --pinentry-mode loopback --passphrase "$passphrase" \
    --quick-generate-key 'phone <phone@example.org>' ed25519 sign never
phone=$(gpg --batch --with-colons --list-keys phone@example.org | awk -F: '$1 == "fpr" { print $10; exit }')
gpg --batch --quiet --pinentry-mode loopback --passphrase "$passphrase" --quick-add-key "$phone" cv25519 encr never
gpg --batch --quiet --pinentry-mode loopback --passphrase "$passphrase" --armor \
    --export-secret-keys "$phone" >"$fixtures/phone-secret.asc"
gpg --batch --armor --export "$pc" >"$fixtures/pc-public.asc"
ssh-keygen -q -t ed25519 -N '' -C pc -f "$work/pc.ssh"
ssh-keygen -q -t ed25519 -N '' -C phone -f "$fixtures/phone.ssh"

# Two drop zones: one will hold the computer's store as it was, the other as it is two changes later.
start_zone() { # start_zone <name>: sets port_<name> and hostkey_<name>
    port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
    DROPZONE_DATA="$zones/$1" DROPZONE_PORT="$port" DROPZONE_KEY_PC="$(cat "$work/pc.ssh.pub")" \
    DROPZONE_KEY_PHONE="$(cat "$fixtures/phone.ssh.pub")" \
        sh "$root/apps/pass/dropzone/entrypoint.sh" >"$work/$1.log" 2>&1 &
    servers="$servers $!"
    for _ in $(seq 100); do
        python3 -c "import socket; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null && break
        sleep 0.1
    done
    hostkey=$(sed -n 's/^dropzone: host key //p' "$work/$1.log")
    [ -n "$hostkey" ] || { cat "$work/$1.log" >&2; fail "the drop zone $1 did not start"; }
    ssh-keyscan -p "$port" 127.0.0.1 2>/dev/null >>"$work/known_hosts"
    eval "port_$1=$port hostkey_$1=$hostkey"
}
start_zone earlier
start_zone later

# The computer: a store made and managed by pass.py. One folder is the computer's alone.
pass() { python3 "$PASS_PY" "$@"; }
pass init "$pc" "$phone" >/dev/null
pass git init
printf 'hunter2\nlogin: alice\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n' | pass insert -m bank >/dev/null
printf 'home-network\n' | pass insert -m wifi >/dev/null
printf 's3cret\n' | pass insert -m Email/work >/dev/null
pass init -p private "$pc" >/dev/null
printf 'not for the phone\n' | pass insert -m private/diary >/dev/null
computer="$PASSWORD_STORE_DIR"
pass sync setup pc "sftp://drop@127.0.0.1:$port_earlier" "$pc" >/dev/null
git -C "$computer" config pass-sync.sftp-options \
    "-i $work/pc.ssh -o UserKnownHostsFile=$work/known_hosts -o StrictHostKeyChecking=yes -o IdentitiesOnly=yes -o BatchMode=yes"
pass sync peer phone "$phone" >/dev/null
pass sync push >/dev/null || fail "pass sync push to the first drop zone"
printf 'pc-version\n' | pass insert -m -f bank >/dev/null
printf 'from-pc\n' | pass insert -m later >/dev/null
git -C "$computer" config pass-sync.url "sftp://drop@127.0.0.1:$port_later"
pass sync push >/dev/null || fail "pass sync push to the second drop zone"

export PASS_E2E_FIXTURES="$fixtures" PASS_E2E_PASSPHRASE="$passphrase"
export PASS_E2E_EARLIER_URL="sftp://drop@127.0.0.1:$port_earlier" PASS_E2E_EARLIER_HOST_KEY="$hostkey_earlier"
export PASS_E2E_LATER_URL="sftp://drop@127.0.0.1:$port_later" PASS_E2E_LATER_HOST_KEY="$hostkey_later"
(cd "$package" && "$root/scripts/swift.sh" test)

# What the Swift tests published in the second drop zone, as the other device deals with it:
# with pass.py's own `pass sync`, which refuses anything SYNC.md does not allow.
fetched=$(pass sync fetch) || fail "pass sync fetch"
case $fetched in
    *"phone: 4 commit(s) to take"*) ;;
    *) fail "pass sync fetch does not see the phone's three commits and its merge: $fetched" ;;
esac
log=$(pass sync log phone)
case $log in
    *"signed by phone"*"Add given password for swift/added to store."*) ;;
    *) fail "pass sync log does not show the phone's signed commits: $log" ;;
esac
pass sync merge phone >/dev/null || fail "pass sync merge refused what the phone published"
[ "$(pass show swift/added | head -n 1)" = from-swift ] || fail "pass does not show swift/added after the merge"
[ "$(pass show bank | head -n 1)" = phone-version ] || fail "the version chosen on the phone is not the one merged"
[ "$(pass show later | head -n 1)" = from-pc ] || fail "the computer's own change did not survive the merge"
[ "$(pass show home/wifi | head -n 1)" = home-network ] || fail "the rename made on the phone is not there"
echo "swift-test.sh: pass.py took what the Swift tests published: signed, merged, and gpg and pass read it"
