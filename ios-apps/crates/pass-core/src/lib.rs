//! A password store: the layout of `pass`, the standard Unix password manager
//! (<https://www.passwordstore.org>), OpenPGP files in a git repository, with
//! neither `gpg` nor `git`. Exposed to Swift through UniFFI.
//!
//! This is the **member** side of the sync protocol of `apps/pass/SYNC.md`: the
//! store is managed on a computer (the owner), and each device publishes its whole
//! history as one signed and encrypted git bundle in a drop zone that understands
//! none of it. Nothing of the owner's is taken before it has been shown and
//! accepted; this device only ever changes entries (`*.gpg`), in signed commits.
//!
//! Three things are kept apart:
//! - the store, a directory ([`PassStore`]): listing it needs no key;
//! - the identity, a secret key held unlocked in memory ([`Identity`]): reading an
//!   entry, changing the store and syncing need it, dropping it locks the store;
//! - the drop zone ([`DropZone`]), given to the call that goes on the network.
//!
//! Every call blocks: call them off the main thread. Nothing decrypted is ever
//! written to disk; the secret key is never written at all.

mod bundle;
mod crypto;
mod git;
mod layout;
mod otp;
mod sftp;
mod ssh;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gix::ObjectId;
use zeroize::Zeroizing;

use crate::crypto::{Certificate, SecretIdentity};

uniffi::setup_scaffolding!();

/// What the phone calls itself where `pass` names the editor: `Edit password for x using pass-ios.`
const EDITOR: &str = "pass-ios";
/// A reader refuses a published file over this size (SYNC.md).
const MAX_PUBLISHED_FILE: u64 = 256 * 1024 * 1024;
/// How many commits of an update are described: the screen that shows them is not a git log.
const MAX_LISTED_COMMITS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PassError {
    #[error("{message}")]
    InvalidName { message: String },
    #[error("{name} is not in the password store")]
    NotFound { name: String },
    #[error("an entry already exists for {name}")]
    AlreadyExists { name: String },
    #[error("the store has no .gpg-id here: the owner sets who can read what, with `pass init`")]
    NotInitialized,
    #[error("no public key on this device for the recipient {id}: only the devices of the sync are known here")]
    UnknownRecipient { id: String },
    #[error("{message}")]
    InvalidKey { message: String },
    #[error("wrong passphrase")]
    WrongPassphrase,
    #[error("this entry is not encrypted to the key on this device")]
    NotForThisKey,
    #[error("this device's key cannot sign (its signing part is not here): the owner would refuse what it publishes")]
    CannotSign,
    #[error("{message}")]
    Crypto { message: String },
    #[error("{message}")]
    InvalidOtp { message: String },
    #[error("{message}")]
    InvalidConfig { message: String },
    #[error("{message}")]
    Network { message: String },
    #[error("the drop zone's host key is not the pinned one (pinned {pinned}, served {served})")]
    HostKeyMismatch { pinned: String, served: String },
    #[error("the drop zone refused this device's SSH key")]
    Unauthorized,
    #[error("the published file is refused: {message}")]
    UntrustedFile { message: String },
    #[error("the published bundle is refused: {message}")]
    BadBundle { message: String },
    #[error("the published file is refused: it is over 256 MiB")]
    TooLarge,
    #[error("both sides changed {}: choose which version to keep", names.join(", "))]
    ChoiceRequired { names: Vec<String> },
    #[error("this is not the update that was last received: look at it again")]
    StaleUpdate,
    #[error("there is no store on this device yet: sync to receive the owner's")]
    NoStore,
    #[error("{message}")]
    Git { message: String },
    #[error("{message}")]
    Io { message: String },
}

/// Another device of the sync, as this one is told about it. Nothing received can add one.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Peer {
    /// `[a-z0-9][a-z0-9-]{0,31}`: names its file in the drop zone.
    pub name: String,
    /// 40 hex digits: the fingerprint of its OpenPGP key.
    pub fingerprint: String,
    /// That key, armored (`gpg --export --armor <fingerprint>`).
    pub public_key: String,
    /// The computer where the store is managed. Exactly one.
    pub is_owner: bool,
}

/// Where the devices leave their files for each other.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DropZone {
    /// `sftp://user@host[:port]`.
    pub url: String,
    /// This device's SSH key, in OpenSSH format, with no passphrase (see [`generate_device_key`]).
    pub private_key: String,
    /// `SHA256:...`: the only host key accepted (see [`fetch_host_key`]).
    pub host_key: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreConfig {
    /// Where the store is, or will be once the owner's has been received.
    pub directory: String,
    /// This device's name.
    pub device: String,
    /// This device's OpenPGP public key, armored (see [`public_key_of`]).
    pub public_key: String,
    /// The other devices. One of them is the owner.
    pub peers: Vec<Peer>,
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

/// An OpenPGP key made on this device.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GeneratedKey {
    /// Armored, protected by the passphrase: for the keychain, and nowhere else.
    pub secret_key: String,
    /// Armored: for the owner to import (`gpg --import`) and add as a recipient.
    pub public_key: String,
    pub fingerprint: String,
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

/// An SSH key for this device: it lets it into the drop zone, and does nothing else.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceKey {
    /// OpenSSH format: for the keychain. Empty when only the public half was asked for.
    pub private_key: String,
    /// `ssh-ed25519 AAAA... comment`: what the drop zone is given (`DROPZONE_KEY_<NAME>`).
    pub public_key: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostKey {
    /// `ssh-ed25519`...
    pub algorithm: String,
    /// `SHA256:...`, as the drop zone prints it when it starts: what to pin.
    pub fingerprint: String,
}

/// Whether a commit is signed by a device this one knows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SignatureStatus {
    Unsigned,
    /// Signed by the key of this device (by name).
    Valid {
        device: String,
    },
    /// Signed, but by no key this device has, or the signature does not verify.
    Invalid,
}

