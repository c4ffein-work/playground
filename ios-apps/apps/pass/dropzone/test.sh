#!/bin/sh
# Tests entrypoint.sh against the real OpenSSH, with no Docker: starts it on a free port and talks
# to it with sftp and ssh. Needs root (as the entrypoint does), sshd, sftp, and an account named
# by DROPZONE_USER (default drop):  useradd -M -s /usr/sbin/nologin drop && usermod -p '*' drop
set -eu
here=$(cd "$(dirname "$0")" && pwd)
[ "$(id -u)" = 0 ] || { echo "test.sh needs root" >&2; exit 1; }

# Not under /tmp: sshd refuses to chroot below a directory others can write.
work=$(mktemp -d /var/lib/dropzone-test.XXXXXX)
chmod 755 "$work"
client=$(mktemp -d)
server=
cleanup() {
    [ -z "$server" ] || kill "$server" 2>/dev/null || true
    rm -rf "$work" "$client"
}
trap cleanup EXIT

fail() {
    echo "FAILED: $*" >&2
    [ ! -f "$client/server.log" ] || sed 's/^/    server: /' "$client/server.log" >&2
    exit 1
}

for name in pc phone stranger; do
    ssh-keygen -q -t ed25519 -N '' -C "$name" -f "$client/$name"
done
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')

# What must not start.
if DROPZONE_DATA="$work/none" DROPZONE_PORT="$port" "$here/entrypoint.sh" >"$client/out" 2>&1; then
    fail "started with no key"
fi
grep -q "no key" "$client/out" || fail "no key: wrong message: $(cat "$client/out")"
if DROPZONE_DATA="$work/bad" DROPZONE_PORT="$port" DROPZONE_KEY_X='command="sh" ssh-ed25519 AAAA' \
    "$here/entrypoint.sh" >"$client/out" 2>&1; then
    fail "started with options in front of a key"
fi
grep -q "not a public key" "$client/out" || fail "key with options: wrong message: $(cat "$client/out")"
if DROPZONE_DATA="$work/bad" DROPZONE_PORT="$port" DROPZONE_KEY_X='ssh-ed25519 not-a-key' \
    "$here/entrypoint.sh" >"$client/out" 2>&1; then
    fail "started with a broken key"
fi

# One key by its own variable, one in the list.
DROPZONE_DATA="$work/data" DROPZONE_PORT="$port" \
DROPZONE_KEY_PC="$(cat "$client/pc.pub")" DROPZONE_AUTHORIZED_KEYS="
$(cat "$client/phone.pub")
" "$here/entrypoint.sh" >"$client/server.log" 2>&1 &
server=$!
for _ in $(seq 50); do
    python3 -c "import socket, sys; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null && break
    kill -0 "$server" 2>/dev/null || fail "the server stopped"
    sleep 0.1
done
grep -q "host key SHA256:" "$client/server.log" || fail "the host key fingerprint is not printed"

# The fingerprint it printed is the one it serves.
ssh-keyscan -p "$port" 127.0.0.1 2>/dev/null >"$client/known_hosts"
served=$(ssh-keygen -l -f "$client/known_hosts" | cut -d' ' -f2)
grep -q "host key $served" "$client/server.log" || fail "printed another fingerprint than $served"

options="-o UserKnownHostsFile=$client/known_hosts -o StrictHostKeyChecking=yes -o IdentitiesOnly=yes -o BatchMode=yes"
as() { # as <key> <sftp batch>
    key=$1
    shift
    printf '%s\n' "$@" | sftp -q -b - -P "$port" -i "$client/$key" $options drop@127.0.0.1
}
# What a batch prints, without the echo of its commands.
said() {
    as "$@" | grep -v '^sftp> ' | tr -d '\r'
}

