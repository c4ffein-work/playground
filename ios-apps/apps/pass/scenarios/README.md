# Sync scenarios

The situations [SYNC.md](../SYNC.md) must get right, run through a real drop zone with real
devices of both kinds: `pass.py` (with `gpg`, `git`, `sftp`) and the phone's core
(`crates/pass-core`, through its `pass-device` example program). `run.py` plays every scenario on
every mix of the two.

```sh
sudo PASS_PY=/path/to/pass.py ./run.py            # everything
sudo PASS_PY=/path/to/pass.py ./run.py 07 12      # some scenarios
sudo PASS_PY=/path/to/pass.py ./run.py --kinds py,core,py
```

Root, because the drop zone is started from `../dropzone/entrypoint.sh` (see `../dropzone/test.sh`).

## The scenarios

Devices are `a`, `b`, `c`. "Syncs" means: publishes, and receives the others' files. "Takes" means
takes that device's update, with the choices given. Every scenario ends by checking that all
devices that took everything list the same entries, read the same contents where they are readers,
and report nothing stale and nothing left to take.

| # | Situation | Expected |
| :-- | :-- | :-- |
| 01 | `a` creates a store with one entry; `b` adopts it | `b` reads the entry |
| 02 | `a` adds, `b` takes; `b` adds, `a` takes | both have both: moving forward, no merge |
| 03 | `a` and `b` each add a different entry, then take each other | both have both |
| 04 | `a` and `b` change the same entry differently | `conflict`; taking without a choice changes nothing; with `--mine` on `a` then `b` taking `a`: `a`'s version everywhere |
| 05 | same, resolved with `--theirs` | `b`'s version everywhere |
| 06 | `a` removes an entry, `b` changes it | `conflict`; each choice gives its outcome |
| 07 | `a` moves an entry, `b` changes its content | no conflict: the new content under the new name |
| 08 | `a` moves a folder of three entries, `b` adds an entry in the old folder and changes one of the three | the three under the new folder, one with the new content; the added one where `b` put it |
| 09 | `a` and `b` move the same entry to different names | `conflict`; the chosen name only |
| 10 | `a` and `b` both make the same change (same new content, typed on each) | no conflict |
| 11 | `a` and `b` each add an entry of the same name with different contents | `conflict` |
| 12 | `a` moves `x` to `y`; `b` adds a new `y` | `conflict` on `y`. Mine: `y` is the moved entry. Theirs: `y` is `b`'s, and the moved one is back at `x`: neither is lost to the other |
| 13 | `shared/` gets its own readers `a b`, the root stays `a`; `b` syncs | `b` reads `shared/*`, lists the rest and cannot read it |
| 14 | `b`, a reader of `shared/` only, adds an entry at the root | `a` reads it; `b` cannot read what it wrote |
| 15 | grant: `a` adds `c` to the readers of `shared/` and re-encrypts | `c` reads `shared/*` after taking; nothing stale |
| 16 | request: `b` adds itself to the readers of `work/`, which it cannot read | `a` is shown `readers work/` and the entries as `stale`; after `a` takes, `b` takes back and reads `work/*` |
| 17 | request left alone: as 16, but `a` does not take | `b` still reads nothing of `work/` |
| 18 | `a` removes `b` from `shared/` and re-encrypts; `b` takes | `b` no longer reads the current `shared/*` |
| 19 | `shared/` is read by `a b c`; `a` makes it `a b`, `b` makes it `a c` | `conflict` on `shared/` only (`a` reads both re-encryptions of the entries); the chosen set everywhere, and the entries encrypted for it |
| 20 | `a` moves an entry from `private/` (readers `a`) to `shared/` (readers `a b`) | `b` reads it after taking; it is re-encrypted for `shared/` |
| 21 | `b` changes an entry in `shared/` while `a` changes the readers of `shared/` to `a b c` | after everyone took: the entry has `b`'s content and is encrypted for `a b c` |
| 22 | three devices: `a` and `b` and `c` each add an entry; taken in a ring (`b` takes `a`, `c` takes `b`, `a` takes `c`), then once more | all three have all three |
| 23 | three devices, a relay of changes: `c` only ever takes from `b`, never from `a` | `a`'s changes reach `c` through `b` |
| 24 | a `.gpg-id` that names a key that is no device's | the update is refused whole; nothing changes |
| 25 | an entry encrypted to a key that is no device's | refused whole |
| 26 | a tip holding `.extensions/x.bash`, or a symbolic link named `x.gpg`, or a changed `.gitattributes` | refused whole; and the device that has it refuses to publish it |
| 27 | a file signed by another device's key, a bundle with two references, a truncated file, an old file served again | refused or ignored; nothing changes |
| 28 | `a` drops its changes and goes back to `b`'s tip after `c` already took them | `c` still has them; `a` gets them back from `c` like any other change |
| 29 | both sides re-encrypt the same stale entry on their own, then take each other | no conflict for devices that read it |
| 30 | a long run: twenty rounds of random adds, changes, moves and removals on three devices, each followed by takes with `--mine` wherever a choice is needed | no crash, no refusal, and the final check |

## The `pass-device` program

What `run.py` needs from the phone's core: `cargo run -p pass-core --example pass-device -- <command>`,
its state (store, keys, settings) under `$PASS_DEVICE_HOME`. Output on stdout, one item per line;
on failure a message on stderr and a non-zero status, having changed nothing.

```
init NAME --dropzone sftp://drop@HOST:PORT --host-key SHA256:... --ssh-key PRIVATE_KEY_FILE
                              makes the device and its OpenPGP key; prints the fingerprint
public-key                    the armored public key
peer NAME PUBLIC_KEY_FILE     another device
create DEVICE...              a new store whose root readers are these devices (this one's name included or not)
insert NAME CONTENT           add or change an entry
show NAME                     its content
rm NAME                       remove an entry, or a folder with a trailing slash
mv FROM TO                    move an entry, or a folder with trailing slashes
ls                            every entry name, sorted
readers DIR                   the readers that govern a folder, its own or inherited (trailing slash, / for the root): device names, sorted
readers-file DIR              the readers named by the folder's own .gpg-id; fails when it has none
set-readers DIR DEVICE...     change them, re-encrypting what this device reads; "set-readers DIR" alone removes the file
stale                         the stale entries, sorted
sync                          publish, and receive every other device's file: one line per device,
                              "NAME: N to take" | "NAME: nothing to take" | "NAME: REFUSED ..." | "NAME: IGNORED ..." | "NAME: nothing published"
                              (an empty store publishes nothing, without complaint; a store that cannot be
                              published still receives, then fails with the reason)
pending DEVICE                what taking its update would do: the lines of SYNC.md
take DEVICE [--mine NAME]... [--theirs NAME]...
drop DEVICE                   forget this device's own changes: go to DEVICE's tip
```

For `pass.py` the runner uses `pass insert / show / rm / mv / ls`, `pass init -p`, and `pass sync`.
