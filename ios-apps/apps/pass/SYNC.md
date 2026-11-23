# pass sync: the protocol

How a password-store is kept in step between devices through a server that understands nothing.
Three programs implement this, and must agree on every line of it: the drop zone (`dropzone/`),
`pass sync` (a command of [pass.py](https://github.com/c4ffein/pass)) and the phone
(`crates/pass-core`). `scenarios/` runs the same situations through both implementations, mixed.

Devices are equals: each keeps its own line of history, publishes it, and takes the others'
changes when its user has looked at them. Nothing designates a main line.

## Devices

- A device has a **name**: `[a-z0-9][a-z0-9-]{0,31}`.
- A device has an **OpenPGP key**. It signs what the device publishes, and it is a recipient of
  what the others publish. Whether it can read a given entry is `.gpg-id`'s business, as in any
  password-store. A device can be a key and nothing more (a backup key that only ever reads).
- A device that syncs has an **SSH key**, which only lets it into the drop zone.
- Each device knows the others from its own configuration: name, OpenPGP fingerprint (40 hex),
  public key. **Nothing in the store or in the drop zone can introduce a device or change a key.**

## The drop zone

An SFTP-only account. After login the working directory is the one directory there is.
(`pass sync` also accepts a local directory in its place: same files, same rules.)

- One file per device: `<name>.bundle.gpg`. A device writes its own file and reads the others'.
- To publish: upload as `<name>.bundle.gpg.part`, then rename over `<name>.bundle.gpg`
  (`posix-rename@openssh.com`, which OpenSSH's `sftp` uses by itself; without it: remove, then rename).
- The drop zone is not trusted: it may lose a file, serve an old one, or serve anything else under
  that name. Everything below holds whatever it does. All it can achieve is that a sync does not happen.

## A published file

A binary OpenPGP message, **signed by the publishing device's key, then encrypted to the key of
every device** (the publisher included). What `gpg --sign --encrypt --recipient A --recipient B`
writes. Publishers should not compress (`--compress-algo none`: a bundle is already packed);
readers accept zip and zlib anyway. The signature uses SHA-256 or stronger (`--digest-algo SHA512`:
left to itself gpg can pick SHA-1); a reader refuses a weaker one. A reader refuses a file over
256 MiB, and stops decrypting when what comes out passes 256 MiB: anyone can encrypt anything to a
device's public key, so that bound holds before any signature is looked at.

Its plaintext is a **git bundle** (version 2, SHA-1 objects) holding **exactly one reference,
`refs/pass-sync/<name>`**, the tip of that device's line, with its full history: no prerequisite
commits.

A reader takes the file of device D only if all of this holds, and otherwise changes nothing:

1. the message decrypts, and carries a valid signature made by the key configured for D;
2. the bundle holds one reference, and it is `refs/pass-sync/D`;
3. the bundle is complete (every object its reference needs is in it).

Then the reader stores that tip as its own `refs/pass-sync/D`. That is all receiving does: no
branch moves, no file changes in the working tree.

That signature is the authentication: D vouches for its tip and all that is in it, including what
it took from others. Commits may be signed too (the phone signs its own, `pass sync setup` turns
it on for pass.py); a reader shows who signed each, and requires nothing.

The signature's time is shown to the user ("published 3 days ago"). A tip that is an ancestor of the
one already held for D is an old file served again: it is reported as such and ignored. A tip that
is neither ahead of nor behind the one held (D rewrote its history) is taken like any other.

## What a synced store holds

Three kinds of path, and nothing else:

- an **entry**: a regular file named `*.gpg`, no part of its path starting with a dot.
  Its name is its path without `.gpg`;
- a **readers file**: a regular file named `.gpg-id`, at the root or in a directory no part of
  whose path starts with a dot. The root has one;
- `.gitattributes` at the root, which `pass git init` writes. It is taken when a device first
  adopts the store and never changes afterwards.

A device does not publish a tip that holds anything else (`.extensions/`, `.gpg-id.sig`, a symbolic
link, a note), and does not take one: the update is refused whole, naming the paths. Extensions
are code: they stay out of git, on the device that runs them.

## Readers

An entry is encrypted to the keys listed in the nearest `.gpg-id` above it. **Any device may change
a `.gpg-id`**, with two things holding always:

- **Readers are devices.** Every line of every `.gpg-id` of a tip must name exactly one key, and
  that key must be a device's: its fingerprint or 16-hex key id (primary or encryption subkey),
  or a text found in the user ID of one device's key and no other's. A tip where a line names
  anything else is refused whole. A device that writes a `.gpg-id` writes full fingerprints.
  Likewise an entry encrypted to a key that is no device's: refused whole.
- **Saying is not doing.** A `.gpg-id` says who should read; an entry is read by those its file is
  encrypted to. An entry whose file and governing `.gpg-id` disagree is **stale**. A device that
  changes a `.gpg-id` re-encrypts the entries below that it can read, in the same commit or the
  next. Having taken an update, a device re-encrypts every stale entry it can read, commits that
  (`Reencrypt for the readers of <dir>.`), and publishes. So a device adding itself where it cannot
  read has only *asked*: it reads nothing until a device that can read takes that change, which
  shows it as what it is ("12 entries below are not encrypted for phone yet; taking this
  re-encrypts them").

A device writes an entry encrypted to the keys of its governing `.gpg-id`, whether or not it is
one of them.

## Taking a device's update

Never automatic. For a device D whose tip is not contained in ours, the user is shown what taking
it would do, then takes it or leaves it.

With *base* the last common ancestor of the two lines, *theirs* what D did since, *mine* what this
device did since:

- no common ancestor: refused, unless this device's store is empty. Then it **adopts** D's tip
  as it is (same screen: everything shows as added);
- nothing of mine: this device moves forward to D's tip;
- otherwise it merges, into a commit whose parents are [mine, theirs], signed like its others.

Then it publishes.

### Entries

Two versions of an entry are **the same** when their bytes are equal, or when this device can
decrypt both and their plaintexts are equal (a re-encryption changes nothing).

Within one side, an entry removed at one name and one added at another with the same content
(as just defined) is a **move**: the same entry under a new name. Matching is one to one, in name
order. An entry both moved and changed on the same side is not recognised: a removal and an addition.

For each entry, following it through moves:

| One side | The other side | Result |
| :-- | :-- | :-- |
| changed it in any way (content, name, removal) | did nothing | the change |
| moved it | changed its content | the new content under the new name |
| did the same thing | | that |
| anything else (two contents, two names, a removal against a change, two new entries of one name) | | **a choice**: mine or theirs, whole |

A choice is named by the entry's name on this device's side, or on theirs if this device has
removed it. Without a choice for each, nothing is taken. Having landed, an entry whose file does
not match its new directory's readers is stale, and is handled as above.

### Readers files

Changed on one side: that side's. Changed on both to the same content: that. Otherwise **a
choice**, named by the directory with a trailing slash (`work/`, and `/` for the root).

### What the user is shown

One line per thing, in this vocabulary (which the scenario runner reads), all about *theirs*:

```
added NAME
changed NAME
moved OLD -> NEW
removed NAME
readers DIR: DEVICE DEVICE ...        the new set, names in order; "inherited" when the file is removed
stale NAME                            in their tip, not encrypted for its readers
conflict NAME                         needs a choice (an entry name, or a directory with its slash)
```

plus, on demand, the versions of an entry decrypted (base, mine, theirs) when this device reads them.

## pass sync (the computer)

Settings are in the store's git configuration (`pass-sync.device`, `pass-sync.url`,
`pass-sync.key`, one `pass-sync.peer` per other device), written by the first two commands.

- `pass sync setup <name> <drop zone> <own fingerprint>`, `pass sync peer [--remove] <name> [<fingerprint>]`.
- `pass sync push`: publish the current tip. Refused when the store holds what a synced store does not.
- `pass sync fetch [<device>...]`: receive, as above; one line per device saying where it stands.
- `pass sync log <device>`: the commits of theirs and who signed each, then the lines above.
- `pass sync diff <device>`: theirs, decrypted.
- `pass sync merge <device> [--mine NAME]... [--theirs NAME]...`: take the update, with the
  choices it needs. Then re-encrypts what is stale and readable, and publishes.

## What this gives, and what it does not

- The drop zone sees one file per device, their sizes and the times they change. No entry name,
  no history.
- Nobody who is not a device can add an entry, a reader, or a commit: it takes a device's OpenPGP
  key to publish anything a reader accepts, and a reader is always a device.
- Any device can change or remove any entry, including one it cannot read, and can ask to read
  anything. Every device sees that before taking it, and git keeps what was there.
- A device that reads an entry can show it to another device (move it, add a reader and
  re-encrypt). Nothing can stop a reader from telling; this at least shows it.
- A device's OpenPGP key reads every published file, past ones included if someone kept them, and
  every entry it was ever a reader of. Removing a device means removing it from the configuration
  of the others and from every `.gpg-id`, and changing the passwords it could read.
- Two devices publishing at the same moment cannot collide: each has its own file.
