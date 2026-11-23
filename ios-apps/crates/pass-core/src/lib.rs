//! A password store: the layout of `pass`, the standard Unix password manager
//! (<https://www.passwordstore.org>), OpenPGP files in a git repository, with
//! neither `gpg` nor `git`. Exposed to Swift through UniFFI.
//!
//! It syncs as `apps/pass/SYNC.md` says. Devices are equals: each keeps its own line
//! of history and publishes all of it as one signed and encrypted git bundle in a
//! drop zone that understands none of it. What another device published is received,
//! checked and kept aside; nothing of it is taken before it has been shown and
//! accepted, and taking it is moving forward or merging, never anything silent.
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
mod merge;
mod otp;
mod sftp;
mod ssh;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gix::ObjectId;
use zeroize::Zeroizing;

use crate::crypto::{Certificate, SecretIdentity};
use crate::merge::Entries;

uniffi::setup_scaffolding!();

/// What the phone calls itself where `pass` names the editor: `Edit password for x using pass-ios.`
const EDITOR: &str = "pass-ios";
/// A reader refuses a published file over this size, and stops decrypting past it (SYNC.md).
const MAX_PUBLISHED_FILE: u64 = 256 * 1024 * 1024;
/// How many commits of an update are described: the screen that shows them is not a git log.
const MAX_LISTED_COMMITS: usize = 200;
/// What `pass git init` writes, and what a store made here holds too.
const GIT_ATTRIBUTES: &str = "*.gpg diff=gpg\n";
const ATTRIBUTES_PATH: &str = ".gitattributes";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PassError {
    #[error("{message}")]
    InvalidName { message: String },
    #[error("{name} is not in the password store")]
    NotFound { name: String },
    #[error("an entry already exists for {name}")]
    AlreadyExists { name: String },
    #[error("the store has no .gpg-id here: nobody is named as a reader")]
    NotInitialized,
    #[error("{id} names no device, or more than one: a reader is one of the devices this one knows")]
    UnknownRecipient { id: String },
    #[error("{message}")]
    InvalidKey { message: String },
    #[error("wrong passphrase")]
    WrongPassphrase,
    #[error("this entry is not encrypted to the key on this device")]
    NotForThisKey,
    #[error("this device's key cannot sign (its signing part is not here): nothing it published would be accepted")]
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
    #[error("not a store that syncs: {message}")]
    RefusedTip { message: String },
    #[error("the two stores have no history in common, and this one is not empty")]
    Unrelated,
    #[error("a choice is needed (mine or theirs) for: {}", names.join(", "))]
    ChoiceRequired { names: Vec<String> },
    #[error("this is not the update that was last received: look at it again")]
    StaleUpdate,
    #[error("no device named {name} is known here")]
    UnknownDevice { name: String },
    #[error("there is no store on this device yet: sync, and take another device's")]
    NoStore,
    #[error("{message}")]
    Git { message: String },
    #[error("{message}")]
    Io { message: String },
}

/// Another device, as this one is told about it. Nothing received can add one or change a key.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Peer {
    /// `[a-z0-9][a-z0-9-]{0,31}`: names its file in the drop zone.
    pub name: String,
    /// 40 hex digits: the fingerprint of its OpenPGP key.
    pub fingerprint: String,
    /// That key, armored (`gpg --export --armor <fingerprint>`).
    pub public_key: String,
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
    /// Where the store is, or will be once one has been made or adopted.
    pub directory: String,
    /// This device's name.
    pub device: String,
    /// This device's OpenPGP public key, armored (see [`public_key_of`]).
    pub public_key: String,
    /// The other devices.
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
    /// Armored: for the other devices.
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

/// Whether a commit is signed by a device this one knows. Shown, never required:
/// what vouches for a tip is the signature of the file it came in.
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

/// A line of a `.gpg-id`: someone who should read what is under it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Reader {
    /// The line itself: a fingerprint, a key id, a text of a user id.
    pub id: String,
    /// The device whose key it is. `None` for a line that names no device, or several:
    /// such a store does not sync, and this device cannot write under that file.
    pub device: Option<String>,
}

/// Who can read an entry, or what is put in a folder.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Access {
    /// The `.gpg-id` that governs it: `.gpg-id`, `team/.gpg-id`. Empty when there is none.
    pub gpg_id: String,
    /// Its lines: who the store says should read this.
    pub readers: Vec<Reader>,
    /// This device is one of them.
    pub listed: bool,
    /// For an existing entry: who the file is actually encrypted to (device names, or
    /// `key <id>` for a key that is no device's). Empty for a folder.
    pub encrypted_to: Vec<String>,
    /// This device can decrypt it: for an entry, by what the file is encrypted to; for a folder, `listed`.
    pub readable: bool,
    /// For an existing entry: the file is not encrypted to exactly the readers of its `.gpg-id`.
    /// Saying is not doing: someone who can read it has to encrypt it again. False for a folder.
    pub stale: bool,
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

/// One thing taking a device's update would do: the vocabulary of SYNC.md, all about theirs.
/// [`review_line_text`] writes one as the protocol spells it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ReviewLine {
    Added {
        name: String,
    },
    Changed {
        name: String,
    },
    Moved {
        from: String,
        to: String,
    },
    Removed {
        name: String,
    },
    /// The readers of a folder (`work/`, `/` for the root) change: the new set, in the order
    /// of the file; or the file is removed, and the folder inherits those of the one above.
    Readers {
        folder: String,
        devices: Vec<String>,
        inherited: bool,
    },
    /// In their tip, an entry that is not encrypted for the readers of its folder.
    Stale {
        name: String,
    },
    /// Needs a choice, mine or theirs: an entry name, or a folder with its slash for its readers.
    Conflict {
        name: String,
    },
}

/// How taking an update happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TakeKind {
    /// This device has no store: it takes theirs as it is.
    Adopt,
    /// Nothing of this device's own since the two last agreed: it moves forward to their tip.
    Forward,
    /// Both went on: a merge commit, signed, whose parents are mine then theirs.
    Merge,
}

/// What a device has published that this one has not taken.
/// Nothing of it is in the store until [`PassStore::take_update`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PendingUpdate {
    /// Whose it is.
    pub device: String,
    /// Their tip: what to give to [`PassStore::take_update`] to take exactly what was shown.
    pub commit: String,
    /// When they signed the file, seconds since 1970.
    pub published_at: i64,
    pub kind: TakeKind,
    /// Their commits that are not here, newest first (at most 200 of them), and how many there are.
    pub commits: Vec<CommitInfo>,
    pub commit_count: u32,
    /// What taking it would do, in the order of the vocabulary, by name within each kind.
    pub lines: Vec<ReviewLine>,
    /// How many stale entries of their tip this device can read: taking the update
    /// encrypts those again, for the readers their folders now have.
    pub reencrypts: u32,
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