# Publish as the protocol says: upload beside, rename over. Twice: the second replaces the first.
echo "first from pc" >"$client/one"
echo "second from pc" >"$client/two"
as pc "put $client/one pc.bundle.gpg.part" "rename pc.bundle.gpg.part pc.bundle.gpg" >/dev/null || fail "pc cannot publish"
as pc "put $client/two pc.bundle.gpg.part" "rename pc.bundle.gpg.part pc.bundle.gpg" >/dev/null || fail "pc cannot publish again"
as phone "get pc.bundle.gpg $client/got" >/dev/null || fail "phone cannot read what pc published"
cmp -s "$client/two" "$client/got" || fail "phone read something else than the last file"
listing=$(said phone "ls -1")
[ "$listing" = "pc.bundle.gpg" ] || fail "the directory holds: $listing"

# The session starts in the directory, and there is nothing above it but the directory.
[ "$(said phone "pwd")" = "Remote working directory: /drop" ] || fail "does not start in /drop"
[ "$(said phone "ls -1 /")" = "/drop" ] || fail "sees more than /drop: $(said phone 'ls -1 /')"
if as phone "put $client/one /escaped" >/dev/null 2>&1; then fail "wrote outside the directory"; fi
if as phone "get /etc/passwd $client/passwd" >/dev/null 2>&1; then fail "read the server's /etc/passwd"; fi
perms=$(stat -c '%a %U' "$work/data/jail/drop/pc.bundle.gpg")
[ "$perms" = "600 ${DROPZONE_USER:-drop}" ] || fail "the file is $perms on the server"

# Nothing but SFTP: a command gets no shell, a forward gets no port.
out=$(ssh -p "$port" -i "$client/pc" $options drop@127.0.0.1 'echo SHELL-$((40+2))' 2>&1 || true)
case $out in *SHELL-42*) fail "ran a command: $out" ;; esac
if ssh -p "$port" -i "$client/pc" $options -W 127.0.0.1:"$port" drop@127.0.0.1 </dev/null 2>/dev/null | grep -q SSH-; then
    fail "forwarded a connection"
fi

# A key that was not given, and no password at all.
if as stranger "ls" >/dev/null 2>&1; then fail "let an unknown key in"; fi
if sftp -q -b /dev/null -P "$port" -o UserKnownHostsFile="$client/known_hosts" -o PubkeyAuthentication=no \
    -o BatchMode=yes drop@127.0.0.1 >/dev/null 2>&1; then
    fail "let someone in with no key"
fi
if sftp -q -b /dev/null -P "$port" -i "$client/pc" $options root@127.0.0.1 >/dev/null 2>&1; then fail "let root in"; fi

# The host key survives a restart (it is in the data directory), unless one is given.
kill "$server"
wait "$server" 2>/dev/null || true
ssh-keygen -q -t ed25519 -N '' -C given -f "$client/given"
DROPZONE_DATA="$work/data" DROPZONE_PORT="$port" DROPZONE_KEY_PC="$(cat "$client/pc.pub")" \
    "$here/entrypoint.sh" >"$client/server.log" 2>&1 &
server=$!
for _ in $(seq 50); do
    python3 -c "import socket; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null && break
    sleep 0.1
done
grep -q "host key $served" "$client/server.log" || fail "the host key changed over a restart"
as pc "get pc.bundle.gpg $client/again" >/dev/null || fail "the file did not survive a restart"
if as phone "ls" >/dev/null 2>&1; then fail "a key that is no longer given still gets in"; fi
kill "$server"
wait "$server" 2>/dev/null || true

DROPZONE_DATA="$work/data" DROPZONE_PORT="$port" DROPZONE_KEY_PC="$(cat "$client/pc.pub")" \
DROPZONE_HOST_KEY="$(cat "$client/given")" "$here/entrypoint.sh" >"$client/server.log" 2>&1 &
server=$!
for _ in $(seq 50); do
    python3 -c "import socket; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null && break
    sleep 0.1
done
given=$(ssh-keygen -l -f "$client/given.pub" | cut -d' ' -f2)
grep -q "host key $given" "$client/server.log" || fail "DROPZONE_HOST_KEY is not the key served"

echo "dropzone: all checks passed"
