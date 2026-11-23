//! What the e2e tests share: a throwaway world with its own GnuPG home, its own
//! git configuration and its own keys, the real `gpg`, `git` and `pass.py` run in it,
//! a real drop zone (`apps/pass/dropzone/entrypoint.sh`: OpenSSH's sshd) to sync through,
//! and fleets of devices made of this crate.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pass_core::{
    generate_device_key, generate_openpgp_key, review_line_text, Choice, DropZone, FetchOutcome, GeneratedKey,
    Identity, PassError, PassStore, Peer, ReviewLine, Side, StoreConfig, Taken,
};

/// No space in it: pass.py splits `PASSWORD_STORE_GPG_OPTS` on spaces.
pub const PASSPHRASE: &str = "correct-horse-battery-staple";

fn random_hex() -> String {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:08x}{:04x}{:02x}", nanos, std::process::id() & 0xffff, count & 0xff)
}

pub struct World {
    pub dir: PathBuf,
}

/// An OpenPGP key made by gpg in this world.
#[derive(Clone)]
pub struct Key {
    pub fingerprint: String,
    pub email: String,
    /// `gpg --export-secret-keys --armor`: what a phone imports.
    pub secret: String,
    /// `gpg --export --armor`.
    pub public: String,
}

impl Key {
    /// This key's device, as another device is told about it.
    pub fn peer(&self, name: &str) -> Peer {
        Peer {
            name: name.into(),
            fingerprint: self.fingerprint.clone(),
            public_key: self.public.clone(),
        }
    }
}

pub enum KeyKind {
    Rsa,
    Curve25519,
}