/// An entry as an update finds it and as it leaves it, each side followed through its moves.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryVersions {
    /// Where the two lines last agreed.
    pub base: EntryVersion,
    /// On this device now.
    pub mine: EntryVersion,
    /// In their tip.
    pub theirs: EntryVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Side {
    /// As this device has it (which may be: removed).
    Mine,
    /// As they have it.
    Theirs,
}

/// Which side to keep, whole, of something both changed: an entry by its name (on this
/// device's side, or theirs if it is gone here), the readers of a folder by `folder/`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Choice {
    pub name: String,
    pub keep: Side,
}

/// What looking at a device's file gave.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FetchOutcome {
    /// It has published nothing (or the drop zone lost it).
    NothingPublished,
    /// The same tip as last time.
    Unchanged,
    /// A new tip was received, checked and stored.
    Received,
    /// A valid file, but older than the one already received: served again, and ignored.
    Stale { published_at: i64 },
    /// Not a file this device takes (wrong signer, wrong reference, incomplete, damaged, too big,
    /// or a tip that holds what a synced store does not). Nothing changed.
    Refused { reason: String },
}

/// Where this device stands with another one.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeerState {
    pub device: String,
    /// A tip of it has been received.
    pub received: bool,
    /// Its commits that are not here: 0, nothing to take.
    pub to_take: u32,
    /// When it signed the last file received, seconds since 1970. 0 when none was.
    pub published_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeerSync {
    pub outcome: FetchOutcome,
    pub state: PeerState,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// Whether this device's tip was sent (it is, whenever it is not what was last published).
    pub published: bool,
    /// One per other device, in the order of the configuration.
    pub peers: Vec<PeerSync>,
}

/// Where this device stands, from what it has: no network.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncState {
    /// There is a store here: made, or adopted from another device.
    pub has_store: bool,
    /// The tip of this device is not the one it last published.
    pub unpublished: bool,
    pub peers: Vec<PeerState>,
    /// The first line of the last commit, and when it was made.
    pub last_subject: String,
    pub last_time: i64,
}

/// What taking an update did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Taken {
    pub kind: TakeKind,
    /// The entries that changed here.
    pub changed: Vec<String>,
    /// The stale entries this device then encrypted again for their readers.
    pub reencrypted: Vec<String>,
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

/// The name of the entry a path of the repository is, when it is one.
fn entry_name(path: &str) -> Option<&str> {
    path.strip_suffix(".gpg")
        .filter(|name| layout::clean_name(name).is_ok_and(|clean| clean == *name))
}

/// The folder a path of the repository is the readers file of (`""` for the root), when it is one.
fn readers_folder(path: &str) -> Option<&str> {
    if path == layout::GPG_ID {
        return Some("");
    }
    path.strip_suffix(&format!("/{}", layout::GPG_ID))
        .filter(|folder| layout::clean_name(folder).is_ok_and(|clean| clean == *folder))
}

/// A folder as the protocol names it: `work/`, and `/` for the root.
fn folder_label(folder: &str) -> String {
    format!("{folder}/")
}

/// A folder given by a user (`work/`, `work`, `/`, or nothing for the root) as the path it is: `work`, or `""`.
fn clean_folder(folder: &str) -> Result<String, PassError> {
    let folder = folder.trim_matches('/');
    if folder.is_empty() {
        Ok(String::new())
    } else {
        layout::clean_name(folder)
    }
}

fn readers_path(folder: &str) -> String {
    if folder.is_empty() {
        layout::GPG_ID.to_string()
    } else {
        format!("{folder}/{}", layout::GPG_ID)
    }
}

/// The folder whose readers file governs `name`, among the folders that have one.
fn governing<'a, T>(readers: &'a BTreeMap<String, T>, name: &str) -> Option<(&'a str, &'a T)> {
    let mut folder = name;
    loop {
        folder = folder.rsplit_once('/').map(|(above, _)| above).unwrap_or("");
        if let Some((key, value)) = readers.get_key_value(folder) {
            return Some((key.as_str(), value));
        }
        if folder.is_empty() {
            return None;
        }
    }
}

/// One [`ReviewLine`] as the protocol writes it: `moved bank -> money/bank`, `readers work/: pc phone`.
#[uniffi::export]
pub fn review_line_text(line: ReviewLine) -> String {
    match line {
        ReviewLine::Added { name } => format!("added {name}"),
        ReviewLine::Changed { name } => format!("changed {name}"),
        ReviewLine::Moved { from, to } => format!("moved {from} -> {to}"),
        ReviewLine::Removed { name } => format!("removed {name}"),
        ReviewLine::Readers {
            folder,
            inherited: true,
            ..
        } => format!("readers {folder}: inherited"),
        ReviewLine::Readers { folder, devices, .. } => format!("readers {folder}: {}", devices.join(" ")),
        ReviewLine::Stale { name } => format!("stale {name}"),
        ReviewLine::Conflict { name } => format!("conflict {name}"),
    }
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
            message: "the key needs a name: it is how the other devices will recognise it".into(),
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

/// What a commit holds, sorted out: a store that syncs holds these three kinds of path and nothing else.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Tip {
    /// Name (`Email/work`) → the file.
    entries: Entries,
    /// Folder (`""` for the root) → its readers file.
    readers: BTreeMap<String, ObjectId>,
    /// The root `.gitattributes`.
    attributes: Option<ObjectId>,
}

impl Tip {
    /// Every file, by its path in the repository.
    fn files(&self) -> BTreeMap<String, ObjectId> {
        let mut files: BTreeMap<String, ObjectId> = self
            .entries
            .iter()
            .map(|(name, blob)| (layout::file_of(name), *blob))
            .collect();
        files.extend(self.readers.iter().map(|(folder, blob)| (readers_path(folder), *blob)));
        files.extend(self.attributes.map(|blob| (ATTRIBUTES_PATH.to_string(), blob)));
        files
    }
}

/// Decides whether two versions of an entry are the same: equal bytes, or equal once
/// decrypted when this device reads both. Remembers what it decrypted, zeroed when dropped.
struct Sameness<'a> {
    repo: &'a gix::Repository,
    identity: &'a SecretIdentity,
    plaintexts: HashMap<ObjectId, Option<Zeroizing<Vec<u8>>>>,
}

impl<'a> Sameness<'a> {
    fn new(repo: &'a gix::Repository, identity: &'a SecretIdentity) -> Self {
        Self {
            repo,
            identity,
            plaintexts: HashMap::new(),
        }
    }

    fn plaintext(&mut self, blob: ObjectId) -> Option<&Zeroizing<Vec<u8>>> {
        let (repo, identity) = (self.repo, self.identity);
        self.plaintexts
            .entry(blob)
            .or_insert_with(|| {
                git::blob(repo, blob)
                    .ok()
                    .and_then(|ciphertext| identity.decrypt(&ciphertext).ok())
            })
            .as_ref()
    }

