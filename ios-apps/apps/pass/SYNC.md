# pass sync: the protocol

How a password-store is kept in step between a computer and a phone through a server that
understands nothing. Three programs implement this, and must agree on every line of it:
the drop zone (`dropzone/`), the computer (`pass sync`, a command of
[pass.py](https://github.com/c4ffein/pass)) and the phone (`crates/pass-core`).

## Devices

- A device has a **name**: `[a-z0-9][a-z0-9-]{0,31}`.
- A device has an **OpenPGP key**. It signs what the device publishes, and it is a recipient of
  what the others publish. It may or may not be a recipient of a given entry: that is `.gpg-id`'s
  business, as in any password-store.
- A device has an **SSH key**, which only lets it into the drop zone.
- Exactly one device is the **owner**: the computer, where the store is managed. The others are
  **members**.
- Each device knows the others from its own configuration: name, OpenPGP fingerprint (40 hex),
  public key, and which one is the owner. Nothing in the store or in the drop zone can introduce
  a device or change a key.

## The drop zone

An SFTP-only account. After login the working directory is the one directory there is.

- One file per device: `<name>.bundle.gpg`. A device writes its own file and reads the others'.
- To publish: upload as `<name>.bundle.gpg.part`, then rename over `<name>.bundle.gpg`
  (`posix-rename@openssh.com`, which OpenSSH's `sftp` uses by itself; without it: remove, then rename).
- The drop zone is not trusted: it may lose a file, serve an old one, or serve anything else under
  that name. Everything below holds whatever it does. All it can achieve is that a sync does not happen.

## A published file

A binary OpenPGP message, **signed by the publishing device's key, then encrypted to the key of
every device** (the publisher included). What `gpg --sign --encrypt --recipient A --recipient B`
writes. Publishers should not compress (`--compress-algo none`: a bundle is already packed);
readers accept zip and zlib anyway. A reader refuses a file over 256 MiB.

Its plaintext is a **git bundle** (version 2) holding **exactly one reference,
`refs/pass-sync/<name>`**, with its full history: no prerequisite commits. For the owner that
reference is the tip of the store's branch. For a member it is the tip of that member's own
line of work.

A reader takes the file of device D only if all of this holds, and otherwise changes nothing:

1. the message decrypts, and carries a valid signature made by the key configured for D;
2. the bundle holds one reference, and it is `refs/pass-sync/D`;
3. the bundle is complete (every object its reference needs is in it).

Then the reader stores that tip as its own `refs/pass-sync/D`. That is all receiving does: no
branch moves, no file changes in the working tree.

The signature's time is shown to the user ("published 3 days ago"). A tip that is an ancestor of the
one already held for D is an old file served again: it is reported as such and ignored.

## Who may change what

- **Reading** is `.gpg-id`, per directory, as in any password-store: an entry is encrypted to the
  keys listed in the nearest `.gpg-id` above it.
- **Only the owner changes who can read**, and anything that is not an entry. A member's commits may
  add, change, rename and remove `*.gpg` files, and nothing else: no `.gpg-id`, no `.gpg-id.sig`,
  no `.gitattributes`, nothing under `.extensions/`.
- A member writes an entry encrypted to the recipients of its governing `.gpg-id` (so it must
  hold their public keys), and signs each of its commits with its OpenPGP key.

## The member's side (the phone)

The member's line of work is the owner's history plus its own commits.

- **Taking the owner's update** (its `refs/pass-sync/<owner>` moved) is never automatic. The member
  shows what the update does: the commits, the entries added, changed, renamed and removed (with
  both versions decrypted on demand, when it can read them), and any change of recipients, entry
  by entry and directory by directory. The user accepts or leaves it for later.
- On accept:
  - no commit of its own outstanding: it moves forward to the owner's tip;
  - otherwise it **merges** the owner's tip into its line, as a merge commit signed like the
    others. Entries are merged as whole files: changed on one side only, that side wins; changed
    on both sides to different contents (or changed on one and removed on the other), the user
    chooses between the two, having seen both. Anything that is not an entry is taken from the
    owner as it is.
- After any commit of its own, and after any accepted update, it publishes its tip.
- It can also drop its own outstanding commits and go back to the owner's tip.

## The owner's side (the computer)

The settings are in the store's git configuration (`pass-sync.device`, `pass-sync.url`,
`pass-sync.key`, one `pass-sync.peer` per member), written by `pass sync setup` and `pass sync peer`.
The drop zone may also be a local directory (a USB stick, a synced folder): same files, same rules.

- `pass sync push`: publish the current tip.
- `pass sync fetch`: take each member's file, as above, then say for each member how many commits
  it has that the store does not, and what they touch.
- `pass sync log <member>` / `pass sync diff <member>`: the member's outstanding commits, with who
  signed each, and the decrypted difference.
- `pass sync merge <member>`: take the member's commits. Refused unless
  - the result is a fast-forward (the member has already merged the owner's tip),
  - every outstanding commit is signed by that member's key or by the owner's, and
  - between the store and the member's tip, nothing differs but entries: regular files named
    `*.gpg`, with no component of their path starting with a dot. No `.gpg-id`, nothing under
    `.extensions/`, no symbolic link.

  When it is not a fast-forward, publish (`pass sync push`) and let the member merge first.
  Then publishes the new tip.

## What this gives, and what it does not

- The drop zone sees two files, their sizes and the times they change. No entry name, no history.
- Nobody who is not a device can add an entry, a recipient, or a commit: it takes a device's
  OpenPGP key to publish anything a reader accepts.
- A member cannot give anyone access, nor change what the owner runs (`.extensions/`).
  It can change or remove any entry, including one it cannot read: the owner sees that before
  merging, and git keeps the previous version.
- A device's OpenPGP key reads every published file, past ones included if someone kept them.
  Removing a device means removing it from the configuration of the others and from every
  `.gpg-id`, and changing the passwords it could read.
- Two devices publishing at the same moment cannot collide: each has its own file.
