//! A password store: the layout of `pass`, the standard Unix password manager
//! (<https://www.passwordstore.org>), OpenPGP files in a git repository, with
//! neither `gpg` nor `git`. Exposed to Swift through UniFFI.
//!
//! Three things are kept apart:
//! - the store, a directory ([`PassStore`]): listing it needs no key;
//! - the identity, a secret key and its passphrase held in memory ([`Identity`]):
//!   reading an entry and changing the store need it, dropping it locks the store;
//! - the remote ([`Remote`]), given to each call that goes on the network.
//!
//! Sync never merges. This device commits its changes on its copy of the main
//! branch and pushes them to a branch of its own, `devices/<name>`; it takes the
//! main branch back only by fast-forward, once whoever keeps the store has merged
//! them. A change of recipients coming from the remote is shown before it is used.
//!
//! Every call blocks: call them off the main thread. Nothing decrypted is ever
//! written to disk; the secret key is never written at all.

mod crypto;
mod git;
mod http;
mod layout;
mod otp;
mod push;
mod ssh;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gix::ObjectId;
use zeroize::Zeroizing;

use crate::crypto::{Certificate, SecretIdentity};

uniffi::setup_scaffolding!();

/// What the phone calls itself where `pass` names the editor: `Edit password for x using pass-ios.`
const EDITOR: &str = "pass-ios";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PassError {
    #[error("{message}")]
    InvalidName { message: String },
    #[error("{name} is not in the password store")]
    NotFound { name: String },
    #[error("an entry already exists for {name}")]
    AlreadyExists { name: String },
    #[error("the store has no .gpg-id: run `pass init <gpg-id>` on the computer first")]
    NotInitialized,
    #[error("no public key on this device for the recipient {id}: import it before writing here")]
    UnknownRecipient { id: String },
    #[error("{message}")]
    UntrustedRecipients { message: String },
    #[error("{message}")]
    UntrustedUpdate { message: String },
    #[error("{message}")]
    InvalidKey { message: String },
    #[error("wrong passphrase")]
    WrongPassphrase,
    #[error("this entry is not encrypted to the key on this device")]
    NotForThisKey,
    #[error("{message}")]
    Crypto { message: String },
    #[error("{message}")]
    InvalidOtp { message: String },
    #[error("{message}")]
    InvalidConfig { message: String },
    #[error("{message}")]
    Network { message: String },
    #[error("the server's certificate is not the pinned one (pinned {pinned}, served {served})")]
    PinMismatch { pinned: String, served: String },
    #[error("the server's host key is not the pinned one (pinned {pinned}, served {served})")]
    HostKeyMismatch { pinned: String, served: String },
    #[error("the server refused this device's key or token")]
    Unauthorized,
    #[error("this device has {ahead} change(s) that are not merged, and the store has {behind} new one(s): merge the device's branch on the computer, or discard the device's changes")]
    Diverged { ahead: u32, behind: u32 },
    #[error("the remote refused the push: {message}")]
    PushRejected { message: String },
    #[error("the store on this device is not a git repository")]
    NotARepository,
    #[error("{message}")]
    Git { message: String },
    #[error("{message}")]
    Io { message: String },
}

/// How an https server is trusted.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TlsTrust {
    /// Chain and hostname against the public roots, as a browser: for a hosting service,
    /// whose certificate cannot be pinned.
    PublicCa,
    /// The pinned certificate, which must also verify against the public roots,
    /// or against `extra_ca_pem` when given.
    PinnedWithCa {
        sha256: String,
        extra_ca_pem: Option<String>,
    },
    /// The pinned certificate alone: for a self-signed one.
    PinnedOnly { sha256: String },
}

/// Where the store is synced with, and as whom.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum Remote {
    /// `ssh://user@host[:port]/path` or `user@host:path`.
    Ssh {
        url: String,
        /// This device's key, in OpenSSH format, with no passphrase (see [`generate_device_key`]).
        private_key: String,
        /// `SHA256:...`: the only host key accepted (see [`fetch_host_key`]).
        host_key: String,
    },
    /// `https://host[:port]/path`, or `http://` with no credentials.
    Http {
        url: String,
        /// `git` when there is a password and no user name.
        username: Option<String>,
        /// A password or an access token. Only ever sent over https.
        password: Option<String>,
        /// Required with https, refused with http.
        trust: Option<TlsTrust>,
    },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreConfig {
    /// The store: created by [`clone_store`], or any `pass` directory.
    pub directory: String,
    /// The public keys of everyone the store is encrypted to, this device's own
    /// included (see [`public_key_of`]): armored, one or several per string.
    pub public_keys: Vec<String>,
    /// As `PASSWORD_STORE_SIGNING_KEY`: fingerprints. When not empty, a `.gpg-id` is
    /// only used with a valid `.gpg-id.sig` from one of these keys.
    pub signing_keys: Vec<String>,
    /// Only take updates of the main branch whose last commit is signed by one of `public_keys`.
    pub require_signed_updates: bool,
    /// Who this device's commits are by.
    pub author_name: String,
    pub author_email: String,
    /// Names this device's branch on the remote: `devices/<device>`.
    pub device: String,
}

