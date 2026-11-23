//! Against the real tools: `cargo test -p pass-core -- --ignored` (`make e2e` does it).
//! Needs `gpg`, `git`, `python3`, pass.py (`PASS_PY=/path/to/pass.py`, by default
//! `../clones/pass/pass.py` beside `ios-apps`), and for the sync tests: root, OpenSSH
//! (`sshd`, `sftp`, `ssh-keygen`, `ssh-keyscan`) and an account named `drop`, which is
//! what `apps/pass/dropzone/entrypoint.sh` needs. Each test makes its own GnuPG home,
//! keys, repositories and drop zone, and uses nothing of the machine's.
//!
//! The store on "the computer" is made and read by gpg, git and pass.py; the store on
//! "the phone" by this crate. They only ever meet in the drop zone, as `apps/pass/SYNC.md` says.

mod support;

use std::path::Path;
use std::sync::Arc;

use pass_core::{
    generate_device_key, generate_openpgp_key, inspect_key, public_key_of, ChangeKind, Choice, DeviceKey, EntryVersion,
    FetchOutcome, GeneratedKey, Identity, PassError, PassStore, PendingUpdate, Side, SignatureStatus, SyncReport,
};
use pass_core::{Access, Reader};
use support::{Computer, Key, KeyKind, World, Zone, PASSPHRASE};

fn unlock(key: &Key) -> Arc<Identity> {
    Identity::unlock(key.secret.clone(), PASSPHRASE.into()).unwrap()
}

/// A member's view of a directory that pass.py made: `me` is the device, `owner` the computer.
fn open(world: &World, store: &Path, me: &Key, owner: &Key) -> Arc<PassStore> {
    let mut config = world.member_config("unused", "phone", &me.public, vec![owner.peer("pc", true)]);
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
        assert_eq!(
            phone.recipients("web/new".into()).unwrap(),
            std::slice::from_ref(&key.fingerprint)
        );
        let files = phone.recipient_files().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].path.as_str(), files[0].unknown.len()), (".gpg-id", 0));

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
    assert_eq!(
        phone.recipients("team/deep/er".into()).unwrap(),
        [alice.fingerprint.clone(), "pc@example.org".to_string()]
    );

    // Who can read what, folder by folder: the root is the phone's alone, `team` is shared.
    let devices = |access: &Access| access.readers.iter().map(|r| r.device.clone()).collect::<Vec<_>>();
    let root = phone.folder_access("".into()).unwrap();
    assert_eq!(
        (root.gpg_id.as_str(), devices(&root)),
        (".gpg-id", vec![Some("phone".to_string())])
    );
    assert!(root.listed && root.readable && root.matches && root.encrypted_to.is_empty());
    let team = phone.folder_access("team/deep".into()).unwrap();
    assert_eq!(team.gpg_id, "team/.gpg-id");
    assert_eq!(devices(&team), [Some("phone".to_string()), Some("pc".to_string())]);
    assert_eq!(
        team.readers[1].id, "pc@example.org",
        "the line as it is written, and whose it is"
    );
    let wifi = phone.entry_access("team/wifi".into()).unwrap();
    assert_eq!(wifi.encrypted_to, ["pc", "phone"]);
    assert!(wifi.readable && wifi.matches && wifi.listed);
    let bank = phone.entry_access("bank".into()).unwrap();
    assert_eq!(
        (bank.encrypted_to.clone(), bank.readable, bank.matches),
        (vec!["phone".to_string()], true, true)
    );
    assert!(phone.unreadable().unwrap().is_empty());
    // The other device: listed in `team` only, and the root's entries are locked to it. No key needed to know.
    let other = open(&world, &store, &pc, &alice);
    assert_eq!(other.unreadable().unwrap(), ["Email/work", "bank"]);
    let bank = other.entry_access("bank".into()).unwrap();
    assert!(!bank.readable && !bank.listed && bank.matches);
    assert!(other.entry_access("team/wifi".into()).unwrap().readable);
    assert!(matches!(
        other.entry_access("nope".into()),
        Err(PassError::NotFound { .. })
    ));
    // A `.gpg-id` that changed without its files being encrypted again: the entry says so.
    std::fs::write(store.join("team/.gpg-id"), format!("{}\n", alice.fingerprint)).unwrap();
    let stale = phone.entry_access("team/wifi".into()).unwrap();
    assert_eq!(
        (devices(&stale), stale.matches),
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
    assert_eq!(
        phone.recipient_files().unwrap()[1].unknown,
        std::slice::from_ref(&carol.fingerprint)
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
            id: carol.fingerprint.clone()
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

    // What the configuration refuses: a key that is not the one with the fingerprint, no owner, two owners.
    let config = |peers| world.member_config("unused", "phone", &alice.public, peers);
    let mut wrong_key = pc.peer("pc", true);
    wrong_key.public_key = alice.public.clone();
    for bad in [
        vec![wrong_key],
        vec![],
        vec![pc.peer("pc", false)],
        vec![pc.peer("pc", true), alice.peer("laptop", true)],
        vec![pc.peer("phone", true)],
        vec![pc.peer("My PC", true)],
    ] {
        assert!(matches!(
            PassStore::open(config(bad)),
            Err(PassError::InvalidConfig { .. })
        ));
    }
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
    let mut config = world.member_config("unused", "phone", &made.public_key, vec![pc.peer("pc", true)]);
    config.directory = store.to_string_lossy().into_owned();
    let phone = PassStore::open(config).unwrap();
    assert_eq!(phone.content("from-gpg".into(), identity).unwrap(), "for the phone\n");
}

// MARK: the sync, through a real drop zone

/// A computer and a phone that have been introduced to each other, and a drop zone between them.
struct Scene {
    world: World,
    zone: Zone,
    /// The computer's OpenPGP key, made by gpg.
    pc: Key,
    /// The phone's, made on the phone.
    phone_key: GeneratedKey,
    phone_ssh: DeviceKey,
}

impl Scene {
    fn new() -> (Self, support::SshKey) {
        let world = World::new();
        let pc = world.make_key(KeyKind::Curve25519, "pc@example.org");
        let phone_key = generate_openpgp_key("phone".into(), PASSPHRASE.into()).unwrap();
        world.import_key(&phone_key.public_key, &phone_key.fingerprint);
        let phone_ssh = generate_device_key("phone".into()).unwrap();
        let pc_ssh = world.ssh_key("pc");
        let zone = world.zone(&[&pc_ssh.public, &phone_ssh.public_key]);
        (
            Self {
                world,
                zone,
                pc,
                phone_key,
                phone_ssh,
            },
            pc_ssh,
        )
    }

    /// The computer: a store made by pass.py that both devices can read.
    fn computer(&self, ssh: support::SshKey) -> Computer<'_> {
        let store = self.world.path("computer-store");
        self.world.pass(
            &store,
            &["init", &self.pc.fingerprint, &self.phone_key.fingerprint],
            None,
        );
        self.world.pass(&store, &["git", "init"], None);
        self.world
            .pass(&store, &["insert", "-m", "bank"], Some(b"hunter2\nlogin: alice\n"));
        self.world
            .pass(&store, &["insert", "-m", "Email/work"], Some(b"s3cret\n"));
        self.world
            .pass(&store, &["insert", "-m", "wifi"], Some(b"home-network\n"));
        Computer {
            world: &self.world,
            zone: &self.zone,
            store,
            name: "pc".into(),
            key: self.pc.clone(),
            ssh,
            devices: vec![self.pc.fingerprint.clone(), self.phone_key.fingerprint.clone()],
        }
    }

    fn phone_in(&self, directory: &str) -> Phone<'_> {
        let config = self.world.member_config(
            directory,
            "phone",
            &self.phone_key.public_key,
            vec![self.pc.peer("pc", true)],
        );
        Phone {
            scene: self,
            directory: self.world.path(directory),
            store: PassStore::open(config).unwrap(),
            identity: Identity::unlock(self.phone_key.secret_key.clone(), PASSPHRASE.into()).unwrap(),
        }
    }

    fn phone(&self) -> Phone<'_> {
        self.phone_in("phone-store")
    }
}

