//! A stand-in for `sshd`, for the e2e tests: there is none on the machines that run them.
//!
//! It does what sshd does with an `authorized_keys` file whose lines carry a forced
//! command: a known key is let in, whatever command it asks for goes into
//! `SSH_ORIGINAL_COMMAND`, and the forced command of its line is run by `sh -c`.
//! So the lines the README tells people to write are the ones under test, with the
//! real `git-shell`, the real git behind it and the real `update` hook.
//! What is NOT under test is OpenSSH itself: the SSH protocol here is russh on both ends.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Arc;

use russh::keys::{Algorithm, HashAlg, PrivateKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};

/// One line of `authorized_keys`: a public key (`ssh-ed25519 AAAA...`) and its `command="..."`.
#[derive(Clone)]
pub struct AuthorizedKey {
    pub public_key: String,
    pub forced_command: String,
}

pub struct Sshd {
    pub port: u16,
    /// `SHA256:...`: what a client pins.
    pub host_key: String,
    shutdown: server::RunningServerHandle,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        self.shutdown.shutdown("the test is over".into());
    }
}

#[derive(Clone)]
struct Daemon {
    authorized_keys: Arc<Vec<AuthorizedKey>>,
    home: PathBuf,
    env: Arc<Vec<(String, String)>>,
}

struct Connection {
    daemon: Daemon,
    forced_command: Option<String>,
    stdin: HashMap<ChannelId, ChildStdin>,
}

impl server::Server for Daemon {
    type Handler = Connection;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Connection {
        Connection {
            daemon: self.clone(),
            forced_command: None,
            stdin: HashMap::new(),
        }
    }
}

/// The key of an `authorized_keys` line, without its comment.
fn key_part(line: &str) -> String {
    line.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

impl server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _user: &str, key: server::VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let offered = key_part(&key.public_key.to_openssh().unwrap_or_default());
        match self
            .daemon
            .authorized_keys
            .iter()
            .find(|line| key_part(&line.public_key) == offered)
        {
            Some(line) => {
                self.forced_command = Some(line.forced_command.clone());
                Ok(Auth::Accept)
            }
            None => Ok(Auth::reject()),
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    // The child is waited for by the thread that forwards its output.
    #[allow(clippy::zombie_processes)]
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let forced = self.forced_command.clone().expect("authenticated");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&forced)
            .env("SSH_ORIGINAL_COMMAND", String::from_utf8_lossy(data).as_ref())
            .env("HOME", &self.daemon.home)
            .envs(self.daemon.env.iter().cloned())
            .current_dir(&self.daemon.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cannot run sh");
        self.stdin.insert(channel, child.stdin.take().unwrap());
        let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        session.channel_success(channel)?;

        let handle = session.handle();
        let runtime = tokio::runtime::Handle::current();
        let (errors, errors_runtime) = (handle.clone(), runtime.clone());
        let error_pump = std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(count @ 1..) = stderr.read(&mut buffer) {
                let _ = errors_runtime.block_on(errors.extended_data(channel, 1, buffer[..count].to_vec()));
            }
        });
        std::thread::spawn(move || {
            let mut buffer = [0u8; 32 * 1024];
            while let Ok(count @ 1..) = stdout.read(&mut buffer) {
                if runtime
                    .block_on(handle.data(channel, buffer[..count].to_vec()))
                    .is_err()
                {
                    break;
                }
            }
            let status = child.wait().ok().and_then(|status| status.code()).unwrap_or(255);
            let _ = error_pump.join();
            runtime.block_on(async {
                let _ = handle.exit_status_request(channel, status as u32).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
        });
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _session: &mut Session) -> Result<(), Self::Error> {
        if let Some(stdin) = self.stdin.get_mut(&channel) {
            // A command that has already exited (a refusal) no longer reads: not this server's problem.
            let _ = stdin.write_all(data);
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        self.stdin.remove(&channel);
        Ok(())
    }
}

impl Sshd {
    /// Starts on a free port of 127.0.0.1. Commands run in `home`, with `env` added to the environment.
    pub fn start(authorized_keys: Vec<AuthorizedKey>, home: PathBuf, env: Vec<(String, String)>) -> Self {
        let host_key = PrivateKey::random(&mut rand10::rng(), Algorithm::Ed25519).unwrap();
        let fingerprint = host_key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![host_key],
            auth_rejection_time: std::time::Duration::from_millis(10),
            auth_rejection_time_initial: Some(std::time::Duration::ZERO),
            ..Default::default()
        });
        let mut daemon = Daemon {
            authorized_keys: Arc::new(authorized_keys),
            home,
            env: Arc::new(env),
        };
        let (started, ready) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let port = socket.local_addr().unwrap().port();
                let server = daemon.run_on_socket(config, &socket);
                started.send((port, server.handle())).unwrap();
                let _ = server.await;
            });
        });
        let (port, shutdown) = ready.recv().expect("the SSH server did not start");
        Self {
            port,
            host_key: fingerprint,
            shutdown,
        }
    }
}
