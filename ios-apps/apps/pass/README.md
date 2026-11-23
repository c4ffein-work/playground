# Pass

A phone client for a [password-store](https://www.passwordstore.org): the layout of `pass`
(and of [pass.py](https://github.com/c4ffein/pass)), OpenPGP files in a git repository.
There is no `gpg` and no `git` on iOS: both are libraries here, in Rust.

The store is managed on a computer. The phone is a **member** of its sync, as [SYNC.md](SYNC.md) defines it:
the two never talk to each other, each leaves one signed and encrypted file in a
[drop zone](dropzone/README.md) that understands none of it. Read those two for the protocol and the
server; this file is about the phone.

## Status

| Part | State |
| :-- | :-- |
| `crates/pass-core` (Rust) | Tested on Linux against the real `gpg` 2.4, `git`, OpenSSH (`sshd` + `internal-sftp`, through `dropzone/entrypoint.sh`) and `pass.py`, including its own `pass sync` |
| `apps/pass/Package` (Swift, no UI: settings, secrets, app state) | Tested on Linux through the generated bindings, against two real drop zones fed by `pass sync push`; what it publishes is then taken by `pass sync fetch`/`log`/`merge`. The keychain code is Apple-only and has not been compiled |
| `apps/pass/App`, `project.yml` | **Written without a Mac: syntax-checked only, never compiled or run** |
| The Rust core for `aarch64-apple-ios` | **Never built.** Every dependency is pure Rust except `ring` (which companion already ships), but nobody has compiled this set for iOS yet |

## Setting a phone up

1. **On the phone, make its key** (Settings → OpenPGP key → *Make this device's key*): Ed25519 to sign,
   Curve25519 to decrypt, generated here, so its private half never exists anywhere else. Importing a key
   made elsewhere is still possible. Send the public key to the computer.
2. **On the phone, make its SSH key**, and give its public line to the drop zone (`DROPZONE_KEY_PHONE`).
3. **On the computer**: `gpg --import` the phone's public key, compare its fingerprint with the one the
   phone shows, then
   ```sh
   pass sync setup pc sftp://drop@your-server:2222 <your-fingerprint>
   pass sync peer phone <phone-fingerprint>
   pass init <your-fingerprint> <phone-fingerprint>   # where the phone should read; -p <folder> for one folder
   pass sync push
   ```
4. **On the phone**: the drop zone's address, *Read the server's host key* and compare it with what the
   drop zone printed when it started; add the computer as a device (its public key, marked as the owner),
   comparing its fingerprint too. Then *Sync*: the owner's store arrives, is shown, and is taken when you accept.

## What the app does

- **Browse and search** the store as folders; each word typed narrows the list. Listing needs no key.
- **Who can read what** is always in sight, because different folders can be shared with different sets of
  devices (a `.gpg-id` per directory): a folder shows its readers by device name; an entry says
  *Readable by: pc, phone*, and warns when one of them is a key that is no device's, or when the file is not
  actually encrypted to the readers its folder lists (a `.gpg-id` changed and the file was not encrypted
  again). **Entries this device cannot read are listed, locked**: the name, nothing else.
- **Read an entry**: the first line is the password, `name: value` lines are fields, `otpauth://` lines
  give time-based codes (RFC 6238). Tap to copy: the clipboard entry is local to the device (no Universal
  Clipboard) and expires (45 s by default).
- **Write entries, and only entries**: add, edit, generate (new, or in place as `pass generate -i`),
  rename/move, delete, each a commit with the message `pass` would write, signed by the phone's key. An
  entry is encrypted to every reader of its folder or not written at all: when one of them is a key the
  phone does not have (no device's), it refuses and says which. The phone has no way to change a `.gpg-id`
  or anything that is not a `*.gpg` file.
- **Sync** is one button: it publishes the phone's history when it changed, and fetches what the owner
  published. Every change made on the phone is published right away when the drop zone is reachable.
- **The owner's update is never applied by itself.** It is checked (signed by the owner's key, the one
  reference, complete), kept aside, and shown: how long ago it was signed, the commits and who signed each,
  the entries added, changed, renamed and removed with both versions decrypted on demand, each `.gpg-id`
  before and after by device name, whose readers each entry gains, and what is not an entry. *Accept* or *Later*.
- **Accepting** moves forward to the owner's tip, or, when the phone has changes the owner has not taken,
  merges whole files in a signed merge commit: an entry changed on one side keeps that side's version; changed
  on both, you choose *Mine* or *The owner's* having seen both; the rest is the owner's. Then it publishes, and
  `pass sync merge phone` on the computer is a fast-forward.
- **Drop this device's changes**: back to the owner's history as last accepted.
- **Unlock** with the key's passphrase, typed each time or kept in the keychain behind Face ID. Locks after
  2 minutes without use, and at once when the app leaves the screen. Everything that touches the drop zone
  needs the key (the files are encrypted to it and signed with it): there is no sync while locked.

Not there: an AutoFill extension (below), directories in rename/delete, reading the other members' files
(only the owner's is fetched), a history browser.

## How it is built

Read in the crates' own sources (November 2026: `gix` 0.91, `pgp` 0.21.1, `russh` 0.65.4), not from memory.

- **OpenPGP: rPGP (`pgp`)**, pure Rust. It reads what GnuPG writes for RSA and ed25519/cv25519 keys, including
  GnuPG's own OCB packet (type 20) and its default compression, and writes what `gpg` reads: entries
  (encrypted), commits (detached signatures, `git verify-commit` accepts them), published files
  (`--sign --encrypt`, uncompressed), and keys (`gpg --import` takes the phone's). **Sequoia was rejected**:
  no OCB for packet 20, and its mature backends are C.
- **Git: gitoxide (`gix`)** for the object database, references, the index and reading a bundle's pack
  (`gix-pack`). No network code is left: git never touches a socket here. Written in the crate, small:
  commits, the whole-file merge, moving the work tree (paths that would leave the store or enter `.git` are
  refused, symbolic links are not written), and reading and writing **bundles**
  ([`bundle.rs`](../../crates/pass-core/src/bundle.rs): a header and a pack of whole objects; `git bundle
  verify` and `git fetch` take them, and `git bundle create`'s are read, deltas included).
- **Transport: SFTP, hand-written over `russh`** ([`sftp.rs`](../../crates/pass-core/src/sftp.rs), about
  250 lines, no new crate): the six requests a drop zone needs, one at a time, with the 256 MiB limit
  checked on the size the server announces and again on what it sends, and publishing by
  `posix-rename@openssh.com` (remove-then-rename without it). `russh` is pure Rust on `ring`; the host key is
  pinned by its SHA-256 fingerprint, the device key is Ed25519. **Tested against OpenSSH 9.6.**
- **`git2` (libgit2) rejected** as before: C, and nothing here needs it anymore.

**Weight.** The workspace without pass locks 165 packages; with it, 498 (it was 507 with the relay design).
`pass-core`'s own tree went from 388 to 340 packages: gone are `gix-transport`, `gix-protocol` and their
kin, `ureq`, `rustls`, `pinned-http` and `base64` as direct dependencies. What remains is three big
trees: `pgp` (145), `russh` with tokio (122), `gix` (92), `pgp` and `russh` being on two different
generations of the RustCrypto crates. The only native code is `ring`'s.

## Security model

The protocol's own guarantees and limits are in [SYNC.md](SYNC.md) ("What this gives, and what it does not").
On the phone:

- **The OpenPGP secret key** is made here or imported once, and kept in the keychain still protected by its
  passphrase, `WhenUnlockedThisDeviceOnly`: not in backups, not in iCloud Keychain, not on a new phone. It is
  never written to the store directory or anywhere else. Erasing the app's data destroys a key made here:
  the owner then removes that device, as SYNC.md says.
- **The passphrase** is either typed at each unlock and kept nowhere, or kept in the keychain behind Face ID
  (`biometryCurrentSet`: no passcode fallback, void when the enrolled faces change).
- Unlocking takes the passphrase off the key once and holds the unlocked key in memory; locking drops it
  (rPGP zeroes its secret numbers on drop). Decrypted entries exist only in memory and are never written to
  disk. **Limits**: a `String` handed to Swift is a copy the runtime frees without zeroing, and SwiftUI may
  keep text it displayed. Zeroing is best-effort, not a guarantee.
- **The SSH key** is made on the phone and only opens the drop zone, which holds nothing readable.
- **What comes from the drop zone is not trusted**: a file is only used when it decrypts and is signed by the
  owner's key as configured on the phone, and even then nothing is applied before it is shown. Anything else
  (unsigned, signed by another device or a stranger, the wrong reference, several, an incomplete or damaged
  bundle, a truncated or oversized file) is refused and changes nothing; an older file served again is
  recognised and ignored. Devices and their keys only ever come from the phone's own settings.
- **What the review cannot tell you**: whether an entry's *content* is right. It shows it; you read it.

## An AutoFill extension, later

Not built. It would take: a Credential Provider extension target (`ASCredentialProviderViewController`) with
the AutoFill entitlement; an app group and a shared keychain access group, so the extension can open the
store directory and the key (today both are the app's alone, and `storeDirectory` would move to the group
container); an index from entry to site (the `url:` field or the entry name), pushed to
`ASCredentialIdentityStore` after each accepted update without decrypting more than needed; and care with
memory, an extension having far less of it than the app. `PassKit` is already free of UI, so the extension
would reuse it as it is. Read-only: no sync from the extension.

## What is tested

`cargo test -p pass-core` (20 tests, nothing outside the repo): names that cannot leave the store, `.gpg-id`
lookup, entry parsing, password generation, RFC 6238 vectors, drop zone URLs, SFTP framing, bundle headers,
the three-way rule, paths from another device that would leave the store.

`cargo test -p pass-core -- --ignored` (14 tests; real tools, throwaway keys, repositories and drop zones;
the sync ones need root, OpenSSH and an account named `drop`, as `dropzone/test.sh` does):
- a store made by `gpg` + `pass.py` is read, for an RSA key and for an ed25519/cv25519 key; so are files
  `gpg` writes on its own (compressed, OCB, armored);
- what is written here is read by `gpg -d` and `pass.py`, the history reads like pass's, `git fsck` and
  `git status` are clean, `git verify-commit` accepts the signatures;
- a root `.gpg-id` and subfolders with other sets: who can read each folder and entry by device name, locked
  entries, a mismatch between a `.gpg-id` and the files under it, re-encryption on move, the refusal to
  write for a reader whose key is not here;
- keys: wrong passphrase, subkeys-only exports (read, but no write: they cannot sign), a key made on the
  phone that `gpg` imports, encrypts to and verifies;
- the sync, the computer played by hand with `git bundle`, `gpg --sign --encrypt` (with and without
  compression) and OpenSSH's `sftp`, exactly as SYNC.md says: first setup; phone edits → publishes → the
  computer checks the signature, `git bundle verify`, the single reference, the commit signatures, that only
  entries are touched, fast-forwards, republishes → the phone is level; different entries changed on both
  sides merge by themselves; the same entry changed on both (and changed/removed, both ways) needs a choice,
  and each choice gives the right result on both machines; a change of recipients is in the review data by
  directory and by entry; dropping outstanding changes;
- the refusals: a file signed by an unknown key, by another device, unsigned, not for this device, not
  OpenPGP at all; a bundle with two references, the wrong one, a branch, prerequisites, a hole in its
  history, cut short; the right file truncated or with a bit flipped; over 256 MiB; an old file again;
  a lost file. And of the server: another host key than the pinned one, an SSH key it does not know;
- the same round trips with the computer's side done by **`pass sync` itself** (`setup`, `peer`, `push`,
  `fetch`, `log`, `diff`, `merge`).

`apps/pass/swift-test.sh` (19 tests, needs root for the drop zones): settings, the tree with locked entries,
who-can-read wording, making and unlocking a key, the lock timeout with a fake clock, devices and their one
owner; then end to end: review and first accept, publish and drop, and a merge with a choice whose result
`pass sync merge` takes on the computer.

All need `PASS_PY=/path/to/pass.py` (default `../clones/pass/pass.py` beside `ios-apps`).

## Not verified

- **Cross-compiling for iOS** (`make apple`): never done. Watch `ring` (works for companion), `russh`,
  `tokio`'s `net` on iOS, and the size of the static library.
- **The SwiftUI views and the keychain/Face ID code**: parsed, never compiled.
- **The drop zone's Docker image** (see its README), and an SSH server other than OpenSSH. An RSA-only
  host key will not work (the build has no RSA).
- Old keys: a device whose key has SHA-1 self-signatures cannot be added (rPGP rejects them), and rPGP
  refuses a published file whose signature uses SHA-1 (see below).
- Big stores: a published file is the whole history, built and read in memory. Fine for thousands of
  entries; not for a store holding large attachments.
- A commit whose files could not all be written afterwards would leave the directory behind `HEAD`.

## Found on the way

- **gpg signs with SHA-1 when a recipient's key lists no digest preference.** A key made by rPGP's defaults
  has none, so `gpg --sign --encrypt -r phone` signed with SHA-1, which rPGP (rightly) refuses for Ed25519.
  The key the phone makes now carries preferences (SHA-512/384/256, AES, zlib), and the tests cover it; a key
  from another tool could bring the problem back. `--digest-algo SHA512` on the publishing side would close it.
- A merge keeps this device's version of an entry as it was encrypted. If the owner's update also changed
  the readers of that folder, that entry is still encrypted to the old ones: the app shows the mismatch on
  the entry, and `pass init` on the computer encrypts it again.
- In pass.py: `pass git <cmd>` other than `init` does not find the repository; `pass generate` draws from
  `random.choice`, not a CSPRNG; `pass sync` does not pass `PASSWORD_STORE_GPG_OPTS` to gpg.