    fn same(&mut self, a: ObjectId, b: ObjectId) -> bool {
        if a == b {
            return true;
        }
        let first = self.plaintext(a).map(|text| text.to_vec());
        match (first, self.plaintext(b)) {
            (Some(first), Some(second)) => {
                let equal = first == **second;
                drop(Zeroizing::new(first));
                equal
            }
            _ => false,
        }
    }
}

/// An update that can be taken, worked out: the three sides, and how.
struct Plan {
    kind: TakeKind,
    head: Option<ObjectId>,
    theirs_commit: ObjectId,
    base: Tip,
    mine: Tip,
    theirs: Tip,
}

/// A password store on this device.
#[derive(uniffi::Object)]
pub struct PassStore {
    root: PathBuf,
    me: Device,
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
        let mut peers: Vec<Device> = Vec::new();
        for peer in config.peers {
            check_name(&peer.name)?;
            if peer.name == me.name || peers.iter().any(|known| known.name == peer.name) {
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
            let same_key = std::iter::once(&me)
                .chain(&peers)
                .find(|known| known.certificate.fingerprint() == certificate.fingerprint());
            if let Some(known) = same_key {
                return Err(invalid(format!("{} and {} have the same key", known.name, peer.name)));
            }
            peers.push(Device {
                name: peer.name,
                certificate,
            });
        }
        let author = git::Author {
            email: format!("{}@pass-sync.invalid", me.name),
            name: me.name.clone(),
        };
        Ok(Self {
            root: PathBuf::from(config.directory),
            me,
            peers,
            author,
            busy: Mutex::new(()),
        })
    }

    fn devices(&self) -> impl Iterator<Item = &Device> {
        std::iter::once(&self.me).chain(&self.peers)
    }

    fn peer(&self, name: &str) -> Result<&Device, PassError> {
        self.peers
            .iter()
            .find(|peer| peer.name == name)
            .ok_or_else(|| PassError::UnknownDevice { name: name.to_string() })
    }

    fn device(&self, name: &str) -> Result<&Device, PassError> {
        self.devices()
            .find(|device| device.name == name)
            .ok_or_else(|| PassError::UnknownDevice { name: name.to_string() })
    }