/// A line of a `.gpg-id`: someone who can read what is under it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Reader {
    /// The line itself: a fingerprint, a key id, an email.
    pub id: String,
    /// The device whose key it is. `None` for a key that is no device's: this device
    /// does not know it, and cannot write for it.
    pub device: Option<String>,
}

/// A `.gpg-id` file: who a directory of the store is encrypted to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientFile {
    /// `.gpg-id`, `work/.gpg-id`.
    pub path: String,
    pub readers: Vec<Reader>,
    /// The ids that are no device's key: this device cannot write under this file.
    pub unknown: Vec<String>,
}

/// A `.gpg-id` that an update adds, changes or removes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientChange {
    pub path: String,
    /// Empty when the file is new.
    pub before: Vec<Reader>,
    /// Empty when the file is removed.
    pub after: Vec<Reader>,
}

/// Who can read an entry, or what is put in a folder.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Access {
    /// The `.gpg-id` that governs it: `.gpg-id`, `team/.gpg-id`. Empty when there is none.
    pub gpg_id: String,
    /// Its lines: who the store says can read this.
    pub readers: Vec<Reader>,
    /// This device is one of them.
    pub listed: bool,
    /// For an existing entry: who the file is actually encrypted to (device names, or
    /// `key <id>` for a key that is no device's). Empty for a folder.
    pub encrypted_to: Vec<String>,
    /// This device can decrypt it: for an entry, by what the file is encrypted to; for a folder, `listed`.
    pub readable: bool,
    /// For an existing entry: the file is encrypted to exactly the readers of its `.gpg-id`.
    /// False when the `.gpg-id` changed and the file was not encrypted again. True for a folder.
    pub matches: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CommitInfo {
    pub id: String,
    /// The first line of its message.
    pub subject: String,
    /// `Name <email>`.
    pub author: String,
    /// Seconds since 1970.
    pub time: i64,
    pub signature: SignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ChangeKind {
    Added,
    Changed,
    Removed,
    /// The same content under another name: `name` is the new one.
    Renamed {
        from: String,
    },
}

/// What an update does to one entry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryChange {
    pub name: String,
    pub kind: ChangeKind,
    /// Who the entry was encrypted to and who it is now (device names, or key ids for
    /// keys that are no device's). Both empty when that does not change.
    pub readers_before: Vec<String>,
    pub readers_after: Vec<String>,
    /// This device changed it too, to something else: accepting needs a choice for it.
    pub conflict: bool,
}

/// What the owner has published that this device has not taken.
/// Nothing of it is in the store until [`PassStore::accept_update`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PendingUpdate {
    /// The owner's tip: what to give to [`PassStore::accept_update`].
    pub commit: String,
    /// When the owner signed the file, seconds since 1970.
    pub published_at: i64,
    /// The owner's commits that are not here, newest first (at most 200 of them).
    pub commits: Vec<CommitInfo>,
    pub commit_count: u32,
    /// The entries it adds, changes, renames or removes. See [`PassStore::update_versions`] for their content.
    pub entries: Vec<EntryChange>,
    /// The `.gpg-id` files it adds, changes or removes: who can read which directory.
    pub recipient_changes: Vec<RecipientChange>,
    /// Anything else it changes (`.gitattributes`, `.extensions/`...).
    pub other_files: Vec<String>,
    /// This device's commits that the owner does not have. Not zero: accepting merges.
    pub outstanding: u32,
}

/// One version of an entry, read for a review.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum EntryVersion {
    /// There is no such entry on that side.
    Absent,
    /// It is not encrypted to this device's key, or does not decrypt.
    Unreadable {
        reason: String,
    },
    Content {
        text: String,
    },
}

/// An entry as an update finds it and as it leaves it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryVersions {
    /// Before the owner's changes: where the two histories last agreed.
    pub before: EntryVersion,
    /// On this device now.
    pub mine: EntryVersion,
    /// In the update.
    pub theirs: EntryVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Side {
    /// This device's version (which may be: removed).
    Mine,
    /// The owner's.
    Theirs,
}

/// Which version to keep of an entry both sides changed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Choice {
    pub name: String,
    pub keep: Side,
}

/// What looking at the owner's file gave.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FetchOutcome {
    /// The owner has published nothing yet (or the drop zone lost it).
    NothingPublished,
    /// The same tip as last time.
    Unchanged,
    /// A new tip was received and stored. `update` is what remains to be accepted;
    /// `None` when this device already has everything in it.
    Received { update: Option<PendingUpdate> },
    /// A valid file, but older than the one already received: served again, and ignored.
    Stale { published_at: i64 },
    /// Not a file this device takes (wrong signer, wrong reference, incomplete, damaged, too big...). Nothing changed.
    Refused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// Whether this device's tip was sent (it is, whenever it is not what was last published).
    pub published: bool,
    pub fetch: FetchOutcome,
    pub state: SyncState,
}

/// Where this device stands, from what it has: no network.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncState {
    /// The owner's store has been received and accepted once.
    pub has_store: bool,
    /// Commits made here that the owner has not published back.
    pub outstanding: u32,
    /// The tip of this device is not the one it last published.
    pub unpublished: bool,
    /// An update of the owner's is waiting to be looked at.
    pub update_waiting: bool,
    /// When the owner signed the last file received, seconds since 1970. 0 when none was.
    pub owner_published_at: i64,
    /// The first line of the last commit, and when it was made.
    pub last_subject: String,
    pub last_time: i64,
}

