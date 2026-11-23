//! `pass-device`: one device of a pass sync, on the command line.
//!
//! **A test and debugging tool, not a password manager**: its OpenPGP key has no passphrase
//! and sits in a file next to the store. It exists so that `apps/pass/scenarios/run.py` can
//! drive the phone's core through the same situations as `pass.py`, and so that a sync can be
//! looked at by hand. Its commands and what they print are a contract: see
//! `apps/pass/scenarios/README.md`.
//!
//! ```sh
//! export PASS_DEVICE_HOME=/some/dir          # the store, the keys and the settings live there
//! cargo run -p pass-core --example pass-device -- init phone --dropzone sftp://drop@host:2222 \
//!     --host-key SHA256:... --ssh-key ./phone.ssh
//! ```
//!
//! Output is on stdout, one item per line. On failure: a message on stderr, a non-zero
//! status, and nothing changed.

use std::path::PathBuf;
use std::sync::Arc;

use pass_core::{
    generate_openpgp_key, inspect_key, public_key_of, review_line_text, Choice, DropZone, FetchOutcome, Identity,
    PassStore, Peer, Side, StoreConfig,
};

type Failure = Box<dyn std::error::Error>;

fn fail<T>(message: impl Into<String>) -> Result<T, Failure> {
    Err(message.into().into())
}

/// Where everything of this device is.
struct Home(PathBuf);

