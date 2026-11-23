//! Against the real tools: `cargo test -p pass-core -- --ignored` (`make e2e` does it).
//! Needs `gpg`, `git`, `python3`, pass.py (`PASS_PY=/path/to/pass.py`, by default
//! `../clones/pass/pass.py` beside `ios-apps`), and for the sync tests: root, OpenSSH
//! (`sshd`, `sftp`, `ssh-keygen`, `ssh-keyscan`) and an account named `drop`, which is
//! what `apps/pass/dropzone/entrypoint.sh` needs. Each test makes its own GnuPG home,
//! keys, repositories and drop zone, and uses nothing of the machine's.
//!
//! The sync tests are the situations of `apps/pass/scenarios/README.md` (their numbers are
//! in the comments), played by devices of this crate; `scenarios/run.py` plays the same ones
//! with `pass.py` mixed in. What only a device gone wrong would publish is made here with
//! `git` and `gpg`.

mod support;

use std::path::Path;
use std::sync::Arc;

use pass_core::{
    generate_device_key, generate_openpgp_key, inspect_key, public_key_of, Access, EntryVersion, FetchOutcome,
    Identity, PassError, PassStore, ReviewLine, Side, SignatureStatus, TakeKind,
};
use support::{Fleet, Key, KeyKind, World, PASSPHRASE};

fn unlock(key: &Key) -> Arc<Identity> {
    Identity::unlock(key.secret.clone(), PASSPHRASE.into()).unwrap()
}

/// A device's view of a directory that pass.py made: `me` is the device (`phone`), `other` another one (`pc`).
fn open(world: &World, store: &Path, me: &Key, other: &Key) -> Arc<PassStore> {
    let mut config = world.config("unused", "phone", &me.public, vec![other.peer("pc")]);
    config.directory = store.to_string_lossy().into_owned();
    PassStore::open(config).unwrap()
}

// MARK: the store itself, with no sync

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn reads_a_store_made_by_gpg_and_pass() {
    for kind in [KeyKind::Rsa, KeyKind::Curve25519] {
        let world = World::new();
        let key = world.make_key(kind, "alice@example.org");
        let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
        let store = world.computer_store(&[&key]);
        world.pass(
            &store,
            &["insert", "-m", "web/été à la plage"],
            Some("pässwörd ✓\nurl: https://example.org/login\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n".as_bytes()),
        );
        world.pass(&store, &["generate", "web/generated", "40"], None);
        // `pass insert` with no option: pass.py stores it with no final newline.
        world.pass(&store, &["insert", "-e", "bare"], Some(b"just-this\n"));

        let phone = open(&world, &store, &key, &pc);
        assert_eq!(
            phone.list().unwrap(),
            ["Email/work", "bank", "bare", "web/generated", "web/été à la plage"]
        );
        assert_eq!(phone.search("WEB gen".into()).unwrap(), ["web/generated"]);
        let ids = |folder: &str| -> Vec<String> {
            let access = phone.folder_access(folder.into()).unwrap();
            access.readers.into_iter().map(|reader| reader.id).collect()
        };
        assert_eq!(ids("web/new"), std::slice::from_ref(&key.fingerprint));
        let root = phone.folder_readers("/".into()).unwrap().unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].device.as_deref(), Some("phone"));
        assert_eq!(
            phone.folder_readers("web".into()).unwrap(),
            None,
            "it has those of the folder above"
        );

        let identity = unlock(&key);
        let bank = phone.show("bank".into(), identity.clone()).unwrap();
        assert_eq!(bank.password, "hunter2");
        assert_eq!(
            (bank.fields[0].name.as_str(), bank.fields[0].value.as_str()),
            ("login", "alice")
        );
        assert_eq!(
            phone.show("bare".into(), identity.clone()).unwrap().password,
            "just-this"
        );
        let accented = phone.show("web/été à la plage".into(), identity.clone()).unwrap();
        assert_eq!(accented.password, "pässwörd ✓");
        assert_eq!(accented.otp_uris, ["otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"]);
        assert_eq!(accented.fields[0].value, "https://example.org/login");
        let generated = phone.show("web/generated".into(), identity.clone()).unwrap().password;
        assert_eq!(generated.len(), 40);
        assert_eq!(generated, world.gpg_decrypt(&store.join("web/generated.gpg")));

        assert!(matches!(
            phone.show("nope".into(), identity.clone()),
            Err(PassError::NotFound { .. })
        ));
        assert!(matches!(
            phone.show("../bank".into(), identity),
            Err(PassError::InvalidName { .. })
        ));
    }
}

