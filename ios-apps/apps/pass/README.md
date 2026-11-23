# Pass

A phone client for a [password-store](https://www.passwordstore.org): the layout of `pass`
(and of [pass.py](https://github.com/c4ffein/pass)), OpenPGP files in a git repository, synced through git.
There is no `gpg` and no `git` on iOS: both are libraries here, in Rust.

## Status

| Part | State |
| :-- | :-- |
| `crates/pass-core` (Rust) | Tested on Linux against the real `gpg` 2.4, `git`, `git-shell`, `git http-backend` and `pass.py`, both ways |
| `apps/pass/Package` (Swift, no UI: settings, secrets, app state) | Tested on Linux through the generated bindings, against a relay served by `git http-backend`; what it pushes is then checked with `git`, `gpg` and `pass.py`. The keychain code is Apple-only and has not been compiled |
| `apps/pass/App`, `project.yml` | **Written without a Mac: syntax-checked only, never compiled or run** |
| The Rust core for `aarch64-apple-ios` | **Never built.** Every dependency is pure Rust except `ring` (which companion already ships), but nobody has compiled this set for iOS yet: see "Not verified" |
| SSH against OpenSSH | **Not tested**: there is no `ssh`/`sshd` where this was written. The SSH tests run against a stand-in server (russh on both ends) that executes the real `git-shell` and the real `update` hook |

## How it syncs

The store has one owner and one place where git is *managed*: the computer. The phone never merges and
never writes to the main branch.

```
 computer (pass, gpg, git) ──push main──▶  relay  ◀──fetch main (fast-forward only)── phone
                           ◀─fetch devices/phone─  (bare repo)  ◀──push devices/phone────────
```

- **Relay**: a bare repository on a machine of yours, reached over SSH. It sees only what git sees
  (see the threat model). `https` with a token works too, for a hosting service.
- **The phone commits locally**, with the messages `pass` writes (`Add given password for x to store.`,
  `Edit password for x using pass-ios.`, `Rename a to b.`...), each commit signed with the OpenPGP key,
  and pushes them to **its own branch**, `devices/<name>`, fast-forward only.
- **The phone takes `main` by fast-forward only.** Once you have merged its branch on the computer, its
  commits are in `main` and it simply moves forward.
- If `main` moved while the phone has unmerged changes, the two have **diverged**: the phone says so,
  takes nothing, and keeps working and pushing to its branch. It resolves itself when you merge. The
  other way out is on the phone: *Discard this device's changes* (back to `main`, and its branch on the
  relay is moved back too; the one push that is not forward-only).
- **A change to any `.gpg-id` coming from the relay is never taken silently**: the sync stops and shows
  which recipients were added or removed, and whether the `.gpg-id.sig` and the last commit are signed
  by a key the phone knows. Encrypting needs no secret, so anyone who can write to the repository can
  add themselves as a recipient: this is the check that matters.

Merge with a real merge (or a fast-forward). After a rebase or a squash the phone's commits are not in
`main` and it stays diverged until its changes are discarded.

### Setting the relay up

On the relay machine, an account that can only do git:

```sh
sudo adduser --disabled-password --gecos '' pass-relay
sudo -u pass-relay git init --bare --initial-branch=main /home/pass-relay/store.git
sudo -u pass-relay install -m 755 apps/pass/relay/update /home/pass-relay/store.git/hooks/update
```

`/home/pass-relay/.ssh/authorized_keys`, one line per key, each forced into `git-shell`
(use its full path if `command -v git-shell` is not in the account's `PATH`):

```
command="exec git-shell -c \"$SSH_ORIGINAL_COMMAND\"",restrict ssh-ed25519 AAAA... laptop
command="PASS_DEVICE=phone exec git-shell -c \"$SSH_ORIGINAL_COMMAND\"",restrict ssh-ed25519 AAAA... pass on phone
```

The phone's line is shown by the app once it has made its key (Settings → This device's key), ready to
send to the computer. `PASS_DEVICE` is what the [`update` hook](relay/update) reads: a key that carries
it can move `refs/heads/devices/<that name>` and nothing else, and cannot delete it. A key without it
(yours) can push anything. `restrict` turns off forwarding, ptys and the rest.

On the computer:

```sh
cd ~/.password-store            # a store with `pass git init` done
git remote add relay pass-relay@relay.example:store.git
git push relay main
```

On the phone: import the key, enter `pass-relay@relay.example:store.git`, read the relay's host key
and compare it with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` run on the relay, clone.

### Reviewing the phone's changes on the computer

`pass git init` configured a `gpg` diff driver (`.gitattributes` + `diff.gpg.textconv`), so git shows
entries decrypted:

```sh
cd ~/.password-store
git fetch relay
git log --show-signature main..relay/devices/phone      # who signed what
git log -p main..relay/devices/phone                    # the changes, decrypted
git diff --stat main...relay/devices/phone -- '*.gpg-id' '*.gpg-id.sig'   # must print nothing
git merge --verify-signatures relay/devices/phone       # refuses an unsigned or badly signed commit
git push relay main
```

(Plain `git`, not `pass git`: in pass.py as it is today `pass git <anything but init>` does not find the
repository. See "Seen in pass.py" below.) The signatures are by your own OpenPGP key, the one the phone
holds, so `gpg` on the computer verifies them with nothing to import. To make the reverse check possible
(phone setting *Only take signed updates*), sign on the computer too: `git config commit.gpgsign true`.

## What the app does

- **Browse and search** the store as folders; each word typed narrows the list. Listing needs no key.
- **Read an entry**: the first line is the password, `name: value` lines are fields, `otpauth://` lines
  give time-based codes (RFC 6238). Tap to copy: the clipboard entry is local to the device (no Universal
  Clipboard) and expires (45 s by default).
- **Write**: add, edit, generate (new, or in place as `pass generate -i`), rename/move, delete. Entries are
  encrypted to the recipients of the governing `.gpg-id`, all of them or not at all; moving an entry under
  another `.gpg-id` re-encrypts it.
- **Unlock** with the key's passphrase, typed each time or kept in the keychain behind Face ID. Locks
  after 2 minutes without use, and at once when the app leaves the screen.
- **Sync** with one button (also while locked: it needs no key).

Not there: an AutoFill extension (below), directories in rename/delete, `pass init` (recipients are managed
on the computer), QR codes, a key generated on the phone.

## Research: which libraries, and why

Read in the crates' own sources (November 2026: `gix` 0.91, `pgp` 0.21.1, `russh` 0.65.4), not from memory.

### OpenPGP: rPGP (`pgp`), not Sequoia

- **rPGP** is pure Rust (RustCrypto). It decrypts what GnuPG writes for RSA and for ed25519/cv25519 keys,
  **including GnuPG's own OCB packet (type 20, "LibrePGP")**, which GnuPG 2.4 uses by default between
  keys it generated; rPGP has it behind `DecryptionOptions::enable_gnupg_aead()`. Tested here with
  `gpg --force-ocb`, compressed, armored, and `pass`'s own `--compress-algo=none`.
- It writes PKESK v3 + SEIPD v1 (AES-256), uncompressed: what every `gpg` reads. Tested: `gpg -d` and
  `pass.py show` read what the phone wrote, to one and to several recipients.
- **Sequoia rejected** for two reasons: it does not implement packet 20 with OCB (its status page lists OCB
  as unsupported), so a store written by a recent GnuPG may simply not open; and its mature crypto backends
  are C (Nettle, OpenSSL), the pure-Rust one being the least used.
- Built without `bzip2`: an entry compressed with `--compress-algo=bzip2` will not open. Zip and zlib do.

### Git: gitoxide (`gix`) for everything it has, the rest written here

- `gix` is pure Rust (zlib-rs, sha1 with collision detection) and has: the object database, references,
  the index, tree editing, commits, and **fetch** (protocol v1/v2, pack indexing).
- **It has no push.** `gix::push` in 0.91 is one configuration enum; nothing in `gix-protocol` speaks
  `receive-pack`. Push is therefore written in [`crates/pass-core/src/push.rs`](../../crates/pass-core/src/push.rs),
  about 300 lines: read the advertisement, check the fast-forward, build a pack of whole (undeltified)
  objects, send one ref update, read `report-status`. Tested against the real `git receive-pack`, over
  both transports, including a hook's refusal.
- **Its SSH transport runs the `ssh` program**, and its file transport runs `git-upload-pack`: neither exists
  on iOS. Its HTTP transport wants curl or reqwest. So the transports are ours:
  - **HTTP(S)**: `gix-transport` with the `http-client` feature alone is the smart-HTTP protocol with no
    backend; [`http.rs`](../../crates/pass-core/src/http.rs) implements its `Http` trait on `ureq` and the
    `pinned-http` agents. **No second TLS stack**: the same rustls + ring as the rest of the workspace,
    with the same certificate pinning (or the public CAs, for a hosting service).
  - **SSH**: [`ssh.rs`](../../crates/pass-core/src/ssh.rs) uses **russh** (pure Rust, built on `ring`, no
    aws-lc, no RSA) to run `git-upload-pack` / `git-receive-pack` on the server and hands the streams to
    gitoxide. Host key pinned by SHA-256 fingerprint; device key Ed25519, generated on the phone.
    `ssh-rs` (smaller, blocking) was rejected: it has no host key verification at all.
- Checkout and fast-forward are also written here ([`git.rs`](../../crates/pass-core/src/git.rs)) instead of
  enabling gix's worktree features: a store is a few hundred small files, and what comes from the remote is
  untrusted (paths that leave the directory or enter `.git` are refused, symbolic links are not written).
- **`git2` (libgit2) rejected**: it has push and SSH, but through C: libgit2 + libssh2 + OpenSSL to
  cross-compile for iOS, and a second TLS stack. That is the fallback if the pure-Rust route fails on a Mac.

### Commit signing: OpenPGP, done; SSH signatures, not

The phone signs its commits with the OpenPGP key it already holds (a detached signature in `gpgsig`,
as `git commit -S`). `git verify-commit`, `git log --show-signature` and `git merge --verify-signatures`
accept them: tested. If the imported key has no signing part (`gpg --export-secret-subkeys` of a key whose
primary signs), commits are simply not signed and the review has to do without.
Signing with the SSH device key (`gpg.format=ssh`) would be possible with the same libraries but could
not be checked here (no `ssh-keygen`), so it is not built.

### Weight

`Cargo.lock` went from 159 to 503 packages (+344) on the base this was built on: `gix` brings 107 (53 of
them `gix-*`), `pgp` 145, `russh` 122 (with tokio), with overlaps. `pgp` and `russh` are on two different
generations of the RustCrypto crates, so many small crates are there twice. The only native code is
`ring`'s, already in the workspace. Dropping SSH would remove russh and tokio; nothing else is optional.

### Open

1. **The iOS build.** See "Not verified".
2. **SSH against a real OpenSSH server**, and whether its default key exchange and ciphers match russh's
   `ring` set (they should: curve25519, chacha20-poly1305, aes-gcm). An RSA-only host key will not work.
3. **Push memory**: the pack is built in memory, whole objects. Fine for password entries, wrong for a store
   holding large attachments.
4. rPGP and russh have had less review than GnuPG and OpenSSH. They are the price of no C.

## Security model

**On the phone**
- The OpenPGP secret key is imported once, armored, and kept in the keychain *as exported* (still protected
  by its passphrase), `WhenUnlockedThisDeviceOnly`: not in backups, not in iCloud Keychain, not on a new
  phone. It is never written to the store directory or anywhere else.
- The passphrase is either typed at each unlock and kept nowhere, or kept in the keychain behind Face ID
  (`biometryCurrentSet`: no passcode fallback, void when the enrolled faces change).
- Unlocking takes the passphrase off the key once and holds the unlocked key in memory; locking drops it
  (rPGP zeroes its secret numbers on drop). Decrypted entries exist only in memory, are never written to
  disk, and the Rust buffers that held them are zeroed. **Limits**: a `String` handed to Swift is a copy the
  runtime frees without zeroing, and SwiftUI may keep text it displayed. Zeroing is best-effort, not a guarantee.
- The SSH key is made on the phone and never leaves it. The store directory holds only ciphertext and git history.

**The relay** (anyone who can read or write the repository, including whoever hosts it)
- **Sees**: entry and folder names, how many there are, when each changed and from which device, the whole
  history, the recipients (`.gpg-id`), ciphertext sizes. Names are metadata `pass` does not hide.
- **Cannot**: read an entry.
- **Can**: refuse service; serve an old `main` (rollback) or drop commits; delete entries; **plant an entry**
  or replace one (encrypted to you: it will decrypt, with content of their choosing, e.g. a wrong URL);
  **add a recipient** so that what you write next is readable by them.
- **What stands against it**: recipient changes are shown before use; with `PASSWORD_STORE_SIGNING_KEY`
  fingerprints set in the app, a `.gpg-id` is only used when its `.gpg-id.sig` is from one of them (as `pass`
  does); with *Only take signed updates*, the phone refuses any update whose last commit is not signed by a
  key of the store, which closes planting and deletion. Rollback to an older *signed* state is not detected.
  On the computer, `--verify-signatures` does the same for what the phone sends.
- A stolen phone key (SSH) lets someone push to `devices/phone` only, and you review that branch before merging.

**If the secret key cannot be exported** (hardware token), or you would rather not put it on a phone: make a
key for the phone, add it as a second recipient on the computer (`pass init <your-id> <phone-id>`, which
re-encrypts everything), and import that one here. The phone then needs your public key too (Settings →
Recipients). Revoking the phone means `pass init <your-id>` again, **and changing every password**: the phone
key could read everything up to that point, and the git history keeps the old ciphertexts.

## An AutoFill extension, later

Not built. It would take: a Credential Provider extension target (`ASCredentialProviderViewController`) with
the AutoFill entitlement; an app group and a shared keychain access group, so the extension can open the
store directory and the key (today both are the app's alone, and `storeDirectory` would move to the group
container); an index from entry to site (the `url:` field or the entry name), pushed to
`ASCredentialIdentityStore` after each sync without decrypting more than needed; and care with memory, an
extension having far less of it than the app. `PassKit` is already free of UI, so the extension would
reuse it as it is. No sync from the extension: read-only.

## What is tested

`cargo test -p pass-core` (19 tests, nothing outside the repo): names that cannot leave the store, `.gpg-id`
lookup, entry parsing, password generation, RFC 6238 vectors, SSH URLs and quoting, the push wire format,
paths from a remote that would leave the store.

`cargo test -p pass-core -- --ignored` (10 tests, real tools, throwaway keys and repositories):
- a store made by `gpg` + `pass.py` is read, for an RSA key and for an ed25519/cv25519 key; so are files
  `gpg` writes on its own (compressed, OCB, armored);
- what is written here is read by `gpg -d` and `pass.py`, the history reads like pass's, `git fsck` and
  `git status` are clean, `git verify-commit` accepts the signatures;
- several recipients, a `.gpg-id` per directory, re-encryption on move, refusal when a public key is missing;
- wrong passphrase, a key the entry is not for, subkeys-only exports, a key with no passphrase;
- a signed `.gpg-id` (`PASSWORD_STORE_SIGNING_KEY`): tampered, signed by someone else, unsigned;
- the whole cycle, once over HTTP (`git http-backend`) and once over SSH (stand-in sshd → `git-shell`):
  clone, laptop pushes → phone fast-forwards, phone edits → pushes its branch → laptop reviews decrypted
  diffs and signatures, merges with `--verify-signatures`, pushes → phone fast-forwards onto the merge;
  divergence reported; discard; a new recipient held for review and accepted;
- the relay's refusals: the `update` hook rejects a push to another branch; and the phone's: wrong host key,
  unknown device key, a rewritten device branch, unsigned or foreign-signed updates when signatures are required;
- https with a pinned self-signed certificate and Basic auth: wrong token, wrong pin, no trust, token over http.

`apps/pass/swift-test.sh` (18 tests): settings, the tree, the model's stages, lock timeout and clipboard
expiry with a fake clock, and end to end through the bindings against the relay; then the script itself
checks the pushed branch with `git`, `gpg` and `pass.py`.

Both need `PASS_PY=/path/to/pass.py` (default `../clones/pass/pass.py` beside `ios-apps`), `gpg`, `git`, `python3`.

## Not verified

- **Cross-compiling for iOS** (`make apple`): never done. Watch `ring` (works for companion), `russh`,
  `tokio`'s `net` on iOS, and the size of the static library.
- **The SwiftUI views and the keychain/Face ID code**: parsed, never compiled.
- **OpenSSH** on the other end (see above), and `git-shell`'s place in `PATH` on your relay.
- A real hosting service over https (GitHub and the like): the protocol is the same one `git http-backend`
  speaks, but none was contacted.
- Old keys: a recipient whose self-signatures rPGP rejects (SHA-1 era) cannot be imported as a public key.
  Your own secret key is not checked that way and should still work.
- Big stores, slow networks, interrupted transfers (a fetch that fails leaves the store as it was; a commit
  whose file write fails afterwards would leave the directory behind `HEAD`).

## Seen in pass.py

Not touched (read-only here), but found while testing against it:
- `pass git <cmd>` other than `init` always fails with "not a git repository": `cmd_git` calls
  `set_git_dir(os.path.join(PASSWORD_STORE_DIR, "/"))`, and joining with `"/"` gives `/`.
- `pass generate` draws from `random.choice` (Mersenne Twister), not from `secrets`/`os.urandom`.
- `pass insert` (without `-m`) and `generate` store the password with no final newline, `init` writes
  `.gpg-id` without one, and `show` prints one newline too many; the original `pass` does none of these.
  Harmless for reading, but the two tools produce different bytes for the same input.
- The `diff.gpg.textconv` command saved by `pass git init` includes whatever `PASSWORD_STORE_GPG_OPTS` held
  at that moment.
