# Pass

A phone client for a [password-store](https://www.passwordstore.org): the layout of `pass`
(and of [pass.py](https://github.com/c4ffein/pass)), OpenPGP files in a git repository.
There is no `gpg` and no `git` on iOS: both are libraries here, in Rust.

The phone is one **device** of a sync among equals, as [SYNC.md](SYNC.md) defines it: no device
owns the store, none talks to another, each leaves one signed and encrypted file in a
[drop zone](dropzone/README.md) that understands none of it, and each takes the others' changes
when its user says so. Read those two for the protocol and the server, and
[scenarios/README.md](scenarios/README.md) for the situations both implementations (this one and
`pass sync` in pass.py) are run through together. This file is about the phone.

## Status

| Part | State |
| :-- | :-- |
| `crates/pass-core` (Rust) | Tested on Linux against the real `gpg` 2.4, `git`, OpenSSH (`sshd` + `internal-sftp`, through `dropzone/entrypoint.sh`) and `pass.py`, including its own `pass sync` |
| `pass-device` (`crates/pass-core/examples`) | The core as a command-line device, for `scenarios/run.py` and for looking at a sync by hand. **A test tool**: its key has no passphrase |
| `apps/pass/Package` (Swift, no UI: settings, secrets, app state) | Tested on Linux through the generated bindings, against two real drop zones fed by `pass sync push`; what it publishes is then taken by `pass sync fetch`/`log`/`merge`. The keychain code is Apple-only and has not been compiled |
| `apps/pass/App`, `project.yml` | **Written without a Mac: syntax-checked only, never compiled or run** |
| The Rust core for `aarch64-apple-ios` | **Never built.** Every dependency is pure Rust except `ring` (which companion already ships), but nobody has compiled this set for iOS yet |

## Setting a phone up

1. **On the phone, name it and make its key** (Settings → OpenPGP key → *Make this device's key*): Ed25519
   to sign, Curve25519 to decrypt, generated here, so its private half never exists anywhere else. Its user
   id is the device's name. Importing a key made elsewhere is still possible. Send the public key to the
   other devices.
2. **On the phone, make its SSH key**, and give its public line to the drop zone (`DROPZONE_KEY_PHONE`).
3. **On each other device**, add the phone: on a computer, `gpg --import` its public key, compare the
   fingerprint with the one the phone shows, then
   ```sh
   pass sync setup pc sftp://drop@your-server:2222 <your-fingerprint>   # once
   pass sync peer phone <phone-fingerprint>
   pass sync push
   ```
4. **On the phone**: the drop zone's address, *Read the server's host key* and compare it with what the
   drop zone printed when it started; add every other device (name and public key), comparing fingerprints.
   Then *Sync*: each device that published shows what it has to take. Open one, read what it would do,
   *Take this store*.

Or the other way round: with no store anywhere, *Make the store here*, choose its readers among the
devices, and the others adopt it.

Being a device is not reading. A folder is read by the devices its `.gpg-id` names; the phone reads
`work/` once it is named there **and** a device that already reads `work/` has taken that change and
encrypted the entries again (below).

## What the app does

- **Browse and search** the store as folders; each word typed narrows the list. Listing needs no key.
- **Who reads what** is always in sight: a folder shows its readers by device name; an entry says
  *Readable by: pc, phone*, and says when the file is **stale**, not encrypted to exactly the readers its
  folder names. **Entries this device cannot read are listed, locked**: the name, nothing else.
- **Read an entry**: the first line is the password, `name: value` lines are fields, `otpauth://` lines
  give time-based codes (RFC 6238). Tap to copy: the clipboard entry is local to the device (no Universal
  Clipboard) and expires (45 s by default).
- **Write**: add, edit, generate (new, or in place as `pass generate -i`), rename/move, delete, for entries
  and for folders, each a commit with the message `pass` would write, signed by the phone's key. An entry
  is encrypted to every reader of its folder, which are all devices or the store would not have been taken.
  The phone can write where it does not read.
- **Change the readers of a folder** (swipe a folder, or the people button for the top of the store): choose
  among the devices, or give the folder back the readers of the one above. The `.gpg-id` gets full
  fingerprints; what the phone reads there is encrypted again at once. Where the phone does not read,
  naming itself is a **request**: the entries stay as they are and show as stale, here and in the review of
  every other device, until one that reads them takes the change.
- **Sync** is one button: it publishes the phone's history when it changed, and fetches every other
  device's file. Every change made on the phone is published right away when the drop zone is reachable.
  A store that holds anything but entries, `.gpg-id` files and the one-line `.gitattributes` is not
  published, and the sync says which paths are in the way.
- **Nothing received is applied by itself.** Each file is checked (signed by that device's key, the one
  reference, complete; a tip that holds only what a synced store holds; every `.gpg-id` line a device,
  every entry encrypted to devices only), kept aside, and counted: *pc: 2 to take*. A file that fails any
  of it is refused whole, with the reason on that device's row.
- **The review**, per device, says what taking it does in the words of SYNC.md, one line each:
  `added bank`, `changed wifi`, `moved a -> b/a`, `removed old`, `readers work/: pc phone`,
  `stale work/w1`, `conflict bank`; then how many entries taking it would encrypt again (that is what
  granting a request looks like), and the commits with who signed each (shown, never required). *Take*
  or *Later*.
- **Taking** adopts the store when the phone has none, moves forward when the update continues what the
  phone has, and merges otherwise, in a signed merge commit. Entries are followed through moves, and two
  encryptions of one content are the same entry (the phone decrypts what it can to tell). What one side
  did is kept; what both did differently is a **choice, whole**: *Mine* or the other device's, having
  seen both. The readers of a folder merge the same way, the choice named by the folder (`work/`).
  Afterwards every stale entry the phone reads is encrypted again for the readers its folder now names,
  in a commit of its own, and the result is published.
- **Drop this device's changes**: stand where another device does. What others already took of them
  comes back from them like any other change.
- **Unlock** with the key's passphrase, typed each time or kept in the keychain behind Face ID. Locks after
  2 minutes without use, and at once when the app leaves the screen. Everything that touches the drop zone
  or a review needs the key (the files are encrypted to it and signed with it): there is no sync while locked.

Not there: an AutoFill extension (below), a history browser, removing a device (take it off every
folder's readers, change what it read, remove it from each device's settings and from the drop zone:
by hand, as SYNC.md says).

## How it is built

Read in the crates' own sources (November 2026: `gix` 0.91, `pgp` 0.21.1, `russh` 0.65.4), not from memory.

- **OpenPGP: rPGP (`pgp`)**, pure Rust. It reads what GnuPG writes for RSA and ed25519/cv25519 keys, including
  GnuPG's own OCB packet (type 20) and its default compression, and writes what `gpg` reads: entries
  (encrypted), commits (detached signatures, `git verify-commit` accepts them), published files
  (`--sign --encrypt`, uncompressed), and keys (`gpg --import` takes the phone's). **Sequoia was rejected**:
  no OCB for packet 20, and its mature backends are C.
- **Git: gitoxide (`gix`)** for the object database, references, the index and reading a bundle's pack
  (`gix-pack`). No network code: git never touches a socket here. Written in the crate, small: commits,
  moving the work tree (paths that would leave the store or enter `.git` are refused, symbolic links are
  not written), and reading and writing **bundles** ([`bundle.rs`](../../crates/pass-core/src/bundle.rs):
  a header and a pack of whole objects; `git bundle verify` and `git fetch` take them, and `git bundle
  create`'s are read, deltas included).
- **The merge** is not git's: [`merge.rs`](../../crates/pass-core/src/merge.rs) is the table of SYNC.md,
  on entries rather than files, with no I/O (the caller says when two files are the same entry), and
  tested as such.
- **Transport: SFTP, hand-written over `russh`** ([`sftp.rs`](../../crates/pass-core/src/sftp.rs), about
  250 lines, no new crate): the six requests a drop zone needs, one at a time, with the 256 MiB limit
  checked on the size the server announces and again on what it sends, and publishing by
  `posix-rename@openssh.com` (remove-then-rename without it). `russh` is pure Rust on `ring`; the host key is
  pinned by its SHA-256 fingerprint, the device key is Ed25519. **Tested against OpenSSH 9.6.**
- **`git2` (libgit2) rejected**: C, and nothing here needs it.

**Weight.** `Cargo.lock` holds 498 packages; 187 without pass (every target and platform counted, as the
lockfile does). 311 are there only for `pass-core`, whose whole tree is 438. Three big trees: `pgp`,
`russh` with tokio, `gix`; `pgp` and `russh` are on two different generations of the RustCrypto crates.
The only native code is `ring`'s. The symmetric model added no crate.

## Security model

The protocol's own guarantees and limits are in [SYNC.md](SYNC.md) ("What this gives, and what it does not").
On the phone:

- **The OpenPGP secret key** is made here or imported once, and kept in the keychain still protected by its
  passphrase, `WhenUnlockedThisDeviceOnly`: not in backups, not in iCloud Keychain, not on a new phone. It is
  never written to the store directory or anywhere else. Erasing the app's data destroys a key made here.
- **The passphrase** is either typed at each unlock and kept nowhere, or kept in the keychain behind Face ID
  (`biometryCurrentSet`: no passcode fallback, void when the enrolled faces change).
- Unlocking takes the passphrase off the key once and holds the unlocked key in memory; locking drops it
  (rPGP zeroes its secret numbers on drop). Decrypted entries exist only in memory and are never written to
  disk. **Limits**: a `String` handed to Swift is a copy the runtime frees without zeroing, and SwiftUI may
  keep text it displayed. Zeroing is best-effort, not a guarantee.
- **The SSH key** is made on the phone and only opens the drop zone, which holds nothing readable.
- **What comes from the drop zone is not trusted.** A file is only used when it decrypts and is signed by
  the key the phone's own settings give the device it is named after. Anything else (unsigned, signed by
  another device or a stranger, signed with SHA-1, the wrong reference, several, an incomplete or damaged
  bundle, a truncated or oversized file, one that decompresses past the limit) is refused and changes
  nothing; an older file served again is recognised and ignored. Devices and their keys only ever come from
  the phone's own settings.
- **What a device publishes is trusted only as far as SYNC.md says**: its tip must hold nothing but
  entries, `.gpg-id` files and the fixed `.gitattributes` (no `.extensions`, no hooks, no links, no
  attributes that run a filter), name only devices as readers, and encrypt only to devices. A device
  **can** change who reads a folder: that is the model, and why the review puts `readers` and `stale`
  lines and the count of re-encryptions in front of the user before anything is taken. Taking an update
  that adds a reader **is** granting it.
- **What the review cannot tell you**: whether an entry's *content* is right. It shows it; you read it.

## An AutoFill extension, later

Not built. It would take: a Credential Provider extension target (`ASCredentialProviderViewController`) with
the AutoFill entitlement; an app group and a shared keychain access group, so the extension can open the
store directory and the key (today both are the app's alone, and `storeDirectory` would move to the group
container); an index from entry to site (the `url:` field or the entry name), pushed to
`ASCredentialIdentityStore` after each update taken without decrypting more than needed; and care with
memory, an extension having far less of it than the app. `PassKit` is already free of UI, so the extension
would reuse it as it is. Read-only: no sync from the extension.

## What is tested

`cargo test -p pass-core` (30 tests, nothing outside the repo): names that cannot leave the store, `.gpg-id`
lookup, entry parsing, password generation, RFC 6238 vectors, drop zone URLs, SFTP framing, bundle headers,
paths from another device that would leave the store, and the merge table: scenarios 04 to 12 and 29 as
pure functions.

`cargo test -p pass-core -- --ignored` (16 tests; real tools, throwaway keys, repositories and drop zones;
the sync ones need root, OpenSSH and an account named `drop`, as `dropzone/test.sh` does):
- a store made by `gpg` + `pass.py` is read, for an RSA key and for an ed25519/cv25519 key; so are files
  `gpg` writes on its own (compressed, OCB, armored);
- what is written here is read by `gpg -d` and `pass.py`, the history reads like pass's, `git fsck` and
  `git status` are clean, `git verify-commit` accepts the signatures;
- a root `.gpg-id` and subfolders with other sets: who reads each folder and entry by device name, locked
  entries, stale entries, re-encryption on move;
- keys: wrong passphrase, subkeys-only exports (read, but no write: they cannot sign), a key made on the
  phone that `gpg` imports, encrypts to and verifies;
- **the scenarios of `scenarios/README.md`, played by two or three devices of this crate** through a real
  drop zone: adopting, moving forward, merging (01–03); every kind of conflict with each choice, and
  nothing taken without one (04–06, 09, 11, 12); what fits together without a choice (07, 08, 10); folders
  with their own readers, writing blind, granting, removing, moving between folders (13–15, 18, 20, 21);
  a request, left alone and granted (16, 17); two sets of readers for one folder (19); a ring, a relay,
  dropping, two devices granting the same request (22, 23, 28, 29). Each ends with every device listing
  the same entries, reading the same contents where it is a reader, nothing stale, nothing to take,
  `git fsck` and `git status` clean;
- **what a device gone wrong would publish**, made with `git`, `gpg` and OpenSSH's `sftp` (24–27): a
  `.gpg-id` naming a stranger, nobody, or two devices at once; an entry encrypted to a stranger's key or
  not encrypted; `.extensions/`, a symbolic link, a note, a `.gpg-id.sig`, a hidden folder, another
  `.gitattributes`; a file signed by a stranger, by another device, with SHA-1, unsigned, not for this
  device, not OpenPGP; a bundle with two references, the wrong one, prerequisites, cut short; the right
  file truncated or with a bit flipped; over 256 MiB as it is, and once decompressed; an old file again;
  a lost file. After all of it the right file is still taken. And this device refuses to publish a store
  of its own that holds a note;
- the server: another host key than the pinned one, an SSH key it does not know, nobody listening;
- **`pass sync` itself as the other device** (`setup`, `peer`, `push`, `fetch`, `log`, `merge`): adoption
  by pass.py, a move there against a change here, a conflict both name the same way, one history at the end.

`scenarios/run.py` (not part of this crate's tests) plays all 30 on every mix of `pass.py` and
`pass-device`.

`apps/pass/swift-test.sh` (21 tests, needs root for the drop zones): settings, the tree with locked entries,
the wording of states, visits and reviews, making and unlocking a key, the lock timeout with a fake clock,
devices by their public key, a store made on the phone; then end to end with pass.py as the other device:
review and adoption, publish and drop, the readers of a folder (own, inherited, a request), and a merge
with a choice whose result `pass sync merge` then takes on the computer.

All need `PASS_PY=/path/to/pass.py` (default `../clones/pass/pass.py` beside `ios-apps`).

## Not verified

- **Cross-compiling for iOS** (`make apple`): never done. Watch `ring` (works for companion), `russh`,
  `tokio`'s `net` on iOS, and the size of the static library.
- **The SwiftUI views and the keychain/Face ID code**: parsed, never compiled.
- **The drop zone's Docker image** (see its README), and an SSH server other than OpenSSH. An RSA-only
  host key will not work (the build has no RSA).
- Old keys: a device whose key has SHA-1 self-signatures cannot be added (rPGP rejects them), and rPGP
  refuses a published file whose signature uses SHA-1 (see below).
- Big stores: a published file is the whole history, built and read in memory, and a review decrypts every
  entry that differs to tell a change from a re-encryption. Fine for thousands of entries; not for a store
  holding large attachments.
- A commit whose files could not all be written afterwards would leave the directory behind `HEAD`.
- Pointing the app at another drop zone does not publish again until the store changes: what is published
  is remembered by tip, not by server.

## Found on the way

- **gpg signs with SHA-1 when a recipient's key lists no digest preference.** A key made by rPGP's defaults
  has none, so `gpg --sign --encrypt -r phone` signed with SHA-1, which rPGP (rightly) refuses for Ed25519.
  The key the phone makes carries preferences (SHA-512/384/256, AES, zlib), and the tests cover it; a key
  from another tool could bring the problem back. `--digest-algo SHA512` on the publishing side closes it.
- **A device that cannot read an entry cannot tell its re-encryption from a change**: to it, the review of
  a grant says `changed shared/s1` beside `readers shared/: ...`. True to what it can know.
- In pass.py: `pass git <cmd>` other than `init` does not find the repository; `pass generate` draws from
  `random.choice`, not a CSPRNG; `pass sync` does not pass `PASSWORD_STORE_GPG_OPTS` to gpg.