fn hex_fingerprint(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A device name, as SYNC.md defines it.
fn is_device_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    name.len() <= 32 && bytes.next().is_some_and(ok) && bytes.all(|b| ok(b) || b == b'-')
}

/// Whether a path of the repository is an entry: the only kind of file a member may change.
fn entry_name(path: &str) -> Option<&str> {
    path.strip_suffix(".gpg")
        .filter(|name| layout::clean_name(name).is_ok_and(|clean| clean == *name))
}

/// A secret key, unlocked: what reading an entry, changing the store and syncing need.
/// Lives in memory only; dropping the last reference to it locks the store.
#[derive(uniffi::Object)]
pub struct Identity {
    inner: SecretIdentity,
}

#[uniffi::export]
impl Identity {
    /// Parses the armored secret key and checks that the passphrase opens it. Nothing is stored.
    #[uniffi::constructor]
    pub fn unlock(secret_key: String, passphrase: String) -> Result<Arc<Self>, PassError> {
        let (secret_key, passphrase) = (Zeroizing::new(secret_key), Zeroizing::new(passphrase));
        let key = crypto::parse_secret_key(secret_key.as_bytes())?;
        Ok(Arc::new(Self {
            inner: SecretIdentity::unlock(key, passphrase)?,
        }))
    }

    /// The public half, armored.
    pub fn public_key(&self) -> Result<String, PassError> {
        self.inner.public_key_armored()
    }

    pub fn fingerprint(&self) -> String {
        self.inner.certificate().fingerprint()
    }