struct Phone<'a> {
    scene: &'a Scene,
    directory: std::path::PathBuf,
    store: Arc<PassStore>,
    identity: Arc<Identity>,
}

impl Phone<'_> {
    fn sync(&self) -> SyncReport {
        self.store
            .sync(
                self.scene.zone.for_device(&self.scene.phone_ssh.private_key),
                self.identity.clone(),
            )
            .unwrap()
    }

    /// Syncs, and returns the update that came, which there must be.
    fn receive(&self) -> PendingUpdate {
        match self.sync().fetch {
            FetchOutcome::Received { update: Some(update) } => update,
            other => panic!("expected an update, got {other:?}"),
        }
    }

    /// Syncs, expecting the owner's file to be refused, and returns why. Nothing must have changed.
    fn refused(&self) -> String {
        let before = (self.store.list().unwrap(), self.store.pending_update().unwrap());
        let reason = match self.sync().fetch {
            FetchOutcome::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(
            (self.store.list().unwrap(), self.store.pending_update().unwrap()),
            before
        );
        reason
    }

    fn accept(&self, update: &PendingUpdate, choices: &[(&str, Side)]) -> Result<Vec<String>, PassError> {
        let choices = choices
            .iter()
            .map(|(name, keep)| Choice {
                name: name.to_string(),
                keep: *keep,
            })
            .collect();
        self.store
            .accept_update(update.commit.clone(), choices, self.identity.clone())
    }

    /// Receives the owner's first file and takes it: a phone that is set up.
    fn set_up(&self) {
        let update = self.receive();
        self.accept(&update, &[]).unwrap();
        self.sync();
    }

    fn insert(&self, name: &str, content: &str) {
        self.store
            .insert(name.into(), content.into(), false, self.identity.clone())
            .unwrap();
    }

    fn edit(&self, name: &str, content: &str) {
        assert!(self
            .store
            .edit(name.into(), content.into(), self.identity.clone())
            .unwrap());
    }

    fn password(&self, name: &str) -> String {
        self.store.show(name.into(), self.identity.clone()).unwrap().password
    }

    fn text(&self, version: EntryVersion) -> String {
        match version {
            EntryVersion::Content { text } => text,
            EntryVersion::Absent => "(absent)".into(),
            EntryVersion::Unreadable { .. } => "(unreadable)".into(),
        }
    }

    /// git's own opinion of the phone's repository: sound, and the directory is what the last commit says.
    fn check_repository(&self) {
        let world = &self.scene.world;
        world.git(&self.directory, &["fsck", "--strict", "--no-dangling"]);
        assert_eq!(world.git(&self.directory, &["status", "--porcelain"]), "");
    }
}

fn kinds(update: &PendingUpdate) -> Vec<(String, ChangeKind)> {
    update
        .entries
        .iter()
        .map(|entry| (entry.name.clone(), entry.kind.clone()))
        .collect()
}