    /// Where the tip last received from a device is kept (and for this one: the tip it last published).
    fn reference_of(&self, device: &Device) -> String {
        format!("refs/pass-sync/{}", device.name)
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

    fn open_repository(&self) -> Result<Option<gix::Repository>, PassError> {
        git::open(&self.root, &self.author)
    }

    /// The repository and its last commit, once there is a store in it.
    fn repository(&self) -> Result<(gix::Repository, ObjectId), PassError> {
        let repo = self.open_repository()?.ok_or(PassError::NoStore)?;
        let head = git::head_commit(&repo)?.ok_or(PassError::NoStore)?;
        Ok((repo, head))
    }

    // MARK: readers

    /// The one device a line of a `.gpg-id` names: by fingerprint or 16-hex key id, or by a
    /// text that is in the user id of its key and of no other device's.
    fn device_of(&self, id: &str) -> Option<&Device> {
        let mut named = self.devices().filter(|device| device.certificate.matches(id));
        let device = named.next()?;
        named.next().is_none().then_some(device)
    }

    fn readers_of(&self, ids: Vec<String>) -> Vec<Reader> {
        ids.into_iter()
            .map(|id| Reader {
                device: self.device_of(&id).map(|device| device.name.clone()),
                id,
            })
            .collect()
    }

    /// The devices a readers file names, in its order, each once. An error when a line names
    /// no device or several, or when it names nobody at all.
    fn resolve(&self, content: &[u8], path: &str) -> Result<Vec<&Device>, PassError> {
        let mut devices: Vec<&Device> = Vec::new();
        for id in layout::parse_gpg_id(&String::from_utf8_lossy(content)) {
            let device = self.device_of(&id).ok_or(PassError::UnknownRecipient {
                id: format!("{path}: {id}"),
            })?;
            if !devices.iter().any(|known| known.name == device.name) {
                devices.push(device);
            }
        }
        if devices.is_empty() {
            return Err(PassError::RefusedTip {
                message: format!("{path} names nobody"),
            });
        }
        Ok(devices)
    }

    /// The long ids of the keys a file must be encrypted to, for these readers. Sorted.
    fn wanted_key_ids(readers: &[&Device]) -> Result<Vec<String>, PassError> {
        let mut wanted = Vec::new();
        for reader in readers {
            wanted.push(reader.certificate.encryption_key_id()?);
        }
        wanted.sort();
        wanted.dedup();
        Ok(wanted)
    }

    /// Who an encrypted file can be read by: device names, or key ids for keys that are no device's.
    fn encrypted_to(&self, ciphertext: &[u8]) -> Vec<String> {
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

    fn for_me(&self, ciphertext: &[u8]) -> bool {
        crypto::recipient_key_ids(ciphertext)
            .unwrap_or_default()
            .iter()
            .any(|key_id| self.me.certificate.has_key_id(key_id))
    }

    // MARK: tips

    /// Sorts out what a commit holds, and refuses it when it holds anything a synced store does not.
    fn read_tip(&self, repo: &gix::Repository, commit: ObjectId) -> Result<Tip, PassError> {
        let mut tip = Tip::default();
        let mut others = Vec::new();
        for (path, kind, blob) in git::paths(repo, commit)? {
            let regular = kind == git::PathKind::File;
            if let (true, Some(name)) = (regular, entry_name(&path)) {
                tip.entries.insert(name.to_string(), blob);
            } else if let (true, Some(folder)) = (regular, readers_folder(&path)) {
                tip.readers.insert(folder.to_string(), blob);
            } else if regular && path == ATTRIBUTES_PATH {
                tip.attributes = Some(blob);
            } else {
                others.push(path);
            }
        }
        if !others.is_empty() {
            return Err(PassError::RefusedTip {
                message: format!(
                    "it holds what a synced store does not (only entries, .gpg-id files and the root .gitattributes): {}",
                    others.join(", ")
                ),
            });
        }
        if !tip.readers.contains_key("") {
            return Err(PassError::RefusedTip {
                message: "it has no .gpg-id at its root".into(),
            });
        }
        Ok(tip)
    }

    /// The readers of every folder of a tip that names some, as devices.
    fn tip_readers(&self, repo: &gix::Repository, tip: &Tip) -> Result<BTreeMap<String, Vec<&Device>>, PassError> {
        let mut readers = BTreeMap::new();
        for (folder, blob) in &tip.readers {
            let devices = self.resolve(&git::blob(repo, *blob)?, &readers_path(folder))?;
            readers.insert(folder.clone(), devices);
        }
        Ok(readers)
    }

    /// The stale entries of a tip: not encrypted to exactly the readers of their folder. Sorted.
    /// Also checks what makes a tip refused whole: a reader that is no device, an entry
    /// encrypted to a key that is no device's.
    fn stale_in(&self, repo: &gix::Repository, tip: &Tip) -> Result<Vec<String>, PassError> {
        let readers = self.tip_readers(repo, tip)?;
        let mut wanted: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (folder, devices) in &readers {
            wanted.insert(folder.as_str(), Self::wanted_key_ids(devices)?);
        }
        let mut stale = Vec::new();
        for (name, blob) in &tip.entries {
            let key_ids = crypto::recipient_key_ids(&git::blob(repo, *blob)?).map_err(|_| PassError::RefusedTip {
                message: format!("{name} is not an encrypted file"),
            })?;
            let strangers = key_ids
                .iter()
                .any(|key_id| !self.devices().any(|device| device.certificate.has_key_id(key_id)));
            if strangers || key_ids.is_empty() {
                return Err(PassError::RefusedTip {
                    message: format!("{name} is encrypted to a key that is no device's"),
                });
            }
            let (folder, _) = governing(&readers, name).expect("the root has readers");
            if key_ids != wanted[folder] {
                stale.push(name.clone());
            }
        }
        Ok(stale)
    }

    /// A tip that can be taken or published: only what a synced store holds, every reader
    /// a device, every entry encrypted to devices only.
    fn valid_tip(&self, repo: &gix::Repository, commit: ObjectId) -> Result<Tip, PassError> {
        let tip = self.read_tip(repo, commit)?;
        self.stale_in(repo, &tip)?;
        Ok(tip)
    }

    // MARK: entries

    fn exists(&self, name: &str) -> bool {
        self.root.join(layout::file_of(name)).is_file()
    }

    fn ids_of(&self, gpg_id: &str) -> Result<Vec<String>, PassError> {
        let content = layout::read(&self.root, gpg_id)?;
        Ok(layout::parse_gpg_id(&String::from_utf8_lossy(&content)))
    }

    /// The keys an entry is to be encrypted to: those of the readers of its folder, whether
    /// or not this device is one. All of them or an error: never fewer readers than the store says.
    fn recipients_of(&self, name: &str) -> Result<Vec<&Certificate>, PassError> {
        let path = layout::governing_gpg_id(&self.root, name)?;
        let devices = self.resolve(&layout::read(&self.root, &path)?, &path)?;
        Ok(devices.into_iter().map(|device| &device.certificate).collect())
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

    /// Commits the edits (signed) on top of the last commit, then moves the directory to that commit.
    fn apply(&self, edits: Vec<git::Edit>, message: &str, identity: &Identity) -> Result<(), PassError> {
        self.check(identity)?;
        let (repo, head) = self.repository()?;
        let tree = git::edit_tree(&repo, Some(git::tree_of(&repo, head)?), &edits)?;
        if tree == git::tree_of(&repo, head)? {
            return Ok(());
        }
        let commit = git::write_commit(&repo, tree, &[head], message, &self.author, &identity.inner)?;
        git::move_to(&repo, commit, &format!("commit: {message}"))?;
        Ok(())
    }

    /// Encrypts again every stale entry this device can read, for the readers its folder
    /// now has: one commit per folder. Returns their names. What it cannot read stays stale,
    /// for a device that can.
    fn reencrypt_stale(&self, identity: &Identity) -> Result<Vec<String>, PassError> {
        let (repo, head) = self.repository()?;
        let tip = self.read_tip(&repo, head)?;
        let readers = self.tip_readers(&repo, &tip)?;
        let mut by_folder: BTreeMap<String, Vec<git::Edit>> = BTreeMap::new();
        let mut names = Vec::new();
        for name in self.stale_in(&repo, &tip)? {
            let Ok(content) = identity.inner.decrypt(&git::blob(&repo, tip.entries[&name])?) else {
                continue;
            };
            let (folder, devices) = governing(&readers, &name).expect("the root has readers");
            let recipients: Vec<&Certificate> = devices.iter().map(|device| &device.certificate).collect();
            let encrypted = crypto::encrypt(&content, &recipients)?;
            by_folder
                .entry(folder.to_string())
                .or_default()
                .push((layout::file_of(&name), Some(encrypted)));
            names.push(name);
        }
        for (folder, edits) in by_folder {
            let message = format!("Reencrypt for the readers of {}.", folder_label(&folder));
            self.apply(edits, &message, identity)?;
        }
        Ok(names)
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
                stale: false,
            });
        };
        let key_ids = crypto::recipient_key_ids(file).unwrap_or_default();
        // What the `.gpg-id` asks for, as far as this device can tell: a line that names no
        // device has a key this cannot name, and nothing then matches.
        let devices: Option<Vec<&Device>> = readers.iter().map(|reader| self.device_of(&reader.id)).collect();
        let stale = match devices {
            Some(devices) => Self::wanted_key_ids(&devices)? != key_ids,
            None => true,
        };
        Ok(Access {
            gpg_id,
            readers,
            listed,
            encrypted_to: self.encrypted_to(file),
            readable: self.for_me(file),
            stale,
        })
    }

    // MARK: the sync

    fn published_at_file(&self, device: &Device) -> PathBuf {
        self.root
            .join(".git")
            .join(format!("pass-sync-published-at-{}", device.name))
    }

