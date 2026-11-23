//! What the e2e tests share: a throwaway world with its own GnuPG home, its own
//! git configuration and its own keys, and the real `gpg`, `git` and `pass.py` run in it.

#![allow(dead_code)]

pub mod sshd;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pass_core::StoreConfig;

/// No space in it: pass.py splits `PASSWORD_STORE_GPG_OPTS` on spaces.
pub const PASSPHRASE: &str = "correct-horse-battery-staple";

pub struct World {
    pub dir: PathBuf,
}

/// An OpenPGP key made by gpg in this world.
#[derive(Clone)]
pub struct Key {
    pub fingerprint: String,
    pub email: String,
    /// `gpg --export-secret-keys --armor`: what the phone imports.
    pub secret: String,
    /// `gpg --export --armor`.
    pub public: String,
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

fn pass_py() -> PathBuf {
    let path = std::env::var_os("PASS_PY")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../clones/pass/pass.py"));
    assert!(path.is_file(), "no pass.py at {}: set PASS_PY", path.display());
    path
}

impl World {
    pub fn new() -> Self {
        // Short: gpg-agent's socket lives under it, and a socket path has little room.
        let dir = std::env::temp_dir().join(format!("pass-e2e-{}", pinned_http::random_hex(5)));
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
            "[user]\n\tname = Laptop\n\temail = laptop@example.org\n[init]\n\tdefaultBranch = main\n",
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
                format!("--pinentry-mode loopback --passphrase {}", PASSPHRASE),
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

    fn run(&self, command: Command, stdin: Option<&[u8]>) -> Ran {
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
                "--pinentry-mode",
                "loopback",
                "--passphrase",
            ])
            .arg(PASSPHRASE)
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
             Expire-Date: 0\nPassphrase: {}\n%commit\n",
            PASSPHRASE
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

    /// A store as its owner has it on the laptop: `pass init`, `pass git init`, two entries.
    pub fn laptop_store(&self, key: &Key) -> PathBuf {
        let store = self.path("laptop-store");
        self.pass(&store, &["init", &key.fingerprint], None);
        self.pass(&store, &["git", "init"], None);
        self.pass(&store, &["insert", "-m", "bank"], Some(b"hunter2\nlogin: alice\n"));
        self.pass(&store, &["insert", "-m", "Email/work"], Some(b"s3cret\n"));
        store
    }

    /// A bare repository holding the laptop's store, known to the laptop as `relay`: the phone's remote.
    pub fn relay(&self, laptop: &Path) -> PathBuf {
        let relay = self.path("relay/store.git");
        std::fs::create_dir_all(&relay).unwrap();
        self.git(&relay, &["init", "--bare", "--quiet", "--initial-branch=main"]);
        self.git(laptop, &["remote", "add", "relay", &relay.to_string_lossy()]);
        self.git(laptop, &["push", "--quiet", "relay", "main"]);
        relay
    }

    /// The configuration of the phone's store, in `directory`, knowing the public keys `public_keys`.
    pub fn phone_config(&self, directory: &str, public_keys: &[&Key]) -> StoreConfig {
        StoreConfig {
            directory: self.path(directory).to_string_lossy().into_owned(),
            public_keys: public_keys.iter().map(|key| key.public.clone()).collect(),
            signing_keys: Vec::new(),
            require_signed_updates: false,
            author_name: "Phone".into(),
            author_email: "phone@example.org".into(),
            device: "phone".into(),
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.command("gpgconf").args(["--kill", "all"]).output();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A process that is killed when the test ends.
pub struct Server {
    child: Child,
    pub port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl World {
    /// `git http-backend` over the repositories of `root`, behind tests/git_http_server.py.
    pub fn git_http_server(&self, root: &Path, extra: &[&str]) -> Server {
        let port = free_port();
        let child = self
            .command("python3")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/git_http_server.py"))
            .arg("--root")
            .arg(root)
            .args(["--port", &port.to_string()])
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cannot run python3");
        let deadline = Instant::now() + Duration::from_secs(15);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "the git http server did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Server { child, port }
    }
}