#[test]
#[ignore = "needs root, OpenSSH, gpg, git and pass.py"]
fn first_setup_then_changes_go_both_ways() {
    let (scene, pc_ssh) = Scene::new();
    let computer = scene.computer(pc_ssh);
    let phone = scene.phone();
    let world = &scene.world;

    // Nothing published yet: nothing to take, and nothing of the phone to publish.
    let report = phone.sync();
    assert_eq!(
        (report.published, report.fetch),
        (false, FetchOutcome::NothingPublished)
    );
    assert!(!report.state.has_store);

    // The owner publishes, as gpg does by default (compressed). The phone receives, and takes nothing.
    computer.publish(&[]);
    let update = phone.receive();
    assert_eq!(update.commit, world.git(&computer.store, &["rev-parse", "HEAD"]));
    assert_eq!(update.outstanding, 0);
    assert_eq!(update.commit_count as usize, world.log(&computer.store, "HEAD").len());
    assert_eq!(update.commits[0].subject, "Add given password for wifi to store.");
    assert_eq!(update.commits[0].author, "Computer <computer@example.org>");
    assert_eq!(update.commits[0].signature, SignatureStatus::Unsigned);
    assert_eq!(
        kinds(&update),
        [
            ("Email/work", ChangeKind::Added),
            ("bank", ChangeKind::Added),
            ("wifi", ChangeKind::Added)
        ]
        .map(|(name, kind)| (name.to_string(), kind))
    );
    assert_eq!(update.entries[0].readers_after, ["pc", "phone"]);
    assert!(update.entries.iter().all(|entry| !entry.conflict));
    assert_eq!(update.recipient_changes.len(), 1);
    assert_eq!(update.recipient_changes[0].path, ".gpg-id");
    assert!(update.recipient_changes[0].before.is_empty());
    assert_eq!(
        update.recipient_changes[0].after,
        [
            Reader {
                id: scene.pc.fingerprint.clone(),
                device: Some("pc".into())
            },
            Reader {
                id: scene.phone_key.fingerprint.clone(),
                device: Some("phone".into())
            }
        ]
    );
    assert_eq!(update.other_files, [".gitattributes"]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(
        (now - update.published_at).abs() < 300,
        "the time of the owner's signature"
    );
    assert!(
        phone.store.list().unwrap().is_empty(),
        "nothing is applied before it is accepted"
    );
    assert!(!phone.store.sync_state().unwrap().has_store);
    assert!(phone.store.sync_state().unwrap().update_waiting);
    // What it would bring can be read first.
    let versions = phone
        .store
        .update_versions("bank".into(), phone.identity.clone())
        .unwrap();
    assert_eq!(
        (versions.before, versions.mine),
        (EntryVersion::Absent, EntryVersion::Absent)
    );
    assert_eq!(phone.text(versions.theirs), "hunter2\nlogin: alice\n");
    // The same file again changes nothing; the update is still there after a restart of the app.
    assert_eq!(phone.sync().fetch, FetchOutcome::Unchanged);
    assert_eq!(scene.phone().store.pending_update().unwrap(), Some(update.clone()));

    // Accepted: the store is the owner's.
    assert!(matches!(
        phone
            .store
            .accept_update("0".repeat(40), vec![], phone.identity.clone()),
        Err(PassError::StaleUpdate)
    ));
    assert_eq!(phone.accept(&update, &[]).unwrap(), ["Email/work", "bank", "wifi"]);
    assert_eq!(phone.store.list().unwrap(), ["Email/work", "bank", "wifi"]);
    assert_eq!(phone.password("bank"), "hunter2");
    assert_eq!(phone.store.pending_update().unwrap(), None);
    assert_eq!(world.log(&phone.directory, "HEAD"), world.log(&computer.store, "HEAD"));
    phone.check_repository();
    let state = phone.store.sync_state().unwrap();
    assert!(state.has_store && state.unpublished && !state.update_waiting);
    assert_eq!(state.outstanding, 0);

    // "After any accepted update, it publishes its tip": what the computer finds is the owner's own tip.
    let report = phone.sync();
    assert_eq!((report.published, report.fetch), (true, FetchOutcome::Unchanged));
    assert_eq!(
        computer.fetch("phone", &scene.phone_key.fingerprint).unwrap(),
        update.commit
    );
    assert!(!phone.sync().published, "nothing new: nothing sent");

    // The phone changes the store, and publishes.
    phone.insert("phone/first", "from-phone\nlogin: me");
    phone.edit("bank", "hunter3\nlogin: alice\n");
    phone.store.remove("wifi".into(), phone.identity.clone()).unwrap();
    assert_eq!(phone.store.sync_state().unwrap().outstanding, 3);
    assert!(phone.sync().published);
    phone.check_repository();

    // The computer: what the phone published passes gpg (signature) and `git bundle verify`, holds
    // the one reference, and its commits are signed by the phone and touch only entries.
    let tip = computer.fetch("phone", &scene.phone_key.fingerprint).unwrap();
    assert_eq!(tip, world.git(&phone.directory, &["rev-parse", "HEAD"]));
    assert_eq!(
        world.log(&computer.store, "HEAD..refs/pass-sync/phone"),
        [
            "Remove wifi from store.",
            "Edit password for bank using pass-ios.",
            "Add given password for phone/first to store.",
        ]
    );
    assert_eq!(
        world.git(
            &computer.store,
            &["log", "-1", "--format=%an <%ae>", "refs/pass-sync/phone"]
        ),
        "phone <phone@pass-sync.invalid>"
    );
    // The review, decrypted by pass's own diff driver.
    let review = world.git(
        &computer.store,
        &[
            "log",
            "-p",
            "--format=%s",
            "HEAD..refs/pass-sync/phone",
            "--",
            "bank.gpg",
        ],
    );
    assert!(review.contains("-hunter2") && review.contains("+hunter3"), "{review}");
    computer.merge("phone", &scene.phone_key.fingerprint).unwrap();
    assert_eq!(
        world.pass(&computer.store, &["show", "phone/first"], None),
        "from-phone\nlogin: me\n\n"
    );
    assert_eq!(
        world.pass(&computer.store, &["show", "bank"], None),
        "hunter3\nlogin: alice\n\n"
    );
    assert!(!computer.store.join("wifi.gpg").exists());

    // The owner has republished the phone's own tip: received, nothing to take, nothing outstanding.
    let report = phone.sync();
    assert_eq!(report.fetch, FetchOutcome::Received { update: None });
    assert_eq!((report.state.outstanding, report.state.unpublished), (0, false));

    // And the owner goes on: the phone moves forward when the user says so.
    world.pass(&computer.store, &["insert", "-m", "later"], Some(b"x\n"));
    world.pass(&computer.store, &["mv", "bank", "money/bank"], None);
    computer.publish(&["--compress-algo", "none"]);
    let update = phone.receive();
    assert_eq!(
        kinds(&update),
        [
            ("later".to_string(), ChangeKind::Added),
            ("money/bank".to_string(), ChangeKind::Renamed { from: "bank".into() }),
        ]
    );
    assert!(update.recipient_changes.is_empty() && update.other_files.is_empty());
    assert_eq!(phone.password("bank"), "hunter3", "not before it is accepted");
    assert_eq!(phone.accept(&update, &[]).unwrap(), ["bank", "later", "money/bank"]);
    assert_eq!(
        phone.store.list().unwrap(),
        ["Email/work", "later", "money/bank", "phone/first"]
    );
    assert_eq!(world.log(&phone.directory, "HEAD"), world.log(&computer.store, "HEAD"));
    phone.check_repository();
}

#[test]
#[ignore = "needs root, OpenSSH, gpg, git and pass.py"]
fn different_entries_changed_on_both_sides_merge_by_themselves() {
    let (scene, pc_ssh) = Scene::new();
    let computer = scene.computer(pc_ssh);
    let phone = scene.phone();
    let world = &scene.world;
    computer.publish(&[]);
    phone.set_up();

    phone.insert("phone/new", "from-phone");
    phone.edit("bank", "changed-on-phone\n");
    world.pass(&computer.store, &["insert", "-m", "pc/new"], Some(b"from-pc\n"));
    world.pass(
        &computer.store,
        &["insert", "-m", "-f", "wifi"],
        Some(b"changed-on-pc\n"),
    );
    world.pass(&computer.store, &["rm", "-f", "Email/work"], None);
    // The same change made on both sides is no conflict either.
    phone.insert("agreed", "same");
    let same = world.path("phone-store/agreed.gpg");
    std::fs::copy(&same, computer.store.join("agreed.gpg")).unwrap();
    world.git(&computer.store, &["add", "agreed.gpg"]);
    world.git(
        &computer.store,
        &["commit", "--quiet", "-m", "Add given password for agreed to store."],
    );

    // The computer cannot take the phone's commits yet: not a fast-forward. It publishes instead.
    assert!(phone.sync().published);
    computer.fetch("phone", &scene.phone_key.fingerprint).unwrap();
    let refused = computer.merge("phone", &scene.phone_key.fingerprint).unwrap_err();
    assert!(refused.contains("not a fast-forward"), "{refused}");
    computer.publish(&["--compress-algo", "none"]);

    let update = phone.receive();
    assert_eq!(update.outstanding, 3);
    assert_eq!(
        kinds(&update),
        [
            ("Email/work".to_string(), ChangeKind::Removed),
            ("agreed".to_string(), ChangeKind::Added),
            ("pc/new".to_string(), ChangeKind::Added),
            ("wifi".to_string(), ChangeKind::Changed),
        ]
    );
    assert!(update.entries.iter().all(|entry| !entry.conflict));
    let wifi = phone
        .store
        .update_versions("wifi".into(), phone.identity.clone())
        .unwrap();
    assert_eq!(phone.text(wifi.before), "home-network\n");
    assert_eq!(phone.text(wifi.mine), "home-network\n");
    assert_eq!(phone.text(wifi.theirs), "changed-on-pc\n");
    assert_eq!(phone.password("wifi"), "home-network", "not before it is accepted");

    // Accepted, with nothing to choose: a merge commit, signed, on the phone's line.
    assert_eq!(phone.accept(&update, &[]).unwrap(), ["Email/work", "pc/new", "wifi"]);
    assert_eq!(
        phone.store.list().unwrap(),
        ["agreed", "bank", "pc/new", "phone/new", "wifi"]
    );
    assert_eq!(phone.password("bank"), "changed-on-phone");
    assert_eq!(phone.password("wifi"), "changed-on-pc");
    assert_eq!(phone.password("pc/new"), "from-pc");
    phone.check_repository();
    let merge = world.git(&phone.directory, &["log", "-1", "--format=%s|%P|%G?"]);
    let parts: Vec<&str> = merge.split('|').collect();
    assert_eq!(parts[0], "Merge the changes of pc into those of phone.");
    assert_eq!(parts[1].split(' ').count(), 2, "two parents");
    assert_eq!(parts[1].split(' ').nth(1), Some(update.commit.as_str()));
    assert_eq!(phone.store.sync_state().unwrap().outstanding, 4);
    assert_eq!(phone.store.pending_update().unwrap(), None);

    // Published: now it is a fast-forward for the computer, which takes it and republishes.
    assert!(phone.sync().published);
    computer.fetch("phone", &scene.phone_key.fingerprint).unwrap();
    computer.merge("phone", &scene.phone_key.fingerprint).unwrap();
    assert_eq!(
        world.pass(&computer.store, &["show", "phone/new"], None),
        "from-phone\n\n"
    );
    assert_eq!(
        world.pass(&computer.store, &["show", "bank"], None),
        "changed-on-phone\n\n"
    );
    assert_eq!(
        world.pass(&computer.store, &["show", "wifi"], None),
        "changed-on-pc\n\n"
    );
    let report = phone.sync();
    assert_eq!(report.fetch, FetchOutcome::Received { update: None });
    assert_eq!(report.state.outstanding, 0);
    assert_eq!(world.log(&phone.directory, "HEAD"), world.log(&computer.store, "HEAD"));
}

#[test]
#[ignore = "needs root, OpenSSH, gpg, git and pass.py"]
fn the_same_entry_changed_on_both_sides_needs_a_choice() {
    for keep in [Side::Mine, Side::Theirs] {
        let (scene, pc_ssh) = Scene::new();
        let computer = scene.computer(pc_ssh);
        let phone = scene.phone();
        let world = &scene.world;
        computer.publish(&[]);
        phone.set_up();

        // Changed on both; changed here and removed there; removed here and changed there.
        phone.edit("bank", "phone-version\n");
        world.pass(&computer.store, &["insert", "-m", "-f", "bank"], Some(b"pc-version\n"));
        phone.edit("wifi", "phone-wifi\n");
        world.pass(&computer.store, &["rm", "-f", "wifi"], None);
        phone.store.remove("Email/work".into(), phone.identity.clone()).unwrap();
        world.pass(
            &computer.store,
            &["insert", "-m", "-f", "Email/work"],
            Some(b"pc-work\n"),
        );
        // And one that only the phone touched: no choice there.
        phone.insert("untouched", "mine");
        computer.publish(&[]);

        let update = phone.receive();
        let conflicts: Vec<&str> = update
            .entries
            .iter()
            .filter(|entry| entry.conflict)
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(conflicts, ["Email/work", "bank", "wifi"]);
        // Both versions can be seen before choosing.
        let bank = phone
            .store
            .update_versions("bank".into(), phone.identity.clone())
            .unwrap();
        assert_eq!(phone.text(bank.before), "hunter2\nlogin: alice\n");
        assert_eq!(phone.text(bank.mine), "phone-version\n");
        assert_eq!(phone.text(bank.theirs), "pc-version\n");
        let wifi = phone
            .store
            .update_versions("wifi".into(), phone.identity.clone())
            .unwrap();
        assert_eq!(
            (phone.text(wifi.mine), wifi.theirs),
            ("phone-wifi\n".to_string(), EntryVersion::Absent)
        );
        let work = phone
            .store
            .update_versions("Email/work".into(), phone.identity.clone())
            .unwrap();
        assert_eq!(
            (work.mine, phone.text(work.theirs)),
            (EntryVersion::Absent, "pc-work\n".to_string())
        );

        // No choice, or not all of them: refused, by name, and nothing moves.
        assert_eq!(
            phone.accept(&update, &[]),
            Err(PassError::ChoiceRequired {
                names: vec!["Email/work".into(), "bank".into(), "wifi".into()]
            })
        );
        assert_eq!(
            phone.accept(&update, &[("bank", keep)]),
            Err(PassError::ChoiceRequired {
                names: vec!["Email/work".into(), "wifi".into()]
            })
        );
        assert_eq!(phone.password("bank"), "phone-version");
        assert_eq!(phone.store.pending_update().unwrap(), Some(update.clone()));

        phone
            .accept(&update, &[("bank", keep), ("wifi", keep), ("Email/work", keep)])
            .unwrap();
        match keep {
            Side::Mine => {
                assert_eq!(phone.store.list().unwrap(), ["bank", "untouched", "wifi"]);
                assert_eq!(phone.password("bank"), "phone-version");
                assert_eq!(phone.password("wifi"), "phone-wifi");
            }
            Side::Theirs => {
                assert_eq!(phone.store.list().unwrap(), ["Email/work", "bank", "untouched"]);
                assert_eq!(phone.password("bank"), "pc-version");
                assert_eq!(phone.password("Email/work"), "pc-work");
            }
        }
        assert_eq!(phone.password("untouched"), "mine");
        phone.check_repository();

        // Either way the result is the phone's line with the owner's merged in: the computer can take it.
        phone.sync();
        computer.fetch("phone", &scene.phone_key.fingerprint).unwrap();
        computer.merge("phone", &scene.phone_key.fingerprint).unwrap();
        assert_eq!(
            world.pass(&computer.store, &["show", "bank"], None),
            if keep == Side::Mine {
                "phone-version\n\n"
            } else {
                "pc-version\n\n"
            }
        );
        assert_eq!(computer.store.join("wifi.gpg").exists(), keep == Side::Mine);
        assert_eq!(computer.store.join("Email/work.gpg").exists(), keep == Side::Theirs);
        assert_eq!(phone.sync().state.outstanding, 0);
    }
}

#[test]
#[ignore = "needs root, OpenSSH, gpg, git and pass.py"]
fn a_change_of_recipients_is_shown_before_it_is_taken() {
    let (scene, pc_ssh) = Scene::new();
    let computer = scene.computer(pc_ssh);
    let phone = scene.phone();
    let world = &scene.world;
    computer.publish(&[]);
    phone.set_up();
    phone.insert("mine", "written before the change");

    // The owner gives someone else access to everything, and makes a directory the phone cannot read.
    let carol = world.make_key(KeyKind::Curve25519, "carol@example.org");
    let (pc, phone_fpr) = (&scene.pc.fingerprint, &scene.phone_key.fingerprint);
    world.pass(&computer.store, &["init", pc, phone_fpr, &carol.fingerprint], None);
    world.pass(&computer.store, &["init", "-p", "private", pc], None);
    world.pass(
        &computer.store,
        &["insert", "-m", "private/diary"],
        Some(b"not for the phone\n"),
    );
    world.pass(&computer.store, &["insert", "-m", ".extensions/note"], Some(b"x\n"));
    std::fs::write(computer.store.join(".extensions/hook.bash"), "echo hi\n").unwrap();
    world.git(&computer.store, &["add", ".extensions/hook.bash"]);
    world.git(&computer.store, &["commit", "--quiet", "-m", "An extension."]);
    computer.publish(&[]);

    let update = phone.receive();
    // Directory by directory...
    let changes: Vec<(&str, usize, usize)> = update
        .recipient_changes
        .iter()
        .map(|change| (change.path.as_str(), change.before.len(), change.after.len()))
        .collect();
    assert_eq!(changes, [(".gpg-id", 2, 3), ("private/.gpg-id", 0, 1)]);
    // Before and after, by device name; and a key that is no device's is told as such.
    let names = |readers: &[Reader]| readers.iter().map(|r| r.device.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&update.recipient_changes[0].before),
        [Some("pc".to_string()), Some("phone".to_string())]
    );
    assert_eq!(
        names(&update.recipient_changes[0].after),
        [Some("pc".to_string()), Some("phone".to_string()), None]
    );
    assert_eq!(update.recipient_changes[0].after[2].id, carol.fingerprint);
    assert_eq!(names(&update.recipient_changes[1].after), [Some("pc".to_string())]);
    // ...and entry by entry: who could read it, who can now. A key that is no device's has no name.
    let bank = update.entries.iter().find(|entry| entry.name == "bank").unwrap();
    assert_eq!(bank.kind, ChangeKind::Changed);
    assert_eq!(bank.readers_before, ["pc", "phone"]);
    assert_eq!(bank.readers_after.len(), 3);
    assert_eq!(&bank.readers_after[1..], ["pc", "phone"]);
    assert!(bank.readers_after[0].starts_with("key "), "{:?}", bank.readers_after);
    let diary = update
        .entries
        .iter()
        .find(|entry| entry.name == "private/diary")
        .unwrap();
    assert_eq!(
        (&diary.kind, &diary.readers_after[..]),
        (&ChangeKind::Added, &["pc".to_string()][..])
    );
    // What is not an entry is told apart: the owner's to change, and here to be seen.
    assert_eq!(update.other_files, [".extensions/hook.bash", ".extensions/note.gpg"]);
    // An entry the phone cannot read is said to be so, not shown empty.
    let versions = phone
        .store
        .update_versions("private/diary".into(), phone.identity.clone())
        .unwrap();
    assert!(matches!(versions.theirs, EntryVersion::Unreadable { .. }));
    assert_eq!(
        phone.store.recipients("new".into()).unwrap().len(),
        2,
        "still the old ones"
    );

    // Accepted (a merge: the phone had a commit of its own). From then on it writes for the new readers,
    // which it cannot: carol is no device, and it says so rather than leave her out.
    phone.accept(&update, &[]).unwrap();
    assert_eq!(phone.store.recipients("new".into()).unwrap().len(), 3);
    assert_eq!(
        phone.store.recipient_files().unwrap()[0].unknown,
        std::slice::from_ref(&carol.fingerprint)
    );
    assert_eq!(
        phone
            .store
            .insert("new".into(), "x".into(), false, phone.identity.clone()),
        Err(PassError::UnknownRecipient {
            id: carol.fingerprint.clone()
        })
    );
    assert!(matches!(
        phone.store.show("private/diary".into(), phone.identity.clone()),
        Err(PassError::NotForThisKey)
    ));
    assert!(
        phone.directory.join(".extensions/hook.bash").is_file(),
        "non-entries are the owner's, as they are"
    );
    // It can still remove or rename what it cannot read, and the computer still takes its line.
    phone
        .store
        .remove("private/diary".into(), phone.identity.clone())
        .unwrap();
    phone.sync();
    computer.fetch("phone", &scene.phone_key.fingerprint).unwrap();
    computer.merge("phone", &scene.phone_key.fingerprint).unwrap();
    assert_eq!(
        world.pass(&computer.store, &["show", "mine"], None),
        "written before the change\n\n"
    );
}