/// A public or a secret OpenPGP key, described.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyInfo {
    /// Uppercase hex, no spaces.
    pub fingerprint: String,
    /// `Name <email>`, one per identity.
    pub user_ids: Vec<String>,
    pub algorithm: String,
    pub is_secret: bool,
    /// A secret key protected by a passphrase.
    pub needs_passphrase: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryField {
    pub name: String,
    pub value: String,
}

/// A decrypted entry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Entry {
    /// The first line.
    pub password: String,
    /// Everything after the first line, as it is.
    pub extra: String,
    /// The `name: value` lines of `extra`.
    pub fields: Vec<EntryField>,
    /// The `otpauth://` lines, for [`otp_code`].
    pub otp_uris: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OtpCode {
    pub code: String,
    /// Seconds a code lives.
    pub period: u32,
    /// Seconds this one still has.
    pub remaining: u32,
}

/// An SSH key for this device.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceKey {
    /// OpenSSH format: for the keychain. Empty when only the public half was asked for.
    pub private_key: String,
    /// `ssh-ed25519 AAAA... comment`: a line for the server's `authorized_keys`.
    pub public_key: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostKey {
    /// `ssh-ed25519`...
    pub algorithm: String,
    /// `SHA256:...`, as `ssh-keygen -lf` prints it: what to pin.
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CertificateFingerprint {
    /// SHA-256 of the leaf certificate, lowercase hex: what to pin.
    pub sha256: String,
    /// `PinnedWithCa` will work. When false, only `PinnedOnly` will.
    pub chain_valid: bool,
}

/// Whether something is signed by a key this device knows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SignatureStatus {
    Unsigned,
    /// Signed by the key with this fingerprint, one of the store's public keys.
    Valid {
        fingerprint: String,
    },
    /// Signed, but by no key this device has, or the signature does not verify.
    Invalid,
}

/// A `.gpg-id` file: who a directory of the store is encrypted to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientFile {
    /// `.gpg-id`, `work/.gpg-id`.
    pub path: String,
    pub ids: Vec<String>,
    /// The ids with no public key on this device: nothing can be written under this file until they are imported.
    pub unknown: Vec<String>,
    /// Of the `.gpg-id.sig` next to it.
    pub signature: SignatureStatus,
}

/// A `.gpg-id` that an update adds, changes or removes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientChange {
    pub path: String,
    /// Empty when the file is new.
    pub before: Vec<String>,
    /// Empty when the file is removed.
    pub after: Vec<String>,
    /// Of the `.gpg-id.sig` that comes with the new file.
    pub signature: SignatureStatus,
}

/// What the main branch of the remote has that this device has not taken yet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PendingUpdate {
    /// The commit it leads to: what to give to [`PassStore::accept_update`].
    pub commit: String,
    pub commits: u32,
    /// The entries added, changed or removed.
    pub changed: Vec<String>,
    /// To be shown and agreed to before the update is taken: whoever can write to the
    /// remote can add a recipient, and from then on every entry written here would
    /// be readable by them.
    pub recipient_changes: Vec<RecipientChange>,
    /// Of the last commit.
    pub signature: SignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PullOutcome {
    /// Nothing new on the main branch.
    UpToDate,
    FastForwarded {
        commits: u32,
        changed: Vec<String>,
    },
    /// The update changes who the store is encrypted to. Nothing was taken: show it,
    /// then [`PassStore::accept_update`] or leave it.
    NeedsReview {
        update: PendingUpdate,
    },
    /// The main branch has moved and this device has changes that are not in it.
    /// Nothing was taken. It resolves itself once the device's branch is merged on
    /// the computer; the other way out is [`PassStore::discard_local_commits`].
    Diverged {
        unmerged: u32,
        behind: u32,
    },
}

/// Where this device stands, from what it knows without the network.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncState {
    pub is_repository: bool,
    /// The main branch: `main`.
    pub branch: String,
    /// This device's branch on the remote: `devices/phone`.
    pub device_branch: String,
    /// The first line of the last commit, and when it was made (seconds since 1970).
    pub last_subject: String,
    pub last_time: i64,
    /// Commits made here that are not in the main branch as last fetched.
    pub unmerged: u32,
    /// Of those, the ones not pushed to the device's branch yet.
    pub unpushed: u32,
    /// Commits of the main branch, as last fetched, that are not here.
    pub behind: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    pub pull: PullOutcome,
    /// Commits sent to the device's branch.
    pub pushed: u32,
    pub state: SyncState,
}