/// Not through pass: what `gpg -e` writes when left to its defaults (compressed), when
/// told to use its own OCB packet (what GnuPG 2.4 does between keys it generated), and armored.
#[test]
#[ignore = "needs gpg"]
fn reads_what_gpg_writes_on_its_own() {
    for kind in [KeyKind::Rsa, KeyKind::Curve25519] {
        let world = World::new();
        let key = world.make_key(kind, "alice@example.org");
        let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
        let store = world.path("store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join(".gpg-id"), &key.fingerprint).unwrap();
        let text = "compress me ".repeat(500);
        let variants = [
            ("default", vec![]),
            ("ocb", vec!["--force-ocb"]),
            ("armored", vec!["--armor"]),
            ("uncompressed", vec!["--compress-algo=none"]),
            ("zlib-aes128", vec!["--compress-algo=zlib", "--cipher-algo=AES128"]),
        ];
        for (name, options) in &variants {
            let path = store.join(format!("{name}.gpg")).to_string_lossy().into_owned();
            let mut args = vec!["--encrypt", "--recipient", key.fingerprint.as_str(), "--output", &path];
            args.extend(options);
            world.gpg(&args, Some(text.as_bytes()));
        }
        let packets = world
            .gpg(&["--list-packets", &store.join("ocb.gpg").to_string_lossy()], None)
            .text();
        assert!(
            packets.contains("aead encrypted packet"),
            "gpg did not use OCB:\n{packets}"
        );

        let phone = open(&world, &store, &key, &pc);
        let identity = unlock(&key);
        for (name, _) in &variants {
            assert_eq!(
                phone.content(name.to_string(), identity.clone()).unwrap(),
                text,
                "{name}"
            );
        }
    }
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn what_it_writes_is_read_by_gpg_and_pass() {
    for kind in [KeyKind::Rsa, KeyKind::Curve25519] {
        let world = World::new();
        let key = world.make_key(kind, "alice@example.org");
        let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
        let store = world.computer_store(&[&key]);
        let phone = open(&world, &store, &key, &pc);
        let identity = unlock(&key);
        assert!(identity.can_sign());
        let commits_before = world.log(&store, "HEAD").len();

        phone
            .insert(
                "web/shop".into(),
                "tr0ub4dor&3\nlogin: alice".into(),
                false,
                identity.clone(),
            )
            .unwrap();
        assert!(matches!(
            phone.insert("web/shop".into(), "x".into(), false, identity.clone()),
            Err(PassError::AlreadyExists { .. })
        ));
        assert_eq!(
            world.gpg_decrypt(&store.join("web/shop.gpg")),
            "tr0ub4dor&3\nlogin: alice\n"
        );
        assert_eq!(
            world.pass(&store, &["show", "web/shop"], None),
            "tr0ub4dor&3\nlogin: alice\n\n"
        );
        assert!(world.pass(&store, &["ls"], None).contains("shop"));

        assert!(phone
            .edit("web/shop".into(), "tr0ub4dor&3\nlogin: bob\n".into(), identity.clone())
            .unwrap());
        assert!(
            !phone
                .edit("web/shop".into(), "tr0ub4dor&3\nlogin: bob".into(), identity.clone())
                .unwrap(),
            "unchanged: no commit"
        );
        assert!(phone
            .edit("notes/new".into(), "first\n".into(), identity.clone())
            .unwrap());
        let generated = phone
            .generate("web/generated".into(), 32, true, false, identity.clone())
            .unwrap();
        assert_eq!(generated.len(), 32);
        assert_eq!(
            world.gpg_decrypt(&store.join("web/generated.gpg")),
            format!("{generated}\n")
        );
        let plain = phone
            .generate("web/generated".into(), 12, false, true, identity.clone())
            .unwrap();
        assert!(plain.chars().all(|c| c.is_ascii_alphanumeric()));
        let replaced = phone.regenerate("web/shop".into(), 20, true, identity.clone()).unwrap();
        assert_eq!(
            world.gpg_decrypt(&store.join("web/shop.gpg")),
            format!("{replaced}\nlogin: bob\n")
        );
        phone
            .rename("web/shop".into(), "shops/corner".into(), false, identity.clone())
            .unwrap();
        assert!(!store.join("web/shop.gpg").exists());
        assert!(world
            .pass(&store, &["show", "shops/corner"], None)
            .starts_with(&replaced));
        phone.remove("notes/new".into(), identity.clone()).unwrap();
        assert!(
            !store.join("notes").exists(),
            "an emptied directory goes, as with pass rm"
        );
        assert!(matches!(
            phone.remove("notes/new".into(), identity.clone()),
            Err(PassError::NotFound { .. })
        ));

        // The history reads as if pass had written it, each commit holding its one change.
        let log = world.log(&store, "HEAD");
        assert_eq!(
            &log[..log.len() - commits_before],
            [
                "Remove notes/new from store.",
                "Rename web/shop to shops/corner.",
                "Replace generated password for web/shop.",
                "Add generated password for web/generated.",
                "Add generated password for web/generated.",
                "Add password for notes/new using pass-ios.",
                "Edit password for web/shop using pass-ios.",
                "Add given password for web/shop to store.",
            ]
        );
        assert_eq!(
            world.git(&store, &["show", "--format=", "--name-status", "HEAD~1"]),
            "R100\tweb/shop.gpg\tshops/corner.gpg"
        );
        // git itself finds the repository sound, the work tree clean, and the commits signed by the key.
        world.git(&store, &["fsck", "--strict", "--no-dangling"]);
        assert_eq!(world.git(&store, &["status", "--porcelain"]), "");
        let verified = world.try_git(&store, &["verify-commit", "--raw", "HEAD"]);
        assert!(
            verified.ok && verified.stderr.contains("VALIDSIG"),
            "{}",
            verified.stderr
        );
        assert!(verified.stderr.contains(&key.fingerprint));
        // And pass goes on from there.
        world.pass(&store, &["insert", "-m", "after"], Some(b"x\n"));
        assert_eq!(world.log(&store, "HEAD")[0], "Add given password for after to store.");
        assert!(phone.list().unwrap().contains(&"after".to_string()));
    }
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn each_directory_has_its_recipients() {
    let world = World::new();
    let alice = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let pc = world.make_key(KeyKind::Rsa, "pc@example.org");
    let carol = world.make_key(KeyKind::Curve25519, "carol@example.org");
    let store = world.computer_store(&[&alice]);
    // By fingerprint and by email: both are things people put in a .gpg-id.
    world.pass(
        &store,
        &["init", "-p", "team", &alice.fingerprint, "pc@example.org"],
        None,
    );
    world.pass(&store, &["insert", "-m", "team/wifi"], Some(b"shared\n"));
    let both = world.encrypted_to(&store.join("team/wifi.gpg"));
    assert_eq!(both.len(), 2);

    let identity = unlock(&alice);
    let phone = open(&world, &store, &alice, &pc);
    let ids: Vec<String> = phone
        .folder_access("team/deep/er".into())
        .unwrap()
        .readers
        .into_iter()
        .map(|reader| reader.id)
        .collect();
    assert_eq!(ids, [alice.fingerprint.clone(), "pc@example.org".to_string()]);

    // Who can read what, folder by folder: the root is the phone's alone, `team` is shared.
    let devices = |access: &Access| access.readers.iter().map(|r| r.device.clone()).collect::<Vec<_>>();
    let root = phone.folder_access("".into()).unwrap();
    assert_eq!(
        (root.gpg_id.as_str(), devices(&root)),
        (".gpg-id", vec![Some("phone".to_string())])
    );
    assert!(root.listed && root.readable && !root.stale && root.encrypted_to.is_empty());
    let team = phone.folder_access("team/deep".into()).unwrap();
    assert_eq!(team.gpg_id, "team/.gpg-id");
    assert_eq!(devices(&team), [Some("phone".to_string()), Some("pc".to_string())]);
    assert_eq!(
        team.readers[1].id, "pc@example.org",
        "the line as it is written, and whose it is"
    );
    let wifi = phone.entry_access("team/wifi".into()).unwrap();
    assert_eq!(wifi.encrypted_to, ["pc", "phone"]);
    assert!(wifi.readable && !wifi.stale && wifi.listed);
    let bank = phone.entry_access("bank".into()).unwrap();
    assert_eq!(
        (bank.encrypted_to.clone(), bank.readable, !bank.stale),
        (vec!["phone".to_string()], true, true)
    );
    assert!(phone.unreadable().unwrap().is_empty());
    // The other device: listed in `team` only, and the root's entries are locked to it. No key needed to know.
    let other = open(&world, &store, &pc, &alice);
    assert_eq!(other.unreadable().unwrap(), ["Email/work", "bank"]);
    let bank = other.entry_access("bank".into()).unwrap();
    assert!(!bank.readable && !bank.listed && !bank.stale);
    assert!(other.entry_access("team/wifi".into()).unwrap().readable);
    assert!(matches!(
        other.entry_access("nope".into()),
        Err(PassError::NotFound { .. })
    ));
    // A `.gpg-id` that changed without its files being encrypted again: the entry says so.
    std::fs::write(store.join("team/.gpg-id"), format!("{}\n", alice.fingerprint)).unwrap();
    let stale = phone.entry_access("team/wifi".into()).unwrap();
    assert_eq!(
        (devices(&stale), !stale.stale),
        (vec![Some("phone".to_string())], false)
    );
    assert_eq!(
        stale.encrypted_to,
        ["pc", "phone"],
        "it is still readable by pc, whatever the folder now says"
    );
    std::fs::write(
        store.join("team/.gpg-id"),
        format!("{}\npc@example.org\n", alice.fingerprint),
    )
    .unwrap();
    phone
        .insert("team/door".into(), "1234".into(), false, identity.clone())
        .unwrap();
    assert_eq!(
        world.encrypted_to(&store.join("team/door.gpg")),
        both,
        "to the same two keys as gpg"
    );
    assert_eq!(world.gpg_decrypt(&store.join("team/door.gpg")), "1234\n");
    // The other device alone reads it too, and not what is not for it.
    let other = open(&world, &store, &pc, &alice);
    assert_eq!(other.show("team/door".into(), unlock(&pc)).unwrap().password, "1234");
    assert!(matches!(
        other.show("bank".into(), unlock(&pc)),
        Err(PassError::NotForThisKey)
    ));

    // Moving into the shared directory re-encrypts to its recipients; a plain rename does not touch the file.
    let before = std::fs::read(store.join("bank.gpg")).unwrap();
    phone
        .rename("bank".into(), "money".into(), false, identity.clone())
        .unwrap();
    assert_eq!(std::fs::read(store.join("money.gpg")).unwrap(), before);
    phone
        .rename("money".into(), "team/money".into(), false, identity.clone())
        .unwrap();
    assert_eq!(world.encrypted_to(&store.join("team/money.gpg")), both);
    assert!(world
        .gpg_decrypt(&store.join("team/money.gpg"))
        .starts_with("hunter2\n"));

    // A recipient that is no device: the phone has no key for it, cannot write there, and says so
    // rather than leave it out.
    world.pass(
        &store,
        &["init", "-p", "family", &alice.fingerprint, &carol.fingerprint],
        None,
    );
    let family = phone.folder_access("family".into()).unwrap();
    assert_eq!(
        devices(&family),
        [Some("phone".to_string()), None],
        "a key that is no device's"
    );
    assert_eq!(
        phone.insert("family/x".into(), "x".into(), false, identity.clone()),
        Err(PassError::UnknownRecipient {
            id: format!("family/.gpg-id: {}", carol.fingerprint)
        })
    );
    assert!(!store.join("family/x.gpg").exists());
    phone.insert("x".into(), "x".into(), false, identity).unwrap();
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn keys_and_passphrases() {
    let world = World::new();
    let alice = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
    let store = world.computer_store(&[&alice]);

    let info = inspect_key(alice.secret.clone()).unwrap();
    assert_eq!(info.fingerprint, alice.fingerprint);
    assert_eq!(info.user_ids, ["Test alice@example.org <alice@example.org>"]);
    assert!(info.is_secret && info.needs_passphrase);
    let public = inspect_key(alice.public.clone()).unwrap();
    assert!(!public.is_secret && public.fingerprint == alice.fingerprint);
    assert_eq!(
        inspect_key(public_key_of(alice.secret.clone()).unwrap())
            .unwrap()
            .fingerprint,
        alice.fingerprint
    );
    assert!(matches!(
        Identity::unlock(alice.secret.clone(), "wrong".into()),
        Err(PassError::WrongPassphrase)
    ));
    assert!(matches!(
        Identity::unlock(alice.public.clone(), PASSPHRASE.into()),
        Err(PassError::InvalidKey { .. })
    ));
    assert!(matches!(
        Identity::unlock("not a key".into(), PASSPHRASE.into()),
        Err(PassError::InvalidKey { .. })
    ));

    // `gpg --export-secret-subkeys`: the primary key stays on the computer. Enough to read;
    // not to write, since the owner only takes signed commits and the signing key is not here.
    let subkeys_only = world
        .gpg(&["--armor", "--export-secret-subkeys", &alice.fingerprint], None)
        .text();
    let identity = Identity::unlock(subkeys_only, PASSPHRASE.into()).unwrap();
    assert!(!identity.can_sign());
    let phone = open(&world, &store, &alice, &pc);
    assert_eq!(phone.show("bank".into(), identity.clone()).unwrap().password, "hunter2");
    assert_eq!(
        phone.insert("unsigned".into(), "x".into(), false, identity),
        Err(PassError::CannotSign)
    );
    assert!(!store.join("unsigned.gpg").exists());
    // And not with another key than the one the device is set up with.
    assert!(matches!(
        phone.insert("x".into(), "x".into(), false, unlock(&pc)),
        Err(PassError::InvalidKey { .. })
    ));

    // What the configuration refuses: a key that is not the one with the fingerprint, a name
    // used twice or that is no device name, the same key under two names.
    let config = |peers| world.config("unused", "phone", &alice.public, peers);
    let mut wrong_key = pc.peer("pc");
    wrong_key.public_key = alice.public.clone();
    for bad in [
        vec![wrong_key],
        vec![pc.peer("phone")],
        vec![pc.peer("My PC")],
        vec![pc.peer("pc"), pc.peer("laptop")],
        vec![alice.peer("pc")],
    ] {
        assert!(matches!(
            PassStore::open(config(bad)),
            Err(PassError::InvalidConfig { .. })
        ));
    }
    assert!(PassStore::open(config(vec![])).is_ok(), "a device alone is a device");
}

/// A key made on the phone: gpg takes its public half, encrypts to it and verifies what it signs.
#[test]
#[ignore = "needs gpg"]
fn a_key_made_on_the_phone_is_one_gpg_knows_how_to_use() {
    let world = World::new();
    let made = generate_openpgp_key("phone <phone@example.org>".into(), PASSPHRASE.into()).unwrap();
    let info = inspect_key(made.secret_key.clone()).unwrap();
    assert_eq!(info.fingerprint, made.fingerprint);
    assert_eq!(info.user_ids, ["phone <phone@example.org>"]);
    assert!(info.is_secret && info.needs_passphrase);
    assert_eq!(
        inspect_key(made.public_key.clone()).unwrap().fingerprint,
        made.fingerprint
    );
    assert!(!made.public_key.contains("PRIVATE"));
    assert!(matches!(
        Identity::unlock(made.secret_key.clone(), "wrong".into()),
        Err(PassError::WrongPassphrase)
    ));
    let identity = Identity::unlock(made.secret_key.clone(), PASSPHRASE.into()).unwrap();
    assert!(identity.can_sign());
    let bare = generate_openpgp_key("no passphrase".into(), String::new()).unwrap();
    assert!(!inspect_key(bare.secret_key.clone()).unwrap().needs_passphrase);
    assert!(Identity::unlock(bare.secret_key, String::new()).is_ok());
    assert_ne!(bare.fingerprint, made.fingerprint);
    assert!(generate_openpgp_key("  ".into(), String::new()).is_err());

    // gpg: imports it, lists it as ed25519 + cv25519 with the right uses, and encrypts to it.
    world.import_key(&made.public_key, &made.fingerprint);
    let listing = world
        .gpg(&["--with-colons", "--list-keys", &made.fingerprint], None)
        .text();
    let line = |kind: &str| listing.lines().find(|line| line.starts_with(kind)).unwrap().to_string();
    assert!(
        line("pub:").contains(":ed25519:") && line("pub:").contains(":scESC:"),
        "{listing}"
    );
    assert!(
        line("sub:").contains(":cv25519:") && line("sub:").contains(":e:"),
        "{listing}"
    );
    let store = world.path("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join(".gpg-id"), &made.fingerprint).unwrap();
    let file = store.join("from-gpg.gpg").to_string_lossy().into_owned();
    world.gpg(
        &["--encrypt", "--recipient", &made.fingerprint, "--output", &file],
        Some(b"for the phone\n"),
    );
    let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
    let mut config = world.config("unused", "phone", &made.public_key, vec![pc.peer("pc")]);
    config.directory = store.to_string_lossy().into_owned();
    let phone = PassStore::open(config).unwrap();
    assert_eq!(phone.content("from-gpg".into(), identity).unwrap(), "for the phone\n");
}

// MARK: the sync, through a real drop zone

fn no_choice(device: &str, other: &str, name: &str) -> Side {
    panic!("{device} needs a choice for {name} to take from {other}: none was expected")
}

fn content(version: EntryVersion) -> String {
    match version {
        EntryVersion::Content { text } => text,
        EntryVersion::Absent => "(absent)".into(),
        EntryVersion::Unreadable { .. } => "(unreadable)".into(),
    }
}

/// Scenarios 01, 02, 03: adopting a store, moving forward both ways, then a merge of two additions.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn a_store_is_adopted_then_changes_go_both_ways() {
    let fleet = Fleet::new(&["a", "b"]);
    let (a, b) = (fleet.device("a"), fleet.device("b"));
    let world = &fleet.world;
    // Nobody has a store: nothing is published, and that is no error.
    assert_eq!(a.sync(), ["b: nothing published"]);
    assert_eq!(std::fs::read_dir(fleet.zone.files()).unwrap().count(), 0);

    a.store
        .create(vec!["a".into(), "b".into()], a.identity.clone())
        .unwrap();
    a.insert("one", "first");
    assert_eq!(a.sync(), ["b: nothing published"]);

    // 01. b receives, and takes nothing by itself.
    assert_eq!(b.sync(), ["a: 2 to take"]);
    let update = b.store.pending_update("a".into(), b.identity.clone()).unwrap().unwrap();
    assert_eq!((update.device.as_str(), update.kind), ("a", TakeKind::Adopt));
    assert_eq!(update.commit, world.git(&a.dir, &["rev-parse", "HEAD"]));
    assert_eq!(update.commit_count, 2);
    assert_eq!(update.commits[0].subject, "Add given password for one to store.");
    assert_eq!(update.commits[0].author, "a <a@pass-sync.invalid>");
    assert_eq!(
        update.commits[0].signature,
        SignatureStatus::Valid { device: "a".into() }
    );
    assert_eq!(
        update.lines,
        [
            ReviewLine::Added { name: "one".into() },
            ReviewLine::Readers {
                folder: "/".into(),
                devices: vec!["a".into(), "b".into()],
                inherited: false
            },
        ]
    );
    assert_eq!(update.reencrypts, 0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!((now - update.published_at).abs() < 300, "the time a signed its file");
    assert!(b.ls().is_empty(), "nothing is applied before it is taken");
    assert!(!b.store.sync_state().unwrap().has_store);
    // What it would bring can be read first.
    let versions = b
        .store
        .update_versions("a".into(), "one".into(), b.identity.clone())
        .unwrap();
    assert_eq!(
        (versions.base, versions.mine),
        (EntryVersion::Absent, EntryVersion::Absent)
    );
    assert_eq!(content(versions.theirs), "first\n");
    // Only the update that was shown is taken.
    assert_eq!(
        b.store
            .take_update("a".into(), Some("0".repeat(40)), vec![], b.identity.clone()),
        Err(PassError::StaleUpdate)
    );
    let taken = b
        .store
        .take_update("a".into(), Some(update.commit.clone()), vec![], b.identity.clone())
        .unwrap();
    assert_eq!((taken.kind, taken.changed), (TakeKind::Adopt, vec!["one".to_string()]));
    assert_eq!(b.show("one").unwrap(), "first");
    assert_eq!(world.log(&b.dir, "HEAD"), world.log(&a.dir, "HEAD"));
    assert_eq!(b.sync(), ["a: nothing to take"]);
    // One file per device in the drop zone, and nothing half-written.
    let mut files: Vec<String> = std::fs::read_dir(fleet.zone.files())
        .unwrap()
        .map(|file| file.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(files, ["a.bundle.gpg", "b.bundle.gpg"]);

    // 02. One adds, the other takes; and back: moving forward, no merge.
    a.insert("two", "from-a");
    a.sync();
    assert_eq!(b.stands_with("a"), "1 to take");
    assert_eq!(b.pending("a"), ["added two"]);
    assert_eq!(b.take("a", &[], &[]).kind, TakeKind::Forward);
    b.insert("three", "from-b");
    b.sync();
    assert_eq!(a.stands_with("b"), "1 to take");
    assert_eq!(a.take("b", &[], &[]).kind, TakeKind::Forward);
    assert_eq!(world.git(&a.dir, &["rev-list", "--merges", "--count", "HEAD"]), "0");

    // 03. Each adds a different entry: a merge on the first to take, signed, its parents mine then theirs.
    a.insert("a-only", "A");
    b.insert("b-only", "B");
    a.sync();
    b.sync();
    let mine_before = world.git(&a.dir, &["rev-parse", "HEAD"]);
    assert_eq!(a.stands_with("b"), "1 to take");
    assert_eq!(a.pending("b"), ["added b-only"]);
    assert_eq!(a.take("b", &[], &[]).kind, TakeKind::Merge);
    world.import_key(&a.key.public_key, &a.key.fingerprint);
    let merge = world.git(&a.dir, &["log", "-1", "--format=%s|%P|%G?"]);
    let parts: Vec<&str> = merge.split('|').collect();
    assert_eq!(parts[0], "Merge the changes of b into those of a.");
    assert_eq!(
        parts[1],
        format!("{mine_before} {}", world.git(&b.dir, &["rev-parse", "HEAD"]))
    );
    assert_eq!(parts[2], "G", "signed like its other commits");
    // The other one then only has to move forward onto it.
    assert_eq!(b.stands_with("a"), "2 to take");
    assert_eq!(b.take("a", &[], &[]).kind, TakeKind::Forward);
    fleet.settle(&no_choice);
    fleet.check(&[
        ("one", "first"),
        ("two", "from-a"),
        ("three", "from-b"),
        ("a-only", "A"),
        ("b-only", "B"),
    ]);
}

/// Scenarios 04, 05, 06, 09, 11, 12: what both sides did differently is a choice, mine or theirs, whole.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn what_both_changed_differently_needs_a_choice() {
    use support::Device;
    struct Case {
        what: &'static str,
        act: fn(&Device, &Device),
        /// What a is shown of b's update.
        shown: &'static [&'static str],
        choice: &'static str,
        mine: &'static [(&'static str, &'static str)],
        theirs: &'static [(&'static str, &'static str)],
    }
    let cases = [
        Case {
            what: "04, 05: two contents",
            act: |a, b| {
                a.insert("seed", "A");
                b.insert("seed", "B");
            },
            shown: &["changed seed", "conflict seed"],
            choice: "seed",
            mine: &[("seed", "A")],
            theirs: &[("seed", "B")],
        },
        Case {
            what: "06: removed here, changed there",
            act: |a, b| {
                a.rm("seed");
                b.insert("seed", "B");
            },
            shown: &["changed seed", "conflict seed"],
            choice: "seed",
            mine: &[],
            theirs: &[("seed", "B")],
        },
        Case {
            what: "06: changed here, removed there",
            act: |a, b| {
                a.insert("seed", "A");
                b.rm("seed");
            },
            shown: &["conflict seed", "removed seed"],
            choice: "seed",
            mine: &[("seed", "A")],
            theirs: &[],
        },
        Case {
            what: "09: two names",
            act: |a, b| {
                a.mv("seed", "pa");
                b.mv("seed", "pb");
            },
            shown: &["conflict pa", "moved seed -> pb"],
            choice: "pa",
            mine: &[("pa", "s")],
            theirs: &[("pb", "s")],
        },
        Case {
            what: "11: two new entries of one name",
            act: |a, b| {
                a.insert("new", "A");
                b.insert("new", "B");
            },
            shown: &["added new", "conflict new"],
            choice: "new",
            mine: &[("seed", "s"), ("new", "A")],
            theirs: &[("seed", "s"), ("new", "B")],
        },
        Case {
            what: "12: moved onto a name the other side added",
            act: |a, b| {
                a.mv("seed", "y");
                b.insert("y", "B");
            },
            shown: &["added y", "conflict y"],
            choice: "y",
            mine: &[("y", "s")],
            // Their state of both entries involved: the seed back where they have it, and their y.
            theirs: &[("seed", "s"), ("y", "B")],
        },
    ];
    for case in &cases {
        for keep in [Side::Mine, Side::Theirs] {
            let fleet = Fleet::started(&["a", "b"], None, &[("seed", "s")]);
            let (a, b) = (fleet.device("a"), fleet.device("b"));
            (case.act)(a, b);
            a.sync();
            b.sync();
            assert_ne!(a.stands_with("b"), "nothing to take", "{}", case.what);
            assert_eq!(a.pending("b"), case.shown, "{}", case.what);
            // Both versions can be seen before choosing, each side followed through its moves.
            let versions = a
                .store
                .update_versions("b".into(), case.choice.into(), a.identity.clone())
                .unwrap();
            let of = |name: &str, side: &[(&str, &str)]| {
                side.iter()
                    .find(|(entry, _)| *entry == name || case.what.starts_with("09"))
                    .map(|(_, text)| format!("{text}\n"))
                    .unwrap_or("(absent)".into())
            };
            if !case.what.starts_with("12") {
                assert_eq!(content(versions.mine), of(case.choice, case.mine), "{}", case.what);
                assert_eq!(content(versions.theirs), of(case.choice, case.theirs), "{}", case.what);
            }
            // Without the choice nothing is taken.
            let before = (a.ls(), a.store.sync_state().unwrap());
            assert_eq!(
                a.try_take("b", &[], &[]),
                Err(PassError::ChoiceRequired {
                    names: vec![case.choice.to_string()]
                }),
                "{}",
                case.what
            );
            assert_eq!((a.ls(), a.store.sync_state().unwrap()), before, "{}", case.what);
            let (mine, theirs): (&[&str], &[&str]) = match keep {
                Side::Mine => (&[case.choice], &[]),
                Side::Theirs => (&[], &[case.choice]),
            };
            assert_eq!(a.take("b", mine, theirs).kind, TakeKind::Merge);
            // And the choice is the same everywhere once the other has taken it back.
            fleet.settle(&no_choice);
            fleet.check(if keep == Side::Mine { case.mine } else { case.theirs });
        }
    }
}

/// Scenarios 07, 08, 10: what the two sides did fits together, and nobody is asked anything.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn what_fits_together_needs_none() {
    // 07. One moves an entry, the other changes its content: the new content under the new name.
    let fleet = Fleet::started(&["a", "b"], None, &[("seed", "s")]);
    let (a, b) = (fleet.device("a"), fleet.device("b"));
    a.mv("seed", "moved");
    b.insert("seed", "new");
    a.sync();
    b.sync();
    assert_eq!(b.pending("a"), ["moved seed -> moved"]);
    assert_eq!(a.stands_with("b"), "1 to take");
    assert_eq!(a.pending("b"), ["changed seed"]);
    fleet.settle(&no_choice);
    fleet.check(&[("moved", "new")]);

    // 08. One moves a folder of three, the other adds an entry in the old folder and changes one of the three.
    let fleet = Fleet::started(&["a", "b"], None, &[("old/x", "1"), ("old/y", "2"), ("old/z", "3")]);
    let (a, b) = (fleet.device("a"), fleet.device("b"));
    a.store
        .rename_folder("old/".into(), "new/".into(), a.identity.clone())
        .unwrap();
    b.insert("old/w", "W");
    b.insert("old/y", "Y");
    a.sync();
    b.sync();
    assert_eq!(
        b.pending("a"),
        ["moved old/x -> new/x", "moved old/y -> new/y", "moved old/z -> new/z"]
    );
    fleet.settle(&no_choice);
    fleet.check(&[("new/x", "1"), ("new/y", "Y"), ("new/z", "3"), ("old/w", "W")]);

    // 10. Both make the same change, typed on each: two encryptions of one content.
    let fleet = Fleet::started(&["a", "b"], None, &[("seed", "s")]);
    let (a, b) = (fleet.device("a"), fleet.device("b"));
    a.insert("seed", "same");
    b.insert("seed", "same");
    b.insert("also", "typed twice");
    a.insert("also", "typed twice");
    a.sync();
    b.sync();
    assert_ne!(
        std::fs::read(a.dir.join("seed.gpg")).unwrap(),
        std::fs::read(b.dir.join("seed.gpg")).unwrap()
    );
    assert_eq!(a.stands_with("b"), "2 to take");
    assert_eq!(a.pending("b"), ["added also", "changed seed"], "told, with no conflict");
    fleet.settle(&no_choice);
    fleet.check(&[("seed", "same"), ("also", "typed twice")]);
}

/// Scenarios 13, 14, 15, 18, 20, 21: folders with their own readers.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn each_folder_is_read_by_those_it_names() {
    // The root read by a alone, shared/ by a and b.
    let fleet = Fleet::new(&["a", "b", "c"]);
    let (a, b, c) = (fleet.device("a"), fleet.device("b"), fleet.device("c"));
    a.store.create(vec!["a".into()], a.identity.clone()).unwrap();
    a.insert("private", "p");
    a.insert("shared/s1", "1");
    a.set_readers("shared/", &["a", "b"]);
    assert_eq!(
        fleet.world.log(&a.dir, "HEAD")[..2],
        ["Reencrypt for the readers of shared/.", "Set GPG id to a, b (shared)."]
    );
    assert_eq!(
        std::fs::read_to_string(a.dir.join("shared/.gpg-id")).unwrap(),
        format!("{}\n{}\n", a.key.fingerprint, b.key.fingerprint),
        "full fingerprints"
    );
    fleet.settle(&no_choice);

    // 13. b reads shared/, lists the rest and cannot read it; c reads nothing, and knows it without a key.
    assert_eq!(b.readers("shared/"), ["a", "b"]);
    assert_eq!(b.readers("/"), ["a"]);
    assert_eq!(b.store.unreadable().unwrap(), ["private"]);
    assert_eq!(c.store.unreadable().unwrap(), ["private", "shared/s1"]);
    let access: Access = b.store.entry_access("shared/s1".into()).unwrap();
    assert_eq!(access.encrypted_to, ["a", "b"]);
    assert!(access.readable && access.listed && !access.stale);
    let access = b.store.entry_access("private".into()).unwrap();
    assert!(!access.readable && !access.listed && !access.stale);
    fleet.check(&[("private", "p"), ("shared/s1", "1")]);

    // 14. b writes where it does not read: for the readers of that folder, which it is not.
    b.insert("written-blind", "x");
    assert_eq!(b.show("written-blind"), Err(PassError::NotForThisKey));
    fleet.settle(&no_choice);
    assert_eq!(a.show("written-blind").unwrap(), "x");

    // 15. A grant: a adds c to the readers of shared/ and encrypts again.
    assert_eq!(c.show("shared/s1"), Err(PassError::NotForThisKey));
    a.set_readers("shared/", &["a", "b", "c"]);
    a.sync();
    assert_ne!(c.stands_with("a"), "nothing to take");
    // (c could not read the entry before: to it the new file is a change, not a re-encryption.)
    assert_eq!(c.pending("a"), ["changed shared/s1", "readers shared/: a b c"]);
    assert_eq!(b.stands_with("a"), "2 to take");
    assert_eq!(
        b.pending("a"),
        ["readers shared/: a b c"],
        "b reads both: the same entry"
    );
    fleet.settle(&no_choice);
    assert_eq!(c.show("shared/s1").unwrap(), "1");

    // 21. b changes an entry of shared/ while a changes its readers: b's content, a's readers.
    b.insert("shared/s1", "B");
    a.set_readers("shared/", &["a", "b"]);
    fleet.settle(&no_choice);
    assert_eq!(
        a.store.entry_access("shared/s1".into()).unwrap().encrypted_to,
        ["a", "b"]
    );
    assert_eq!(
        c.show("shared/s1"),
        Err(PassError::NotForThisKey),
        "18: c no longer reads what is there now"
    );
    fleet.check(&[("private", "p"), ("shared/s1", "B"), ("written-blind", "x")]);

    // 20. An entry moved to a folder with other readers is encrypted for them.
    a.mv("private", "shared/was-private");
    fleet.settle(&no_choice);
    assert_eq!(b.show("shared/was-private").unwrap(), "p");
    // Removing a folder's own readers gives it those of the folder above.
    a.set_readers("shared/", &[]);
    a.sync();
    assert_eq!(b.stands_with("a"), "2 to take");
    assert!(b.pending("a").contains(&"readers shared/: inherited".to_string()));
    fleet.settle(&no_choice);
    assert_eq!(b.readers("shared/"), ["a"]);
    assert_eq!(b.store.folder_readers("shared/".into()).unwrap(), None);
    fleet.check(&[("shared/was-private", "p"), ("shared/s1", "B"), ("written-blind", "x")]);
    // The root always names its readers.
    assert!(matches!(
        a.store.set_readers("/".into(), vec![], a.identity.clone()),
        Err(PassError::InvalidConfig { .. })
    ));
    assert!(matches!(
        a.store
            .set_readers("x/".into(), vec!["nobody".into()], a.identity.clone()),
        Err(PassError::UnknownDevice { .. })
    ));
}

/// Scenarios 16, 17: naming oneself where one does not read is asking, not getting.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn asking_to_read_is_not_reading() {
    let fleet = Fleet::new(&["a", "b"]);
    let (a, b) = (fleet.device("a"), fleet.device("b"));
    a.store
        .create(vec!["a".into(), "b".into()], a.identity.clone())
        .unwrap();
    a.insert("open", "o");
    a.insert("work/w1", "1");
    a.insert("work/w2", "2");
    a.set_readers("work/", &["a"]);
    fleet.settle(&no_choice);
    assert_eq!(b.show("work/w1"), Err(PassError::NotForThisKey));

    b.set_readers("work/", &["a", "b"]);
    assert_eq!(
        b.show("work/w1"),
        Err(PassError::NotForThisKey),
        "having asked changes nothing"
    );
    assert_eq!(b.store.stale().unwrap(), ["work/w1", "work/w2"]);
    assert!(b.store.entry_access("work/w1".into()).unwrap().stale);
    a.sync();
    b.sync();
    // 17. Left alone, it stays a request.
    assert_eq!(a.stands_with("b"), "1 to take");
    b.sync();
    assert_eq!(b.show("work/w1"), Err(PassError::NotForThisKey));
    assert_eq!(a.readers("work/"), ["a"]);

    // 16. a is shown it as what it is: the readers, and the entries that taking it would encrypt again.
    assert_eq!(a.pending("b"), ["readers work/: a b", "stale work/w1", "stale work/w2"]);
    let update = a.store.pending_update("b".into(), a.identity.clone()).unwrap().unwrap();
    assert_eq!((update.kind, update.reencrypts), (TakeKind::Forward, 2));
    let taken = a.take("b", &[], &[]);
    assert_eq!(taken.reencrypted, ["work/w1", "work/w2"]);
    assert_eq!(
        fleet.world.log(&a.dir, "HEAD")[0],
        "Reencrypt for the readers of work/."
    );
    assert_eq!(b.stands_with("a"), "1 to take");
    b.take("a", &[], &[]);
    assert_eq!(b.show("work/w1").unwrap(), "1");
    fleet.settle(&no_choice);
    fleet.check(&[("open", "o"), ("work/w1", "1"), ("work/w2", "2")]);
}

/// Scenario 19: the readers of one folder changed differently on both sides.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn two_sets_of_readers_for_one_folder_need_a_choice() {
    for (keep, expected) in [(Side::Mine, vec!["a", "b"]), (Side::Theirs, vec!["a", "c"])] {
        let fleet = Fleet::new(&["a", "b", "c"]);
        let (a, b) = (fleet.device("a"), fleet.device("b"));
        a.store
            .create(vec!["a".into(), "b".into(), "c".into()], a.identity.clone())
            .unwrap();
        a.insert("shared/s1", "1");
        a.set_readers("shared/", &["a", "b", "c"]);
        fleet.settle(&no_choice);
        a.set_readers("shared/", &["a", "b"]);
        b.set_readers("shared/", &["a", "c"]);
        b.insert("shared/s2", "2");
        a.sync();
        b.sync();
        assert_ne!(a.stands_with("b"), "nothing to take");
        let shown = a.pending("b");
        assert!(shown.contains(&"conflict shared/".to_string()), "{shown:?}");
        assert!(shown.contains(&"readers shared/: a c".to_string()), "{shown:?}");
        assert!(shown.contains(&"added shared/s2".to_string()), "{shown:?}");
        assert_eq!(
            a.try_take("b", &[], &[]),
            Err(PassError::ChoiceRequired {
                names: vec!["shared/".into()]
            })
        );
        assert_eq!(a.readers("shared/"), ["a", "b"], "nothing was taken");
        match keep {
            Side::Mine => a.take("b", &["shared/"], &[]),
            Side::Theirs => a.take("b", &[], &["shared/"]),
        };
        fleet.settle(&no_choice);
        for device in &fleet.devices {
            assert_eq!(device.readers("shared/"), expected, "on {}", device.name);
        }
        // The chosen set, and the entries encrypted for it: nothing stale, read by those and only those.
        fleet.check(&[("shared/s1", "1"), ("shared/s2", "2")]);
    }
}