#[test]
#[ignore = "needs root, OpenSSH, gpg, git and pass.py"]
fn outstanding_changes_can_be_dropped() {
    let (scene, pc_ssh) = Scene::new();
    let computer = scene.computer(pc_ssh);
    let phone = scene.phone();
    let world = &scene.world;
    computer.publish(&[]);
    phone.set_up();
    assert_eq!(phone.store.discard_outstanding().unwrap(), 0);

    phone.insert("regret", "oops");
    phone.edit("bank", "oops\n");
    phone.sync();
    // An update is waiting meanwhile: dropping does not take it, it goes back to what was accepted.
    world.pass(&computer.store, &["insert", "-m", "later"], Some(b"x\n"));
    computer.publish(&[]);
    let update = phone.receive();
    assert_eq!(update.outstanding, 2);

    assert_eq!(phone.store.discard_outstanding().unwrap(), 2);
    assert_eq!(phone.store.list().unwrap(), ["Email/work", "bank", "wifi"]);
    assert_eq!(phone.password("bank"), "hunter2");
    phone.check_repository();
    let waiting = phone.store.pending_update().unwrap().unwrap();
    assert_eq!(
        (waiting.commit.as_str(), waiting.outstanding),
        (update.commit.as_str(), 0)
    );
    // Published: the computer sees the phone back on the owner's history, with nothing to take.
    assert!(phone.sync().published);
    let tip = computer.fetch("phone", &scene.phone_key.fingerprint).unwrap();
    assert_eq!(tip, world.git(&computer.store, &["rev-parse", "HEAD~1"]));
    phone.accept(&waiting, &[]).unwrap();
    assert_eq!(world.log(&phone.directory, "HEAD"), world.log(&computer.store, "HEAD"));
}

