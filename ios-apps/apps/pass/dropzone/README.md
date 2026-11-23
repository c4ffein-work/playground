# pass drop zone

The server side of [pass sync](../SYNC.md): an OpenSSH server that does SFTP into one directory and
nothing else. No git, no gpg, no code of ours beyond a start script: it holds one encrypted file per
device and cannot read any of them.

## Run it

```sh
docker build -t pass-dropzone .
docker run -d --name pass-dropzone --restart unless-stopped \
    -p 2222:2222 -v pass-dropzone:/data \
    -e DROPZONE_KEY_PC='ssh-ed25519 AAAA... pc' \
    -e DROPZONE_KEY_PHONE='ssh-ed25519 AAAA... phone' \
    pass-dropzone
docker logs pass-dropzone      # prints the host key fingerprint the devices must pin
```

Devices then use `sftp://drop@your-server:2222`.

| Variable | |
| :-- | :-- |
| `DROPZONE_KEY_<NAME>` | One authorized public key per variable. The name is for you; any number of them |
| `DROPZONE_AUTHORIZED_KEYS` | More keys, one per line (optional) |
| `DROPZONE_HOST_KEY` | The server's private key, OpenSSH format (optional). Without it one is generated on the first start and kept in `/data`, so the fingerprint is stable as long as the volume is |
| `DROPZONE_PORT` | Port to listen on, default `2222` |
| `DROPZONE_DATA` | Data directory, default `/data` |
| `DROPZONE_USER` | Account name, default `drop` |

A key is a key: a line with options in front (`command=`, `from=`) is refused. To remove a device,
remove its variable and restart.

## What a device can do

Log in with its key, and read, write, rename and remove files in the one directory. Not: get a
shell, run a command, forward a port, see anything of the server outside that directory, log in
with a password, or log in as another account.

Every device can overwrite or remove any file, another device's included: there is one account.
That costs a sync, nothing else: a file is only accepted by its reader when it is signed by the
device it is named after (see the protocol).

There is no quota: a device can fill the volume. Give it a small one.

## Tested

`./test.sh` runs `entrypoint.sh` against the real `sshd`, `sftp` and `ssh` (as root, no Docker):
publishing by upload-then-rename, the confinement to the directory, the refusals listed above,
the configuration errors, and the host key across restarts.

**The image itself has not been built**: where this was written, a Docker build has no network to
fetch `openssh-server`. The Dockerfile is five lines around the tested script; build it once and
run `docker logs` to check it starts.