/// Scenarios 22, 23, 28, 29: three devices.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn three_devices() {
    // 22. Each adds an entry; taken in a ring, then once more.
    let fleet = Fleet::started(&["a", "b", "c"], None, &[("seed", "s")]);
    let (a, b, c) = (fleet.device("a"), fleet.device("b"), fleet.device("c"));
    a.insert("from-a", "A");
    b.insert("from-b", "B");
    c.insert("from-c", "C");
    for device in &fleet.devices {
        device.sync();
    }
    for _ in 0..2 {
        for (device, from) in [(b, "a"), (c, "b"), (a, "c")] {
            device.sync();
            device.take(from, &[], &[]);
        }
    }
    fleet.settle(&no_choice);
    let all = [("seed", "s"), ("from-a", "A"), ("from-b", "B"), ("from-c", "C")];
    fleet.check(&all);

    // 23. A relay: c only ever takes from b, never from a. What a does reaches c through b.
    a.insert("relayed", "R");
    a.mv("from-a", "moved-by-a");
    let relay = |device: &str, other: &str| device != "c" || other == "b";
    fleet.settle_among(&no_choice, &relay);
    assert_eq!(c.show("relayed").unwrap(), "R");
    assert!(c.ls().contains(&"moved-by-a".to_string()));
    // c holds everything of a without ever having taken its file: there is nothing left to take from it either.
    assert_eq!(c.stands_with("a"), "nothing to take");

    // 28. a drops its changes and goes back to b's tip, after c already took them.
    a.insert("x", "1");
    a.sync();
    c.sync();
    c.take("a", &[], &[]);
    assert_eq!(a.store.drop_to("b".into()).unwrap(), 1);
    assert!(!a.ls().contains(&"x".to_string()));
    assert_eq!(a.store.drop_to("b".into()).unwrap(), 0);
    assert!(matches!(
        a.store.drop_to("nobody".into()),
        Err(PassError::UnknownDevice { .. })
    ));
    // c still has them, and a gets them back from c like any other change.
    fleet.settle(&no_choice);
    assert_eq!(a.show("x").unwrap(), "1");

    // 29. Two devices grant the same request on their own: two re-encryptions of the same entries.
    let fleet = Fleet::new(&["a", "b", "c"]);
    let (a, b, c) = (fleet.device("a"), fleet.device("b"), fleet.device("c"));
    a.store
        .create(vec!["a".into(), "b".into(), "c".into()], a.identity.clone())
        .unwrap();
    a.insert("shared/s1", "1");
    a.set_readers("shared/", &["a", "b"]);
    fleet.settle(&no_choice);
    c.set_readers("shared/", &["a", "b", "c"]);
    c.sync();
    for device in [a, b] {
        assert_eq!(device.stands_with("c"), "1 to take");
    }
    for device in [a, b] {
        assert_eq!(device.try_take("c", &[], &[]).unwrap().reencrypted, ["shared/s1"]);
    }
    a.sync();
    b.sync();
    assert_ne!(
        std::fs::read(a.dir.join("shared/s1.gpg")).unwrap(),
        std::fs::read(b.dir.join("shared/s1.gpg")).unwrap()
    );
    assert_eq!(a.stands_with("b"), "1 to take");
    assert_eq!(
        a.pending("b"),
        Vec::<String>::new(),
        "the same entry, encrypted twice: nothing to tell, no conflict"
    );
    fleet.settle(&no_choice);
    fleet.check(&[("shared/s1", "1")]);
}

