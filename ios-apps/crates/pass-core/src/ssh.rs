//! SSH with no `ssh` program: `russh` (pure Rust, on `ring`) opens a session,
//! starts a subsystem in it (SFTP: see sftp.rs) and hands its input and output over
//! as blocking streams.
//!
//! The server is known by its host key, pinned by SHA-256 fingerprint as OpenSSH
//! prints it (`SHA256:...`): there is no known_hosts file and no trust on first use
//! without the user having compared it. The device is known by an Ed25519 key that
//! never leaves it.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{DeviceKey, HostKey, PassError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// For one silence, not for a whole transfer.
const IO_TIMEOUT: Duration = Duration::from_secs(60);

fn invalid(message: &str) -> PassError {
    PassError::InvalidConfig {
        message: message.to_string(),
    }
}

fn network(context: &str, e: impl std::fmt::Display) -> PassError {
    PassError::Network {
        message: format!("{context}: {e}"),
    }
}

/// Where a drop zone is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) user: String,
    pub(crate) host: String,
    pub(crate) port: u16,
}

/// Reads `sftp://user@host[:port]`. There is no path: a drop zone is one directory, the one the session starts in.
pub(crate) fn parse_url(url: &str) -> Result<Target, PassError> {
    let bad = || invalid("the drop zone is sftp://user@host[:port], as its README gives it");
    let url = url.trim();
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid("the drop zone URL holds a space or a control character"));
    }
    let rest = url.strip_prefix("sftp://").ok_or_else(bad)?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains('/') {
        return Err(bad());
    }
    let (user, host_port) = authority.split_once('@').ok_or_else(bad)?;
    let (host, port) = if let Some(v6) = host_port.strip_prefix('[') {
        let (host, after) = v6.split_once(']').ok_or_else(bad)?;
        (host, after.strip_prefix(':'))
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    let port = match port {
        None | Some("") => 22,
        Some(port) => port.parse().map_err(|_| bad())?,
    };
    if user.is_empty() || user.contains(':') || host.is_empty() || host.starts_with('-') {
        return Err(bad());
    }
    Ok(Target {
        user: user.to_string(),
        host: host.to_string(),
        port,
    })
}

/// `SHA256:...` with whatever was pasted around it removed (`ssh-keygen -lf` prints `256 SHA256:... host (ED25519)`).
fn normalize_fingerprint(fingerprint: &str) -> Result<String, PassError> {
    fingerprint
        .split_whitespace()
        .find(|word| word.starts_with("SHA256:") && word.len() > "SHA256:".len())
        .map(str::to_string)
        .ok_or_else(|| invalid("the host key fingerprint looks like SHA256:..., as `ssh-keygen -lf` prints it"))
}

/// Accepts the pinned host key and no other, and remembers what was served.
struct Verifier {
    pinned: Option<String>,
    served: Arc<Mutex<Option<HostKey>>>,
}

impl client::Handler for Verifier {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        // A host certificate would need a CA to check it against: there is none here.
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        *self.served.lock().unwrap() = Some(HostKey {
            algorithm: key.algorithm().to_string(),
            fingerprint: fingerprint.clone(),
        });
        Ok(self.pinned.as_deref() == Some(fingerprint.as_str()))
    }
}

fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: Some(IO_TIMEOUT),
        ..Default::default()
    })
}

async fn connect(
    target: &Target,
    pinned: Option<String>,
    served: Arc<Mutex<Option<HostKey>>>,
) -> Result<Handle<Verifier>, PassError> {
    let verifier = Verifier {
        pinned: pinned.clone(),
        served: served.clone(),
    };
    let address = (target.host.as_str(), target.port);
    match tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config(), address, verifier)).await {
        Err(_) => Err(network("cannot connect", "timed out")),
        Ok(Ok(handle)) => Ok(handle),
        Ok(Err(e)) => Err(match (pinned, served.lock().unwrap().clone()) {
            (Some(pinned), Some(served)) if served.fingerprint != pinned => PassError::HostKeyMismatch {
                pinned,
                served: served.fingerprint,
            },
            _ => network("cannot connect", e),
        }),
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, PassError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| network("cannot start", e))
}

/// Connects and reports the host key the server presents, authenticating nothing.
/// It is NOT trusted by this call: the fingerprint is to be compared with the one
/// read on the server itself (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`).
pub(crate) fn fetch_host_key(url: &str) -> Result<HostKey, PassError> {
    let target = parse_url(url)?;
    let served: Arc<Mutex<Option<HostKey>>> = Arc::default();
    let result = runtime()?.block_on(connect(&target, None, served.clone()));
    let seen = served.lock().unwrap().clone();
    match (seen, result) {
        (Some(key), _) => Ok(key),
        (None, Err(e)) => Err(e),
        (None, Ok(_)) => Err(network("cannot read the host key", "the server sent none")),
    }
}