    /// Whether this key can sign: it must, to change the store or publish anything.
    pub fn can_sign(&self) -> bool {
        self.inner.can_sign()
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

/// Makes this device's OpenPGP key, here: its private half then never exists anywhere
/// else. Ed25519 to sign, Curve25519 to decrypt, as `gpg` makes them today.
/// `user_id` is `Name <email>` or just a name; an empty `passphrase` leaves the key unprotected.
#[uniffi::export]
pub fn generate_openpgp_key(user_id: String, passphrase: String) -> Result<GeneratedKey, PassError> {
    let passphrase = Zeroizing::new(passphrase);
    let user_id = user_id.trim();
    if user_id.is_empty() || user_id.chars().any(char::is_control) {
        return Err(PassError::InvalidKey {
            message: "the key needs a name: it is how the owner will recognise it".into(),
        });
    }
    let secret_key = crypto::generate_secret_key(user_id, &passphrase)?;
    let parsed = crypto::parse_secret_key(secret_key.as_bytes())?;
    Ok(GeneratedKey {
        public_key: crypto::public_key_armored(&parsed)?,
        fingerprint: crypto::certificate_of(&parsed).fingerprint(),
        secret_key: secret_key.to_string(),
    })
}

/// A new SSH key for this device. `comment` ends its public line: name the device.
#[uniffi::export]
pub fn generate_device_key(comment: String) -> Result<DeviceKey, PassError> {
    ssh::generate_key(&comment)
}

/// The public half of a device key, to show it again.
#[uniffi::export]
pub fn device_public_key(private_key: String) -> Result<DeviceKey, PassError> {
    ssh::public_key_of(&private_key)
}

/// Reads the host key a drop zone presents, authenticating nothing. It is not trusted
/// by this call: show the fingerprint, and have it compared with the one the drop zone
/// printed when it started (`docker logs pass-dropzone`) before pinning it.
#[uniffi::export]
pub fn fetch_host_key(url: String) -> Result<HostKey, PassError> {
    ssh::fetch_host_key(&url)
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

/// A device and its key.
struct Device {
    name: String,
    certificate: Certificate,
}

/// The entries of the three sides of an update, and what a whole-file merge makes of them.
struct ThreeWay {
    base: std::collections::BTreeMap<String, ObjectId>,
    ours: std::collections::BTreeMap<String, ObjectId>,
    theirs: std::collections::BTreeMap<String, ObjectId>,
}

impl ThreeWay {
    /// Changed on both sides, to different things (one of them may be: removed).
    fn is_conflict(&self, path: &str) -> bool {
        let (base, ours, theirs) = (self.base.get(path), self.ours.get(path), self.theirs.get(path));
        ours != base && theirs != base && ours != theirs
    }
}

/// A password store on this device.
#[derive(uniffi::Object)]
pub struct PassStore {
    root: PathBuf,
    me: Device,
    /// The other devices, the owner first.
    peers: Vec<Device>,
    author: git::Author,
    /// One change or one sync at a time.
    busy: Mutex<()>,
}

impl PassStore {
    fn new(config: StoreConfig) -> Result<Self, PassError> {
        let invalid = |message: String| PassError::InvalidConfig { message };
        if config.directory.is_empty() {
            return Err(invalid("no store directory".into()));
        }
        let check_name = |name: &str| {
            if is_device_name(name) {
                Ok(())
            } else {
                Err(invalid(format!(
                    "{name:?} is not a device name: lowercase letters, digits and hyphens, 32 at most, not starting with a hyphen"
                )))
            }
        };
        check_name(&config.device)?;
        let single = |text: &str, whose: &str| -> Result<Certificate, PassError> {
            let mut certificates = Certificate::parse(text.as_bytes())?;
            if certificates.len() != 1 {
                return Err(invalid(format!(
                    "the public key of {whose} holds several keys: give the one"
                )));
            }
            Ok(certificates.remove(0))
        };
        let me = Device {
            certificate: single(&config.public_key, &config.device)?,
            name: config.device,
        };
        let mut owner = None;
        let mut members = Vec::new();
        for peer in config.peers {
            check_name(&peer.name)?;
            if peer.name == me.name || members.iter().chain(&owner).any(|d: &Device| d.name == peer.name) {
                return Err(invalid(format!("two devices are named {}", peer.name)));
            }
            let certificate = single(&peer.public_key, &peer.name)?;
            // The fingerprint is what was compared by eye: the key must be that one.
            if certificate.fingerprint() != hex_fingerprint(&peer.fingerprint) {
                return Err(invalid(format!(
                    "the public key given for {} is not the one with its fingerprint (it is {})",
                    peer.name,
                    certificate.fingerprint()
                )));
            }
            let device = Device {
                name: peer.name,
                certificate,
            };
            if !peer.is_owner {
                members.push(device);
            } else if owner.replace(device).is_some() {
                return Err(invalid(
                    "two devices are marked as the owner: there is exactly one".into(),
                ));
            }
        }
        let owner = owner.ok_or_else(|| invalid("no device is marked as the owner: there is exactly one".into()))?;
        let author = git::Author {
            email: format!("{}@pass-sync.invalid", me.name),
            name: me.name.clone(),
        };
        let mut peers = vec![owner];
        peers.extend(members);
        Ok(Self {
            root: PathBuf::from(config.directory),
            me,
            peers,
            author,
            busy: Mutex::new(()),
        })
    }

    fn owner(&self) -> &Device {
        &self.peers[0]
    }

    fn devices(&self) -> impl Iterator<Item = &Device> {
        std::iter::once(&self.me).chain(&self.peers)
    }

    fn owner_reference(&self) -> String {
        format!("refs/pass-sync/{}", self.owner().name)
    }

    fn my_reference(&self) -> String {
        format!("refs/pass-sync/{}", self.me.name)
    }

    /// The identity must be this device's: the key the others know it by.
    fn check(&self, identity: &Identity) -> Result<(), PassError> {
        if identity.fingerprint() == self.me.certificate.fingerprint() {
            Ok(())
        } else {
            Err(PassError::InvalidKey {
                message: "the unlocked key is not the one this device is set up with".into(),
            })
        }
    }

    /// The repository, once there is a store in it.
    fn repository(&self) -> Result<gix::Repository, PassError> {
        let repo = git::open(&self.root, &self.author)?.ok_or(PassError::NoStore)?;
        if git::head_commit(&repo)?.is_none() {
            return Err(PassError::NoStore);
        }
        Ok(repo)
    }

    /// Who signed `data`, among the devices.
    fn signature_status(&self, signature: &[u8], data: &[u8]) -> SignatureStatus {
        match self
            .devices()
            .find(|device| device.certificate.verifies(signature, data))
        {
            Some(device) => SignatureStatus::Valid {
                device: device.name.clone(),
            },
            None => SignatureStatus::Invalid,
        }
    }

    fn ids_of(&self, gpg_id: &str) -> Result<Vec<String>, PassError> {
        let content = layout::read(&self.root, gpg_id)?;
        Ok(layout::parse_gpg_id(&String::from_utf8_lossy(&content)))
    }

    /// The device a line of a `.gpg-id` designates, if it is one.
    fn device_of(&self, id: &str) -> Option<&Device> {
        self.devices().find(|device| device.certificate.matches(id))
    }

    fn readers_of(&self, ids: Vec<String>) -> Vec<Reader> {
        ids.into_iter()
            .map(|id| Reader {
                device: self.device_of(&id).map(|device| device.name.clone()),
                id,
            })
            .collect()
    }

    /// Who can read `name`: an entry, existing or not. `file` is its encrypted content when it exists.
    fn access_of(&self, name: &str, file: Option<&[u8]>) -> Result<Access, PassError> {
        let gpg_id = match layout::governing_gpg_id(&self.root, name) {
            Ok(gpg_id) => gpg_id,
            Err(PassError::NotInitialized) => String::new(),
            Err(e) => return Err(e),
        };
        let readers = if gpg_id.is_empty() {
            Vec::new()
        } else {
            self.readers_of(self.ids_of(&gpg_id)?)
        };
        let listed = readers
            .iter()
            .any(|reader| reader.device.as_deref() == Some(self.me.name.as_str()));
        let Some(file) = file else {
            return Ok(Access {
                gpg_id,
                readers,
                listed,
                encrypted_to: Vec::new(),
                readable: listed,
                matches: true,
            });
        };
        let key_ids = crypto::recipient_key_ids(file).unwrap_or_default();
        // What the `.gpg-id` asks for, as far as this device can tell: the keys of the devices among
        // its readers. A reader that is no device has a key this cannot name, only count.
        let mut wanted = BTreeSet::new();
        for reader in &readers {
            if let Some(device) = self.device_of(&reader.id) {
                wanted.insert(device.certificate.encryption_key_id()?);
            }
        }
        let unknown = readers.iter().filter(|reader| reader.device.is_none()).count();
        let matches = wanted.iter().all(|key_id| key_ids.contains(key_id)) && key_ids.len() == wanted.len() + unknown;
        Ok(Access {
            gpg_id,
            readers,
            listed,
            encrypted_to: self.readers(file),
            readable: key_ids.iter().any(|key_id| self.me.certificate.has_key_id(key_id)),
            matches,
        })
    }

    /// The public keys an entry is to be encrypted to. All of them or an error:
    /// an entry is never written for fewer readers than the store says.
    fn recipients_of(&self, name: &str) -> Result<Vec<&Certificate>, PassError> {
        let ids = self.ids_of(&layout::governing_gpg_id(&self.root, name)?)?;
        if ids.is_empty() {
            return Err(PassError::NotInitialized);
        }
        let mut recipients: Vec<&Certificate> = Vec::new();
        for id in ids {
            let certificate = self
                .devices()
                .map(|device| &device.certificate)
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

    /// Who an encrypted file can be read by: device names, or key ids for keys that are no device's.
    fn readers(&self, ciphertext: &[u8]) -> Vec<String> {
        let mut readers: Vec<String> = crypto::recipient_key_ids(ciphertext)
            .unwrap_or_default()
            .into_iter()
            .map(|key_id| {
                self.devices()
                    .find(|device| device.certificate.has_key_id(&key_id))
                    .map(|device| device.name.clone())
                    .unwrap_or(format!("key {key_id}"))
            })
            .collect();
        readers.sort();
        readers
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

    /// Commits the edits (signed), then moves the directory to that commit.
    /// Only ever called with entry files: a member changes nothing else.
    fn apply(&self, edits: Vec<git::Edit>, message: &str, identity: &Identity) -> Result<(), PassError> {
        self.check(identity)?;
        debug_assert!(edits.iter().all(|(path, _)| entry_name(path).is_some()));
        let repo = self.repository()?;
        let head = git::head_commit(&repo)?.ok_or(PassError::NoStore)?;
        let tree = git::edit_tree(&repo, Some(git::tree_of(&repo, head)?), &edits)?;
        let commit = git::write_commit(&repo, tree, &[head], message, &self.author, &identity.inner)?;
        git::move_to(&repo, commit, &format!("commit: {message}"))?;
        Ok(())
    }

    fn published_at_file(&self) -> PathBuf {
        self.root.join(".git").join("pass-sync-published-at")
    }

    fn owner_published_at(&self) -> i64 {
        std::fs::read_to_string(self.published_at_file())
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    fn state_of(&self, repo: &gix::Repository) -> Result<SyncState, PassError> {
        let head = git::head_commit(repo)?;
        let owner = git::reference(repo, &self.owner_reference());
        let outstanding = match (head, owner) {
            (Some(head), Some(owner)) => git::commits_since(repo, Some(owner), head)?.len() as u32,
            _ => 0,
        };
        let update_waiting = match (head, owner) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(head), Some(owner)) => !git::ancestors(repo, head)?.contains(&owner),
        };
        let last = head.map(|head| git::summary(repo, head)).transpose()?;
        Ok(SyncState {
            has_store: head.is_some(),
            outstanding,
            unpublished: head.is_some() && head != git::reference(repo, &self.my_reference()),
            update_waiting,
            owner_published_at: self.owner_published_at(),
            last_subject: last.as_ref().map(|last| last.subject.clone()).unwrap_or_default(),
            last_time: last.map(|last| last.time).unwrap_or(0),
        })
    }

    /// The three sides of the update that is waiting: where the histories last agreed, here, and the owner's.
    /// `None` when there is nothing to take.
    #[allow(clippy::type_complexity)]
    fn update_sides(
        &self,
        repo: &gix::Repository,
    ) -> Result<Option<(Option<ObjectId>, Option<ObjectId>, ObjectId)>, PassError> {
        let head = git::head_commit(repo)?;
        let Some(theirs) = git::reference(repo, &self.owner_reference()) else {
            return Ok(None);
        };
        let base = match head {
            None => None,
            Some(head) if git::ancestors(repo, head)?.contains(&theirs) => return Ok(None),
            Some(head) => git::merge_base(repo, head, theirs)?,
        };
        Ok(Some((base, head, theirs)))
    }

    fn three_way(
        &self,
        repo: &gix::Repository,
        base: Option<ObjectId>,
        ours: Option<ObjectId>,
        theirs: ObjectId,
    ) -> Result<ThreeWay, PassError> {
        Ok(ThreeWay {
            base: git::files_at(repo, base)?,
            ours: git::files_at(repo, ours)?,
            theirs: git::files(repo, theirs)?,
        })
    }

    fn describe_update(&self, repo: &gix::Repository) -> Result<Option<PendingUpdate>, PassError> {
        let Some((base, head, theirs)) = self.update_sides(repo)? else {
            return Ok(None);
        };
        let sides = self.three_way(repo, base, head, theirs)?;
        let new_commits = git::commits_since(repo, head, theirs)?;
        let mut commits = Vec::new();
        for id in new_commits.iter().take(MAX_LISTED_COMMITS) {
            let summary = git::summary(repo, *id)?;
            commits.push(CommitInfo {
                id: id.to_string(),
                subject: summary.subject,
                author: summary.author,
                time: summary.time,
                signature: match git::signature_of(repo, *id)? {
                    Some(found) => self.signature_status(&found.signature, &found.signed),
                    None => SignatureStatus::Unsigned,
                },
            });
        }
        let ids = |blob: Option<ObjectId>| -> Result<Vec<Reader>, PassError> {
            Ok(match blob {
                Some(blob) => self.readers_of(layout::parse_gpg_id(&String::from_utf8_lossy(&git::blob(repo, blob)?))),
                None => Vec::new(),
            })
        };
        let readers = |blob: Option<ObjectId>| -> Result<Vec<String>, PassError> {
            Ok(match blob {
                Some(blob) => self.readers(&git::blob(repo, blob)?),
                None => Vec::new(),
            })
        };

        let mut update = PendingUpdate {
            commit: theirs.to_string(),
            published_at: self.owner_published_at(),
            commits,
            commit_count: new_commits.len() as u32,
            entries: Vec::new(),
            recipient_changes: Vec::new(),
            other_files: Vec::new(),
            outstanding: match head {
                Some(head) => git::commits_since(repo, Some(theirs), head)?.len() as u32,
                None => 0,
            },
        };
        let changes = git::changes(repo, base, theirs)?;
        // The same content gone from one name and come under another is a rename.
        let mut renamed_from: std::collections::BTreeMap<String, String> = Default::default();
        let mut renamed_away = BTreeSet::new();
        for added in changes
            .iter()
            .filter(|c| c.before.is_none() && entry_name(&c.path).is_some())
        {
            let source = changes.iter().find(|removed| {
                removed.after.is_none()
                    && removed.before == added.after
                    && entry_name(&removed.path).is_some()
                    && !renamed_away.contains(&removed.path)
            });
            if let Some(source) = source {
                renamed_away.insert(source.path.clone());
                renamed_from.insert(added.path.clone(), source.path.clone());
            }
        }
        for change in changes {
            if change.path.rsplit('/').next() == Some(layout::GPG_ID) {
                update.recipient_changes.push(RecipientChange {
                    before: ids(change.before)?,
                    after: ids(change.after)?,
                    path: change.path,
                });
                continue;
            }
            let Some(name) = entry_name(&change.path) else {
                update.other_files.push(change.path);
                continue;
            };
            // A rename is told once, under the new name; unless this device changed the old one.
            if renamed_away.contains(&change.path) && !sides.is_conflict(&change.path) {
                continue;
            }
            let kind = match (&change.before, &change.after, renamed_from.get(&change.path)) {
                (_, None, _) => ChangeKind::Removed,
                (None, Some(_), Some(from)) => ChangeKind::Renamed {
                    from: entry_name(from).unwrap_or(from).to_string(),
                },
                (None, Some(_), None) => ChangeKind::Added,
                (Some(_), Some(_), _) => ChangeKind::Changed,
            };
            let (mut readers_before, mut readers_after) = (readers(change.before)?, readers(change.after)?);
            if change.after.is_none() || (change.before.is_some() && readers_before == readers_after) {
                readers_before.clear();
                readers_after.clear();
            }
            update.entries.push(EntryChange {
                name: name.to_string(),
                kind,
                readers_before,
                readers_after,
                conflict: sides.is_conflict(&change.path),
            });
        }
        Ok(Some(update))
    }

    /// Takes the owner's file out of what the drop zone served, when it is one to take.
    fn receive(
        &self,
        repo: &gix::Repository,
        file: Option<Vec<u8>>,
        identity: &Identity,
    ) -> Result<FetchOutcome, PassError> {
        let Some(file) = file else {
            return Ok(FetchOutcome::NothingPublished);
        };
        let owner = self.owner();
        let reference = self.owner_reference();
        // 1. It decrypts, and is signed by the owner's key.
        let (bundle, published_at) = identity.inner.open_sealed(&file, &owner.certificate)?;
        // 2. One reference, the owner's.
        let header = bundle::read_header(&bundle, &reference)?;
        let held = git::reference(repo, &reference);
        if held == Some(header.tip) {
            return Ok(FetchOutcome::Unchanged);
        }
        if let Some(held) = held {
            if git::ancestors(repo, held)?.contains(&header.tip) {
                return Ok(FetchOutcome::Stale { published_at });
            }
        }
        // 3. Complete. Its objects are in the repository after this, and nothing else has changed.
        bundle::unpack(repo.git_dir(), &bundle, &header, || {
            git::open(&self.root, &self.author)?.ok_or(PassError::NoStore)
        })?;
        let repo = git::open(&self.root, &self.author)?.ok_or(PassError::NoStore)?;
        git::set_reference(&repo, &reference, header.tip, "pass sync: received")?;
        std::fs::write(self.published_at_file(), published_at.to_string()).map_err(|e| PassError::Io {
            message: format!("cannot note when the owner published: {e}"),
        })?;
        Ok(FetchOutcome::Received {
            update: self.describe_update(&repo)?,
        })
    }

    /// The file this device publishes: its whole history, signed, for every device.
    fn sealed_bundle(&self, repo: &gix::Repository, tip: ObjectId, identity: &Identity) -> Result<Vec<u8>, PassError> {
        let bundle = bundle::write(repo, tip, &self.my_reference())?;
        let recipients: Vec<&Certificate> = self.devices().map(|device| &device.certificate).collect();
        identity.inner.seal(bundle, &recipients)
    }

    fn version(&self, repo: &gix::Repository, blob: Option<&ObjectId>, identity: &Identity) -> EntryVersion {
        let Some(blob) = blob else {
            return EntryVersion::Absent;
        };
        let decrypted = git::blob(repo, *blob).and_then(|ciphertext| identity.inner.decrypt(&ciphertext));
        match decrypted {
            Ok(content) => EntryVersion::Content {
                text: String::from_utf8_lossy(&content).into_owned(),
            },
            Err(e) => EntryVersion::Unreadable { reason: e.to_string() },
        }
    }
}

#[uniffi::export]
impl PassStore {
    /// Opens the store at `config.directory`, which may not exist yet. Reads nothing.
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

    /// Every `.gpg-id` of the store, and the recipients in it that are no device's key.
    /// Read-only: who can read what is the owner's to set.
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
            let readers = self.readers_of(self.ids_of(&path)?);
            files.push(RecipientFile {
                unknown: readers
                    .iter()
                    .filter(|reader| reader.device.is_none())
                    .map(|reader| reader.id.clone())
                    .collect(),
                readers,
                path,
            });
        }
        Ok(files)
    }

    /// The ids an entry (existing or not) is encrypted to: the lines of the `.gpg-id` that governs it.
    pub fn recipients(&self, name: String) -> Result<Vec<String>, PassError> {
        let name = layout::clean_name(&name)?;
        self.ids_of(&layout::governing_gpg_id(&self.root, &name)?)
    }

    /// Who can read an entry: the readers its `.gpg-id` lists, who the file is really
    /// encrypted to, and whether this device is among them. Needs no key.
    pub fn entry_access(&self, name: String) -> Result<Access, PassError> {
        let name = layout::clean_name(&name)?;
        if !self.exists(&name) {
            return Err(PassError::NotFound { name });
        }
        let file = layout::read(&self.root, &layout::file_of(&name))?;
        self.access_of(&name, Some(&file))
    }

    /// Who can read what is put in a folder (`""` for the top of the store): the readers
    /// of the `.gpg-id` that governs it. To write there, this device needs every one of them to be a device.
    pub fn folder_access(&self, folder: String) -> Result<Access, PassError> {
        let folder = folder.trim_matches('/');
        let inside = if folder.is_empty() {
            "entry".to_string()
        } else {
            format!("{}/entry", layout::clean_name(folder)?)
        };
        self.access_of(&inside, None)
    }

    /// The entries this device cannot decrypt: not encrypted to its key. Needs no key itself.
    pub fn unreadable(&self) -> Result<Vec<String>, PassError> {
        let mut locked = Vec::new();
        for name in self.list()? {
            let file = layout::read(&self.root, &layout::file_of(&name))?;
            let for_me = crypto::recipient_key_ids(&file)
                .unwrap_or_default()
                .iter()
                .any(|key_id| self.me.certificate.has_key_id(key_id));
            if !for_me {
                locked.push(name);
            }
        }
        Ok(locked)
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
                has_store: false,
                outstanding: 0,
                unpublished: false,
                update_waiting: false,
                owner_published_at: 0,
                last_subject: String::new(),
                last_time: 0,
            }),
        }
    }