    fn published_at(&self, device: &Device) -> i64 {
        std::fs::read_to_string(self.published_at_file(device))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    fn peer_state(&self, repo: &gix::Repository, peer: &Device) -> Result<PeerState, PassError> {
        let tip = git::reference(repo, &self.reference_of(peer));
        let to_take = match tip {
            Some(tip) => git::commits_since(repo, git::head_commit(repo)?, tip)?.len() as u32,
            None => 0,
        };
        Ok(PeerState {
            device: peer.name.clone(),
            received: tip.is_some(),
            to_take,
            published_at: self.published_at(peer),
        })
    }

    fn state_of(&self, repo: &gix::Repository) -> Result<SyncState, PassError> {
        let head = git::head_commit(repo)?;
        let last = head.map(|head| git::summary(repo, head)).transpose()?;
        let mut peers = Vec::new();
        for peer in &self.peers {
            peers.push(self.peer_state(repo, peer)?);
        }
        Ok(SyncState {
            has_store: head.is_some(),
            unpublished: head.is_some() && head != git::reference(repo, &self.reference_of(&self.me)),
            peers,
            last_subject: last.as_ref().map(|last| last.subject.clone()).unwrap_or_default(),
            last_time: last.map(|last| last.time).unwrap_or(0),
        })
    }

    /// Takes the file of the device `from` out of what the drop zone served, when it is one to take:
    /// its tip is then stored, and nothing else has changed.
    fn receive(
        &self,
        repo: &gix::Repository,
        file: Option<Vec<u8>>,
        from: &Device,
        identity: &Identity,
    ) -> Result<FetchOutcome, PassError> {
        let Some(file) = file else {
            return Ok(FetchOutcome::NothingPublished);
        };
        let reference = self.reference_of(from);
        // 1. It decrypts, and is signed by the key of the device it is named after.
        let (bundle, published_at) = identity
            .inner
            .open_sealed(&file, &from.certificate, MAX_PUBLISHED_FILE)?;
        // 2. One reference, theirs.
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
            self.open_repository()?.ok_or(PassError::NoStore)
        })?;
        let repo = self.open_repository()?.ok_or(PassError::NoStore)?;
        // And it is a store that syncs: nothing but entries and readers files, every reader a device.
        let tip = self.valid_tip(&repo, header.tip)?;
        if let Some(head) = git::head_commit(&repo)? {
            self.same_attributes(&self.read_tip(&repo, head)?, &tip)?;
        }
        git::set_reference(&repo, &reference, header.tip, "pass sync: received")?;
        std::fs::write(self.published_at_file(from), published_at.to_string()).map_err(|e| PassError::Io {
            message: format!("cannot note when {} published: {e}", from.name),
        })?;
        Ok(FetchOutcome::Received)
    }

    /// `.gitattributes` is taken with the store and never changes afterwards.
    fn same_attributes(&self, mine: &Tip, theirs: &Tip) -> Result<(), PassError> {
        if mine.attributes == theirs.attributes {
            Ok(())
        } else {
            Err(PassError::RefusedTip {
                message: ".gitattributes is not the one this store was adopted with".into(),
            })
        }
    }

    /// The file this device publishes: its whole history, signed, for every device.
    fn sealed_bundle(&self, repo: &gix::Repository, tip: ObjectId, identity: &Identity) -> Result<Vec<u8>, PassError> {
        let bundle = bundle::write(repo, tip, &self.reference_of(&self.me))?;
        let recipients: Vec<&Certificate> = self.devices().map(|device| &device.certificate).collect();
        identity.inner.seal(bundle, &recipients)
    }

    /// Works out what taking the tip of `from` would be. `None` when it has nothing this device has not.
    fn plan(&self, repo: &gix::Repository, from: &Device) -> Result<Option<Plan>, PassError> {
        let head = git::head_commit(repo)?;
        let Some(theirs_commit) = git::reference(repo, &self.reference_of(from)) else {
            return Ok(None);
        };
        let theirs = self.valid_tip(repo, theirs_commit)?;
        let Some(head) = head else {
            return Ok(Some(Plan {
                kind: TakeKind::Adopt,
                head: None,
                theirs_commit,
                base: Tip::default(),
                mine: Tip::default(),
                theirs,
            }));
        };
        if git::ancestors(repo, head)?.contains(&theirs_commit) {
            return Ok(None);
        }
        let mine = self.read_tip(repo, head)?;
        self.same_attributes(&mine, &theirs)?;
        let base_commit = git::merge_base(repo, head, theirs_commit)?.ok_or(PassError::Unrelated)?;
        Ok(Some(Plan {
            kind: if base_commit == head {
                TakeKind::Forward
            } else {
                TakeKind::Merge
            },
            head: Some(head),
            theirs_commit,
            base: self.read_tip(repo, base_commit)?,
            mine,
            theirs,
        }))
    }

    /// The readers files of a merge: each folder's, or the folders that need a choice.
    fn merge_readers(plan: &Plan, choices: &HashMap<String, Side>) -> (BTreeMap<String, ObjectId>, Vec<String>) {
        let folders: BTreeSet<&String> = plan
            .base
            .readers
            .keys()
            .chain(plan.mine.readers.keys())
            .chain(plan.theirs.readers.keys())
            .collect();
        let (mut merged, mut conflicts) = (BTreeMap::new(), Vec::new());
        for folder in folders {
            let label = folder_label(folder);
            let get = |tip: &Tip| tip.readers.get(folder).copied();
            // Asked with no choice first: that tells whether one is needed at all.
            let needed = merge::merge_whole(get(&plan.base), get(&plan.mine), get(&plan.theirs), None).is_err();
            if needed {
                conflicts.push(label.clone());
            }
            let choice = choices.get(&label).copied();
            if let Ok(Some(blob)) = merge::merge_whole(get(&plan.base), get(&plan.mine), get(&plan.theirs), choice) {
                merged.insert(folder.clone(), blob);
            }
        }
        (merged, conflicts)
    }

    fn describe(
        &self,
        repo: &gix::Repository,
        from: &Device,
        plan: &Plan,
        identity: &Identity,
    ) -> Result<PendingUpdate, PassError> {
        let new_commits = git::commits_since(repo, plan.head, plan.theirs_commit)?;
        let mut commits = Vec::new();
        for id in new_commits.iter().take(MAX_LISTED_COMMITS) {
            let summary = git::summary(repo, *id)?;
            commits.push(CommitInfo {
                id: id.to_string(),
                subject: summary.subject,
                author: summary.author,
                time: summary.time,
                signature: match git::signature_of(repo, *id)? {
                    Some(found) => match self
                        .devices()
                        .find(|device| device.certificate.verifies(&found.signature, &found.signed))
                    {
                        Some(device) => SignatureStatus::Valid {
                            device: device.name.clone(),
                        },
                        None => SignatureStatus::Invalid,
                    },
                    None => SignatureStatus::Unsigned,
                },
            });
        }

        let mut sameness = Sameness::new(repo, &identity.inner);
        let mut same = |a: ObjectId, b: ObjectId| sameness.same(a, b);
        let told = merge::changes(&plan.base.entries, &plan.theirs.entries, &mut same);
        let mut lines = Vec::new();
        lines.extend(told.added.into_iter().map(|name| ReviewLine::Added { name }));
        lines.extend(told.changed.into_iter().map(|name| ReviewLine::Changed { name }));
        lines.extend(told.moved.into_iter().map(|(from, to)| ReviewLine::Moved { from, to }));
        lines.extend(told.removed.into_iter().map(|name| ReviewLine::Removed { name }));
        let their_readers = self.tip_readers(repo, &plan.theirs)?;
        let folders: BTreeSet<&String> = plan.base.readers.keys().chain(plan.theirs.readers.keys()).collect();
        for folder in folders {
            if plan.base.readers.get(folder) == plan.theirs.readers.get(folder) {
                continue;
            }
            let devices = their_readers.get(folder);
            lines.push(ReviewLine::Readers {
                folder: folder_label(folder),
                devices: devices
                    .map(|devices| devices.iter().map(|device| device.name.clone()).collect())
                    .unwrap_or_default(),
                inherited: devices.is_none(),
            });
        }
        let stale = self.stale_in(repo, &plan.theirs)?;
        let mut reencrypts = 0;
        for name in &stale {
            if self.for_me(&git::blob(repo, plan.theirs.entries[name])?) {
                reencrypts += 1;
            }
        }
        lines.extend(stale.into_iter().map(|name| ReviewLine::Stale { name }));
        if plan.kind == TakeKind::Merge {
            let none = HashMap::new();
            let entries = merge::merge(
                &plan.base.entries,
                &plan.mine.entries,
                &plan.theirs.entries,
                &none,
                &mut same,
            );
            let mut conflicts = entries.conflicts;
            conflicts.extend(Self::merge_readers(plan, &none).1);
            conflicts.sort();
            lines.extend(conflicts.into_iter().map(|name| ReviewLine::Conflict { name }));
        }
        Ok(PendingUpdate {
            device: from.name.clone(),
            commit: plan.theirs_commit.to_string(),
            published_at: self.published_at(from),
            kind: plan.kind,
            commits,
            commit_count: new_commits.len() as u32,
            lines,
            reencrypts,
        })
    }

    fn version(&self, repo: &gix::Repository, blob: Option<ObjectId>, identity: &Identity) -> EntryVersion {
        let Some(blob) = blob else {
            return EntryVersion::Absent;
        };
        let decrypted = git::blob(repo, blob).and_then(|ciphertext| identity.inner.decrypt(&ciphertext));
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

    /// Makes a new store here, whose root readers are these devices (by name: this one,
    /// the others, or both). Only where there is none yet.
    pub fn create(&self, readers: Vec<String>, identity: Arc<Identity>) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        self.check(&identity)?;
        let repo = git::open_or_init(&self.root, &self.author)?;
        if git::head_commit(&repo)?.is_some() {
            return Err(PassError::Io {
                message: "there is already a store on this device".into(),
            });
        }
        let content = self.readers_content(&readers)?;
        let edits = vec![
            (layout::GPG_ID.to_string(), Some(content.into_bytes())),
            (ATTRIBUTES_PATH.to_string(), Some(GIT_ATTRIBUTES.as_bytes().to_vec())),
        ];
        let tree = git::edit_tree(&repo, None, &edits)?;
        let message = format!("Set GPG id to {}.", readers.join(", "));
        let commit = git::write_commit(&repo, tree, &[], &message, &self.author, &identity.inner)?;
        git::move_to(&repo, commit, "pass: a new store")?;
        Ok(())
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

    /// Who can read what is put in a folder (`""` or `/` for the top of the store): the
    /// readers of the `.gpg-id` that governs it.
    pub fn folder_access(&self, folder: String) -> Result<Access, PassError> {
        let folder = clean_folder(&folder)?;
        let inside = if folder.is_empty() {
            "entry".to_string()
        } else {
            format!("{folder}/entry")
        };
        self.access_of(&inside, None)
    }

    /// The entries this device cannot decrypt: not encrypted to its key. Needs no key itself.
    pub fn unreadable(&self) -> Result<Vec<String>, PassError> {
        let mut locked = Vec::new();
        for name in self.list()? {
            if !self.for_me(&layout::read(&self.root, &layout::file_of(&name))?) {
                locked.push(name);
            }
        }
        Ok(locked)
    }

    /// The stale entries: those whose file is not encrypted to exactly the readers of their
    /// folder. Sorted. Needs no key.
    pub fn stale(&self) -> Result<Vec<String>, PassError> {
        let mut stale = Vec::new();
        for name in self.list()? {
            let file = layout::read(&self.root, &layout::file_of(&name))?;
            if self.access_of(&name, Some(&file))?.stale {
                stale.push(name);
            }
        }
        Ok(stale)
    }

    /// Changes who should read a folder (`work/`, `/` for the root): writes its `.gpg-id`
    /// with the full fingerprints of these devices, and encrypts again the entries below that
    /// this device can read. Those it cannot read stay stale, for a device that can: naming
    /// oneself where one does not read is asking, not getting. With no device, the file is
    /// removed and the folder has the readers of the one above (not for the root).
    pub fn set_readers(&self, folder: String, readers: Vec<String>, identity: Arc<Identity>) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        let folder = clean_folder(&folder)?;
        let path = readers_path(&folder);
        let (edit, message) = if readers.is_empty() {
            if folder.is_empty() {
                return Err(PassError::InvalidConfig {
                    message: "the root of the store always names its readers".into(),
                });
            }
            (None, format!("Deinitialize {path} ({folder})."))
        } else {
            let content = self.readers_content(&readers)?;
            let place = if folder.is_empty() {
                String::new()
            } else {
                format!(" ({folder})")
            };
            (
                Some(content.into_bytes()),
                format!("Set GPG id to {}{place}.", readers.join(", ")),
            )
        };
        self.apply(vec![(path, edit)], &message, &identity)?;
        self.reencrypt_stale(&identity)?;
        Ok(())
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

    /// `pass insert`: writes an entry, `content` being the password on its first
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
            if self
                .decrypt(&name, &identity)
                .is_ok_and(|current| *current == content.as_bytes())
            {
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

    /// `pass rm -r`: removes a folder, with every entry and every readers file in it.
    pub fn remove_folder(&self, folder: String, identity: Arc<Identity>) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        let folder = layout::clean_name(folder.trim_matches('/'))?;
        let (repo, head) = self.repository()?;
        let inside = format!("{folder}/");
        let edits: Vec<git::Edit> = git::files(&repo, head)?
            .into_keys()
            .filter(|path| path.starts_with(&inside))
            .map(|path| (path, None))
            .collect();
        if edits.is_empty() {
            return Err(PassError::NotFound { name: inside });
        }
        self.apply(edits, &format!("Remove {folder} from store."), &identity)
    }

    /// `pass mv`: renames or moves an entry. Where the new place has other readers and this
    /// device can read the entry, it is encrypted again for them; where it cannot, the entry
    /// moves as it is, and is stale until a device that reads it comes by.
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
        let encrypted = layout::read(&self.root, &layout::file_of(&from))?;
        let message = format!("Rename {from} to {to}.");
        let edits = vec![(layout::file_of(&from), None), (layout::file_of(&to), Some(encrypted))];
        self.apply(edits, &message, &identity)?;
        self.reencrypt_stale(&identity)?;
        Ok(())
    }

    /// `pass mv` of a folder: every entry and every readers file in it, under the new name.
    pub fn rename_folder(&self, from: String, to: String, identity: Arc<Identity>) -> Result<(), PassError> {
        let _busy = self.busy.lock().unwrap();
        let from = layout::clean_name(from.trim_matches('/'))?;
        let to = layout::clean_name(to.trim_matches('/'))?;
        let (repo, head) = self.repository()?;
        let files = git::files(&repo, head)?;
        let (inside, target) = (format!("{from}/"), format!("{to}/"));
        if target.starts_with(&inside) || inside.starts_with(&target) {
            return Err(PassError::InvalidName {
                message: "a folder cannot be moved into itself".into(),
            });
        }
        let mut edits: Vec<git::Edit> = Vec::new();
        for (path, blob) in &files {
            if let Some(rest) = path.strip_prefix(&inside) {
                let moved = format!("{target}{rest}");
                if files.contains_key(&moved) {
                    return Err(PassError::AlreadyExists {
                        name: entry_name(&moved).unwrap_or(&moved).to_string(),
                    });
                }
                edits.push((path.clone(), None));
                edits.push((moved, Some(git::blob(&repo, *blob)?)));
            }
        }
        if edits.is_empty() {
            return Err(PassError::NotFound { name: inside });
        }
        self.apply(edits, &format!("Rename {from} to {to}."), &identity)?;
        self.reencrypt_stale(&identity)?;
        Ok(())
    }

    /// Where this device stands, from what it has: no network.
    pub fn sync_state(&self) -> Result<SyncState, PassError> {
        match self.open_repository()? {
            Some(repo) => self.state_of(&repo),
            None => Ok(SyncState {
                has_store: false,
                unpublished: false,
                peers: self
                    .peers
                    .iter()
                    .map(|peer| PeerState {
                        device: peer.name.clone(),
                        received: false,
                        to_take: 0,
                        published_at: 0,
                    })
                    .collect(),
                last_subject: String::new(),
                last_time: 0,
            }),
        }
    }

    /// One visit to the drop zone: publishes this device's tip when it is not the one last
    /// published, then looks at the file of every other device. What they published is
    /// received (checked, and kept aside) but never applied: see [`Self::pending_update`].
    /// Refuses, having done nothing, when this device's own store holds what a synced store does not.
    pub fn sync(&self, zone: DropZone, identity: Arc<Identity>) -> Result<SyncReport, PassError> {
        let _busy = self.busy.lock().unwrap();
        self.check(&identity)?;
        let remote = ssh::SshRemote::new(&zone.url, &zone.private_key, &zone.host_key)?;
        let repo = git::open_or_init(&self.root, &self.author)?;
        let head = git::head_commit(&repo)?;
        let to_publish = match head {
            Some(head) if git::reference(&repo, &self.reference_of(&self.me)) != Some(head) => {
                self.valid_tip(&repo, head)?;
                Some((head, self.sealed_bundle(&repo, head, &identity)?))
            }
            _ => None,
        };
        let mut sftp = sftp::Sftp::connect(&remote)?;
        if let Some((head, file)) = &to_publish {
            sftp.publish(&format!("{}.bundle.gpg", self.me.name), file)?;
            git::set_reference(&repo, &self.reference_of(&self.me), *head, "pass sync: published")?;
        }
        let mut peers = Vec::new();
        for peer in &self.peers {
            let fetched = match sftp.download(&format!("{}.bundle.gpg", peer.name), MAX_PUBLISHED_FILE) {
                Ok(file) => self.receive(&repo, file, peer, &identity),
                Err(e) => Err(e),
            };
            // What makes a file not one to take is told, not thrown: the rest of the sync happens.
            let outcome = match fetched {
                Ok(outcome) => outcome,
                Err(
                    e @ (PassError::UntrustedFile { .. }
                    | PassError::BadBundle { .. }
                    | PassError::TooLarge
                    | PassError::RefusedTip { .. }
                    | PassError::UnknownRecipient { .. }),
                ) => FetchOutcome::Refused { reason: e.to_string() },
                Err(e) => return Err(e),
            };
            let repo = self.open_repository()?.ok_or(PassError::NoStore)?;
            peers.push(PeerSync {
                outcome,
                state: self.peer_state(&repo, peer)?,
            });
        }
        Ok(SyncReport {
            published: to_publish.is_some(),
            peers,
        })
    }

    /// What `device` has published that is not in the store yet, described. `None` when
    /// there is nothing to take. Needs the key: whether an entry changed is asked of its content.
    pub fn pending_update(&self, device: String, identity: Arc<Identity>) -> Result<Option<PendingUpdate>, PassError> {
        let from = self.peer(&device)?;
        let Some(repo) = self.open_repository()? else {
            return Ok(None);
        };
        match self.plan(&repo, from)? {
            Some(plan) => self.describe(&repo, from, &plan, &identity).map(Some),
            None => Ok(None),
        }
    }

    /// An entry of the update of `device`, decrypted: where the two lines last agreed, on
    /// this device, and in their tip. `name` is as a review line or a conflict gives it;
    /// each side is followed through its moves.
    pub fn update_versions(
        &self,
        device: String,
        name: String,
        identity: Arc<Identity>,
    ) -> Result<EntryVersions, PassError> {
        let from = self.peer(&device)?;
        let name = layout::clean_name(&name)?;
        let repo = self.open_repository()?.ok_or(PassError::NoStore)?;
        let plan = self.plan(&repo, from)?.ok_or(PassError::StaleUpdate)?;
        let mut sameness = Sameness::new(&repo, &identity.inner);
        let mut same = |a: ObjectId, b: ObjectId| sameness.same(a, b);
        let items = merge::items(&plan.base.entries, &plan.mine.entries, &plan.theirs.entries, &mut same);
        let named = |place: &Option<(String, ObjectId)>| place.as_ref().is_some_and(|(at, _)| *at == name);
        let item = items
            .iter()
            .find(|item| named(&item.mine) || named(&item.theirs))
            .or_else(|| items.iter().find(|item| named(&item.base)));
        let blob =
            |place: Option<&Option<(String, ObjectId)>>| place.and_then(|place| place.as_ref().map(|(_, blob)| *blob));
        Ok(EntryVersions {
            base: self.version(&repo, blob(item.map(|item| &item.base)), &identity),
            mine: self.version(&repo, blob(item.map(|item| &item.mine)), &identity),
            theirs: self.version(&repo, blob(item.map(|item| &item.theirs)), &identity),
        })
    }

    /// Takes the update of `device`. With `commit` ([`PendingUpdate::commit`]) it must still
    /// be the one that was shown. No network: publish (sync) afterwards.
    ///
    /// An empty store adopts theirs; with nothing of its own since the two agreed, this device
    /// moves forward; otherwise it merges, in a signed commit, and what both sides did
    /// differently needs a [`Choice`] each (`ChoiceRequired` names those missing, and nothing
    /// is taken). Then every stale entry this device can read is encrypted again.
    pub fn take_update(
        &self,
        device: String,
        commit: Option<String>,
        choices: Vec<Choice>,
        identity: Arc<Identity>,
    ) -> Result<Taken, PassError> {
        let _busy = self.busy.lock().unwrap();
        self.check(&identity)?;
        let from = self.peer(&device)?;
        let repo = self.open_repository()?.ok_or(PassError::NoStore)?;
        let Some(plan) = self.plan(&repo, from)? else {
            return Ok(Taken {
                kind: TakeKind::Forward,
                changed: Vec::new(),
                reencrypted: Vec::new(),
            });
        };
        if commit.is_some_and(|commit| commit != plan.theirs_commit.to_string()) {
            return Err(PassError::StaleUpdate);
        }
        let target = match (plan.kind, plan.head) {
            (TakeKind::Merge, Some(head)) => {
                let choices: HashMap<String, Side> =
                    choices.into_iter().map(|choice| (choice.name, choice.keep)).collect();
                let mut sameness = Sameness::new(&repo, &identity.inner);
                let mut same = |a: ObjectId, b: ObjectId| sameness.same(a, b);
                let entries = merge::merge(
                    &plan.base.entries,
                    &plan.mine.entries,
                    &plan.theirs.entries,
                    &choices,
                    &mut same,
                );
                let (readers, reader_conflicts) = Self::merge_readers(&plan, &choices);
                let mut undecided = entries.undecided;
                undecided.extend(
                    reader_conflicts
                        .into_iter()
                        .filter(|folder| !choices.contains_key(folder)),
                );
                if !undecided.is_empty() {
                    undecided.sort();
                    return Err(PassError::ChoiceRequired { names: undecided });
                }
                let merged = Tip {
                    entries: entries.entries,
                    readers,
                    attributes: plan.mine.attributes,
                };
                let tree = git::tree_of_files(&repo, &merged.files())?;
                let message = format!("Merge the changes of {} into those of {}.", from.name, self.me.name);
                let parents = [head, plan.theirs_commit];
                git::write_commit(&repo, tree, &parents, &message, &self.author, &identity.inner)?
            }
            _ => plan.theirs_commit,
        };
        let changed = git::move_to(&repo, target, &format!("pass sync: took the update of {}", from.name))?;
        Ok(Taken {
            kind: plan.kind,
            changed: changed
                .iter()
                .filter_map(|path| entry_name(path))
                .map(str::to_string)
                .collect(),
            reencrypted: self.reencrypt_stale(&identity)?,
        })
    }

    /// Forgets this device's own changes: its store becomes the tip of `device`, as last
    /// received. Returns how many commits of its own that leaves behind. No network: sync
    /// afterwards, to publish where this device now stands.
    pub fn drop_to(&self, device: String) -> Result<u32, PassError> {
        let _busy = self.busy.lock().unwrap();
        let from = self.peer(&device)?;
        let (repo, head) = self.repository()?;
        let tip = git::reference(&repo, &self.reference_of(from)).ok_or_else(|| PassError::Git {
            message: format!("nothing of {} has been received", from.name),
        })?;
        self.valid_tip(&repo, tip)?;
        let dropped = git::commits_since(&repo, Some(tip), head)?.len() as u32;
        if tip != head {
            git::move_to(&repo, tip, &format!("pass sync: went to the tip of {}", from.name))?;
        }
        Ok(dropped)
    }
}