/// A computer played with `git`, `gpg` and `sftp`: what publishes the files this crate must refuse, and one it must take.
struct Computer<'a> {
    fleet: &'a Fleet,
    key: Key,
    ssh: support::SshKey,
    /// Its repository: a clone of a's store.
    repo: std::path::PathBuf,
}

impl Computer<'_> {
    fn world(&self) -> &World {
        &self.fleet.world
    }

    fn git(&self, args: &[&str]) -> String {
        self.world().git(&self.repo, args)
    }

    /// A commit holding whatever is in the directory now.
    fn commit(&self, message: &str) {
        self.git(&["add", "--all", "--force"]);
        self.git(&["commit", "--quiet", "--allow-empty", "-m", message]);
    }

    /// A bundle of these references (its tip under its own name, when none is given).
    fn bundle(&self, references: &[&str]) -> std::path::PathBuf {
        self.git(&["update-ref", "refs/pass-sync/pc", "HEAD"]);
        let file = self.world().path("pc.bundle");
        let path = file.to_string_lossy().into_owned();
        let mut args = vec!["bundle", "create", "--quiet", "--version=2", path.as_str()];
        args.extend(if references.is_empty() {
            &["refs/pass-sync/pc"][..]
        } else {
            references
        });
        self.git(&args);
        file
    }

    /// `gpg --sign --encrypt` of a file, by `signer`, to `recipients` (fingerprints).
    fn seal(&self, file: &Path, signer: &str, recipients: &[&str], options: &[&str]) -> std::path::PathBuf {
        let sealed = self.world().path("pc.sealed");
        let (file, output) = (
            file.to_string_lossy().into_owned(),
            sealed.to_string_lossy().into_owned(),
        );
        let mut args = vec!["--local-user", signer, "--sign", "--encrypt", "--output", &output];
        for recipient in recipients {
            args.extend(["--recipient", recipient]);
        }
        args.extend(options);
        args.push(&file);
        self.world().gpg(&args, None);
        sealed
    }

    fn everyone(&self) -> Vec<&str> {
        let mut everyone: Vec<&str> = self
            .fleet
            .devices
            .iter()
            .map(|device| device.key.fingerprint.as_str())
            .collect();
        everyone.push(&self.key.fingerprint);
        everyone
    }

    /// Puts a file in the computer's place in the drop zone.
    fn put(&self, file: &Path) {
        self.fleet.zone.publish(self.world(), &self.ssh, file, "pc.bundle.gpg");
    }

    /// Publishes its tip as the protocol says, with these gpg options.
    fn publish(&self, options: &[&str]) {
        let bundle = self.bundle(&[]);
        self.put(&self.seal(&bundle, &self.key.fingerprint, &self.everyone(), options));
    }

    /// Back to the last good commit, then `change`, committed and published: a tip gone wrong.
    fn publish_wrong(&self, good: &str, change: impl FnOnce(&Path)) {
        self.git(&["reset", "--quiet", "--hard", good]);
        self.git(&["clean", "--quiet", "-fdx"]);
        change(&self.repo);
        self.commit("Something a synced store does not hold.");
        self.publish(&["--digest-algo", "SHA512", "--compress-algo", "none"]);
    }
}