    /// One visit to the drop zone: publishes this device's tip when it is not the one
    /// last published, then looks at the owner's file. What the owner published is
    /// received (checked, and kept aside) but never applied: see [`Self::pending_update`].
    pub fn sync(&self, zone: DropZone, identity: Arc<Identity>) -> Result<SyncReport, PassError> {
        let _busy = self.busy.lock().unwrap();
        self.check(&identity)?;
        let remote = ssh::SshRemote::new(&zone.url, &zone.private_key, &zone.host_key)?;
        let repo = git::open_or_init(&self.root, &self.author)?;
        let mut sftp = sftp::Sftp::connect(&remote)?;

        let mut published = false;
        if let Some(head) = git::head_commit(&repo)? {
            if git::reference(&repo, &self.my_reference()) != Some(head) {
                let file = self.sealed_bundle(&repo, head, &identity)?;
                sftp.publish(&format!("{}.bundle.gpg", self.me.name), &file)?;
                git::set_reference(&repo, &self.my_reference(), head, "pass sync: published")?;
                published = true;
            }
        }

        let name = format!("{}.bundle.gpg", self.owner().name);
        let fetch = match sftp.download(&name, MAX_PUBLISHED_FILE) {
            Ok(file) => self.receive(&repo, file, &identity),
            Err(e) => Err(e),
        };
        // What makes a file not one to take is told, not thrown: the rest of the sync happened.
        let fetch = match fetch {
            Ok(outcome) => outcome,
            Err(e @ (PassError::UntrustedFile { .. } | PassError::BadBundle { .. } | PassError::TooLarge)) => {
                FetchOutcome::Refused { reason: e.to_string() }
            }
            Err(e) => return Err(e),
        };
        let repo = git::open(&self.root, &self.author)?.ok_or(PassError::NoStore)?;
        Ok(SyncReport {
            published,
            fetch,
            state: self.state_of(&repo)?,
        })
    }