/// What a command printed, and whether it succeeded.
pub struct Ran {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Ran {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

pub fn pass_py() -> PathBuf {
    let path = std::env::var_os("PASS_PY")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../clones/pass/pass.py"));
    assert!(path.is_file(), "no pass.py at {}: set PASS_PY", path.display());
    path
}

impl World {
    pub fn new() -> Self {
        // Short: gpg-agent's socket lives under it, and a socket path has little room.
        let dir = std::env::temp_dir().join(format!("pass-e2e-{}", random_hex()));
        for sub in ["gnupg", "home"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("gnupg"), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::write(
            dir.join("gitconfig"),
            "[user]\n\tname = Computer\n\temail = computer@example.org\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        Self { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The environment every program of this world runs in: nothing of the machine's own gpg or git setup.
    pub fn env(&self) -> Vec<(String, String)> {
        let text = |path: PathBuf| path.to_string_lossy().into_owned();
        vec![
            ("GNUPGHOME".into(), text(self.dir.join("gnupg"))),
            ("HOME".into(), text(self.dir.join("home"))),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_GLOBAL".into(), text(self.dir.join("gitconfig"))),
            (
                "PASSWORD_STORE_GPG_OPTS".into(),
                format!("--pinentry-mode loopback --passphrase {PASSPHRASE}"),
            ),
        ]
    }

    pub fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.envs(self.env()).env_remove("PASSWORD_STORE_DIR");
        command
    }

    pub fn try_run(&self, mut command: Command, stdin: Option<&[u8]>) -> Ran {
        use std::io::Write;
        command
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("cannot run the command");
        if let Some(input) = stdin {
            child.stdin.take().unwrap().write_all(input).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        Ran {
            ok: output.status.success(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    pub fn run(&self, command: Command, stdin: Option<&[u8]>) -> Ran {
        let described = format!("{command:?}");
        let ran = self.try_run(command, stdin);
        assert!(ran.ok, "{described} failed:\n{}\n{}", ran.text(), ran.stderr);
        ran
    }

    /// `gpg --batch <args>`, with the passphrase at hand.
    pub fn gpg_command(&self, args: &[&str]) -> Command {
        let mut command = self.command("gpg");
        command
            .args([
                "--batch",
                "--yes",
                "--quiet",
                "--trust-model",
                "always",
                "--pinentry-mode",
                "loopback",
                "--passphrase",
                PASSPHRASE,
            ])
            .args(args);
        command
    }

    pub fn gpg(&self, args: &[&str], stdin: Option<&[u8]>) -> Ran {
        self.run(self.gpg_command(args), stdin)
    }

    /// Makes a key the way people have them: a primary key that signs, a subkey that
    /// decrypts, protected by a passphrase.
    pub fn make_key(&self, kind: KeyKind, email: &str) -> Key {
        let algorithms = match kind {
            KeyKind::Rsa => "Key-Type: RSA\nKey-Length: 2048\nSubkey-Type: RSA\nSubkey-Length: 2048",
            KeyKind::Curve25519 => "Key-Type: EDDSA\nKey-Curve: ed25519\nSubkey-Type: ECDH\nSubkey-Curve: cv25519",
        };
        let parameters = format!(
            "{algorithms}\nKey-Usage: sign,cert\nSubkey-Usage: encrypt\nName-Real: Test {email}\nName-Email: {email}\n\
             Expire-Date: 0\nPassphrase: {PASSPHRASE}\n%commit\n"
        );
        self.gpg(&["--gen-key"], Some(parameters.as_bytes()));
        let listing = self.gpg(&["--with-colons", "--list-keys", email], None).text();
        let fingerprint = listing
            .lines()
            .find_map(|line| line.strip_prefix("fpr:"))
            .map(|rest| rest.trim_matches(':').to_string())
            .expect("no fingerprint");
        Key {
            secret: self
                .gpg(&["--armor", "--export-secret-keys", &fingerprint], None)
                .text(),
            public: self.gpg(&["--armor", "--export", &fingerprint], None).text(),
            fingerprint,
            email: email.to_string(),
        }
    }

    /// Gives gpg a public key made elsewhere (on a phone), as its owner would: imported and trusted.
    pub fn import_key(&self, public_key: &str, fingerprint: &str) {
        self.gpg(&["--import"], Some(public_key.as_bytes()));
        // (Not through `gpg()`: with `--trust-model always` gpg does not make the trust database this writes to.)
        let mut trust = self.command("gpg");
        trust.args(["--batch", "--quiet", "--import-ownertrust"]);
        self.run(trust, Some(format!("{fingerprint}:6:\n").as_bytes()));
    }

    /// `gpg -d <file>`: what the real thing reads in a file of the store.
    pub fn gpg_decrypt(&self, file: &Path) -> String {
        self.gpg(&["--decrypt", &file.to_string_lossy()], None).text()
    }

    /// The long ids of the keys a file is encrypted to, as gpg sees them, sorted.
    pub fn encrypted_to(&self, file: &Path) -> Vec<String> {
        let mut command = self.gpg_command(&["--status-fd", "1", "--list-only", "--decrypt"]);
        command.arg(file);
        let mut ids: Vec<String> = self
            .try_run(command, None)
            .text()
            .lines()
            .filter_map(|line| line.strip_prefix("[GNUPG:] ENC_TO "))
            .filter_map(|rest| rest.split(' ').next().map(str::to_string))
            .collect();
        ids.sort();
        ids
    }

    pub fn pass_command(&self, store: &Path, args: &[&str]) -> Command {
        let mut command = self.command("python3");
        command.arg(pass_py()).args(args).env("PASSWORD_STORE_DIR", store);
        command
    }

    /// `pass <args>` on `store`, with the real `pass.py`.
    pub fn pass(&self, store: &Path, args: &[&str], stdin: Option<&[u8]>) -> String {
        self.run(self.pass_command(store, args), stdin).text()
    }

    pub fn git_command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut command = self.command("git");
        command.arg("-C").arg(dir).args(args);
        command
    }

    /// `git -C <dir> <args>`, with the real git.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        self.run(self.git_command(dir, args), None)
            .text()
            .trim_end()
            .to_string()
    }

    pub fn try_git(&self, dir: &Path, args: &[&str]) -> Ran {
        self.try_run(self.git_command(dir, args), None)
    }

    /// The subjects of the commits of `revision`, newest first.
    pub fn log(&self, dir: &Path, revision: &str) -> Vec<String> {
        self.git(dir, &["log", "--format=%s", revision])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A store as pass.py makes one: `pass init` for these keys, `pass git init`, two entries.
    pub fn computer_store(&self, recipients: &[&Key]) -> PathBuf {
        let store = self.path("computer-store");
        let mut args = vec!["init"];
        args.extend(recipients.iter().map(|key| key.fingerprint.as_str()));
        self.pass(&store, &args, None);
        self.pass(&store, &["git", "init"], None);
        self.pass(&store, &["insert", "-m", "bank"], Some(b"hunter2\nlogin: alice\n"));
        self.pass(&store, &["insert", "-m", "Email/work"], Some(b"s3cret\n"));
        store
    }

    /// The configuration of a device's store in `directory`: named `name`, whose OpenPGP
    /// public key is `public_key`, knowing `peers`.
    pub fn config(&self, directory: &str, name: &str, public_key: &str, peers: Vec<Peer>) -> StoreConfig {
        StoreConfig {
            directory: self.path(directory).to_string_lossy().into_owned(),
            device: name.into(),
            public_key: public_key.into(),
            peers,
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.command("gpgconf").args(["--kill", "all"]).output();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A drop zone: `apps/pass/dropzone/entrypoint.sh` running the real sshd on a free port.
/// Needs root and an account named `drop`, as the entrypoint does.
pub struct Zone {
    child: Child,
    data: PathBuf,
    pub port: u16,
    /// `SHA256:...`, as the drop zone printed it.
    pub host_key: String,
    pub known_hosts: PathBuf,
}

impl Drop for Zone {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

/// An SSH key made by `ssh-keygen`, for a computer.
pub struct SshKey {
    pub file: PathBuf,
    pub public: String,
}

impl World {
    pub fn ssh_key(&self, name: &str) -> SshKey {
        let file = self.path(&format!("{name}.ssh"));
        let mut command = Command::new("ssh-keygen");
        command
            .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
            .arg(&file);
        self.run(command, None);
        SshKey {
            public: std::fs::read_to_string(file.with_extension("ssh.pub"))
                .unwrap()
                .trim()
                .to_string(),
            file,
        }
    }

    /// Starts a drop zone that lets these SSH public keys in.
    pub fn zone(&self, authorized: &[&str]) -> Zone {
        let entrypoint = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/pass/dropzone/entrypoint.sh");
        // Not under /tmp: sshd refuses to chroot below a directory others can write.
        let data = PathBuf::from(format!("/var/lib/pass-dropzone-e2e-{}", random_hex()));
        std::fs::create_dir_all(&data).expect("the drop zone tests need root");
        let port = free_port();
        let log = self.path(&format!("zone-{port}.log"));
        let mut command = Command::new("sh");
        command
            .arg(entrypoint)
            .env("DROPZONE_DATA", &data)
            .env("DROPZONE_PORT", port.to_string())
            .env("DROPZONE_AUTHORIZED_KEYS", authorized.join("\n"))
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::File::options().append(true).open(&log).unwrap());
        let mut child = command.spawn().expect("cannot run the drop zone's entrypoint");
        let deadline = Instant::now() + Duration::from_secs(15);
        let host_key = loop {
            let said = std::fs::read_to_string(&log).unwrap_or_default();
            let printed = said
                .lines()
                .find_map(|line| line.strip_prefix("dropzone: host key "))
                .map(str::to_string);
            if let Some(host_key) = printed {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    break host_key;
                }
            }
            if let Ok(Some(status)) = child.try_wait() {
                panic!("the drop zone stopped ({status}): {said}\n(it needs root, sshd and an account named drop)");
            }
            assert!(Instant::now() < deadline, "the drop zone did not start: {said}");
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut scan = Command::new("ssh-keyscan");
        scan.args(["-p", &port.to_string(), "127.0.0.1"]);
        let known_hosts = self.path(&format!("known_hosts-{port}"));
        std::fs::write(&known_hosts, self.run(scan, None).stdout).unwrap();
        Zone {
            child,
            data,
            port,
            host_key,
            known_hosts,
        }
    }
}

impl Zone {
    pub fn url(&self) -> String {
        format!("sftp://drop@127.0.0.1:{}", self.port)
    }

    /// How a device with this SSH private key reaches the zone.
    pub fn for_device(&self, private_key: &str) -> DropZone {
        DropZone {
            url: self.url(),
            private_key: private_key.into(),
            host_key: self.host_key.clone(),
        }
    }

    /// The directory the files are in, on the server: for what only a faulty or hostile server would do.
    pub fn files(&self) -> PathBuf {
        self.data.join("jail/drop")
    }

    /// The options that make OpenSSH's `sftp` use this key and trust this server only.
    pub fn ssh_options(&self, key: &SshKey) -> String {
        format!(
            "-i {} -o UserKnownHostsFile={} -o StrictHostKeyChecking=yes -o IdentitiesOnly=yes -o BatchMode=yes",
            key.file.display(),
            self.known_hosts.display()
        )
    }

    /// Publishes `file` under `name` with OpenSSH's `sftp`, the way the protocol says: uploaded beside, renamed over.
    pub fn publish(&self, world: &World, key: &SshKey, file: &Path, name: &str) {
        let mut command = Command::new("sftp");
        command.args(["-q", "-b", "-", "-P", &self.port.to_string()]);
        command.args(self.ssh_options(key).split(' '));
        command.arg("drop@127.0.0.1");
        let batch = format!("put {} {name}.part\nrename {name}.part {name}\n", file.display());
        let ran = world.try_run(command, Some(batch.as_bytes()));
        assert!(ran.ok, "sftp could not publish {name}: {}", ran.stderr);
    }
}

/// A device made of this crate: what the phone is.
pub struct Device {
    pub name: String,
    pub key: GeneratedKey,
    pub store: Arc<PassStore>,
    pub identity: Arc<Identity>,
    pub zone: DropZone,
    /// Its store, to look at with the real git.
    pub dir: PathBuf,
}

impl Device {
    /// Publishes, and receives the others' files: one line per other device, as `pass-device sync` prints them.
    pub fn sync(&self) -> Vec<String> {
        let report = self.store.sync(self.zone.clone(), self.identity.clone()).unwrap();
        assert_eq!(report.publish_refused, None);
        report
            .peers
            .into_iter()
            .map(|peer| {
                let stands = match peer.outcome {
                    FetchOutcome::Refused { reason } => format!("REFUSED {reason}"),
                    FetchOutcome::Stale { .. } => "IGNORED".to_string(),
                    FetchOutcome::NothingPublished => "nothing published".to_string(),
                    _ if peer.state.to_take > 0 => format!("{} to take", peer.state.to_take),
                    _ => "nothing to take".to_string(),
                };
                format!("{}: {stands}", peer.state.device)
            })
            .collect()
    }

    /// How this device stands with `other` after a sync: what follows `other: ` in its line.
    pub fn stands_with(&self, other: &str) -> String {
        let prefix = format!("{other}: ");
        self.sync()
            .into_iter()
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_string))
            .expect("no such device")
    }

    /// What taking the update of `other` would do: the lines of SYNC.md, sorted.
    pub fn pending(&self, other: &str) -> Vec<String> {
        let mut lines: Vec<String> = self.review(other).into_iter().map(review_line_text).collect();
        lines.sort();
        lines
    }

    pub fn review(&self, other: &str) -> Vec<ReviewLine> {
        self.store
            .pending_update(other.into(), self.identity.clone())
            .unwrap()
            .map(|update| update.lines)
            .unwrap_or_default()
    }

    pub fn try_take(&self, other: &str, mine: &[&str], theirs: &[&str]) -> Result<Taken, PassError> {
        let choice = |keep: Side| {
            move |name: &&str| Choice {
                name: name.to_string(),
                keep,
            }
        };
        let choices = mine
            .iter()
            .map(choice(Side::Mine))
            .chain(theirs.iter().map(choice(Side::Theirs)))
            .collect();
        self.store
            .take_update(other.into(), None, choices, self.identity.clone())
    }

    /// Takes the update of `other` with these choices, then publishes.
    pub fn take(&self, other: &str, mine: &[&str], theirs: &[&str]) -> Taken {
        let taken = self.try_take(other, mine, theirs).unwrap();
        self.sync();
        taken
    }

    pub fn insert(&self, name: &str, content: &str) {
        self.store
            .insert(name.into(), content.into(), true, self.identity.clone())
            .unwrap();
    }

    pub fn show(&self, name: &str) -> Result<String, PassError> {
        self.store
            .content(name.into(), self.identity.clone())
            .map(|content| content.trim_end_matches('\n').to_string())
    }

    pub fn rm(&self, name: &str) {
        self.store.remove(name.into(), self.identity.clone()).unwrap();
    }

    pub fn mv(&self, from: &str, to: &str) {
        self.store
            .rename(from.into(), to.into(), false, self.identity.clone())
            .unwrap();
    }

    pub fn ls(&self) -> Vec<String> {
        self.store.list().unwrap()
    }

    pub fn set_readers(&self, folder: &str, readers: &[&str]) {
        let readers = readers.iter().map(|name| name.to_string()).collect();
        self.store
            .set_readers(folder.into(), readers, self.identity.clone())
            .unwrap();
    }

    /// The devices that should read what is in `folder`, sorted.
    pub fn readers(&self, folder: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .store
            .folder_access(folder.into())
            .unwrap()
            .readers
            .into_iter()
            .map(|reader| reader.device.unwrap_or(format!("?{}", reader.id)))
            .collect();
        names.sort();
        names
    }
}

/// Devices of this crate that know each other, and their drop zone.
pub struct Fleet {
    pub devices: Vec<Device>,
    pub zone: Zone,
    pub world: World,
}

impl Fleet {
    /// `names` devices of this crate; `extra_ssh` are more SSH public keys the drop zone lets in,
    /// and `extra_peers` more devices everyone knows (a computer played by gpg).
    pub fn with(world: World, names: &[&str], extra_ssh: &[&str], extra_peers: &[Peer]) -> Self {
        let keys: Vec<(GeneratedKey, pass_core::DeviceKey)> = names
            .iter()
            .map(|name| {
                (
                    generate_openpgp_key(name.to_string(), String::new()).unwrap(),
                    generate_device_key(name.to_string()).unwrap(),
                )
            })
            .collect();
        let mut authorized: Vec<&str> = keys.iter().map(|(_, ssh)| ssh.public_key.as_str()).collect();
        authorized.extend(extra_ssh);
        let zone = world.zone(&authorized);
        let devices = names
            .iter()
            .zip(&keys)
            .map(|(name, (key, ssh))| {
                let mut peers: Vec<Peer> = names
                    .iter()
                    .zip(&keys)
                    .filter(|(other, _)| other != &name)
                    .map(|(other, (key, _))| Peer {
                        name: other.to_string(),
                        fingerprint: key.fingerprint.clone(),
                        public_key: key.public_key.clone(),
                    })
                    .collect();
                peers.extend(extra_peers.iter().cloned());
                let directory = format!("{name}-store");
                Device {
                    name: name.to_string(),
                    store: PassStore::open(world.config(&directory, name, &key.public_key, peers)).unwrap(),
                    identity: Identity::unlock(key.secret_key.clone(), String::new()).unwrap(),
                    zone: zone.for_device(&ssh.private_key),
                    dir: world.path(&directory),
                    key: key.clone(),
                }
            })
            .collect();
        Self { devices, zone, world }
    }

    pub fn new(names: &[&str]) -> Self {
        Self::with(World::new(), names, &[], &[])
    }

    pub fn device(&self, name: &str) -> &Device {
        self.devices
            .iter()
            .find(|device| device.name == name)
            .expect("no such device")
    }

    /// A fleet whose devices all hold the same store, made by the first: root readers everyone
    /// (or `readers`), and these entries.
    pub fn started(names: &[&str], readers: Option<&[&str]>, entries: &[(&str, &str)]) -> Self {
        let fleet = Self::new(names);
        let first = &fleet.devices[0];
        let readers = readers.unwrap_or(names).iter().map(|name| name.to_string()).collect();
        first.store.create(readers, first.identity.clone()).unwrap();
        for (name, content) in entries {
            first.insert(name, content);
        }
        fleet.settle(&|_, _, _| panic!("no choice is expected here"));
        fleet
    }

    /// Everybody syncs and takes what there is, until nothing moves. `resolve` answers where a
    /// choice is needed: (device, other, name) → mine or theirs.
    pub fn settle(&self, resolve: &dyn Fn(&str, &str, &str) -> Side) {
        self.settle_among(resolve, &|_, _| true);
    }

    /// The same, where `takes(device, other)` says who takes from whom.
    pub fn settle_among(&self, resolve: &dyn Fn(&str, &str, &str) -> Side, takes: &dyn Fn(&str, &str) -> bool) {
        let mut quiet = 0;
        for _ in 0..16 {
            let mut moved = false;
            for device in &self.devices {
                for line in device.sync() {
                    let (other, stands) = line.split_once(": ").unwrap();
                    assert!(!stands.starts_with("REFUSED"), "{}: {line}", device.name);
                    if !stands.ends_with(" to take") || stands.starts_with("nothing") || !takes(&device.name, other) {
                        continue;
                    }
                    let (mut mine, mut theirs) = (Vec::new(), Vec::new());
                    for line in device.pending(other) {
                        if let Some(name) = line.strip_prefix("conflict ") {
                            match resolve(&device.name, other, name) {
                                Side::Mine => mine.push(name.to_string()),
                                Side::Theirs => theirs.push(name.to_string()),
                            }
                        }
                    }
                    fn borrow(names: &[String]) -> Vec<&str> {
                        names.iter().map(String::as_str).collect()
                    }
                    device.take(other, &borrow(&mine), &borrow(&theirs));
                    moved = true;
                }
            }
            // One quiet round is not enough: the first to sync had not seen what the last then published.
            quiet = if moved { 0 } else { quiet + 1 };
            if quiet == 2 {
                return;
            }
        }
        panic!("the devices never stop having something to take from each other");
    }

    /// All devices hold these entries, read them where they are readers and not elsewhere, and
    /// have nothing stale and nothing left to take.
    pub fn check(&self, contents: &[(&str, &str)]) {
        let mut names: Vec<&str> = contents.iter().map(|(name, _)| *name).collect();
        names.sort();
        for device in &self.devices {
            assert_eq!(device.ls(), names, "what {} lists", device.name);
            assert_eq!(
                device.store.stale().unwrap(),
                Vec::<String>::new(),
                "stale on {}",
                device.name
            );
            for (name, content) in contents {
                let folder = name.rsplit_once('/').map(|(folder, _)| folder).unwrap_or("");
                let shown = device.show(name);
                if device.readers(folder).contains(&device.name) {
                    assert_eq!(shown.as_deref(), Ok(*content), "{name} on {}", device.name);
                } else {
                    assert_eq!(shown, Err(PassError::NotForThisKey), "{name} on {}", device.name);
                }
            }
            for line in device.sync() {
                assert!(line.ends_with(": nothing to take"), "{} still has: {line}", device.name);
            }
            // git's own opinion of the repository: sound, and the directory is what the last commit says.
            self.world.git(&device.dir, &["fsck", "--strict", "--no-dangling"]);
            assert_eq!(self.world.git(&device.dir, &["status", "--porcelain"]), "");
        }
    }
}