fn hex_fingerprint(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A secret key, unlocked: what reading an entry and changing the store need.
/// Lives in memory only; dropping the last reference to it locks the store.
#[derive(uniffi::Object)]
pub struct Identity {
    inner: SecretIdentity,
}

#[uniffi::export]
impl Identity {
    /// Parses the armored secret key (`gpg --export-secret-keys --armor <id>`) and
    /// checks that the passphrase opens it. Nothing is stored.
    #[uniffi::constructor]
    pub fn unlock(secret_key: String, passphrase: String) -> Result<Arc<Self>, PassError> {
        let (secret_key, passphrase) = (Zeroizing::new(secret_key), Zeroizing::new(passphrase));
        let key = crypto::parse_secret_key(secret_key.as_bytes())?;
        Ok(Arc::new(Self {
            inner: SecretIdentity::unlock(key, passphrase)?,
        }))
    }

    /// The public half, armored: what goes in [`StoreConfig::public_keys`].
    pub fn public_key(&self) -> Result<String, PassError> {
        self.inner.public_key_armored()
    }

    pub fn fingerprint(&self) -> String {
        self.inner.certificate().fingerprint()
    }

    /// Whether this device's commits will be signed: the key has a signing part with its secret here.
    pub fn can_sign(&self) -> bool {
        matches!(self.inner.sign(b"probe"), Ok(Some(_)))
    }
}

/// Describes an armored key, public or secret, before it is used. Needs no passphrase.
#[uniffi::export]
pub fn inspect_key(key: String) -> Result<KeyInfo, PassError> {
    let key = Zeroizing::new(key);
    let (certificate, is_secret, needs_passphrase) = match crypto::parse_secret_key(key.as_bytes()) {
        Ok(secret) => (crypto::certificate_of(&secret), true, crypto::needs_passphrase(&secret)),
        Err(_) => {
            let mut certificates = Certificate::parse(key.as_bytes())?;
            (certificates.remove(0), false, false)
        }
    };
    Ok(KeyInfo {
        fingerprint: certificate.fingerprint(),
        user_ids: certificate.user_ids(),
        algorithm: certificate.algorithm(),
        is_secret,
        needs_passphrase,
    })
}

/// The armored public key of an armored secret key. Needs no passphrase.
#[uniffi::export]
pub fn public_key_of(secret_key: String) -> Result<String, PassError> {
    let secret_key = Zeroizing::new(secret_key);
    crypto::public_key_armored(&crypto::parse_secret_key(secret_key.as_bytes())?)
}

/// A new SSH key for this device. `comment` ends its `authorized_keys` line: name the device.
#[uniffi::export]
pub fn generate_device_key(comment: String) -> Result<DeviceKey, PassError> {
    ssh::generate_key(&comment)
}

/// The public half of a device key, to show it again.
#[uniffi::export]
pub fn device_public_key(private_key: String) -> Result<DeviceKey, PassError> {
    ssh::public_key_of(&private_key)
}

/// Reads the host key an SSH server presents, authenticating nothing. It is not
/// trusted by this call: show the fingerprint, and have it compared with the one
/// read on the server (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`) before pinning it.
#[uniffi::export]
pub fn fetch_host_key(url: String) -> Result<HostKey, PassError> {
    ssh::fetch_host_key(&url)
}

/// Reads the certificate an https server presents, sending it nothing. Not trusted
/// by this call either: compare its fingerprint before pinning it.
#[uniffi::export]
pub fn fetch_fingerprint(url: String, extra_ca_pem: Option<String>) -> Result<CertificateFingerprint, PassError> {
    let found = pinned_http::fetch_fingerprint(&url, extra_ca_pem.as_deref()).map_err(|e| match e {
        pinned_http::Error::Connect(_) | pinned_http::Error::Tls(_) => PassError::Network { message: e.to_string() },
        _ => PassError::InvalidConfig { message: e.to_string() },
    })?;
    Ok(CertificateFingerprint {
        sha256: found.sha256,
        chain_valid: found.chain_valid,
    })
}

/// The one-time password of an `otpauth://totp/` line at `unix_time` (seconds since 1970).
#[uniffi::export]
pub fn otp_code(uri: String, unix_time: u64) -> Result<OtpCode, PassError> {
    otp::totp(&uri, unix_time)
}

/// The message of an error, for people. (The Swift `localizedDescription` of a
/// generated error is its case name, not this.)
#[uniffi::export]
pub fn error_message(error: PassError) -> String {
    error.to_string()
}

fn remote_of(remote: &Remote) -> Result<git::Remote, PassError> {
    Ok(match remote {
        Remote::Ssh {
            url,
            private_key,
            host_key,
        } => git::Remote::Ssh(ssh::SshRemote::new(url, private_key, host_key)?),
        Remote::Http {
            url,
            username,
            password,
            trust,
        } => git::Remote::Http(http::HttpRemote::new(
            url,
            username.as_deref(),
            password.as_deref(),
            trust.as_ref(),
        )?),
    })
}

/// A branch name made of a device name: `devices/<name>`, with nothing git would refuse.
fn device_branch(device: &str) -> Result<String, PassError> {
    let name: String = device
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-');
    if name.is_empty() {
        return Err(PassError::InvalidConfig {
            message: "the device needs a name: it names its branch on the remote".into(),
        });
    }
    Ok(format!("devices/{}", name.to_ascii_lowercase()))
}

/// A password store on this device.
#[derive(uniffi::Object)]
pub struct PassStore {
    root: PathBuf,
    certificates: Vec<Certificate>,
    signing_keys: Vec<String>,
    require_signed_updates: bool,
    author: git::Author,
    device_branch: String,
    /// One change or one sync at a time.
    busy: Mutex<()>,
}

impl PassStore {
    fn new(config: StoreConfig) -> Result<Self, PassError> {
        let invalid = |message: &str| PassError::InvalidConfig {
            message: message.to_string(),
        };
        if config.directory.is_empty() {
            return Err(invalid("no store directory"));
        }
        let mut certificates = Vec::new();
        for text in &config.public_keys {
            certificates.extend(Certificate::parse(text.as_bytes())?);
        }
        let author = git::Author {
            name: config.author_name.trim().to_string(),
            email: config.author_email.trim().to_string(),
        };
        let unusable = |text: &str| text.is_empty() || text.contains(['<', '>', '\n']);
        if unusable(&author.name) || unusable(&author.email) {
            return Err(invalid("commits need an author name and email, with no < > in them"));
        }
        Ok(Self {
            root: PathBuf::from(config.directory),
            certificates,
            signing_keys: config.signing_keys.iter().map(|key| hex_fingerprint(key)).collect(),
            require_signed_updates: config.require_signed_updates,
            author,
            device_branch: device_branch(&config.device)?,
            busy: Mutex::new(()),
        })
    }

    fn repository(&self) -> Result<gix::Repository, PassError> {
        git::open(&self.root, &self.author)?.ok_or(PassError::NotARepository)
    }

    /// Who signed `data`, among the store's public keys.
    fn signature_status(&self, signature: Option<&[u8]>, data: &[u8]) -> SignatureStatus {
        let Some(signature) = signature else {
            return SignatureStatus::Unsigned;
        };
        match self.certificates.iter().find(|c| c.verifies(signature, data)) {
            Some(certificate) => SignatureStatus::Valid {
                fingerprint: certificate.fingerprint(),
            },
            None => SignatureStatus::Invalid,
        }
    }

    /// The ids of a `.gpg-id` given by path, refused when `signing_keys` is set and
    /// its `.gpg-id.sig` is not from one of them: what `pass` does with `PASSWORD_STORE_SIGNING_KEY`.
    fn trusted_ids(&self, gpg_id: &str) -> Result<Vec<String>, PassError> {
        let content = layout::read(&self.root, gpg_id)?;
        if !self.signing_keys.is_empty() {
            let signature = std::fs::read(self.root.join(format!("{gpg_id}.sig"))).ok();
            let signed_by_one_of_them = match self.signature_status(signature.as_deref(), &content) {
                SignatureStatus::Valid { fingerprint } => self.signing_keys.contains(&fingerprint),
                _ => false,
            };
            if !signed_by_one_of_them {
                return Err(PassError::UntrustedRecipients {
                    message: format!("{gpg_id} is not signed by one of the store's signing keys: it is not used"),
                });
            }
        }
        Ok(layout::parse_gpg_id(&String::from_utf8_lossy(&content)))
    }

    /// The public keys an entry is to be encrypted to. All of them or an error:
    /// an entry is never written for fewer readers than the store says.
    fn recipients_of(&self, name: &str) -> Result<Vec<&Certificate>, PassError> {
        let ids = self.trusted_ids(&layout::governing_gpg_id(&self.root, name)?)?;
        if ids.is_empty() {
            return Err(PassError::NotInitialized);
        }
        let mut recipients: Vec<&Certificate> = Vec::new();
        for id in ids {
            let certificate = self
                .certificates
                .iter()
                .find(|certificate| certificate.matches(&id))
                .ok_or(PassError::UnknownRecipient { id })?;
            if !recipients
                .iter()
                .any(|known| known.fingerprint() == certificate.fingerprint())
            {
                recipients.push(certificate);
            }
        }
        Ok(recipients)
    }

    fn exists(&self, name: &str) -> bool {
        self.root.join(layout::file_of(name)).is_file()
    }

    fn decrypt(&self, name: &str, identity: &Identity) -> Result<Zeroizing<Vec<u8>>, PassError> {
        if !self.exists(name) {
            return Err(PassError::NotFound { name: name.to_string() });
        }
        identity
            .inner
            .decrypt(&layout::read(&self.root, &layout::file_of(name))?)
    }

    fn encrypt(&self, name: &str, content: &str) -> Result<Vec<u8>, PassError> {
        crypto::encrypt(content.as_bytes(), &self.recipients_of(name)?)
    }

    /// Commits the edits (when the store is a repository), then writes them to the directory.
    fn apply(&self, edits: Vec<git::Edit>, message: &str, identity: &Identity) -> Result<(), PassError> {
        if let Some(repo) = git::open(&self.root, &self.author)? {
            git::commit(&repo, &edits, message, &self.author, Some(&identity.inner))?;
        }
        for (path, content) in &edits {
            match content {
                Some(content) => layout::write(&self.root, path, content)?,
                None => layout::remove(&self.root, path)?,
            }
        }
        Ok(())
    }

    fn state_of(&self, repo: &gix::Repository) -> Result<SyncState, PassError> {
        let branch = git::branch(repo)?;
        let head = git::head_commit(repo)?;
        let main = git::remote_tip(repo, &branch);
        let device = git::remote_tip(repo, &self.device_branch);
        let (unmerged, behind) = git::ahead_behind(repo, head, main)?;
        let unpushed = match head {
            Some(head) if unmerged > 0 => {
                let mut local = git::ancestors(repo, head)?;
                for tip in [main, device].into_iter().flatten() {
                    for commit in git::ancestors(repo, tip)? {
                        local.remove(&commit);
                    }
                }
                local.len() as u32
            }
            _ => 0,
        };
        let (last_subject, last_time) = match head {
            Some(head) => git::summary(repo, head)?,
            None => (String::new(), 0),
        };
        Ok(SyncState {
            is_repository: true,
            branch,
            device_branch: self.device_branch.clone(),
            last_subject,
            last_time,
            unmerged,
            unpushed,
            behind,
        })
    }

    fn commit_signature(&self, repo: &gix::Repository, commit: ObjectId) -> Result<SignatureStatus, PassError> {
        Ok(match git::signature_of(repo, commit)? {
            Some(found) => self.signature_status(Some(&found.signature), &found.signed),
            None => SignatureStatus::Unsigned,
        })
    }

    /// What taking the main branch up to `to` would change.
    fn describe_update(
        &self,
        repo: &gix::Repository,
        from: Option<ObjectId>,
        to: ObjectId,
    ) -> Result<PendingUpdate, PassError> {
        let (_, commits) = git::ahead_behind(repo, from, Some(to))?;
        let changes = git::changes(repo, from, to)?;
        let after_files = git::files(repo, to)?;
        let ids = |blob: Option<ObjectId>| -> Result<Vec<String>, PassError> {
            Ok(match blob {
                Some(blob) => layout::parse_gpg_id(&String::from_utf8_lossy(&git::blob(repo, blob)?)),
                None => Vec::new(),
            })
        };
        let mut update = PendingUpdate {
            commit: to.to_string(),
            commits,
            changed: Vec::new(),
            recipient_changes: Vec::new(),
            signature: self.commit_signature(repo, to)?,
        };
        for change in changes {
            let file_name = change.path.rsplit('/').next().unwrap_or("");
            if file_name == layout::GPG_ID {
                let signature = match (change.after, after_files.get(&format!("{}.sig", change.path))) {
                    (Some(content), Some(signature)) => {
                        self.signature_status(Some(&git::blob(repo, *signature)?), &git::blob(repo, content)?)
                    }
                    _ => SignatureStatus::Unsigned,
                };
                update.recipient_changes.push(RecipientChange {
                    before: ids(change.before)?,
                    after: ids(change.after)?,
                    path: change.path,
                    signature,
                });
            } else if let Some(name) = change.path.strip_suffix(".gpg") {
                if layout::clean_name(name).is_ok() {
                    update.changed.push(name.to_string());
                }
            }
        }
        Ok(update)
    }

    /// Takes the main branch as last fetched, when that is a fast-forward and nothing in it needs to be seen first.
    fn take_main(&self, repo: &gix::Repository) -> Result<PullOutcome, PassError> {
        let head = git::head_commit(repo)?;
        let Some(main) = git::remote_tip(repo, &git::branch(repo)?) else {
            return Ok(PullOutcome::UpToDate);
        };
        let (unmerged, behind) = git::ahead_behind(repo, head, Some(main))?;
        if behind == 0 {
            return Ok(PullOutcome::UpToDate);
        }
        if unmerged > 0 {
            return Ok(PullOutcome::Diverged { unmerged, behind });
        }
        let update = self.describe_update(repo, head, main)?;
        if !update.recipient_changes.is_empty() {
            return Ok(PullOutcome::NeedsReview { update });
        }
        self.fast_forward(repo, main, &update)?;
        Ok(PullOutcome::FastForwarded {
            commits: update.commits,
            changed: update.changed,
        })
    }

    fn fast_forward(&self, repo: &gix::Repository, to: ObjectId, update: &PendingUpdate) -> Result<(), PassError> {
        if self.require_signed_updates && !matches!(update.signature, SignatureStatus::Valid { .. }) {
            return Err(PassError::UntrustedUpdate {
                message: format!(
                    "the last commit of the update ({}) is not signed by a key of this store: it is not taken",
                    &update.commit[..update.commit.len().min(8)]
                ),
            });
        }
        git::move_to(repo, to, "pull: fast-forward")?;
        Ok(())
    }

    /// Sends this device's unmerged commits to its branch. Returns how many were new there.
    fn push_device_branch(&self, repo: &gix::Repository, remote: &git::Remote, force: bool) -> Result<u32, PassError> {
        let state = self.state_of(repo)?;
        let Some(head) = git::head_commit(repo)? else {
            return Ok(0);
        };
        let on_remote = git::remote_tip(repo, &self.device_branch);
        if !force && (state.unmerged == 0 || on_remote == Some(head)) {
            return Ok(0);
        }
        let pushed = push::push(repo, remote, head, &self.device_branch, force)?;
        git::set_remote_tip(repo, &self.device_branch, head)?;
        Ok(if pushed { state.unpushed } else { 0 })
    }
}

#[uniffi::export]
impl PassStore {
    /// Opens the store at `config.directory`. Reads nothing yet.
    #[uniffi::constructor]
    pub fn open(config: StoreConfig) -> Result<Arc<Self>, PassError> {
        Self::new(config).map(Arc::new)
    }

    /// Every entry, sorted: `Email/work`, `bank`. An empty list when there is no store yet.
    pub fn list(&self) -> Result<Vec<String>, PassError> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        layout::list(&self.root)
    }

    /// The entries whose name holds every word of `query`, whatever the case.
    pub fn search(&self, query: String) -> Result<Vec<String>, PassError> {
        Ok(layout::search(&self.list()?, &query))
    }

    /// Every `.gpg-id` of the store, with what this device knows of its recipients.
    /// To show after a clone, before anything is written.
    pub fn recipient_files(&self) -> Result<Vec<RecipientFile>, PassError> {
        fn walk(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
            let Ok(items) = std::fs::read_dir(dir) else { return };
            for item in items.flatten() {
                let Ok(name) = item.file_name().into_string() else {
                    continue;
                };
                if name == layout::GPG_ID {
                    out.push(format!("{prefix}{name}"));
                } else if !name.starts_with('.') && item.file_type().is_ok_and(|kind| kind.is_dir()) {
                    walk(&item.path(), &format!("{prefix}{name}/"), out);
                }
            }
        }
        let mut paths = Vec::new();
        walk(&self.root, "", &mut paths);
        paths.sort();
        let mut files = Vec::new();
        for path in paths {
            let content = layout::read(&self.root, &path)?;
            let signature = std::fs::read(self.root.join(format!("{path}.sig"))).ok();
            let ids = layout::parse_gpg_id(&String::from_utf8_lossy(&content));
            files.push(RecipientFile {
                unknown: ids
                    .iter()
                    .filter(|id| !self.certificates.iter().any(|certificate| certificate.matches(id)))
                    .cloned()
                    .collect(),
                signature: self.signature_status(signature.as_deref(), &content),
                ids,
                path,
            });
        }
        Ok(files)
    }

    /// The ids an entry (existing or not) is encrypted to: the lines of the `.gpg-id` that governs it.
    pub fn recipients(&self, name: String) -> Result<Vec<String>, PassError> {
        let name = layout::clean_name(&name)?;
        self.trusted_ids(&layout::governing_gpg_id(&self.root, &name)?)
    }

    /// Decrypts an entry. Nothing is written anywhere.
    pub fn show(&self, name: String, identity: Arc<Identity>) -> Result<Entry, PassError> {
        let name = layout::clean_name(&name)?;
        Ok(layout::parse_entry(&self.decrypt(&name, &identity)?))
    }

    /// The whole decrypted content of an entry, to edit it.
    pub fn content(&self, name: String, identity: Arc<Identity>) -> Result<String, PassError> {
        let name = layout::clean_name(&name)?;
        Ok(String::from_utf8_lossy(&self.decrypt(&name, &identity)?).into_owned())
    }

    /// `pass insert`: writes a new entry, `content` being the password on its first
    /// line and anything after. Replaces an existing one only with `overwrite`.
    pub fn insert(
        &self,
        name: String,
        content: String,
        overwrite: bool,
        identity: Arc<Identity>,
    ) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        let (name, content) = (layout::clean_name(&name)?, with_final_newline(content));
        if !overwrite && self.exists(&name) {
            return Err(PassError::AlreadyExists { name });
        }
        let encrypted = self.encrypt(&name, &content)?;
        let message = format!("Add given password for {name} to store.");
        self.apply(vec![(layout::file_of(&name), Some(encrypted))], &message, &identity)
    }

    /// `pass edit`: replaces the content of an entry, or creates it. Returns false,
    /// and changes nothing, when the content is what the entry already holds.
    pub fn edit(&self, name: String, content: String, identity: Arc<Identity>) -> Result<bool, PassError> {
        let _busy = self.busy.lock().unwrap();
        let (name, content) = (layout::clean_name(&name)?, with_final_newline(content));
        let action = if self.exists(&name) {
            if *self.decrypt(&name, &identity)? == content.as_bytes() {
                return Ok(false);
            }
            "Edit"
        } else {
            "Add"
        };
        let encrypted = self.encrypt(&name, &content)?;
        let message = format!("{action} password for {name} using {EDITOR}.");
        self.apply(vec![(layout::file_of(&name), Some(encrypted))], &message, &identity)?;
        Ok(true)
    }

    /// `pass generate`: a new entry holding a random password of `length` characters,
    /// letters and digits, and punctuation with `symbols`. Returns the password.
    pub fn generate(
        &self,
        name: String,
        length: u32,
        symbols: bool,
        overwrite: bool,
        identity: Arc<Identity>,
    ) -> Result<String, PassError> {
        let _busy = self.busy.lock().unwrap();
        let name = layout::clean_name(&name)?;
        if !overwrite && self.exists(&name) {
            return Err(PassError::AlreadyExists { name });
        }
        let password = layout::generate_password(length, symbols)?;
        let encrypted = self.encrypt(&name, &format!("{}\n", *password))?;
        let message = format!("Add generated password for {name}.");
        self.apply(vec![(layout::file_of(&name), Some(encrypted))], &message, &identity)?;
        Ok(password.to_string())
    }

    /// `pass generate --in-place`: replaces the first line of an existing entry with
    /// a new random password, and keeps the rest. Returns the password.
    pub fn regenerate(
        &self,
        name: String,
        length: u32,
        symbols: bool,
        identity: Arc<Identity>,
    ) -> Result<String, PassError> {
        let _busy = self.busy.lock().unwrap();
        let name = layout::clean_name(&name)?;
        let current = self.decrypt(&name, &identity)?;
        let password = layout::generate_password(length, symbols)?;
        let content = layout::replace_first_line(&String::from_utf8_lossy(&current), &password);
        let encrypted = self.encrypt(&name, &content)?;
        let message = format!("Replace generated password for {name}.");
        self.apply(vec![(layout::file_of(&name), Some(encrypted))], &message, &identity)?;
        Ok(password.to_string())
    }

    /// `pass rm`: removes an entry.
    pub fn remove(&self, name: String, identity: Arc<Identity>) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        let name = layout::clean_name(&name)?;
        if !self.exists(&name) {
            return Err(PassError::NotFound { name });
        }
        let message = format!("Remove {name} from store.");
        self.apply(vec![(layout::file_of(&name), None)], &message, &identity)
    }

    /// `pass mv`: renames or moves an entry. Where the new place is governed by
    /// another `.gpg-id`, the entry is encrypted again, to those recipients.
    pub fn rename(&self, from: String, to: String, overwrite: bool, identity: Arc<Identity>) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        let (from, to) = (layout::clean_name(&from)?, layout::clean_name(&to)?);
        if !self.exists(&from) {
            return Err(PassError::NotFound { name: from });
        }
        if from == to {
            return Ok(());
        }
        if !overwrite && self.exists(&to) {
            return Err(PassError::AlreadyExists { name: to });
        }
        let mut encrypted = layout::read(&self.root, &layout::file_of(&from))?;
        let mut wanted = Vec::new();
        for recipient in self.recipients_of(&to)? {
            wanted.push(recipient.encryption_key_id()?);
        }
        wanted.sort();
        if crypto::recipient_key_ids(&encrypted)? != wanted {
            let content = self.decrypt(&from, &identity)?;
            encrypted = crypto::encrypt(&content, &self.recipients_of(&to)?)?;
        }
        let message = format!("Rename {from} to {to}.");
        let edits = vec![(layout::file_of(&from), None), (layout::file_of(&to), Some(encrypted))];
        self.apply(edits, &message, &identity)
    }

    /// Where this device stands, from what it has: no network.
    pub fn sync_state(&self) -> Result<SyncState, PassError> {
        match git::open(&self.root, &self.author)? {
            Some(repo) => self.state_of(&repo),
            None => Ok(SyncState {
                is_repository: false,
                branch: String::new(),
                device_branch: self.device_branch.clone(),
                last_subject: String::new(),
                last_time: 0,
                unmerged: 0,
                unpushed: 0,
                behind: 0,
            }),
        }
    }

    /// Fetches, and takes the main branch when that is a fast-forward and it does
    /// not change who the store is encrypted to. Never merges: see [`PullOutcome`].
    pub fn pull(&self, remote: Remote) -> Result<PullOutcome, PassError> {
        let _busy = self.busy.lock().unwrap();
        let repo = self.repository()?;
        git::fetch(&repo, &remote_of(&remote)?)?;
        self.take_main(&repo)
    }

    /// Takes an update that [`PullOutcome::NeedsReview`] showed, exactly that one:
    /// `commit` is [`PendingUpdate::commit`]. No network: what was agreed to is what is taken.
    pub fn accept_update(&self, commit: String) -> Result<PullOutcome, PassError> {
        let _busy = self.busy.lock().unwrap();
        let repo = self.repository()?;
        let stale = || {
            git::git_error(
                "cannot take this update",
                "it is not the one that was fetched last: pull again",
            )
        };
        let to = ObjectId::from_hex(commit.as_bytes()).map_err(|_| stale())?;
        let head = git::head_commit(&repo)?;
        if git::remote_tip(&repo, &git::branch(&repo)?) != Some(to) {
            return Err(stale());
        }
        let (unmerged, behind) = git::ahead_behind(&repo, head, Some(to))?;
        if unmerged > 0 {
            return Err(PassError::Diverged {
                ahead: unmerged,
                behind,
            });
        }
        let update = self.describe_update(&repo, head, to)?;
        self.fast_forward(&repo, to, &update)?;
        Ok(PullOutcome::FastForwarded {
            commits: update.commits,
            changed: update.changed,
        })
    }

    /// Sends this device's commits to its own branch on the remote, `devices/<device>`,
    /// forward only. Never to the main branch. Returns how many commits were new there.
    pub fn push(&self, remote: Remote) -> Result<u32, PassError> {
        let _busy = self.busy.lock().unwrap();
        let repo = self.repository()?;
        self.push_device_branch(&repo, &remote_of(&remote)?, false)
    }

    /// Pull, then push: one call for the sync button.
    pub fn sync(&self, remote: Remote) -> Result<SyncReport, PassError> {
        let _busy = self.busy.lock().unwrap();
        let repo = self.repository()?;
        let remote = remote_of(&remote)?;
        git::fetch(&repo, &remote)?;
        let pull = self.take_main(&repo)?;
        let pushed = self.push_device_branch(&repo, &remote, false)?;
        Ok(SyncReport {
            pull,
            pushed,
            state: self.state_of(&repo)?,
        })
    }

    /// Gives up the changes made on this device that are not merged: the store goes
    /// back to the main branch as the remote has it now, and the device's branch on
    /// the remote is moved there too (the one push that is not forward only).
    /// Returns how many commits were dropped. An update that changes the recipients is
    /// not taken this way: it is refused, to be reviewed with a pull first.
    pub fn discard_local_commits(&self, remote: Remote) -> Result<u32, PassError> {
        let _busy = self.busy.lock().unwrap();
        let repo = self.repository()?;
        let remote = remote_of(&remote)?;
        git::fetch(&repo, &remote)?;
        let head = git::head_commit(&repo)?;
        let main = git::remote_tip(&repo, &git::branch(&repo)?)
            .ok_or_else(|| git::git_error("cannot discard", "the remote has no main branch"))?;
        let (unmerged, _) = git::ahead_behind(&repo, head, Some(main))?;
        if unmerged == 0 {
            return Ok(0);
        }
        // This device never writes a .gpg-id: any difference there is what the main branch brings.
        let update = self.describe_update(&repo, head, main)?;
        if !update.recipient_changes.is_empty() {
            return Err(PassError::UntrustedRecipients {
                message: "the main branch changes who the store is encrypted to: this cannot be taken while discarding. Have this device's branch merged on the computer instead".into(),
            });
        }
        self.fast_forward(&repo, main, &update)?;
        if git::remote_tip(&repo, &self.device_branch).is_some() {
            self.push_device_branch(&repo, &remote, true)?;
        }
        Ok(unmerged)
    }
}

