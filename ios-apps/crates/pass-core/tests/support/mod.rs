//! What the e2e tests share: a throwaway world with its own GnuPG home, its own
//! git configuration and its own keys, the real `gpg`, `git` and `pass.py` run in it,
//! and a real drop zone (`apps/pass/dropzone/entrypoint.sh`: OpenSSH's sshd) to sync through.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pass_core::{DropZone, Peer, StoreConfig};

/// No space in it: pass.py splits `PASSWORD_STORE_GPG_OPTS` on spaces.
pub const PASSPHRASE: &str = "correct-horse-battery-staple";

fn random_hex() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{:08x}{:04x}", nanos, std::process::id() & 0xffff)
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
    pub fn peer(&self, name: &str, is_owner: bool) -> Peer {
        Peer {
            name: name.into(),
            fingerprint: self.fingerprint.clone(),
            public_key: self.public.clone(),
            is_owner,
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

    /// Gives gpg a public key made elsewhere (on the phone), as its owner would: imported and trusted.
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

    /// A store as its owner has it on the computer: `pass init` for these keys,
    /// `pass git init`, two entries.
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

    /// The configuration of a member's store in `directory`: the device `name`, whose
    /// OpenPGP public key is `public_key`, knowing `peers`.
    pub fn member_config(&self, directory: &str, name: &str, public_key: &str, peers: Vec<Peer>) -> StoreConfig {
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
    known_hosts: PathBuf,
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

    /// Runs a batch of OpenSSH's `sftp` as the holder of `key`.
    pub fn sftp(&self, world: &World, key: &SshKey, batch: &[String]) -> Ran {
        let mut command = Command::new("sftp");
        command.args(["-q", "-b", "-", "-P", &self.port.to_string()]);
        command.args(self.ssh_options(key).split(' '));
        command.arg("drop@127.0.0.1");
        world.try_run(command, Some(format!("{}\n", batch.join("\n")).as_bytes()))
    }

    /// Publishes `file` under `name` the way the protocol says: uploaded beside, renamed over.
    pub fn publish(&self, world: &World, key: &SshKey, file: &Path, name: &str) {
        let ran = self.sftp(
            world,
            key,
            &[
                format!("put {} {name}.part", file.display()),
                format!("rename {name}.part {name}"),
            ],
        );
        assert!(ran.ok, "sftp could not publish {name}: {}", ran.stderr);
    }

    /// Reads what is published under `name` into `to`. False when there is no such file.
    pub fn download(&self, world: &World, key: &SshKey, name: &str, to: &Path) -> bool {
        let _ = std::fs::remove_file(to);
        self.sftp(world, key, &[format!("get {name} {}", to.display())]).ok && to.is_file()
    }
}

/// The owner's side of SYNC.md, done by hand with the real `git`, `gpg` and `sftp`:
/// what `pass sync` does, and what the phone must interoperate with.
pub struct Computer<'a> {
    pub world: &'a World,
    pub zone: &'a Zone,
    pub store: PathBuf,
    pub name: String,
    pub key: Key,
    pub ssh: SshKey,
    /// The fingerprints of every device's OpenPGP key, the computer's included: who a published file is for.
    pub devices: Vec<String>,
}

impl Computer<'_> {
    fn file(&self, name: &str) -> PathBuf {
        self.world.path(name)
    }

    /// A bundle of the store's tip under `refs/pass-sync/<name>`, signed by this key and
    /// encrypted to every device. `gpg_options` is where `--compress-algo none` goes, or nothing.
    pub fn sealed_tip(&self, gpg_options: &[&str]) -> PathBuf {
        let reference = format!("refs/pass-sync/{}", self.name);
        self.world.git(&self.store, &["update-ref", &reference, "HEAD"]);
        let bundle = self.file("out.bundle");
        self.world.git(
            &self.store,
            &[
                "bundle",
                "create",
                "--quiet",
                "--version=2",
                &bundle.to_string_lossy(),
                &reference,
            ],
        );
        self.seal(&bundle, &self.key.fingerprint, &self.devices, gpg_options)
    }

    /// `gpg --sign --encrypt` of `file`, by `signer`, for `recipients`.
    pub fn seal(&self, file: &Path, signer: &str, recipients: &[String], gpg_options: &[&str]) -> PathBuf {
        let sealed = self.file(&format!("sealed-{}.gpg", random_hex()));
        let (file, output) = (
            file.to_string_lossy().into_owned(),
            sealed.to_string_lossy().into_owned(),
        );
        let mut args = vec!["--local-user", signer, "--sign", "--encrypt", "--output", &output];
        for recipient in recipients {
            args.extend(["--recipient", recipient]);
        }
        args.extend(gpg_options);
        args.push(&file);
        self.world.gpg(&args, None);
        sealed
    }

    /// `pass sync push`: publishes the store's tip.
    pub fn publish(&self, gpg_options: &[&str]) {
        let sealed = self.sealed_tip(gpg_options);
        self.put(&sealed);
    }

    /// Puts any file in the computer's place in the drop zone.
    pub fn put(&self, file: &Path) {
        self.zone
            .publish(self.world, &self.ssh, file, &format!("{}.bundle.gpg", self.name));
    }

    /// `pass sync fetch` for one member: takes its file if it is what SYNC.md says it must
    /// be, and returns its tip; or says why not.
    pub fn fetch(&self, member: &str, member_key: &str) -> Result<String, String> {
        let message = self.file("in.gpg");
        if !self
            .zone
            .download(self.world, &self.ssh, &format!("{member}.bundle.gpg"), &message)
        {
            return Err("nothing published".into());
        }
        // 1. It decrypts, and carries a valid signature by the member's key.
        let bundle = self.file("in.bundle");
        let mut decrypt =
            self.world
                .gpg_command(&["--status-fd", "2", "--output", &bundle.to_string_lossy(), "--decrypt"]);
        decrypt.arg(&message);
        let decrypted = self.world.try_run(decrypt, None);
        if !decrypted.ok {
            return Err(format!("gpg does not decrypt it: {}", decrypted.stderr));
        }
        let signed_by_member = decrypted
            .stderr
            .lines()
            .any(|line| line.starts_with("[GNUPG:] VALIDSIG ") && line.ends_with(member_key));
        if !signed_by_member {
            return Err(format!("not signed by {member}: {}", decrypted.stderr));
        }
        // 2. One reference, the member's.
        let reference = format!("refs/pass-sync/{member}");
        let heads = self
            .world
            .git(&self.store, &["bundle", "list-heads", &bundle.to_string_lossy()]);
        let [head] = heads.lines().collect::<Vec<_>>()[..] else {
            return Err(format!("not one reference: {heads}"));
        };
        let Some((tip, name)) = head.split_once(' ') else {
            return Err(format!("a malformed reference: {head}"));
        };
        if name != reference {
            return Err(format!("the wrong reference: {name}"));
        }
        // 3. Complete: git itself checks it, and that it asks for nothing it does not hold.
        let verified = self
            .world
            .try_git(&self.store, &["bundle", "verify", &bundle.to_string_lossy()]);
        if !verified.ok || verified.stderr.contains("requires") {
            return Err(format!("git bundle verify: {}", verified.stderr));
        }
        self.world.git(
            &self.store,
            &[
                "fetch",
                "--quiet",
                &bundle.to_string_lossy(),
                &format!("+{reference}:{reference}"),
            ],
        );
        self.world.git(&self.store, &["fsck", "--strict", "--no-dangling"]);
        Ok(tip.to_string())
    }

    /// `pass sync merge`: takes the member's commits when SYNC.md allows it (all signed by
    /// the member or the owner, nothing but entries touched, a fast-forward), then publishes.
    pub fn merge(&self, member: &str, member_key: &str) -> Result<(), String> {
        let reference = format!("refs/pass-sync/{member}");
        let range = format!("HEAD..{reference}");
        let signatures = self.world.git(&self.store, &["log", "--format=%G? %GF %s", &range]);
        for line in signatures.lines() {
            let signed = [member_key, self.key.fingerprint.as_str()]
                .iter()
                .any(|key| line.starts_with(&format!("G {key} ")));
            if !signed {
                return Err(format!("a commit not signed by {member} or the owner: {line}"));
            }
        }
        let touched = self.world.git(
            &self.store,
            &["diff", "--name-only", "--no-renames", "HEAD", &reference],
        );
        if let Some(other) = touched
            .lines()
            .find(|path| !path.ends_with(".gpg") || path.split('/').any(|part| part.starts_with('.')))
        {
            return Err(format!("it touches something that is not an entry: {other}"));
        }
        let merged = self
            .world
            .try_git(&self.store, &["merge", "--quiet", "--ff-only", &reference]);
        if !merged.ok {
            return Err(format!("not a fast-forward: {}", merged.stderr));
        }
        self.publish(&["--compress-algo", "none"]);
        Ok(())
    }
}