/// Scenarios 24, 25, 26, 27, and everything else a drop zone could serve under a device's name
/// that is not that device's file, or not a store that syncs.
#[test]
#[ignore = "needs root, OpenSSH, gpg and git"]
fn what_is_not_a_devices_store_is_refused() {
    let world = World::new();
    let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
    let tablet = world.make_key(KeyKind::Curve25519, "tablet@example.org");
    let mallory = world.make_key(KeyKind::Curve25519, "mallory@example.org");
    let ssh = world.ssh_key("pc");
    let public = ssh.public.clone();
    let fleet = Fleet::with(world, &["a"], &[&public], &[pc.peer("pc"), tablet.peer("tablet")]);
    let a = fleet.device("a");
    let world = &fleet.world;
    world.import_key(&a.key.public_key, &a.key.fingerprint);
    a.store
        .create(vec!["a".into(), "pc".into()], a.identity.clone())
        .unwrap();
    a.insert("seed", "s");
    a.sync();

    // The computer adopts a's store (a clone of it stands for that) and adds an entry, as pass would.
    let repo = world.path("pc-repo");
    let mut clone = world.command("git");
    clone.args(["clone", "--quiet"]).arg(&a.dir).arg(&repo);
    world.run(clone, None);
    let computer = Computer {
        fleet: &fleet,
        key: pc.clone(),
        ssh,
        repo,
    };
    let encrypt_to = |recipients: &[&str], file: &Path, text: &str| {
        let output = file.to_string_lossy().into_owned();
        let mut args = vec!["--encrypt", "--output", output.as_str()];
        for recipient in recipients {
            args.extend(["--recipient", recipient]);
        }
        world.gpg(&args, Some(text.as_bytes()));
    };
    let readers = [a.key.fingerprint.as_str(), pc.fingerprint.as_str()];
    encrypt_to(&readers, &computer.repo.join("from-pc.gpg"), "typed on the computer\n");
    computer.commit("Add given password for from-pc to store.");
    let good = computer.git(&["rev-parse", "HEAD"]);

    /// What a says of the computer's file, which it must refuse; and nothing of a's may have changed.
    fn refused(a: &support::Device) -> String {
        let before = (a.ls(), a.store.sync_state().unwrap().peers[0].clone());
        let stands = a.stands_with("pc");
        assert!(stands.starts_with("REFUSED "), "not refused: {stands}");
        assert_eq!((a.ls(), a.store.sync_state().unwrap().peers[0].clone()), before);
        assert_eq!(a.store.pending_update("pc".into(), a.identity.clone()).unwrap(), None);
        stands
    }
    let none = ["--digest-algo", "SHA512", "--compress-algo", "none"];
    let everyone = computer.everyone();
    let good_bundle = computer.bundle(&[]);

    // 27. Signed by a key that is nobody's; by another device's key; not signed; signed with SHA-1.
    for (signer, options) in [
        (&mallory.fingerprint, &none[..]),
        (&tablet.fingerprint, &none[..]),
        (
            &pc.fingerprint,
            &["--digest-algo", "SHA1", "--compress-algo", "none"][..],
        ),
    ] {
        computer.put(&computer.seal(&good_bundle, signer, &everyone, options));
        assert!(refused(a).contains("not signed by the key of the device it is named after"));
    }
    let unsigned = world.path("unsigned.gpg");
    let bundle_bytes = std::fs::read(&good_bundle).unwrap();
    {
        let output = unsigned.to_string_lossy().into_owned();
        let mut encrypt = world.gpg_command(&["--encrypt", "--recipient", &a.key.fingerprint, "--output", &output]);
        encrypt.arg(&good_bundle);
        world.run(encrypt, None);
    }
    computer.put(&unsigned);
    assert!(refused(a).contains("it is not signed"));
    // Rightly signed, but for someone else; or the bundle in clear; or anything.
    computer.put(&computer.seal(&good_bundle, &pc.fingerprint, &[&pc.fingerprint], &none));
    assert!(refused(a).contains("not encrypted to this device's key"));
    computer.put(&good_bundle);
    refused(a);
    std::fs::write(world.path("junk"), b"<html>503</html>").unwrap();
    computer.put(&world.path("junk"));
    refused(a);

    // 27. Rightly signed, but not the bundle the protocol says.
    let seal = |file: &Path| computer.seal(file, &pc.fingerprint, &everyone, &none);
    computer.put(&seal(&computer.bundle(&["refs/pass-sync/pc", "refs/heads/main"])));
    assert!(refused(a).contains("it holds 2 references, not exactly one"));
    computer.put(&seal(&computer.bundle(&["main"])));
    assert!(refused(a).contains("its reference is refs/heads/main, not refs/pass-sync/pc"));
    computer.put(&seal(&computer.bundle(&["refs/pass-sync/pc", "^HEAD~1"])));
    assert!(refused(a).contains("it is not a whole history"));
    std::fs::write(world.path("short.bundle"), &bundle_bytes[..bundle_bytes.len() * 2 / 3]).unwrap();
    computer.put(&seal(&world.path("short.bundle")));
    assert!(refused(a).contains("its pack is damaged or cut short"));
    std::fs::write(world.path("not-a-bundle"), b"hello").unwrap();
    computer.put(&seal(&world.path("not-a-bundle")));
    assert!(refused(a).contains("it is not a git bundle"));
    // The right file, cut short on the way; with one bit changed.
    let sealed = std::fs::read(seal(&good_bundle)).unwrap();
    std::fs::write(world.path("cut.gpg"), &sealed[..sealed.len() / 2]).unwrap();
    computer.put(&world.path("cut.gpg"));
    refused(a);
    let mut flipped = sealed.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x01;
    std::fs::write(world.path("flipped.gpg"), &flipped).unwrap();
    computer.put(&world.path("flipped.gpg"));
    refused(a);
    // Small on the wire, huge once decompressed: anyone can make one for a device's public key.
    let bomb = world.path("bomb");
    std::fs::File::create(&bomb)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 4096)
        .unwrap();
    let sealed_bomb = computer.seal(&bomb, &mallory.fingerprint, &everyone, &["--compress-algo", "zlib"]);
    assert!(std::fs::metadata(&sealed_bomb).unwrap().len() < 1024 * 1024);
    computer.put(&sealed_bomb);
    assert!(refused(a).contains("over 256 MiB"));
    // Far too big as it is: refused by its size, before it is read.
    let in_zone = fleet.zone.files().join("pc.bundle.gpg");
    std::fs::File::options()
        .write(true)
        .open(&in_zone)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();
    let started = std::time::Instant::now();
    assert!(refused(a).contains("over 256 MiB"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "it was not downloaded"
    );

    // 26. A tip holding what a synced store does not: refused whole, naming the paths.
    type Wrong = Box<dyn Fn(&Path)>;
    let wrongs: [(&str, Wrong); 6] = [
        (
            ".extensions/x.bash",
            Box::new(|repo| {
                std::fs::create_dir_all(repo.join(".extensions")).unwrap();
                std::fs::write(repo.join(".extensions/x.bash"), "#!/bin/sh\n").unwrap();
            }),
        ),
        (
            "link.gpg",
            Box::new(|repo| {
                std::os::unix::fs::symlink("/etc/passwd", repo.join("link.gpg")).unwrap();
            }),
        ),
        (
            "README",
            Box::new(|repo| std::fs::write(repo.join("README"), "not an entry\n").unwrap()),
        ),
        (
            ".gpg-id.sig",
            Box::new(|repo| std::fs::write(repo.join(".gpg-id.sig"), "a signature\n").unwrap()),
        ),
        (
            "hidden/.x/y.gpg",
            Box::new(|repo| {
                std::fs::create_dir_all(repo.join("hidden/.x")).unwrap();
                std::fs::copy(repo.join("seed.gpg"), repo.join("hidden/.x/y.gpg")).unwrap();
            }),
        ),
        (
            ".gitattributes is not the one line",
            Box::new(|repo| {
                std::fs::write(repo.join(".gitattributes"), "*.gpg diff=gpg filter=evil\n").unwrap();
            }),
        ),
    ];
    for (named, wrong) in &wrongs {
        computer.publish_wrong(&good, wrong);
        let why = refused(a);
        assert!(why.contains(named), "{named}: {why}");
    }
    // 24. A `.gpg-id` that names a key that is no device's; or nobody; or two devices at once.
    for (content, named) in [
        (
            format!("{}\n{}\n", pc.fingerprint, mallory.fingerprint),
            mallory.fingerprint.as_str(),
        ),
        ("# nobody\n".to_string(), "names nobody"),
        ("example.org\n".to_string(), "example.org"),
    ] {
        computer.publish_wrong(&good, |repo| std::fs::write(repo.join(".gpg-id"), &content).unwrap());
        let why = refused(a);
        assert!(why.contains(named), "{named}: {why}");
    }
    computer.publish_wrong(&good, |repo| {
        std::fs::create_dir_all(repo.join("team")).unwrap();
        std::fs::write(repo.join("team/.gpg-id"), "mallory@example.org\n").unwrap();
    });
    assert!(refused(a).contains("team/.gpg-id: mallory@example.org"));
    // 25. An entry encrypted to a key that is no device's.
    computer.publish_wrong(&good, |repo| {
        encrypt_to(&[&pc.fingerprint, &mallory.fingerprint], &repo.join("leak.gpg"), "x\n");
    });
    assert!(refused(a).contains("leak is encrypted to a key that is no device's"));
    computer.publish_wrong(&good, |repo| {
        std::fs::write(repo.join("plain.gpg"), "not encrypted\n").unwrap()
    });
    assert!(refused(a).contains("plain is not an encrypted file"));

    // After all of that a has taken nothing, and the right file still works: as gpg and git write
    // it by default (compressed, a pack with deltas), and with a readers file that names by key id and by text.
    assert_eq!(a.ls(), ["seed"]);
    computer.git(&["reset", "--quiet", "--hard", &good]);
    computer.git(&["clean", "--quiet", "-fdx"]);
    std::fs::create_dir_all(computer.repo.join("team")).unwrap();
    let key_id = &a.key.fingerprint[24..];
    std::fs::write(
        computer.repo.join("team/.gpg-id"),
        format!("0x{key_id}\npc@example.org # the computer\n"),
    )
    .unwrap();
    encrypt_to(&readers, &computer.repo.join("team/wifi.gpg"), "shared\n");
    computer.commit("A folder for the team.");
    computer.publish(&["--digest-algo", "SHA512"]);
    assert_eq!(a.stands_with("pc"), "2 to take");
    assert_eq!(
        a.pending("pc"),
        ["added from-pc", "added team/wifi", "readers team/: a pc"]
    );
    let update = a
        .store
        .pending_update("pc".into(), a.identity.clone())
        .unwrap()
        .unwrap();
    assert_eq!(
        update.commits[0].signature,
        SignatureStatus::Unsigned,
        "shown, not required"
    );
    assert_eq!(a.take("pc", &[], &[]).kind, TakeKind::Forward);
    assert_eq!(a.show("from-pc").unwrap(), "typed on the computer");
    assert_eq!(a.readers("team/"), ["a", "pc"]);

    // 27. An old file served again: valid, but behind what was already received. Told, and ignored.
    let newer = world.path("newer.gpg");
    std::fs::copy(&in_zone, &newer).unwrap();
    computer.git(&["reset", "--quiet", "--hard", &good]);
    computer.publish(&none);
    assert_eq!(a.stands_with("pc"), "IGNORED");
    assert_eq!(a.stands_with("pc"), "IGNORED");
    computer.put(&newer);
    assert_eq!(a.stands_with("pc"), "nothing to take");
    // The drop zone lost the file.
    std::fs::remove_file(&in_zone).unwrap();
    assert_eq!(a.stands_with("pc"), "nothing published");

    // What a publishes, the real tools take: gpg checks the signature, git the bundle.
    a.insert("from-a", "typed on the phone");
    a.sync();
    let message = world.path("a.message");
    std::fs::copy(fleet.zone.files().join("a.bundle.gpg"), &message).unwrap();
    let bundle = world.path("a.bundle");
    let mut decrypt = world.gpg_command(&["--status-fd", "2", "--output", &bundle.to_string_lossy(), "--decrypt"]);
    decrypt.arg(&message);
    let decrypted = world.run(decrypt, None);
    let signed_by_a = decrypted
        .stderr
        .lines()
        .any(|line| line.starts_with("[GNUPG:] VALIDSIG ") && line.ends_with(&a.key.fingerprint));
    assert!(signed_by_a, "{}", decrypted.stderr);
    let bundle_path = bundle.to_string_lossy().into_owned();
    let heads = computer.git(&["bundle", "list-heads", &bundle_path]);
    assert_eq!(
        heads,
        format!("{} refs/pass-sync/a", world.git(&a.dir, &["rev-parse", "HEAD"]))
    );
    let verified = world.try_git(&computer.repo, &["bundle", "verify", &bundle_path]);
    assert!(
        verified.ok && !verified.stderr.contains("requires"),
        "{}",
        verified.stderr
    );
    computer.git(&["fetch", "--quiet", &bundle_path, "+refs/pass-sync/a:refs/pass-sync/a"]);
    computer.git(&["fsck", "--strict", "--no-dangling"]);
    computer.git(&["merge", "--quiet", "--ff-only", "refs/pass-sync/a"]);
    assert_eq!(
        world.gpg_decrypt(&computer.repo.join("from-a.gpg")),
        "typed on the phone\n"
    );
    assert_eq!(
        computer.git(&["log", "-1", "--format=%G? %GF %s"]),
        format!("G {} Add given password for from-a to store.", a.key.fingerprint)
    );

    // 26, the other way round: a store of this device that holds what a synced store does not is not published,
    // and says why; what the others published is received all the same.
    std::fs::write(a.dir.join("README"), "not an entry\n").unwrap();
    world.git(&a.dir, &["add", "README"]);
    world.git(
        &a.dir,
        &[
            "-c",
            "user.name=x",
            "-c",
            "user.email=x@x",
            "commit",
            "--quiet",
            "-m",
            "A note.",
        ],
    );
    computer.commit("Something more.");
    computer.publish(&none);
    let published_before = std::fs::read(fleet.zone.files().join("a.bundle.gpg")).unwrap();
    let report = a.store.sync(a.zone.clone(), a.identity.clone()).unwrap();
    assert!(!report.published);
    assert!(
        report
            .publish_refused
            .as_deref()
            .is_some_and(|why| why.contains("README")),
        "{:?}",
        report.publish_refused
    );
    assert_eq!(report.peers[0].outcome, FetchOutcome::Received);
    assert_eq!(
        std::fs::read(fleet.zone.files().join("a.bundle.gpg")).unwrap(),
        published_before
    );
}