/// As `pass insert` (the original) stores what is typed: with a final newline.
fn with_final_newline(content: String) -> Zeroizing<String> {
    let mut content = Zeroizing::new(content);
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

/// Clones the remote into `config.directory`, which must be empty or absent, and
/// opens the store. Look at [`PassStore::recipient_files`] before writing to it.
#[uniffi::export]
pub fn clone_store(config: StoreConfig, remote: Remote) -> Result<Arc<PassStore>, PassError> {
    let store = PassStore::new(config)?;
    git::clone(&store.root, &remote_of(&remote)?, &store.author)?;
    Ok(Arc::new(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_branches() {
        assert_eq!(device_branch("phone").unwrap(), "devices/phone");
        assert_eq!(device_branch(" Alice's iPhone ").unwrap(), "devices/alice-s-iphone");
        assert_eq!(device_branch("a/../b").unwrap(), "devices/a----b");
        assert!(device_branch("").is_err());
        assert!(device_branch(" ..//.. ").is_err());
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            error_message(PassError::NotFound { name: "bank".into() }),
            "bank is not in the password store"
        );
        assert!(error_message(PassError::Diverged { ahead: 2, behind: 1 }).contains("2 change(s)"));
    }

    #[test]
    fn a_store_needs_an_author_and_a_device() {
        let config = |author: &str, device: &str| StoreConfig {
            directory: "/nowhere".into(),
            public_keys: vec![],
            signing_keys: vec![],
            require_signed_updates: false,
            author_name: author.into(),
            author_email: "phone@example.org".into(),
            device: device.into(),
        };
        let store = PassStore::open(config("Alice", "phone")).unwrap();
        assert_eq!(store.list().unwrap(), Vec::<String>::new());
        assert!(!store.sync_state().unwrap().is_repository);
        assert!(matches!(
            PassStore::open(config("", "phone")),
            Err(PassError::InvalidConfig { .. })
        ));
        assert!(matches!(
            PassStore::open(config("A <b>", "phone")),
            Err(PassError::InvalidConfig { .. })
        ));
        assert!(matches!(
            PassStore::open(config("Alice", "")),
            Err(PassError::InvalidConfig { .. })
        ));
    }
}