    /// What the owner has published that is not in the store yet, described. `None` when there is nothing to take.
    pub fn pending_update(&self) -> Result<Option<PendingUpdate>, PassError> {
        match git::open(&self.root, &self.author)? {
            Some(repo) => self.describe_update(&repo),
            None => Ok(None),
        }
    }

    /// An entry of the pending update, decrypted: before the owner's changes, on this
    /// device, and in the update. `name` is an [`EntryChange::name`] (or its `from`).
    pub fn update_versions(&self, name: String, identity: Arc<Identity>) -> Result<EntryVersions, PassError> {
        let path = layout::file_of(&layout::clean_name(&name)?);
        let repo = git::open(&self.root, &self.author)?.ok_or(PassError::NoStore)?;
        let (base, head, theirs) = self.update_sides(&repo)?.ok_or(PassError::StaleUpdate)?;
        let sides = self.three_way(&repo, base, head, theirs)?;
        Ok(EntryVersions {
            before: self.version(&repo, sides.base.get(&path), &identity),
            mine: self.version(&repo, sides.ours.get(&path), &identity),
            theirs: self.version(&repo, sides.theirs.get(&path), &identity),
        })
    }

    /// Takes the update that was shown: `commit` is [`PendingUpdate::commit`], and it must
    /// still be the one waiting. No network. Returns the entries that changed here.
    ///
    /// With no commit of this device outstanding, the store moves forward to the owner's
    /// tip. Otherwise the owner's tip is merged, in a signed merge commit: an entry changed
    /// on one side only keeps that side's version; changed on both, it needs a [`Choice`]
    /// (`ChoiceRequired` names the ones that are missing); everything that is not an entry
    /// is the owner's.
    pub fn accept_update(
        &self,
        commit: String,
        choices: Vec<Choice>,
        identity: Arc<Identity>,
    ) -> Result<Vec<String>, PassError> {
        let _busy = self.busy.lock().unwrap();
        self.check(&identity)?;
        let repo = git::open(&self.root, &self.author)?.ok_or(PassError::NoStore)?;
        let (base, head, theirs) = self.update_sides(&repo)?.ok_or(PassError::StaleUpdate)?;
        if ObjectId::from_hex(commit.as_bytes()).ok() != Some(theirs) {
            return Err(PassError::StaleUpdate);
        }
        let outstanding = match head {
            Some(head) => git::commits_since(&repo, Some(theirs), head)?,
            None => Vec::new(),
        };
        let target = match head {
            Some(head) if !outstanding.is_empty() => {
                let sides = self.three_way(&repo, base, Some(head), theirs)?;
                let mut edits: Vec<git::Edit> = Vec::new();
                let mut undecided = Vec::new();
                let paths: BTreeSet<&String> = sides
                    .base
                    .keys()
                    .chain(sides.ours.keys())
                    .chain(sides.theirs.keys())
                    .collect();
                for path in paths {
                    let Some(name) = entry_name(path) else { continue };
                    let (ours, theirs_blob) = (sides.ours.get(path), sides.theirs.get(path));
                    let keep_mine = if sides.is_conflict(path) {
                        match choices.iter().find(|choice| choice.name == name) {
                            Some(choice) => choice.keep == Side::Mine,
                            None => {
                                undecided.push(name.to_string());
                                continue;
                            }
                        }
                    } else {
                        // Changed here only: here wins. Otherwise the owner's, changed or not.
                        theirs_blob == sides.base.get(path)
                    };
                    // The merge starts from the owner's tree: only what differs from it is written.
                    if keep_mine && ours != theirs_blob {
                        let content = ours.map(|blob| git::blob(&repo, *blob)).transpose()?;
                        edits.push((path.clone(), content));
                    }
                }
                if !undecided.is_empty() {
                    return Err(PassError::ChoiceRequired { names: undecided });
                }
                let tree = git::edit_tree(&repo, Some(git::tree_of(&repo, theirs)?), &edits)?;
                let message = format!(
                    "Merge the changes of {} into those of {}.",
                    self.owner().name,
                    self.me.name
                );
                git::write_commit(&repo, tree, &[head, theirs], &message, &self.author, &identity.inner)?
            }
            _ => theirs,
        };
        let changed = git::move_to(&repo, target, "pass sync: accepted the owner's update")?;
        Ok(changed
            .iter()
            .filter_map(|path| entry_name(path))
            .map(str::to_string)
            .collect())
    }