/// A new Ed25519 key for this device. The private half is for the keychain,
/// the public half is a line for the server's `authorized_keys`.
pub(crate) fn generate_key(comment: &str) -> Result<DeviceKey, PassError> {
    let failed = |e: &dyn std::fmt::Display| PassError::InvalidKey {
        message: format!("cannot make an SSH key: {e}"),
    };
    let mut key = PrivateKey::random(&mut rand10::rng(), Algorithm::Ed25519).map_err(|e| failed(&e))?;
    let comment: String = comment.chars().filter(|c| !c.is_control()).collect();
    key.set_comment(comment.trim());
    Ok(DeviceKey {
        private_key: key.to_openssh(LineEnding::LF).map_err(|e| failed(&e))?.to_string(),
        public_key: key.public_key().to_openssh().map_err(|e| failed(&e))?,
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// The `authorized_keys` line of a private key.
pub(crate) fn public_key_of(private_key: &str) -> Result<DeviceKey, PassError> {
    let key = parse_private_key(private_key)?;
    Ok(DeviceKey {
        private_key: String::new(),
        public_key: key.public_key().to_openssh().map_err(|e| PassError::InvalidKey {
            message: format!("cannot write the SSH public key: {e}"),
        })?,
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

fn parse_private_key(text: &str) -> Result<PrivateKey, PassError> {
    let key = PrivateKey::from_openssh(text.trim()).map_err(|e| PassError::InvalidKey {
        message: format!("not an OpenSSH private key: {e}"),
    })?;
    if key.is_encrypted() {
        return Err(PassError::InvalidKey {
            message: "this SSH key is protected by a passphrase: generate one on this device instead".into(),
        });
    }
    Ok(key)
}

/// An SSH server, and who this device is to it.
pub(crate) struct SshRemote {
    target: Target,
    key: Arc<PrivateKey>,
    host_key: String,
}

/// What a remote command printed on its error stream and how it ended.
#[derive(Default)]
pub(crate) struct Ending {
    stderr: Vec<u8>,
    exit_status: Option<u32>,
    failure: Option<PassError>,
}

/// The standard output of the remote command.
pub(crate) struct Output {
    chunks: Receiver<Vec<u8>>,
    current: Vec<u8>,
    position: usize,
    ending: Arc<Mutex<Ending>>,
}

/// The standard input of the remote command. Dropping it closes it.
pub(crate) struct Input {
    sender: UnboundedSender<Vec<u8>>,
}

impl Read for Output {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.position == self.current.len() {
            match self.chunks.recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.position = 0;
                }
                // The session is over: an end of file, unless it broke.
                Err(_) => {
                    return match self.ending.lock().unwrap().failure.clone() {
                        Some(failure) => Err(std::io::Error::other(failure.to_string())),
                        None => Ok(0),
                    }
                }
            }
        }
        let count = buf.len().min(self.current.len() - self.position);
        buf[..count].copy_from_slice(&self.current[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

impl Write for Input {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sender
            .send(buf.to_vec())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the SSH session is over"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Output {
    /// A handle on how the command ends, that outlives the stream.
    pub(crate) fn outcome(&self) -> Outcome {
        Outcome(self.ending.clone())
    }
}

pub(crate) struct Outcome(Arc<Mutex<Ending>>);

impl Outcome {
    /// Why the command failed, if it did: a session error, or what it said before exiting with an error.
    pub(crate) fn failure(&self) -> Option<PassError> {
        let ending = self.0.lock().unwrap();
        if let Some(failure) = &ending.failure {
            return Some(failure.clone());
        }
        match ending.exit_status {
            Some(0) | None => None,
            Some(status) => {
                let said = String::from_utf8_lossy(&ending.stderr).trim().to_string();
                Some(PassError::Network {
                    message: if said.is_empty() {
                        format!("the server's git exited with status {status}")
                    } else {
                        format!("the server says: {said}")
                    },
                })
            }
        }
    }
}

impl SshRemote {
    /// Validates everything. Connects to nothing.
    pub(crate) fn new(url: &str, private_key: &str, host_key: &str) -> Result<Self, PassError> {
        Ok(Self {
            target: parse_url(url)?,
            key: Arc::new(parse_private_key(private_key)?),
            host_key: normalize_fingerprint(host_key)?,
        })
    }

    /// Opens a session and starts the subsystem `name` (`sftp`) in it. Returns once it
    /// is running, or with the reason it could not.
    pub(crate) fn subsystem(&self, name: &str) -> Result<(Output, Input), PassError> {
        let command = name.to_string();
        let (target, key, pinned) = (self.target.clone(), self.key.clone(), self.host_key.clone());
        let (chunk_sender, chunks) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
        let (sender, input) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (started_sender, started) = std::sync::mpsc::sync_channel::<Result<(), PassError>>(1);
        let ending: Arc<Mutex<Ending>> = Arc::default();
        let session_ending = ending.clone();
        let runtime = runtime()?;
        std::thread::spawn(move || {
            let result = runtime.block_on(session(
                target,
                key,
                pinned,
                command,
                &started_sender,
                chunk_sender,
                input,
                &session_ending,
            ));
            if let Err(failure) = result {
                // Before the command runs, the caller is waiting for this; after, the reader finds it.
                session_ending.lock().unwrap().failure = Some(failure.clone());
                let _ = started_sender.try_send(Err(failure));
            }
        });
        started
            .recv()
            .map_err(|_| network("cannot connect", "the SSH session ended"))??;
        Ok((
            Output {
                chunks,
                current: Vec::new(),
                position: 0,
                ending,
            },
            Input { sender },
        ))
    }
}

#[allow(clippy::too_many_arguments)]
async fn session(
    target: Target,
    key: Arc<PrivateKey>,
    pinned: String,
    command: String,
    started: &SyncSender<Result<(), PassError>>,
    chunks: SyncSender<Vec<u8>>,
    mut input: UnboundedReceiver<Vec<u8>>,
    ending: &Mutex<Ending>,
) -> Result<(), PassError> {
    let mut handle = connect(&target, Some(pinned), Arc::default()).await?;
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|e| network("cannot authenticate", e))?
        .flatten();
    let authenticated = handle
        .authenticate_publickey(target.user.clone(), PrivateKeyWithHashAlg::new(key, hash))
        .await
        .map_err(|e| network("cannot authenticate", e))?;
    if !authenticated.success() {
        return Err(PassError::Unauthorized);
    }
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| network("cannot open a session", e))?;
    channel
        .request_subsystem(true, command)
        .await
        .map_err(|e| network("the server does not do SFTP", e))?;
    let _ = started.try_send(Ok(()));

    let (mut output, writer) = channel.split();
    let mut input_open = true;
    loop {
        tokio::select! {
            message = output.wait() => match message {
                None | Some(ChannelMsg::Close) => break,
                Some(ChannelMsg::Data { data }) => {
                    // The reader may be slow (it is indexing a pack): wait for it off the runtime's thread.
                    let chunks = chunks.clone();
                    let delivered = tokio::task::spawn_blocking(move || chunks.send(data.to_vec())).await;
                    if !matches!(delivered, Ok(Ok(()))) {
                        break; // Nobody reads anymore.
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let mut ending = ending.lock().unwrap();
                    if ending.stderr.len() < 16 * 1024 {
                        ending.stderr.extend_from_slice(&data);
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    ending.lock().unwrap().exit_status = Some(exit_status);
                }
                Some(_) => {}
            },
            bytes = input.recv(), if input_open => match bytes {
                Some(bytes) => writer
                    .data_bytes(bytes)
                    .await
                    .map_err(|e| network("the connection broke", e))?,
                None => {
                    input_open = false;
                    writer.eof().await.map_err(|e| network("the connection broke", e))?;
                }
            },
        }
    }
    let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let target = |user: &str, host: &str, port: u16| Target {
            user: user.into(),
            host: host.into(),
            port,
        };
        assert_eq!(
            parse_url("sftp://drop@zone.example:2222").unwrap(),
            target("drop", "zone.example", 2222)
        );
        assert_eq!(
            parse_url(" sftp://drop@zone.example/ ").unwrap(),
            target("drop", "zone.example", 22)
        );
        assert_eq!(
            parse_url("sftp://drop@[::1]:2200").unwrap(),
            target("drop", "::1", 2200)
        );
        for bad in [
            "",
            "sftp://zone.example", // no user: there is no ~/.ssh/config to find one in
            "sftp://drop@zone.example/some/path",
            "sftp://drop@zone.example:port",
            "sftp://drop@-oProxyCommand=x",
            "sftp://drop:secret@zone.example",
            "sftp://@zone.example",
            "sftp://drop@",
            "ssh://drop@zone.example",
            "drop@zone.example:store",
            "https://zone.example",
        ] {
            assert!(
                matches!(parse_url(bad), Err(PassError::InvalidConfig { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn fingerprints_as_pasted() {
        let fingerprint = "SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8";
        assert_eq!(normalize_fingerprint(fingerprint).unwrap(), fingerprint);
        assert_eq!(
            normalize_fingerprint(&format!("256 {fingerprint} root@relay (ED25519)\n")).unwrap(),
            fingerprint
        );
        assert!(normalize_fingerprint("").is_err());
        assert!(normalize_fingerprint("SHA256:").is_err());
        assert!(normalize_fingerprint("aa:bb:cc").is_err());
    }

    #[test]
    fn device_keys() {
        let key = generate_key("pass on phone\n").unwrap();
        assert!(key.private_key.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(key.public_key.starts_with("ssh-ed25519 AAAA"));
        assert!(key.public_key.ends_with(" pass on phone"));
        assert!(key.fingerprint.starts_with("SHA256:"));
        let again = public_key_of(&key.private_key).unwrap();
        assert_eq!((again.public_key, again.fingerprint), (key.public_key, key.fingerprint));
        assert_ne!(
            generate_key("x").unwrap().fingerprint,
            generate_key("x").unwrap().fingerprint
        );
        assert!(matches!(public_key_of("nope"), Err(PassError::InvalidKey { .. })));
        assert!(SshRemote::new("sftp://drop@h", &key.private_key, "SHA256:abc").is_ok());
    }
}