impl PassStore {
    /// A readers file for these devices: their full fingerprints, one per line.
    fn readers_content(&self, readers: &[String]) -> Result<String, PassError> {
        let mut content = String::new();
        for name in readers {
            content.push_str(&self.device(name)?.certificate.fingerprint());
            content.push('\n');
        }
        if content.is_empty() {
            return Err(PassError::InvalidConfig {
                message: "a store needs at least one reader".into(),
            });
        }
        Ok(content)
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
    fn what_a_synced_store_holds() {
        assert_eq!(entry_name("bank.gpg"), Some("bank"));
        assert_eq!(entry_name("Email/work.gpg"), Some("Email/work"));
        assert_eq!(readers_folder(".gpg-id"), Some(""));
        assert_eq!(readers_folder("work/team/.gpg-id"), Some("work/team"));
        for other in [
            ".gpg-id.sig",
            ".gitattributes",
            ".extensions/hook.bash",
            ".extensions/x.gpg",
            ".extensions/.gpg-id",
            "notes.txt",
            ".gpg",
            "a/.hidden.gpg",
            "work/.gpg-id.sig",
        ] {
            assert_eq!((entry_name(other), readers_folder(other)), (None, None), "{other}");
        }
    }

    #[test]
    fn folders_and_who_governs_what() {
        assert_eq!(clean_folder("/").unwrap(), "");
        assert_eq!(clean_folder("").unwrap(), "");
        assert_eq!(clean_folder("work/").unwrap(), "work");
        assert_eq!(clean_folder("work/team").unwrap(), "work/team");
        assert!(clean_folder("../x/").is_err());
        assert_eq!(
            (folder_label(""), folder_label("work")),
            ("/".to_string(), "work/".to_string())
        );
        assert_eq!(
            (readers_path(""), readers_path("work")),
            (".gpg-id".to_string(), "work/.gpg-id".to_string())
        );

        let readers: BTreeMap<String, u8> = [
            ("".to_string(), 0),
            ("work".to_string(), 1),
            ("work/team".to_string(), 2),
        ]
        .into();
        let of = |name: &str| governing(&readers, name).map(|(folder, _)| folder.to_string());
        assert_eq!(of("bank"), Some("".into()));
        assert_eq!(of("home/wifi"), Some("".into()));
        assert_eq!(of("work/vpn"), Some("work".into()));
        assert_eq!(of("work/team/deep/er"), Some("work/team".into()));
        // A folder is not its own content: `work` the entry is at the root.
        assert_eq!(of("work"), Some("".into()));
        assert_eq!(governing(&BTreeMap::<String, u8>::new(), "bank"), None);
    }

    #[test]
    fn review_lines_as_the_protocol_spells_them() {
        let text = |line| review_line_text(line);
        assert_eq!(text(ReviewLine::Added { name: "bank".into() }), "added bank");
        assert_eq!(
            text(ReviewLine::Changed {
                name: "Email/work".into()
            }),
            "changed Email/work"
        );
        assert_eq!(
            text(ReviewLine::Moved {
                from: "bank".into(),
                to: "money/bank".into()
            }),
            "moved bank -> money/bank"
        );
        assert_eq!(text(ReviewLine::Removed { name: "bank".into() }), "removed bank");
        assert_eq!(
            text(ReviewLine::Readers {
                folder: "work/".into(),
                devices: vec!["pc".into(), "phone".into()],
                inherited: false
            }),
            "readers work/: pc phone"
        );
        assert_eq!(
            text(ReviewLine::Readers {
                folder: "/".into(),
                devices: vec![],
                inherited: true
            }),
            "readers /: inherited"
        );
        assert_eq!(
            text(ReviewLine::Stale {
                name: "work/vpn".into()
            }),
            "stale work/vpn"
        );
        assert_eq!(
            text(ReviewLine::Conflict { name: "shared/".into() }),
            "conflict shared/"
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            error_message(PassError::NotFound { name: "bank".into() }),
            "bank is not in the password store"
        );
        assert_eq!(
            error_message(PassError::ChoiceRequired {
                names: vec!["bank".into(), "work/".into()]
            }),
            "a choice is needed (mine or theirs) for: bank, work/"
        );
    }
}
