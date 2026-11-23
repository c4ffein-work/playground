//! Against the real tools: `cargo test -p pass-core -- --ignored` (`make e2e` does it).
//! Needs `gpg`, `git`, `python3` and pass.py (`PASS_PY=/path/to/pass.py`, by default
//! `../clones/pass/pass.py` beside `ios-apps`). Each test makes its own GnuPG home,
//! its own keys and its own repositories, and uses nothing of the machine's.
//!
//! The store on "the laptop" is made and read by gpg and pass.py; the store on "the
//! phone" by this crate. Interoperability is checked both ways.

mod support;

use std::path::Path;
use std::sync::Arc;

use pass_core::{
    clone_store, inspect_key, public_key_of, Identity, PassError, PassStore, PullOutcome, Remote, SignatureStatus,
    TlsTrust,
};
use support::sshd::{AuthorizedKey, Sshd};
use support::{Key, KeyKind, World, PASSPHRASE};

fn unlock(key: &Key) -> Arc<Identity> {
    Identity::unlock(key.secret.clone(), PASSPHRASE.into()).unwrap()
}

/// The phone's view of a directory that pass.py made: same files, no clone involved.
fn open(world: &World, store: &Path, keys: &[&Key]) -> Arc<PassStore> {
    let mut config = world.phone_config("unused", keys);
    config.directory = store.to_string_lossy().into_owned();
    PassStore::open(config).unwrap()
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn reads_a_store_made_by_gpg_and_pass() {
    for kind in [KeyKind::Rsa, KeyKind::Curve25519] {
        let world = World::new();
        let key = world.make_key(kind, "alice@example.org");
        let store = world.laptop_store(&key);
        world.pass(
            &store,
            &["insert", "-m", "web/été à la plage"],
            Some("pässwörd ✓\nurl: https://example.org/login\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n".as_bytes()),
        );
        world.pass(&store, &["generate", "web/generated", "40"], None);
        // `pass insert` with no option: pass.py stores it with no final newline.
        world.pass(&store, &["insert", "-e", "bare"], Some(b"just-this\n"));

        let phone = open(&world, &store, &[&key]);
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
        assert_eq!(
            generated,
            world.gpg_decrypt(&store.join("web/generated.gpg")),
            "what gpg reads"
        );

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
        let store = world.path("store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join(".gpg-id"), &key.fingerprint).unwrap();
        let text = "compress me ".repeat(500);
        for (name, options) in [
            ("default", vec![]),
            ("ocb", vec!["--force-ocb"]),
            ("armored", vec!["--armor"]),
            ("uncompressed", vec!["--compress-algo=none"]),
            ("bzip-not", vec!["--compress-algo=zlib", "--cipher-algo=AES128"]),
        ] {
            let file = store.join(format!("{name}.gpg"));
            let mut args = vec!["--encrypt", "--recipient", key.fingerprint.as_str(), "--output"];
            let path = file.to_string_lossy().into_owned();
            args.push(&path);
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

        let phone = open(&world, &store, &[&key]);
        let identity = unlock(&key);
        for name in ["default", "ocb", "armored", "uncompressed", "bzip-not"] {
            let content = phone.content(name.into(), identity.clone()).unwrap();
            assert_eq!(content, text, "{name}");
        }
    }
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn what_it_writes_is_read_by_gpg_and_pass() {
    for kind in [KeyKind::Rsa, KeyKind::Curve25519] {
        let world = World::new();
        let key = world.make_key(kind, "alice@example.org");
        let store = world.laptop_store(&key);
        let phone = open(&world, &store, &[&key]);
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
        assert_eq!(
            world.git(&store, &["log", "-1", "--format=%an <%ae>"]),
            "Phone <phone@example.org>"
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
    let bob = world.make_key(KeyKind::Rsa, "bob@example.org");
    let store = world.laptop_store(&alice);
    // By fingerprint and by email: both are things people put in a .gpg-id.
    world.pass(
        &store,
        &["init", "-p", "team", &alice.fingerprint, "bob@example.org"],
        None,
    );
    world.pass(&store, &["insert", "-m", "team/wifi"], Some(b"shared\n"));
    let both = world.encrypted_to(&store.join("team/wifi.gpg"));
    assert_eq!(both.len(), 2);

    let identity = unlock(&alice);
    let phone = open(&world, &store, &[&alice, &bob]);
    assert_eq!(
        phone.recipients("team/deep/er".into()).unwrap(),
        [alice.fingerprint.clone(), "bob@example.org".to_string()]
    );
    phone
        .insert("team/door".into(), "1234".into(), false, identity.clone())
        .unwrap();
    assert_eq!(
        world.encrypted_to(&store.join("team/door.gpg")),
        both,
        "to the same two keys as gpg"
    );
    assert_eq!(world.gpg_decrypt(&store.join("team/door.gpg")), "1234\n");
    // Bob alone reads it too.
    assert_eq!(
        open(&world, &store, &[&bob])
            .show("team/door".into(), unlock(&bob))
            .unwrap()
            .password,
        "1234"
    );
    assert!(matches!(
        open(&world, &store, &[&bob]).show("bank".into(), unlock(&bob)),
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

    // Without Bob's public key the phone cannot write there, and says so rather than leave him out.
    let alone = open(&world, &store, &[&alice]);
    assert_eq!(alone.recipient_files().unwrap()[1].unknown, ["bob@example.org"]);
    assert_eq!(
        alone.insert("team/x".into(), "x".into(), false, identity.clone()),
        Err(PassError::UnknownRecipient {
            id: "bob@example.org".into()
        })
    );
    assert!(!store.join("team/x.gpg").exists());
    alone.insert("x".into(), "x".into(), false, identity).unwrap();
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn keys_and_passphrases() {
    let world = World::new();
    let alice = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let store = world.laptop_store(&alice);

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

    // `gpg --export-secret-subkeys`: the primary key stays on the laptop. Enough to read
    // and to write; commits are then not signed (the signing key is not there).
    let subkeys_only = world
        .gpg(&["--armor", "--export-secret-subkeys", &alice.fingerprint], None)
        .text();
    let identity = Identity::unlock(subkeys_only, PASSPHRASE.into()).unwrap();
    assert!(!identity.can_sign());
    let phone = open(&world, &store, &[&alice]);
    assert_eq!(phone.show("bank".into(), identity.clone()).unwrap().password, "hunter2");
    phone.insert("unsigned".into(), "x".into(), false, identity).unwrap();
    assert_eq!(world.gpg_decrypt(&store.join("unsigned.gpg")), "x\n");
    assert!(!world.try_git(&store, &["verify-commit", "HEAD"]).ok);

    // A key with no passphrase works too.
    let bare = world.path("bare-gnupg");
    std::fs::create_dir_all(&bare).unwrap();
    let mut generate = world.command("gpg");
    generate.env("GNUPGHOME", &bare).args([
        "--batch",
        "--quiet",
        "--passphrase",
        "",
        "--quick-generate-key",
        "bare@example.org",
    ]);
    assert!(world.try_run(generate, None).ok);
    let mut export = world.command("gpg");
    export
        .env("GNUPGHOME", &bare)
        .args(["--batch", "--armor", "--export-secret-keys", "bare@example.org"]);
    let secret = world.try_run(export, None).text();
    assert!(!inspect_key(secret.clone()).unwrap().needs_passphrase);
    assert!(Identity::unlock(secret, String::new()).is_ok());
    let mut kill = world.command("gpgconf");
    kill.env("GNUPGHOME", &bare).args(["--kill", "all"]);
    world.try_run(kill, None);
}

/// `PASSWORD_STORE_SIGNING_KEY`: with it, a `.gpg-id` counts only when signed by that key.
#[test]
#[ignore = "needs gpg, git and pass.py"]
fn a_signed_gpg_id_is_checked() {
    let world = World::new();
    let alice = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let mallory = world.make_key(KeyKind::Curve25519, "mallory@example.org");
    let store = world.path("store");
    let mut init = world.pass_command(&store, &["init", &alice.fingerprint]);
    init.env("PASSWORD_STORE_SIGNING_KEY", &alice.fingerprint);
    assert!(world.try_run(init, None).ok);
    assert!(store.join(".gpg-id.sig").is_file(), "pass signed it");

    let identity = unlock(&alice);
    let mut config = world.phone_config("unused", &[&alice, &mallory]);
    config.directory = store.to_string_lossy().into_owned();
    config.signing_keys = vec![alice.fingerprint.clone()];
    let phone = PassStore::open(config.clone()).unwrap();
    phone.insert("a".into(), "x".into(), false, identity.clone()).unwrap();
    assert_eq!(
        phone.recipient_files().unwrap()[0].signature,
        SignatureStatus::Valid {
            fingerprint: alice.fingerprint.clone()
        }
    );

    // Someone with write access adds a recipient: the signature no longer matches.
    let original = std::fs::read(store.join(".gpg-id")).unwrap();
    std::fs::write(
        store.join(".gpg-id"),
        format!("{}\n{}\n", alice.fingerprint, mallory.fingerprint),
    )
    .unwrap();
    assert!(matches!(
        phone.insert("b".into(), "x".into(), false, identity.clone()),
        Err(PassError::UntrustedRecipients { .. })
    ));
    assert_eq!(phone.recipient_files().unwrap()[0].signature, SignatureStatus::Invalid);
    // ...and signs it with a key of their own: valid, but not the signing key.
    let gpg_id = store.join(".gpg-id").to_string_lossy().into_owned();
    world.gpg(&["--local-user", &mallory.fingerprint, "--detach-sign", &gpg_id], None);
    assert!(matches!(
        phone.insert("b".into(), "x".into(), false, identity.clone()),
        Err(PassError::UntrustedRecipients { .. })
    ));
    assert!(!store.join("b.gpg").exists());
    // No signature at all is refused as well; with no signing key configured, nothing is asked.
    std::fs::write(store.join(".gpg-id"), original).unwrap();
    std::fs::remove_file(store.join(".gpg-id.sig")).unwrap();
    assert!(phone.recipients("b".into()).is_err());
    config.signing_keys.clear();
    let trusting = PassStore::open(config).unwrap();
    trusting.insert("b".into(), "x".into(), false, identity).unwrap();
}

/// The relay's `authorized_keys` line for a device, as the README gives it.
fn forced_command(device: &str) -> String {
    format!("PASS_DEVICE={device} exec /usr/lib/git-core/git-shell -c \"$SSH_ORIGINAL_COMMAND\"")
}

fn install_update_hook(relay: &Path) {
    let hook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/pass/relay/update");
    std::fs::copy(hook, relay.join("hooks/update")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(relay.join("hooks/update"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn pulled(outcome: PullOutcome) -> (u32, Vec<String>) {
    match outcome {
        PullOutcome::FastForwarded { commits, changed } => (commits, changed),
        other => panic!("expected a fast-forward, got {other:?}"),
    }
}

/// The whole life of a phone and a laptop around a relay, over whichever transport `remote` is.
/// `laptop` is the owner's store, with the relay as its remote `relay`.
fn relay_cycle(world: &World, key: &Key, laptop: &Path, remote: Remote) {
    let identity = unlock(key);
    let phone_dir = world.path("phone-store");
    let phone = clone_store(world.phone_config("phone-store", &[key]), remote.clone()).unwrap();

    // The clone is the laptop's store, and git agrees it is a sound checkout of it.
    assert_eq!(phone.list().unwrap(), ["Email/work", "bank"]);
    assert_eq!(phone.show("bank".into(), identity.clone()).unwrap().password, "hunter2");
    assert_eq!(world.log(&phone_dir, "HEAD"), world.log(laptop, "main"));
    assert_eq!(world.git(&phone_dir, &["status", "--porcelain"]), "");
    world.git(&phone_dir, &["fsck", "--strict", "--no-dangling"]);
    let state = phone.sync_state().unwrap();
    assert_eq!(
        (state.branch.as_str(), state.device_branch.as_str()),
        ("main", "devices/phone")
    );
    assert_eq!((state.unmerged, state.unpushed, state.behind), (0, 0, 0));
    assert!(matches!(
        clone_store(world.phone_config("phone-store", &[key]), remote.clone()),
        Err(PassError::Io { .. })
    ));

    // The laptop changes the store: the phone takes it, fast-forward.
    world.pass(laptop, &["insert", "-m", "wifi"], Some(b"from-laptop\n"));
    world.pass(laptop, &["rm", "-f", "Email/work"], None);
    world.git(laptop, &["push", "--quiet", "relay", "main"]);
    assert_eq!(
        phone.pull(remote.clone()).unwrap(),
        PullOutcome::FastForwarded {
            commits: 2,
            changed: vec!["Email/work".into(), "wifi".into()],
        }
    );
    assert_eq!(phone.list().unwrap(), ["bank", "wifi"]);
    assert!(!phone_dir.join("Email").exists());
    assert_eq!(
        phone.show("wifi".into(), identity.clone()).unwrap().password,
        "from-laptop"
    );
    assert_eq!(phone.pull(remote.clone()).unwrap(), PullOutcome::UpToDate);
    assert_eq!(world.git(&phone_dir, &["status", "--porcelain"]), "");
    // With nothing of its own, the phone has nothing to push: no branch appears on the relay.
    assert_eq!(phone.push(remote.clone()).unwrap(), 0);
    assert_eq!(world.git(laptop, &["ls-remote", "--heads", "relay"]).lines().count(), 1);

    // The phone changes the store: its commits go to its own branch, never to main.
    phone
        .insert("phone/first".into(), "from-phone".into(), false, identity.clone())
        .unwrap();
    phone
        .edit("bank".into(), "hunter3\nlogin: alice\n".into(), identity.clone())
        .unwrap();
    let state = phone.sync_state().unwrap();
    assert_eq!((state.unmerged, state.unpushed, state.behind), (2, 2, 0));
    assert_eq!(phone.push(remote.clone()).unwrap(), 2);
    assert_eq!(phone.push(remote.clone()).unwrap(), 0);
    let state = phone.sync_state().unwrap();
    assert_eq!((state.unmerged, state.unpushed), (2, 0));
    phone.remove("wifi".into(), identity.clone()).unwrap();
    let report = phone.sync(remote.clone()).unwrap();
    assert_eq!((report.pull, report.pushed), (PullOutcome::UpToDate, 1));

    world.git(laptop, &["fetch", "--quiet", "relay"]);
    assert_eq!(
        world.log(laptop, "relay/main"),
        world.log(laptop, "main"),
        "main did not move"
    );
    assert_eq!(
        world.log(laptop, "main..relay/devices/phone"),
        [
            "Remove wifi from store.",
            "Edit password for bank using pass-ios.",
            "Add given password for phone/first to store.",
        ]
    );
    // The review on the laptop: who signed, and what changed, decrypted by pass's own diff driver.
    let signed = world.git(laptop, &["log", "--format=%G? %GF", "main..relay/devices/phone"]);
    assert!(
        signed.lines().all(|line| line == format!("G {}", key.fingerprint)),
        "{signed}"
    );
    // (`pass git log` is what one would type; pass.py's `git` subcommand does not find the repository, so: git.)
    let review = world.git(
        laptop,
        &[
            "log",
            "-p",
            "--format=%s",
            "main..relay/devices/phone",
            "--",
            "bank.gpg",
        ],
    );
    assert!(review.contains("-hunter2") && review.contains("+hunter3"), "{review}");

    // Meanwhile the laptop goes on: the two have diverged, and the phone says so and waits.
    world.pass(laptop, &["insert", "-m", "laptop/second"], Some(b"x\n"));
    world.git(laptop, &["push", "--quiet", "relay", "main"]);
    assert_eq!(
        phone.pull(remote.clone()).unwrap(),
        PullOutcome::Diverged { unmerged: 3, behind: 1 }
    );
    assert_eq!(phone.list().unwrap(), ["bank", "phone/first"], "nothing was taken");
    assert_eq!(phone.sync_state().unwrap().behind, 1);
    // It can still work and push while it waits.
    phone
        .insert("phone/second".into(), "more".into(), false, identity.clone())
        .unwrap();
    let report = phone.sync(remote.clone()).unwrap();
    assert_eq!(
        (report.pull, report.pushed),
        (PullOutcome::Diverged { unmerged: 4, behind: 1 }, 1)
    );

    // The laptop merges the phone's branch, checking the signatures, and publishes main.
    world.git(laptop, &["fetch", "--quiet", "relay"]);
    world.git(
        laptop,
        &[
            "merge",
            "--quiet",
            "--verify-signatures",
            "--no-edit",
            "relay/devices/phone",
        ],
    );
    world.git(laptop, &["push", "--quiet", "relay", "main"]);
    assert_eq!(world.pass(laptop, &["show", "phone/second"], None), "more\n\n");

    // Now the phone's commits are in main: it fast-forwards onto the merge, and is level again.
    let (commits, changed) = pulled(phone.pull(remote.clone()).unwrap());
    assert_eq!((commits, changed), (2, vec!["laptop/second".to_string()]));
    let state = phone.sync_state().unwrap();
    assert_eq!((state.unmerged, state.unpushed, state.behind), (0, 0, 0));
    assert_eq!(world.log(&phone_dir, "HEAD"), world.log(laptop, "main"));
    assert_eq!(world.git(&phone_dir, &["status", "--porcelain"]), "");
    assert_eq!(
        phone.list().unwrap(),
        ["bank", "laptop/second", "phone/first", "phone/second"]
    );
    assert_eq!(phone.push(remote.clone()).unwrap(), 0);

    // The other way out of a divergence: the phone gives its changes up.
    phone
        .insert("phone/regret".into(), "oops".into(), false, identity.clone())
        .unwrap();
    phone.push(remote.clone()).unwrap();
    world.pass(laptop, &["insert", "-m", "laptop/third"], Some(b"x\n"));
    world.git(laptop, &["push", "--quiet", "relay", "main"]);
    assert!(matches!(
        phone.pull(remote.clone()).unwrap(),
        PullOutcome::Diverged { unmerged: 1, behind: 1 }
    ));
    assert_eq!(phone.discard_local_commits(remote.clone()).unwrap(), 1);
    assert_eq!(phone.discard_local_commits(remote.clone()).unwrap(), 0);
    assert!(!phone_dir.join("phone/regret.gpg").exists());
    assert_eq!(world.log(&phone_dir, "HEAD"), world.log(laptop, "main"));
    world.git(laptop, &["fetch", "--quiet", "relay"]);
    assert_eq!(
        world.git(laptop, &["rev-parse", "relay/devices/phone"]),
        world.git(laptop, &["rev-parse", "main"]),
        "the phone's branch on the relay was moved back too"
    );

    // A change of recipients coming from the relay is shown, not taken.
    let mallory = world.make_key(KeyKind::Curve25519, "mallory@example.org");
    world.pass(laptop, &["init", &key.fingerprint, &mallory.fingerprint], None);
    world.git(laptop, &["push", "--quiet", "relay", "main"]);
    let PullOutcome::NeedsReview { update } = phone.pull(remote.clone()).unwrap() else {
        panic!("a new recipient went through unseen");
    };
    assert_eq!(update.recipient_changes.len(), 1);
    let change = &update.recipient_changes[0];
    assert_eq!(change.path, ".gpg-id");
    assert_eq!(change.before, std::slice::from_ref(&key.fingerprint));
    assert_eq!(change.after, [key.fingerprint.clone(), mallory.fingerprint.clone()]);
    assert_eq!(
        phone.recipients("new".into()).unwrap(),
        std::slice::from_ref(&key.fingerprint),
        "still the old ones"
    );
    assert!(
        update.changed.contains(&"bank".to_string()),
        "pass re-encrypted the entries"
    );
    assert!(
        phone.accept_update("0".repeat(40)).is_err(),
        "only the update that was shown"
    );
    pulled(phone.accept_update(update.commit).unwrap());
    assert_eq!(phone.recipients("new".into()).unwrap().len(), 2);
    assert_eq!(world.log(&phone_dir, "HEAD"), world.log(laptop, "main"));
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn sync_over_http() {
    let world = World::new();
    let key = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let laptop = world.laptop_store(&key);
    world.relay(&laptop);
    let server = world.git_http_server(&world.path("relay"), &[]);
    let remote = Remote::Http {
        url: format!("http://127.0.0.1:{}/store.git", server.port),
        username: None,
        password: None,
        trust: None,
    };
    relay_cycle(&world, &key, &laptop, remote);
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn sync_over_ssh() {
    let world = World::new();
    let key = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let laptop = world.laptop_store(&key);
    let relay = world.relay(&laptop);
    install_update_hook(&relay);
    let device = pass_core::generate_device_key("phone".into()).unwrap();
    let sshd = Sshd::start(
        vec![AuthorizedKey {
            public_key: device.public_key.clone(),
            forced_command: forced_command("phone"),
        }],
        world.path("relay"),
        world.env(),
    );
    let url = format!("ssh://git@127.0.0.1:{}/~/store.git", sshd.port);
    assert_eq!(
        pass_core::fetch_host_key(url.clone()).unwrap().fingerprint,
        sshd.host_key
    );
    let remote = Remote::Ssh {
        url,
        private_key: device.private_key,
        host_key: sshd.host_key.clone(),
    };
    relay_cycle(&world, &key, &laptop, remote);
}

/// What the relay refuses, and what the phone refuses of the relay.
#[test]
#[ignore = "needs gpg, git and pass.py"]
fn the_relay_is_not_trusted_and_does_not_trust() {
    let world = World::new();
    let key = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let laptop = world.laptop_store(&key);
    let relay = world.relay(&laptop);
    install_update_hook(&relay);
    let device = pass_core::generate_device_key("phone".into()).unwrap();
    let stranger = pass_core::generate_device_key("stranger".into()).unwrap();
    let sshd = Sshd::start(
        vec![AuthorizedKey {
            public_key: device.public_key.clone(),
            forced_command: forced_command("phone"),
        }],
        world.path("relay"),
        world.env(),
    );
    let url = format!("ssh://git@127.0.0.1:{}/~/store.git", sshd.port);
    let remote = |private_key: &str, host_key: &str, url: &str| Remote::Ssh {
        url: url.to_string(),
        private_key: private_key.to_string(),
        host_key: host_key.to_string(),
    };
    let good = remote(&device.private_key, &sshd.host_key, &url);
    let identity = unlock(&key);

    // Another host key than the pinned one: nothing is sent, not even the device's public key.
    let other_host = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert_eq!(
        clone_store(
            world.phone_config("a", &[&key]),
            remote(&device.private_key, other_host, &url)
        )
        .err(),
        Some(PassError::HostKeyMismatch {
            pinned: other_host.into(),
            served: sshd.host_key.clone()
        })
    );
    // A key the relay does not know.
    assert_eq!(
        clone_store(
            world.phone_config("a", &[&key]),
            remote(&stranger.private_key, &sshd.host_key, &url)
        )
        .err(),
        Some(PassError::Unauthorized)
    );
    // A repository that is not there: git's own words come back.
    let missing = format!("ssh://git@127.0.0.1:{}/~/nope.git", sshd.port);
    let error = clone_store(
        world.phone_config("a", &[&key]),
        remote(&device.private_key, &sshd.host_key, &missing),
    )
    .err()
    .unwrap();
    assert!(
        matches!(&error, PassError::Network { message } if message.contains("nope.git")),
        "{error:?}"
    );
    // Nobody listening.
    let closed = format!("ssh://git@127.0.0.1:{}/~/store.git", support::free_port());
    assert!(matches!(
        clone_store(
            world.phone_config("a", &[&key]),
            remote(&device.private_key, &sshd.host_key, &closed)
        ),
        Err(PassError::Network { .. })
    ));
    assert!(!world.path("a").exists(), "a failed clone leaves nothing");

    // The update hook: this key is the phone's, it moves devices/phone and nothing else.
    let mut config = world.phone_config("tablet-store", &[&key]);
    config.device = "tablet".into();
    let tablet = clone_store(config, good.clone()).unwrap();
    tablet.insert("x".into(), "x".into(), false, identity.clone()).unwrap();
    let refused = tablet.push(good.clone()).unwrap_err();
    assert!(
        matches!(&refused, PassError::PushRejected { message } if message.contains("hook declined")),
        "{refused:?}"
    );
    assert_eq!(tablet.sync_state().unwrap().unpushed, 1, "still to be pushed");
    assert_eq!(
        world.git(&laptop, &["ls-remote", "--heads", "relay"]).lines().count(),
        1
    );

    // The relay rewrites the phone's branch behind its back: the next push is not a fast-forward, and is refused here.
    let phone = clone_store(world.phone_config("phone-store", &[&key]), good.clone()).unwrap();
    phone.insert("a".into(), "x".into(), false, identity.clone()).unwrap();
    phone.push(good.clone()).unwrap();
    world.pass(&laptop, &["insert", "-m", "elsewhere"], Some(b"x\n"));
    world.git(&laptop, &["push", "--quiet", "--force", "relay", "main:devices/phone"]);
    phone.insert("b".into(), "x".into(), false, identity.clone()).unwrap();
    let refused = phone.push(good.clone()).unwrap_err();
    assert!(
        matches!(&refused, PassError::PushRejected { message } if message.contains("fast-forward")),
        "{refused:?}"
    );

    // With signed updates required, an unsigned commit planted on main is not taken...
    let mut config = world.phone_config("strict-store", &[&key]);
    config.require_signed_updates = true;
    let strict = clone_store(config, good.clone()).unwrap();
    world.git(&laptop, &["push", "--quiet", "relay", "main"]);
    let refused = strict.pull(good.clone()).unwrap_err();
    assert!(matches!(refused, PassError::UntrustedUpdate { .. }), "{refused:?}");
    assert!(!strict.list().unwrap().contains(&"elsewhere".to_string()));
    // ...nor one signed by a key that is not of the store...
    let mallory = world.make_key(KeyKind::Curve25519, "mallory@example.org");
    let sign_as = format!("user.signingkey={}", mallory.fingerprint);
    let gpg_program = world.path("gpg-with-passphrase");
    std::fs::write(
        &gpg_program,
        format!("#!/bin/sh\nexec gpg --batch --pinentry-mode loopback --passphrase {PASSPHRASE} \"$@\"\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gpg_program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let program = format!("gpg.program={}", gpg_program.display());
    world.git(
        &laptop,
        &[
            "-c",
            &sign_as,
            "-c",
            &program,
            "commit",
            "--quiet",
            "--allow-empty",
            "-S",
            "-m",
            "planted",
        ],
    );
    world.git(&laptop, &["push", "--quiet", "relay", "main"]);
    assert!(matches!(
        strict.pull(good.clone()),
        Err(PassError::UntrustedUpdate { .. })
    ));
    // ...but the owner's signature is.
    let sign_as = format!("user.signingkey={}", key.fingerprint);
    world.git(
        &laptop,
        &[
            "-c",
            &sign_as,
            "-c",
            &program,
            "commit",
            "--quiet",
            "--allow-empty",
            "-S",
            "-m",
            "by the owner",
        ],
    );
    world.git(&laptop, &["push", "--quiet", "relay", "main"]);
    assert_eq!(pulled(strict.pull(good).unwrap()).0, 3);
    assert!(strict.list().unwrap().contains(&"elsewhere".to_string()));
}

#[test]
#[ignore = "needs gpg, git and pass.py"]
fn https_with_a_token_and_a_pinned_certificate() {
    let world = World::new();
    let key = world.make_key(KeyKind::Curve25519, "alice@example.org");
    let laptop = world.laptop_store(&key);
    world.relay(&laptop);

    let certificate = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    std::fs::write(world.path("cert.pem"), certificate.cert.pem()).unwrap();
    std::fs::write(world.path("key.pem"), certificate.signing_key.serialize_pem()).unwrap();
    let pin = pinned_http::sha256_hex(certificate.cert.der());
    let (cert, key_file) = (world.path("cert.pem"), world.path("key.pem"));
    let server = world.git_http_server(
        &world.path("relay"),
        &[
            "--user",
            "alice",
            "--password",
            "t0ken",
            "--cert",
            &cert.to_string_lossy(),
            "--key",
            &key_file.to_string_lossy(),
        ],
    );
    let url = format!("https://127.0.0.1:{}/store.git", server.port);
    assert_eq!(pass_core::fetch_fingerprint(url.clone(), None).unwrap().sha256, pin);

    let remote = |password: Option<&str>, trust: Option<TlsTrust>| Remote::Http {
        url: url.clone(),
        username: Some("alice".into()),
        password: password.map(str::to_string),
        trust,
    };
    let pinned = Some(TlsTrust::PinnedOnly { sha256: pin.clone() });
    let good = remote(Some("t0ken"), pinned.clone());

    assert_eq!(
        clone_store(world.phone_config("a", &[&key]), remote(Some("wrong"), pinned.clone())).err(),
        Some(PassError::Unauthorized)
    );
    assert_eq!(
        clone_store(world.phone_config("a", &[&key]), remote(None, pinned.clone())).err(),
        Some(PassError::Unauthorized)
    );
    let other_pin = "ab".repeat(32);
    assert_eq!(
        clone_store(
            world.phone_config("a", &[&key]),
            remote(
                Some("t0ken"),
                Some(TlsTrust::PinnedOnly {
                    sha256: other_pin.clone()
                })
            )
        )
        .err(),
        Some(PassError::PinMismatch {
            pinned: other_pin,
            served: pin
        })
    );
    // A self-signed certificate is not a public one, and https is never unpinned by omission.
    assert!(matches!(
        clone_store(
            world.phone_config("a", &[&key]),
            remote(Some("t0ken"), Some(TlsTrust::PublicCa))
        ),
        Err(PassError::Network { .. })
    ));
    assert!(matches!(
        clone_store(world.phone_config("a", &[&key]), remote(Some("t0ken"), None)),
        Err(PassError::InvalidConfig { .. })
    ));
    // The token never goes over plain http.
    let plain = Remote::Http {
        url: url.replace("https://", "http://"),
        username: Some("alice".into()),
        password: Some("t0ken".into()),
        trust: None,
    };
    assert!(matches!(
        clone_store(world.phone_config("a", &[&key]), plain),
        Err(PassError::InvalidConfig { .. })
    ));
    assert!(!world.path("a").exists());

    let identity = unlock(&key);
    let phone = clone_store(world.phone_config("phone-store", &[&key]), good.clone()).unwrap();
    assert_eq!(phone.show("bank".into(), identity.clone()).unwrap().password, "hunter2");
    phone.insert("phone/first".into(), "x".into(), false, identity).unwrap();
    assert_eq!(phone.push(good.clone()).unwrap(), 1);
    assert_eq!(
        phone.push(remote(Some("wrong"), pinned)).err(),
        None,
        "nothing to push: nothing is sent"
    );
    world.git(&laptop, &["fetch", "--quiet", "relay"]);
    assert_eq!(
        world.log(&laptop, "main..relay/devices/phone"),
        ["Add given password for phone/first to store."]
    );
}