impl Home {
    fn from_environment() -> Result<Self, Failure> {
        match std::env::var_os("PASS_DEVICE_HOME") {
            Some(home) if !home.is_empty() => Ok(Self(PathBuf::from(home))),
            _ => fail("set PASS_DEVICE_HOME to the directory of this device"),
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn read(&self, name: &str) -> Result<String, Failure> {
        std::fs::read_to_string(self.file(name)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "no device here yet: run `pass-device init` first".into(),
            _ => format!("cannot read {name}: {e}").into(),
        })
    }

    fn write(&self, name: &str, content: &str) -> Result<(), Failure> {
        let path = self.file(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content).map_err(|e| format!("cannot write {}: {e}", path.display()).into())
    }

    fn peers(&self) -> Result<Vec<Peer>, Failure> {
        let mut peers = Vec::new();
        let Ok(files) = std::fs::read_dir(self.file("peers")) else {
            return Ok(peers);
        };
        for file in files {
            let path = file?.path();
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let public_key = std::fs::read_to_string(&path)?;
            peers.push(Peer {
                name: name.to_string(),
                fingerprint: inspect_key(public_key.clone())?.fingerprint,
                public_key,
            });
        }
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(peers)
    }

    /// The store of this device, and its key unlocked.
    fn open(&self) -> Result<(Arc<PassStore>, Arc<Identity>), Failure> {
        let secret_key = self.read("secret-key.asc")?;
        let store = PassStore::open(StoreConfig {
            directory: self.file("store").to_string_lossy().into_owned(),
            device: self.read("name")?.trim().to_string(),
            public_key: public_key_of(secret_key.clone())?,
            peers: self.peers()?,
        })?;
        Ok((store, Identity::unlock(secret_key, String::new())?))
    }

    fn drop_zone(&self) -> Result<DropZone, Failure> {
        Ok(DropZone {
            url: self.read("dropzone")?.trim().to_string(),
            private_key: self.read("ssh-key")?,
            host_key: self.read("host-key")?.trim().to_string(),
        })
    }
}

/// The value that follows `--name` in the arguments, taken out of them.
fn option(arguments: &mut Vec<String>, name: &str) -> Result<String, Failure> {
    let Some(position) = arguments.iter().position(|argument| argument == name) else {
        return fail(format!("missing {name}"));
    };
    if position + 1 >= arguments.len() {
        return fail(format!("{name} needs a value"));
    }
    arguments.remove(position);
    Ok(arguments.remove(position))
}

fn exactly<const N: usize>(arguments: Vec<String>, usage: &str) -> Result<[String; N], Failure> {
    arguments
        .try_into()
        .map_err(|_| format!("usage: pass-device {usage}").into())
}

/// One line per other device, after a sync.
fn sync(store: &PassStore, zone: DropZone, identity: Arc<Identity>) -> Result<Vec<String>, Failure> {
    let report = store.sync(zone, identity)?;
    Ok(report
        .peers
        .into_iter()
        .map(|peer| {
            let stands = match peer.outcome {
                FetchOutcome::Refused { reason } => format!("REFUSED {reason}"),
                FetchOutcome::Stale { .. } => "IGNORED an older file than the one already received".to_string(),
                FetchOutcome::NothingPublished => "nothing published".to_string(),
                FetchOutcome::Unchanged | FetchOutcome::Received if peer.state.to_take > 0 => {
                    format!("{} to take", peer.state.to_take)
                }
                FetchOutcome::Unchanged | FetchOutcome::Received => "nothing to take".to_string(),
            };
            format!("{}: {stands}", peer.state.device)
        })
        .collect())
}

fn run(command: &str, mut arguments: Vec<String>) -> Result<Vec<String>, Failure> {
    let home = Home::from_environment()?;
    let printed = match command {
        "init" => {
            let dropzone = option(&mut arguments, "--dropzone")?;
            let host_key = option(&mut arguments, "--host-key")?;
            let ssh_key = option(&mut arguments, "--ssh-key")?;
            let [name] = exactly(
                arguments,
                "init NAME --dropzone URL --host-key SHA256:... --ssh-key FILE",
            )?;
            if home.file("secret-key.asc").exists() {
                return fail("there is already a device here");
            }
            let ssh_key = std::fs::read_to_string(&ssh_key).map_err(|e| format!("cannot read {ssh_key}: {e}"))?;
            // The user id is the name and nothing else: a `.gpg-id` line that says `phone` then names this key only.
            let key = generate_openpgp_key(name.clone(), String::new())?;
            // Checked before anything is written: the name, the URL and the keys must make a device.
            PassStore::open(StoreConfig {
                directory: home.file("store").to_string_lossy().into_owned(),
                device: name.clone(),
                public_key: key.public_key.clone(),
                peers: Vec::new(),
            })?;
            pass_core::device_public_key(ssh_key.clone())?;
            home.write("name", &name)?;
            home.write("dropzone", &dropzone)?;
            home.write("host-key", &host_key)?;
            home.write("ssh-key", &ssh_key)?;
            home.write("secret-key.asc", &key.secret_key)?;
            vec![key.fingerprint]
        }
        "public-key" => {
            exactly::<0>(arguments, "public-key")?;
            vec![public_key_of(home.read("secret-key.asc")?)?.trim_end().to_string()]
        }
        "peer" => {
            let [name, file] = exactly(arguments, "peer NAME PUBLIC_KEY_FILE")?;
            let public_key = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
            if inspect_key(public_key.clone())?.is_secret {
                return fail("that is a secret key: a device is known by its public key");
            }
            let path = format!("peers/{name}.asc");
            let before = std::fs::read_to_string(home.file(&path)).ok();
            home.write(&path, &public_key)?;
            // Not kept when it does not make a valid set of devices (a bad name, a key already known).
            if let Err(e) = home.open() {
                match before {
                    Some(before) => home.write(&path, &before)?,
                    None => std::fs::remove_file(home.file(&path))?,
                }
                return Err(e);
            }
            Vec::new()
        }
        "create" => {
            let (store, identity) = home.open()?;
            store.create(arguments, identity)?;
            Vec::new()
        }
        "insert" => {
            let [name, content] = exactly(arguments, "insert NAME CONTENT")?;
            let (store, identity) = home.open()?;
            store.insert(name, content, true, identity)?;
            Vec::new()
        }
        "show" => {
            let [name] = exactly(arguments, "show NAME")?;
            let (store, identity) = home.open()?;
            let content = store.content(name, identity)?;
            vec![content.strip_suffix('\n').unwrap_or(&content).to_string()]
        }
        "rm" => {
            let [name] = exactly(arguments, "rm NAME | rm FOLDER/")?;
            let (store, identity) = home.open()?;
            if name.ends_with('/') {
                store.remove_folder(name, identity)?;
            } else {
                store.remove(name, identity)?;
            }
            Vec::new()
        }
        "mv" => {
            let [from, to] = exactly(arguments, "mv FROM TO | mv FROM/ TO/")?;
            let (store, identity) = home.open()?;
            match (from.ends_with('/'), to.ends_with('/')) {
                (true, true) => store.rename_folder(from, to, identity)?,
                (false, false) => store.rename(from, to, false, identity)?,
                _ => return fail("an entry moves to an entry name, a folder (with its slash) to a folder"),
            }
            Vec::new()
        }
        "ls" => {
            exactly::<0>(arguments, "ls")?;
            home.open()?.0.list()?
        }
        "readers" => {
            let [folder] = exactly(arguments, "readers DIR")?;
            let access = home.open()?.0.folder_access(folder)?;
            let mut names: Vec<String> = access
                .readers
                .into_iter()
                .map(|reader| reader.device.unwrap_or(format!("?{}", reader.id)))
                .collect();
            names.sort();
            names
        }
        "set-readers" => {
            if arguments.is_empty() {
                return fail("usage: pass-device set-readers DIR [DEVICE...]");
            }
            let folder = arguments.remove(0);
            let (store, identity) = home.open()?;
            store.set_readers(folder, arguments, identity)?;
            Vec::new()
        }
        "stale" => {
            exactly::<0>(arguments, "stale")?;
            home.open()?.0.stale()?
        }
        "sync" => {
            exactly::<0>(arguments, "sync")?;
            let (store, identity) = home.open()?;
            sync(&store, home.drop_zone()?, identity)?
        }
        "pending" => {
            let [device] = exactly(arguments, "pending DEVICE")?;
            let (store, identity) = home.open()?;
            match store.pending_update(device, identity)? {
                Some(update) => update.lines.into_iter().map(review_line_text).collect(),
                None => Vec::new(),
            }
        }
        "take" => {
            let mut choices = Vec::new();
            for (flag, keep) in [("--mine", Side::Mine), ("--theirs", Side::Theirs)] {
                while arguments.iter().any(|argument| argument == flag) {
                    choices.push(Choice {
                        name: option(&mut arguments, flag)?,
                        keep,
                    });
                }
            }
            let [device] = exactly(arguments, "take DEVICE [--mine NAME]... [--theirs NAME]...")?;
            let (store, identity) = home.open()?;
            store.take_update(device, None, choices, identity.clone())?;
            // "Then it publishes."
            sync(&store, home.drop_zone()?, identity)?;
            Vec::new()
        }
        "drop" => {
            let [device] = exactly(arguments, "drop DEVICE")?;
            home.open()?.0.drop_to(device)?;
            Vec::new()
        }
        _ => return fail(format!("unknown command {command}: see apps/pass/scenarios/README.md")),
    };
    Ok(printed)
}

fn main() {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty() {
        eprintln!("usage: pass-device COMMAND ...   (see apps/pass/scenarios/README.md)");
        std::process::exit(2);
    }
    let command = arguments.remove(0);
    match run(&command, arguments) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(e) => {
            eprintln!("pass-device {command}: {e}");
            std::process::exit(1);
        }
    }
}