    /// Gives up the commits of this device that the owner has not taken: the store goes
    /// back to the owner's history as it was last accepted. Returns how many were
    /// dropped. No network: sync afterwards, to publish where this device now stands.
    pub fn discard_outstanding(&self) -> Result<u32, PassError> {
        let _busy = self.busy.lock().unwrap();
        let repo = self.repository()?;
        let head = git::head_commit(&repo)?.ok_or(PassError::NoStore)?;
        let Some(owner) = git::reference(&repo, &self.owner_reference()) else {
            return Ok(0);
        };
        // Not the owner's tip itself when an update is waiting: that one has not been accepted.
        let target = git::merge_base(&repo, head, owner)?
            .ok_or_else(|| git::git_error("cannot go back", "this device shares no history with the owner"))?;
        let dropped = git::commits_since(&repo, Some(target), head)?.len() as u32;
        if dropped > 0 {
            git::move_to(&repo, target, "pass sync: dropped this device's changes")?;
        }
        Ok(dropped)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names() {
        for good in ["pc", "phone", "0", "alices-iphone-15", &"a".repeat(32)] {
            assert!(is_device_name(good), "{good:?}");
        }
        for bad in ["", "-phone", "Phone", "my phone", "a/b", "../x", "é", &"a".repeat(33)] {
            assert!(!is_device_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_entries_are_a_members_to_change() {
        assert_eq!(entry_name("bank.gpg"), Some("bank"));
        assert_eq!(entry_name("Email/work.gpg"), Some("Email/work"));
        for not_an_entry in [
            ".gpg-id",
            "work/.gpg-id",
            ".gpg-id.sig",
            ".gitattributes",
            ".extensions/hook.bash",
            ".extensions/x.gpg",
            "notes.txt",
            ".gpg",
            "a/.hidden.gpg",
        ] {
            assert_eq!(entry_name(not_an_entry), None, "{not_an_entry}");
        }
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            error_message(PassError::NotFound { name: "bank".into() }),
            "bank is not in the password store"
        );
        assert_eq!(
            error_message(PassError::ChoiceRequired {
                names: vec!["bank".into(), "wifi".into()]
            }),
            "both sides changed bank, wifi: choose which version to keep"
        );
    }

    #[test]
    fn a_whole_file_merge() {
        let id = |n: u8| ObjectId::from_hex(format!("{n:040x}").as_bytes()).unwrap();
        let side = |files: &[(&str, u8)]| files.iter().map(|(path, n)| (path.to_string(), id(*n))).collect();
        let sides = ThreeWay {
            base: side(&[
                ("same", 1),
                ("mine", 1),
                ("theirs", 1),
                ("both", 1),
                ("agreed", 1),
                ("gone-here", 1),
                ("gone-there", 1),
            ]),
            ours: side(&[
                ("same", 1),
                ("mine", 2),
                ("theirs", 1),
                ("both", 2),
                ("agreed", 4),
                ("gone-there", 2),
                ("new-here", 5),
            ]),
            theirs: side(&[
                ("same", 1),
                ("mine", 1),
                ("theirs", 3),
                ("both", 3),
                ("agreed", 4),
                ("gone-here", 3),
                ("new-there", 6),
            ]),
        };
        for path in ["same", "mine", "theirs", "agreed", "new-here", "new-there", "nowhere"] {
            assert!(!sides.is_conflict(path), "{path}");
        }
        // Changed on both sides to different contents, or changed on one and removed on the other.
        for path in ["both", "gone-here", "gone-there"] {
            assert!(sides.is_conflict(path), "{path}");
        }
    }
}
