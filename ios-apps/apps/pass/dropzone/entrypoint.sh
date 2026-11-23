#!/bin/sh
# The pass drop zone: an OpenSSH server that does SFTP into one directory, and nothing else.
# Configured by environment variables only:
#
#   DROPZONE_KEY_<NAME>      one authorized public key per variable (DROPZONE_KEY_PHONE=ssh-ed25519 AAAA...)
#   DROPZONE_AUTHORIZED_KEYS more of them, one per line (optional)
#   DROPZONE_HOST_KEY        the server's private key, in OpenSSH format (optional: without it, one is
#                            made on the first start and kept in the data directory)
#   DROPZONE_PORT            the port to listen on (default 2222)
#   DROPZONE_DATA            where the files and the generated host key live (default /data: mount a volume)
#   DROPZONE_USER            the account devices log in as (default drop; it must exist)
#
# Must start as root: sshd needs it to lock each session inside the directory (chroot).
set -eu

data=${DROPZONE_DATA:-/data}
port=${DROPZONE_PORT:-2222}
user=${DROPZONE_USER:-drop}
etc=$data/etc

fail() {
    echo "dropzone: $*" >&2
    exit 1
}

[ "$(id -u)" = 0 ] || fail "must start as root (sshd needs it to chroot)"
id "$user" >/dev/null 2>&1 || fail "no account named $user"
case $port in '' | *[!0-9]*) fail "DROPZONE_PORT is not a number: $port" ;; esac

# sshd refuses to chroot into a directory that anyone but root can write, at any level.
umask 077
mkdir -p "$data" "$etc" "$data/jail/drop" /run/sshd
chown root:root "$data" "$etc" "$data/jail"
chmod 755 "$data" "$data/jail"
chmod 700 "$etc"
chown "$user" "$data/jail/drop"
chmod 700 "$data/jail/drop"

# The keys: every DROPZONE_KEY_* variable, then the lines of DROPZONE_AUTHORIZED_KEYS.
keys=$etc/authorized_keys
candidates=$(mktemp)
trap 'rm -f "$candidates" "$candidates.one"' EXIT
env | sed -n 's/^DROPZONE_KEY_[A-Za-z0-9_]*=//p' >"$candidates"
printf '%s\n' "${DROPZONE_AUTHORIZED_KEYS:-}" >>"$candidates"
: >"$keys"
while IFS= read -r line; do
    [ -n "$line" ] || continue
    # A line is a key and nothing else: options (command=, from=...) are not for the environment to set.
    case $line in
        ssh-ed25519\ * | ssh-rsa\ * | ecdsa-sha2-* | sk-ssh-ed25519@openssh.com\ * | sk-ecdsa-sha2-*) ;;
        *) fail "not a public key: $(printf '%s' "$line" | cut -c1-40)" ;;
    esac
    printf '%s\n' "$line" >"$candidates.one"
    ssh-keygen -l -f "$candidates.one" >/dev/null 2>&1 || fail "not a valid public key: $(printf '%s' "$line" | cut -c1-40)"
    printf 'restrict %s\n' "$line" >>"$keys"
done <"$candidates"
[ -s "$keys" ] || fail "no key: set DROPZONE_KEY_<NAME> to a device's public key"
chmod 644 "$keys"
chmod 711 "$etc"

host_key=$etc/host_key
if [ -n "${DROPZONE_HOST_KEY:-}" ]; then
    printf '%s\n' "$DROPZONE_HOST_KEY" >"$host_key"
    chmod 600 "$host_key"
    ssh-keygen -y -f "$host_key" >/dev/null 2>&1 || fail "DROPZONE_HOST_KEY is not a private key in OpenSSH format"
elif [ ! -s "$host_key" ]; then
    ssh-keygen -q -t ed25519 -N '' -C dropzone -f "$host_key"
    rm -f "$host_key.pub"
fi
chmod 600 "$host_key"

cat >"$etc/sshd_config" <<CONFIG
Port $port
HostKey $host_key
PidFile none
AllowUsers $user
PermitRootLogin no
AuthorizedKeysFile $keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
MaxAuthTries 3
LoginGraceTime 20
MaxStartups 10:30:60
ClientAliveInterval 60
ClientAliveCountMax 3
# SFTP into the one directory, whatever the client asks for.
Subsystem sftp internal-sftp
ChrootDirectory $data/jail
ForceCommand internal-sftp -d /drop -u 0077
DisableForwarding yes
PermitTTY no
PermitTunnel no
PermitUserEnvironment no
PermitUserRC no
CONFIG
sshd=$(command -v sshd) || fail "no sshd"
"$sshd" -t -f "$etc/sshd_config" || fail "sshd refuses the configuration"

echo "dropzone: port $port, $(wc -l <"$keys" | tr -d ' ') key(s), files in $data/jail/drop"
echo "dropzone: host key $(ssh-keygen -l -f "$host_key" | cut -d' ' -f2)"
rm -f "$candidates" "$candidates.one"
# -D: stay in the foreground. -e: log to stderr, where a container's logs are read.
exec "$sshd" -D -e -f "$etc/sshd_config"