#[test]
#[ignore = "needs root and OpenSSH"]
fn the_drop_zone_is_the_pinned_one_and_knows_this_device() {
    let fleet = Fleet::new(&["a", "b"]);
    let a = fleet.device("a");
    assert_eq!(
        pass_core::fetch_host_key(fleet.zone.url()).unwrap().fingerprint,
        fleet.zone.host_key
    );
    assert_eq!(
        pass_core::fetch_host_key(fleet.zone.url()).unwrap().algorithm,
        "ssh-ed25519"
    );
    let sync = |zone: pass_core::DropZone| a.store.sync(zone, a.identity.clone());

    // Another host key than the pinned one: nothing is sent, not even this device's public key.
    let other = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let mut wrong_host = a.zone.clone();
    wrong_host.host_key = other.into();
    assert_eq!(
        sync(wrong_host).err(),
        Some(PassError::HostKeyMismatch {
            pinned: other.into(),
            served: fleet.zone.host_key.clone()
        })
    );
    // An SSH key the drop zone was not given.
    let mut stranger = a.zone.clone();
    stranger.private_key = generate_device_key("stranger".into()).unwrap().private_key;
    assert_eq!(sync(stranger).err(), Some(PassError::Unauthorized));
    // Nobody listening; not a drop zone URL; another device's key to unlock with.
    let mut closed = a.zone.clone();
    closed.url = format!("sftp://drop@127.0.0.1:{}", support::free_port());
    assert!(matches!(sync(closed), Err(PassError::Network { .. })));
    let mut not_sftp = a.zone.clone();
    not_sftp.url = "https://example.org/store.git".into();
    assert!(matches!(sync(not_sftp), Err(PassError::InvalidConfig { .. })));
    assert!(matches!(
        a.store.sync(a.zone.clone(), fleet.device("b").identity.clone()),
        Err(PassError::InvalidKey { .. })
    ));
    // The right one works, and leaves nothing behind in the zone when there is nothing to publish.
    assert_eq!(a.sync(), ["b: nothing published"]);
    assert_eq!(std::fs::read_dir(fleet.zone.files()).unwrap().count(), 0);
}