/// Everything a drop zone could serve under the owner's name that is not the owner's file.
#[test]
#[ignore = "needs root, OpenSSH, gpg, git and pass.py"]
fn what_is_not_the_owners_file_is_refused() {
    let (scene, pc_ssh) = Scene::new();
    let computer = scene.computer(pc_ssh);
    let world = &scene.world;
    let mallory = world.make_key(KeyKind::Curve25519, "mallory@example.org");
    // A third device, known to the phone as a member: its key is a device's, but not the owner's.
    let tablet = world.make_key(KeyKind::Curve25519, "tablet@example.org");
    let config = world.member_config(
        "phone-store",
        "phone",
        &scene.phone_key.public_key,
        vec![scene.pc.peer("pc", true), tablet.peer("tablet", false)],
    );
    let phone = Phone {
        scene: &scene,
        directory: world.path("phone-store"),
        store: PassStore::open(config).unwrap(),
        identity: Identity::unlock(scene.phone_key.secret_key.clone(), PASSPHRASE.into()).unwrap(),
    };
    let everyone = computer.devices.clone();
    let store = &computer.store;
    let bundle_of = |name: &str, refs: &[&str]| {
        let file = world.path(name);
        let mut args = vec!["bundle", "create", "--quiet", "--version=2"];
        let path = file.to_string_lossy().into_owned();
        args.push(&path);
        args.extend(refs);
        world.git(store, &args);
        file
    };
    world.git(store, &["update-ref", "refs/pass-sync/pc", "HEAD"]);
    world.git(store, &["update-ref", "refs/pass-sync/phone", "HEAD"]);
    let good_bundle = bundle_of("good.bundle", &["refs/pass-sync/pc"]);
    let none = ["--compress-algo", "none"];

    // Signed by a key that is nobody's; by a device that is not the owner; not signed at all.
    computer.put(&computer.seal(&good_bundle, &mallory.fingerprint, &everyone, &none));
    assert!(phone
        .refused()
        .contains("not signed by the key of the device it is named after"));
    computer.put(&computer.seal(&good_bundle, &tablet.fingerprint, &everyone, &none));
    assert!(phone
        .refused()
        .contains("not signed by the key of the device it is named after"));
    let unsigned = world.path("unsigned.gpg");
    world.gpg(
        &[
            "--encrypt",
            "--recipient",
            &scene.phone_key.fingerprint,
            "--output",
            &unsigned.to_string_lossy(),
            &good_bundle.to_string_lossy(),
        ],
        None,
    );
    computer.put(&unsigned);
    assert!(phone.refused().contains("not signed"));
    // Signed by the owner, but for someone else; or the bundle in clear; or anything.
    computer.put(&computer.seal(
        &good_bundle,
        &scene.pc.fingerprint,
        std::slice::from_ref(&scene.pc.fingerprint),
        &none,
    ));
    assert!(phone.refused().contains("not encrypted to this device's key"));
    computer.put(&good_bundle);
    assert!(phone.refused().contains("refused"));
    std::fs::write(world.path("junk"), b"<html>503</html>").unwrap();
    computer.put(&world.path("junk"));
    assert!(phone.refused().contains("refused"));

    // Rightly signed by the owner, but not the bundle the protocol says.
    let seal = |file: &Path| computer.seal(file, &scene.pc.fingerprint, &everyone, &none);
    computer.put(&seal(&bundle_of(
        "two.bundle",
        &["refs/pass-sync/pc", "refs/heads/main"],
    )));
    assert!(phone.refused().contains("it holds 2 references, not exactly one"));
    computer.put(&seal(&bundle_of("other.bundle", &["refs/pass-sync/phone"])));
    assert!(phone
        .refused()
        .contains("its reference is refs/pass-sync/phone, not refs/pass-sync/pc"));
    computer.put(&seal(&bundle_of("branch.bundle", &["main"])));
    assert!(phone.refused().contains("its reference is refs/heads/main"));
    computer.put(&seal(&bundle_of("partial.bundle", &["refs/pass-sync/pc", "^HEAD~1"])));
    assert!(phone.refused().contains("it is not a whole history"));
    std::fs::write(world.path("not-a-bundle"), b"hello").unwrap();
    computer.put(&seal(&world.path("not-a-bundle")));
    assert!(phone.refused().contains("it is not a git bundle"));
    // A bundle cut short before it was signed: the signature is good, the pack is not.
    let whole = std::fs::read(&good_bundle).unwrap();
    std::fs::write(world.path("short.bundle"), &whole[..whole.len() * 2 / 3]).unwrap();
    computer.put(&seal(&world.path("short.bundle")));
    assert!(phone.refused().contains("its pack is damaged or cut short"));
    // A bundle whose pack lacks an object its history needs (a shallow copy of the store).
    let shallow = world.path("shallow");
    let mut clone = world.command("git");
    clone
        .args(["clone", "--quiet", "--depth", "1", "--no-local"])
        .arg(store)
        .arg(&shallow);
    world.run(clone, None);
    world.git(&shallow, &["update-ref", "refs/pass-sync/pc", "HEAD"]);
    let incomplete = world.path("incomplete.bundle");
    world.git(
        &shallow,
        &[
            "bundle",
            "create",
            "--quiet",
            "--version=2",
            &incomplete.to_string_lossy(),
            "refs/pass-sync/pc",
        ],
    );
    computer.put(&seal(&incomplete));
    assert!(
        phone.refused().contains("it is not complete"),
        "a history with a hole in it"
    );

    // The right file, cut short on the way; and with one byte changed.
    let good = computer.sealed_tip(&none);
    let sealed = std::fs::read(&good).unwrap();
    std::fs::write(world.path("cut.gpg"), &sealed[..sealed.len() / 2]).unwrap();
    computer.put(&world.path("cut.gpg"));
    assert!(phone.refused().contains("refused"));
    let mut flipped = sealed.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x01;
    std::fs::write(world.path("flipped.gpg"), &flipped).unwrap();
    computer.put(&world.path("flipped.gpg"));
    assert!(phone.refused().contains("refused"));
    // Small on the wire, huge once decompressed: anyone can make one for this device's public key.
    let bomb = world.path("bomb");
    let zeros = std::fs::File::create(&bomb).unwrap();
    zeros.set_len(256 * 1024 * 1024 + 4096).unwrap();
    drop(zeros);
    let sealed_bomb = computer.seal(&bomb, &mallory.fingerprint, &everyone, &["--compress-algo", "zlib"]);
    assert!(std::fs::metadata(&sealed_bomb).unwrap().len() < 1024 * 1024);
    computer.put(&sealed_bomb);
    assert!(phone.refused().contains("over 256 MiB"));
    // Far too big: refused by its size, before it is read.
    let huge = scene.zone.files().join("pc.bundle.gpg");
    let grown = std::fs::File::options().write(true).open(&huge).unwrap();
    grown.set_len(256 * 1024 * 1024 + 1).unwrap();
    drop(grown);
    let started = std::time::Instant::now();
    assert!(phone.refused().contains("over 256 MiB"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "it was not downloaded"
    );

    // After all of that the phone has taken nothing, and the right file still works.
    assert!(!phone.store.sync_state().unwrap().has_store);
    computer.put(&good);
    let first = phone.receive();
    phone.accept(&first, &[]).unwrap();

    // An old file served again: valid, but behind what was already received. Told, and ignored.
    world.pass(store, &["insert", "-m", "newer"], Some(b"x\n"));
    computer.publish(&none);
    let newer = phone.receive();
    computer.put(&good);
    assert!(matches!(phone.sync().fetch, FetchOutcome::Stale { published_at } if published_at > 0));
    assert_eq!(
        phone.store.pending_update().unwrap(),
        Some(newer.clone()),
        "the newer one is still the one waiting"
    );
    phone.accept(&newer, &[]).unwrap();
    computer.put(&good);
    assert!(matches!(phone.sync().fetch, FetchOutcome::Stale { .. }));
    assert_eq!(phone.store.list().unwrap(), ["Email/work", "bank", "newer", "wifi"]);
    // The drop zone lost the file.
    std::fs::remove_file(&huge).unwrap();
    assert_eq!(phone.sync().fetch, FetchOutcome::NothingPublished);
    assert_eq!(phone.store.list().unwrap().len(), 4);
}

#[test]
#[ignore = "needs root and OpenSSH"]
fn the_drop_zone_is_the_pinned_one_and_knows_this_device() {
    let (scene, _pc_ssh) = Scene::new();
    let phone = scene.phone();
    let zone = scene.zone.for_device(&scene.phone_ssh.private_key);
    assert_eq!(
        pass_core::fetch_host_key(scene.zone.url()).unwrap().fingerprint,
        scene.zone.host_key
    );
    assert_eq!(
        pass_core::fetch_host_key(scene.zone.url()).unwrap().algorithm,
        "ssh-ed25519"
    );

    // Another host key than the pinned one: nothing is sent, not even this device's public key.
    let other = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let mut wrong_host = zone.clone();
    wrong_host.host_key = other.into();
    assert_eq!(
        phone.store.sync(wrong_host, phone.identity.clone()).err(),
        Some(PassError::HostKeyMismatch {
            pinned: other.into(),
            served: scene.zone.host_key.clone()
        })
    );
    // An SSH key the drop zone was not given.
    let mut stranger = zone.clone();
    stranger.private_key = generate_device_key("stranger".into()).unwrap().private_key;
    assert_eq!(
        phone.store.sync(stranger, phone.identity.clone()).err(),
        Some(PassError::Unauthorized)
    );
    // Nobody listening; not a drop zone URL.
    let mut closed = zone.clone();
    closed.url = format!("sftp://drop@127.0.0.1:{}", support::free_port());
    assert!(matches!(
        phone.store.sync(closed, phone.identity.clone()),
        Err(PassError::Network { .. })
    ));
    let mut not_sftp = zone.clone();
    not_sftp.url = "https://example.org/store.git".into();
    assert!(matches!(
        phone.store.sync(not_sftp, phone.identity.clone()),
        Err(PassError::InvalidConfig { .. })
    ));
    // The right one works, and leaves nothing half-written behind in the zone.
    assert_eq!(
        phone.store.sync(zone, phone.identity.clone()).unwrap().fetch,
        FetchOutcome::NothingPublished
    );
    assert_eq!(std::fs::read_dir(scene.zone.files()).unwrap().count(), 0);
}

/// The computer's side done by the real thing: `pass sync`, pass.py's own command.
#[test]
#[ignore = "needs root, OpenSSH, gpg, git and a pass.py that has `sync`"]
fn with_pass_sync_on_the_computer() {
    let (scene, pc_ssh) = Scene::new();
    let world = &scene.world;
    let store = world.path("computer-store");
    let (pc, phone_fpr) = (&scene.pc.fingerprint, &scene.phone_key.fingerprint);
    let pass = |args: &[&str], stdin: Option<&[u8]>| world.pass(&store, args, stdin);
    pass(&["init", pc, phone_fpr], None);
    pass(&["git", "init"], None);
    pass(&["insert", "-m", "bank"], Some(b"hunter2\nlogin: alice\n"));
    pass(&["insert", "-m", "wifi"], Some(b"home-network\n"));
    // `pass sync` calls gpg without `PASSWORD_STORE_GPG_OPTS`, so it cannot be handed the passphrase
    // the way the rest of pass.py is here: the agent is given it once instead, as a session would have.
    world.gpg(&["--local-user", pc, "--output", "/dev/null", "--sign"], Some(b"x"));
    world.gpg_decrypt(&store.join("bank.gpg"));
    pass(&["sync", "setup", "pc", &scene.zone.url(), pc], None);
    world.git(
        &store,
        &["config", "pass-sync.sftp-options", &scene.zone.ssh_options(&pc_ssh)],
    );
    pass(&["sync", "peer", "phone", phone_fpr], None);
    pass(&["sync", "push"], None);

    // The phone takes what pass.py published, changes things, publishes.
    let phone = scene.phone();
    let update = phone.receive();
    assert_eq!(update.commit, world.git(&store, &["rev-parse", "HEAD"]));
    phone.accept(&update, &[]).unwrap();
    assert_eq!(phone.password("bank"), "hunter2");
    phone.insert("from-phone", "typed on the phone");
    phone.edit("bank", "hunter3\nlogin: alice\n");
    assert!(phone.sync().published);

    // pass.py sees them, names their signer, shows them decrypted, and takes them.
    let fetched = pass(&["sync", "fetch"], None);
    assert!(fetched.contains("phone: 2 commit(s) to take"), "{fetched}");
    let log = pass(&["sync", "log", "phone"], None);
    assert!(
        log.contains("signed by phone") && log.contains("Edit password for bank using pass-ios."),
        "{log}"
    );
    let diff = pass(&["sync", "diff", "phone"], None);
    assert!(diff.contains("hunter3"), "{diff}");
    pass(&["sync", "merge", "phone"], None);
    assert_eq!(pass(&["show", "from-phone"], None), "typed on the phone\n\n");
    assert_eq!(pass(&["show", "bank"], None), "hunter3\nlogin: alice\n\n");
    // It has published the merged tip: the phone has nothing outstanding anymore.
    let report = phone.sync();
    assert_eq!(report.fetch, FetchOutcome::Received { update: None });
    assert_eq!(report.state.outstanding, 0);

    // Both change different entries: pass.py refuses to merge until the phone has merged its tip.
    pass(&["insert", "-m", "from-pc"], Some(b"typed on the computer\n"));
    phone.insert("second", "again");
    phone.sync();
    pass(&["sync", "fetch"], None);
    let refused = world.try_run(world.pass_command(&store, &["sync", "merge", "phone"]), None);
    assert!(!refused.ok, "pass sync merge took a history that is not a fast-forward");
    pass(&["sync", "push"], None);
    let update = phone.receive();
    assert_eq!(update.outstanding, 1);
    phone.accept(&update, &[]).unwrap();
    assert_eq!(phone.password("from-pc"), "typed on the computer");
    phone.sync();
    pass(&["sync", "fetch"], None);
    pass(&["sync", "merge", "phone"], None);
    assert_eq!(pass(&["show", "second"], None), "again\n\n");
    assert_eq!(phone.sync().state.outstanding, 0);
    assert_eq!(world.log(&phone.directory, "HEAD"), world.log(&store, "HEAD"));
}