/// The other implementation on the other side: `pass sync`, pass.py's own command, as device `b`.
#[test]
#[ignore = "needs root, OpenSSH, gpg, git and a pass.py that has `sync`"]
fn with_pass_py_as_the_other_device() {
    let world = World::new();
    // The computer's key has no passphrase: pass.py asks gpg, and gpg would ask a terminal.
    let mut generate = world.command("gpg");
    generate.args([
        "--batch",
        "--quiet",
        "--passphrase",
        "",
        "--quick-generate-key",
        "b <b@example.org>",
        "ed25519",
        "sign",
        "never",
    ]);
    world.run(generate, None);
    let listing = world
        .gpg(&["--with-colons", "--list-keys", "b@example.org"], None)
        .text();
    let fingerprint = listing
        .lines()
        .find_map(|line| line.strip_prefix("fpr:"))
        .map(|rest| rest.trim_matches(':').to_string())
        .unwrap();
    let mut subkey = world.command("gpg");
    subkey.args([
        "--batch",
        "--quiet",
        "--passphrase",
        "",
        "--quick-add-key",
        &fingerprint,
        "cv25519",
        "encr",
        "never",
    ]);
    world.run(subkey, None);
    let pc = Key {
        public: world.gpg(&["--armor", "--export", &fingerprint], None).text(),
        secret: String::new(),
        email: "b@example.org".into(),
        fingerprint,
    };
    let ssh = world.ssh_key("b");
    let public = ssh.public.clone();
    let fleet = Fleet::with(world, &["a"], &[&public], &[pc.peer("b")]);
    let (a, world) = (fleet.device("a"), &fleet.world);
    world.import_key(&a.key.public_key, &a.key.fingerprint);
    let store = world.path("b-store");
    let pass = |args: &[&str], stdin: Option<&[u8]>| world.pass(&store, args, stdin);
    pass(&["sync", "setup", "b", &fleet.zone.url(), &pc.fingerprint], None);
    world.git(
        &store,
        &["config", "pass-sync.sftp-options", &fleet.zone.ssh_options(&ssh)],
    );
    pass(&["sync", "peer", "a", &a.key.fingerprint], None);

    // The phone makes the store; pass.py adopts it.
    a.store
        .create(vec!["a".into(), "b".into()], a.identity.clone())
        .unwrap();
    a.insert("seed", "s");
    a.insert("bank", "hunter2");
    a.sync();
    let fetched = pass(&["sync", "fetch"], None);
    assert!(fetched.contains("a: ") && fetched.contains("to take"), "{fetched}");
    let shown = pass(&["sync", "log", "a"], None);
    for line in ["added bank", "added seed", "readers /: a b"] {
        assert!(shown.lines().any(|shown| shown == line), "{line} is not in:\n{shown}");
    }
    pass(&["sync", "merge", "a"], None);
    assert_eq!(pass(&["show", "bank"], None).trim_end(), "hunter2");
    assert_eq!(a.stands_with("b"), "nothing to take");

    // Each changes the store; the phone merges, pass.py moves forward onto the merge.
    pass(&["insert", "-m", "-f", "from-b"], Some(b"typed on the computer\n"));
    pass(&["mv", "-f", "seed", "moved-by-b"], None);
    pass(&["sync", "push"], None);
    a.insert("from-a", "typed on the phone");
    a.insert("seed", "changed on the phone");
    assert_ne!(a.stands_with("b"), "nothing to take");
    assert_eq!(a.pending("b"), ["added from-b", "moved seed -> moved-by-b"]);
    assert_eq!(a.take("b", &[], &[]).kind, TakeKind::Merge);
    assert_eq!(
        a.show("moved-by-b").unwrap(),
        "changed on the phone",
        "moved there, changed here"
    );
    pass(&["sync", "fetch"], None);
    pass(&["sync", "merge", "a"], None);
    assert_eq!(pass(&["show", "from-a"], None).trim_end(), "typed on the phone");
    assert_eq!(pass(&["show", "moved-by-b"], None).trim_end(), "changed on the phone");

    // Both change the same entry: each says so, in the same words, and the choice made on one holds on both.
    pass(&["insert", "-m", "-f", "bank"], Some(b"from-b\n"));
    pass(&["sync", "push"], None);
    a.insert("bank", "from-a");
    a.sync();
    assert_eq!(a.pending("b"), ["changed bank", "conflict bank"]);
    pass(&["sync", "fetch"], None);
    let shown = pass(&["sync", "log", "a"], None);
    for line in ["changed bank", "conflict bank"] {
        assert!(shown.lines().any(|shown| shown == line), "{line} is not in:\n{shown}");
    }
    let refused = world.try_run(world.pass_command(&store, &["sync", "merge", "a"]), None);
    assert!(!refused.ok, "pass sync merge took a conflict without a choice");
    assert!(matches!(
        a.try_take("b", &[], &[]),
        Err(PassError::ChoiceRequired { .. })
    ));
    a.take("b", &[], &["bank"]);
    pass(&["sync", "fetch"], None);
    pass(&["sync", "merge", "a"], None);
    assert_eq!(pass(&["show", "bank"], None).trim_end(), "from-b");
    assert_eq!(a.show("bank").unwrap(), "from-b");
    assert_eq!(a.stands_with("b"), "nothing to take");
    assert_eq!(world.log(&a.dir, "HEAD"), world.log(&store, "HEAD"));
    assert_eq!(a.store.stale().unwrap(), Vec::<String>::new());
}
