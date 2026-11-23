//! Client for the Qonto business-banking API, exposed to Swift through UniFFI.
//!
//! A port of the network side of `bank.py` (<https://github.com/c4ffein/bank>):
//! the same endpoints, request shapes, authorization header, pagination and
//! checks. What it knows of the API is what that CLI knows, not Qonto's
//! documentation.
//!
//! Every call blocks until the server answers: call them off the main thread.
//! The client holds no state but its configuration, and is safe to share.
//!
//! Two hosts, two lists of pins, as in the CLI: the API, and the storage
//! (S3) that the API hands pre-signed attachment URLs for. Both are verified
//! against the public roots too ([`Trust::PinnedWithCa`]). The credentials
//! only ever go to the API.
//!
//! Money is integers of cents, as the API gives them: nothing here is a float.

use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pinned_http::{copy_with_progress, PinnedAgent, ProgressReader, Trust};
use ring::digest;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

uniffi::setup_scaffolding!();

/// Where the API is: the one `bank.py` talks to. Not a setting.
pub const API_BASE_URL: &str = "https://thirdparty.qonto.com";
/// Where attachments are downloaded from, as `bank help config` has it. Only used to read
/// its certificate before pinning it: a download goes where the API says, and the pin decides.
pub const DEFAULT_ATTACHMENTS_HOST: &str = "qonto.s3.eu-central-1.amazonaws.com";

/// For the calls that move no file. `bank.py`: REQUEST_TIMEOUT.
const API_TIMEOUT: Duration = Duration::from_secs(30);
/// Transactions per page: the most Qonto accepts. `bank.py`: PER_PAGE.
const PER_PAGE: u32 = 100;
/// Bytes read from an error response, to say what the server answered. `bank.py`: HTTP_ERROR_BODY_MAX.
const HTTP_ERROR_BODY_MAX: u64 = 4096;
/// Ours: a page of 100 transactions is far below, and an answer is read into memory.
const JSON_BODY_MAX: u64 = 32 * 1024 * 1024;
/// Ours: a receipt is read into memory to be sent (as `bank.py` does), so it cannot be anything.
const UPLOAD_MAX: u64 = 100 * 1024 * 1024;
/// What Qonto accepts as an attachment, recognised by the first bytes of the file:
/// (magic number, extension, mimetype). `bank.py`: FILE_TYPES.
const FILE_TYPES: [(&[u8], &str, &str); 3] = [
    (b"%PDF-", "pdf", "application/pdf"),
    (b"\xff\xd8\xff", "jpg", "image/jpeg"),
    (b"\x89PNG\r\n\x1a\n", "png", "image/png"),
];
/// The extension of a downloaded attachment that is none of `FILE_TYPES`.
const UNKNOWN_EXTENSION: &str = "bin";

#[derive(Debug, Clone, uniffi::Record)]
pub struct BankConfig {
    /// The login of the API key: the organization slug.
    pub organization_slug: String,
    pub secret_key: String,
    /// SHA-256 of the certificates to accept from the API. Never empty: there is no unpinned https.
    pub api_pins: Vec<String>,
    /// The same for the attachments storage. Empty: nothing can be downloaded.
    pub attachments_pins: Vec<String>,
}

/// Which of the two pinned hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PinnedHost {
    Api,
    Attachments,
}

/// A bank account of the organization. `bank.py` only reads `id` and prints the rest as it comes:
/// every other field is optional here, and absent rather than an error when it is not what is expected.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct BankAccount {
    pub id: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub iban: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub currency: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub balance_cents: Option<i64>,
    /// The balance, less what is authorized but not settled yet.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub authorized_balance_cents: Option<i64>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub status: Option<String>,
}

/// A transaction, with the fields `bank.py` reads. The first block is always there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct Transaction {
    /// The readable id (`<slug>-1-transaction-123`): what the CLI shows the end of.
    pub transaction_id: String,
    /// The UUID: what the attachment endpoints take.
    pub id: String,
    #[serde(deserialize_with = "string_or_empty")]
    pub label: String,
    /// In the currency of the account, always positive: `side` says which way.
    pub amount_cents: i64,
    /// In `local_currency`, for a purchase abroad.
    pub local_amount_cents: i64,
    pub currency: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub local_currency: Option<String>,
    /// "debit" or "credit".
    pub side: String,
    /// ISO 8601, UTC.
    pub emitted_at: String,

    #[serde(default, deserialize_with = "lenient_string")]
    pub settled_at: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub status: Option<String>,
    /// "card", "transfer", "income"...
    #[serde(default, deserialize_with = "lenient_string")]
    pub operation_type: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub clean_counterparty_name: Option<String>,
    /// Free text of a transfer: where an invoice number usually is.
    #[serde(default, deserialize_with = "lenient_string")]
    pub reference: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub note: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub card_last_digits: Option<String>,
    /// Up to 5 per transaction.
    #[serde(default, deserialize_with = "list_or_empty")]
    pub attachment_ids: Vec<String>,
}

/// Which transactions to list. `bank.py`: `no-invoice` / `only-invoice`, and the `date` parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TransactionFilter {
    /// `Some(false)`: only the ones with no attachment. `Some(true)`: only the ones with some.
    pub with_attachments: Option<bool>,
    /// `2024-01-01T00:00:00Z`: this exact format.
    pub emitted_at_from: Option<String>,
    /// `2024-12-31T23:59:59Z`
    pub emitted_at_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TransactionsPage {
    pub transactions: Vec<Transaction>,
    /// The page to ask for next, `None` after the last one.
    pub next_page: Option<u32>,
    /// How many transactions match in all, when the API says.
    pub total_count: Option<u64>,
}

/// An attachment as the API lists it. `bank.py` reads `id` and `url`; the rest is shown when it is there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct Attachment {
    pub id: String,
    /// Pre-signed, short-lived: list the attachments again rather than keep it.
    #[serde(default, deserialize_with = "lenient_string")]
    pub url: Option<String>,
    /// As the API says. Not trusted: never used as a path.
    #[serde(default, deserialize_with = "lenient_string")]
    pub file_name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub file_content_type: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub created_at: Option<String>,
}

/// An attachment on this device.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalAttachment {
    /// `<directory>/<attachment id>.<pdf|jpg|png|bin>`: the extension is from the first bytes.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// `None` when it is not a PDF, a JPEG or a PNG.
    pub mimetype: Option<String>,
}

/// What a file is, from its first bytes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FileType {
    pub extension: String,
    pub mimetype: String,
}

/// What [`BankClient::attach_file`] did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AttachOutcome {
    Uploaded {
        sha256: String,
        idempotency_key: String,
    },
    /// The transaction already has this very file: nothing was sent.
    AlreadyThere {
        sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CertificateFingerprint {
    /// SHA-256 of the leaf certificate, lowercase hex: what to pin.
    pub sha256: String,
    /// The chain and the name verify against the public roots. A pin is only usable when they do.
    pub chain_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BankError {
    #[error("{message}")]
    InvalidConfig { message: String },
    #[error("{message}")]
    InvalidInput { message: String },
    #[error("Connection error: {message}")]
    Network { message: String },
    #[error("the certificate of {host} is none of the pinned ones (served {served})")]
    PinMismatch {
        which: PinnedHost,
        host: String,
        served: String,
    },
    /// The server answered, with an error status. `message` is `HTTP 401 Unauthorized: ...`, as `bank.py` says it.
    #[error("{message}")]
    Http { status: u16, message: String },
    #[error("Unexpected answer: {message}")]
    UnexpectedAnswer { message: String },
    #[error("Unsupported file type: only PDF, JPEG and PNG are accepted (checked on the first bytes)")]
    UnsupportedFile,
    #[error("No certificate is pinned for the attachments storage: attachments cannot be downloaded")]
    NoAttachmentsPin,
    #[error("Could not check what this transaction already has, nothing sent: {message}")]
    NotChecked { message: String },
    #[error("{message}")]
    Io { message: String },
}

impl From<pinned_http::Error> for BankError {
    fn from(e: pinned_http::Error) -> Self {
        match e {
            pinned_http::Error::Connect(m) | pinned_http::Error::Tls(m) => Self::Network { message: m },
            _ => Self::InvalidConfig { message: e.to_string() },
        }
    }
}

fn io_error(context: &str, e: std::io::Error) -> BankError {
    BankError::Io {
        message: format!("{context}: {e}"),
    }
}

fn invalid_config(message: &str) -> BankError {
    BankError::InvalidConfig {
        message: message.to_string(),
    }
}

fn invalid_input(message: impl Into<String>) -> BankError {
    BankError::InvalidInput {
        message: message.into(),
    }
}

fn unexpected(message: impl Into<String>) -> BankError {
    BankError::UnexpectedAnswer {
        message: message.into(),
    }
}

// What the API may send where a string is expected: anything else than a string or a number is "not there".
fn lenient_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// An integer or nothing: a float is not read as an amount.
fn lenient_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    Ok(Value::deserialize(deserializer)?.as_i64())
}

/// The key must be there (as for `bank.py`), but may be null.
fn string_or_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(lenient_string(deserializer)?.unwrap_or_default())
}

/// `bank.py`: `list(data.get("attachment_ids") or [])`
fn list_or_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct OrganizationResponse {
    organization: OrganizationBody,
}

#[derive(Deserialize)]
struct OrganizationBody {
    bank_accounts: Vec<BankAccount>,
}

#[derive(Deserialize)]
struct TransactionsResponse {
    transactions: Vec<Transaction>,
    meta: Value,
}

#[derive(Deserialize)]
struct AttachmentsResponse {
    #[serde(default)]
    attachments: Vec<Attachment>,
}

/// Told how far a transfer is. Called from the thread doing the transfer, often.
#[uniffi::export(with_foreign)]
pub trait ProgressListener: Send + Sync {
    /// `total` is 0 when the size is not known.
    fn on_progress(&self, done: u64, total: u64);
}

fn report(listener: &Option<Arc<dyn ProgressListener>>) -> impl FnMut(u64, u64) + '_ {
    move |done, total| {
        if let Some(listener) = listener {
            listener.on_progress(done, total);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

/// What can be shown of a string that is not ours (what a server answers): `bank.py`'s `printable`.
/// A U+FFFD replaces what is not: control characters, and what reorders or hides text.
fn printable(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            let hidden =
                matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
            if c.is_control() || hidden {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// `bank.py`: `validate_uuid`. The ids that go in a path: 8-4-4-4-12 hex digits, nothing else.
fn validate_uuid(uuid: &str) -> Result<(), BankError> {
    if uuid.len() != 36 {
        return Err(invalid_input(format!(
            "Invalid UUID format: expected 36 characters, got {}",
            uuid.chars().count()
        )));
    }
    if !uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(invalid_input("Invalid UUID format: contains invalid characters"));
    }
    let lengths: Vec<usize> = uuid.split('-').map(str::len).collect();
    if lengths != [8, 4, 4, 4, 12] {
        return Err(invalid_input(format!(
            "Invalid UUID format: expected 8-4-4-4-12 format, got {uuid}"
        )));
    }
    Ok(())
}

fn format_uuid(mut bytes: [u8; 16], version: u8) -> String {
    bytes[6] = (bytes[6] & 0x0f) | (version << 4);
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = hex(&bytes);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Python's `uuid5(NAMESPACE_URL, name)`.
fn uuid5_url(name: &str) -> String {
    const NAMESPACE_URL: [u8; 16] = [
        0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
    ];
    let mut context = digest::Context::new(&digest::SHA1_FOR_LEGACY_USE_ONLY);
    context.update(&NAMESPACE_URL);
    context.update(name.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&context.finish().as_ref()[..16]);
    format_uuid(bytes, 5)
}

fn uuid4() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system has no randomness");
    format_uuid(bytes, 4)
}

/// Same transaction and same file: same key, here and in `bank.py`. Qonto answers a key that it has seen in
/// the last 30 minutes without doing anything again, so that sending again after a lost answer (from the
/// phone or from the CLI) cannot attach the file twice.
fn idempotency_key(transaction_id: &str, sha256: &str) -> String {
    uuid5_url(&format!("bank:justify:{transaction_id}:{sha256}"))
}

fn file_type_of(head: &[u8]) -> Option<(&'static str, &'static str)> {
    FILE_TYPES
        .iter()
        .find(|(magic, _, _)| head.starts_with(magic))
        .map(|(_, extension, mimetype)| (*extension, *mimetype))
}

/// `2024-01-01T00:00:00Z`, and nothing else: it goes in a query string as it is.
fn validate_timestamp(value: &str) -> Result<(), BankError> {
    let pattern = b"dddd-dd-ddTdd:dd:ddZ";
    let ok = value.len() == pattern.len()
        && value
            .bytes()
            .zip(pattern)
            .all(|(b, p)| if *p == b'd' { b.is_ascii_digit() } else { b == *p });
    if ok {
        Ok(())
    } else {
        Err(invalid_input(format!(
            "Invalid date: expected 2024-01-31T00:00:00Z, got {value:?}"
        )))
    }
}

/// The query of `/v2/transactions`, built as `bank.py` builds it: same parameters, same order.
fn transactions_query(bank_account_id: &str, filter: &TransactionFilter, page: u32) -> Result<String, BankError> {
    validate_uuid(bank_account_id)?;
    let mut params = Vec::new();
    if let Some(with_attachments) = filter.with_attachments {
        params.push(format!("with_attachments={with_attachments}"));
    }
    if let Some(from) = &filter.emitted_at_from {
        validate_timestamp(from)?;
        params.push(format!("emitted_at_from={from}"));
    }
    if let Some(to) = &filter.emitted_at_to {
        validate_timestamp(to)?;
        params.push(format!("emitted_at_to={to}"));
    }
    if let (Some(from), Some(to)) = (&filter.emitted_at_from, &filter.emitted_at_to) {
        if from > to {
            return Err(invalid_input(
                "Invalid date range: 'from' date must be before 'to' date",
            ));
        }
    }
    if page == 0 {
        return Err(invalid_input("Pages start at 1"));
    }
    params.push(format!("bank_account_id={bank_account_id}"));
    params.push(format!("per_page={PER_PAGE}"));
    params.push(format!("page={page}"));
    Ok(format!("/v2/transactions?{}", params.join("&")))
}

/// `bank.py`: `describe_http_error`. What the server answered, on one line: the status, then what its body
/// says went wrong.
fn describe_http_error(status: u16, reason: &str, raw: &[u8]) -> String {
    // Qonto: {"errors": [{"code": ..., "detail": ...}]}
    fn qonto_errors(raw: &[u8]) -> Option<String> {
        let body: Value = serde_json::from_slice(raw).ok()?;
        let mut errors = Vec::new();
        for error in body.get("errors")?.as_array()? {
            let error = error.as_object()?;
            let parts: Vec<String> = ["code", "detail"]
                .iter()
                .filter_map(|key| match error.get(*key)? {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect();
            errors.push(parts.join(": "));
        }
        Some(errors.join("; "))
    }
    // Anything else (S3 answers in XML) is shown as it is
    let detail = qonto_errors(raw).unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned());
    let detail: String = printable(&detail.split_whitespace().collect::<Vec<_>>().join(" "))
        .chars()
        .take(300)
        .collect();
    let mut message = format!("HTTP {status}");
    if !reason.is_empty() {
        message.push(' ');
        message.push_str(&printable(reason));
    }
    if !detail.is_empty() {
        message.push_str(": ");
        message.push_str(&detail);
    }
    message
}

/// The name of an attachment on disk. Its id is a UUID, but it is the API that says so.
fn cache_name(attachment_id: &str) -> String {
    let safe = !attachment_id.is_empty()
        && attachment_id.len() <= 64
        && attachment_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if safe {
        attachment_id.to_string()
    } else {
        pinned_http::sha256_hex(attachment_id.as_bytes())
    }
}

/// A new file that only this user can read (financial documents), as `bank.py` writes them.
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Hashes what goes through, and keeps its first bytes: what says the type of the file.
struct HashingWriter<W: Write> {
    inner: W,
    context: digest::Context,
    head: Vec<u8>,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            context: digest::Context::new(&digest::SHA256),
            head: Vec::with_capacity(16),
        }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.context.update(&buf[..n]);
        let wanted = 16usize.saturating_sub(self.head.len()).min(n);
        self.head.extend_from_slice(&buf[..wanted]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn local_attachment(path: &Path) -> std::io::Result<LocalAttachment> {
    let mut hashing = HashingWriter::new(std::io::sink());
    let size = std::io::copy(&mut File::open(path)?, &mut hashing)?;
    Ok(LocalAttachment {
        path: path.to_string_lossy().into_owned(),
        size,
        sha256: hex(hashing.context.finish().as_ref()),
        mimetype: file_type_of(&hashing.head).map(|(_, mimetype)| mimetype.to_string()),
    })
}

#[derive(uniffi::Object)]
pub struct BankClient {
    base_url: String,
    /// `<organization slug>:<secret key>`: the whole header, with no scheme, as Qonto wants it.
    authorization: String,
    api: PinnedAgent,
    /// `None`: no pin for the attachments storage.
    attachments: Option<PinnedAgent>,
    /// What a download URL must start with: `https://`, except against a test server over plain http.
    download_scheme: &'static str,
}

/// What is sent in a header: visible ASCII, so that nothing can add a line to the request.
fn is_header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_graphic())
}

/// `https://host[:port]` or `http://host[:port]` of this very machine, with no path.
fn is_loopback_url(url: &str) -> bool {
    let Some((scheme, authority)) = url.split_once("://") else {
        return false;
    };
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']').map(|(host, _)| host).unwrap_or(""),
        None => authority.rsplit_once(':').map(|(host, _)| host).unwrap_or(authority),
    };
    matches!(scheme, "http" | "https")
        && !authority.contains(['/', '?', '#', '@'])
        && !url.contains(char::is_whitespace)
        && matches!(host, "127.0.0.1" | "localhost" | "::1")
}

impl BankClient {
    fn build(config: BankConfig, base_url: &str, extra_ca_pem: Option<String>) -> Result<Arc<Self>, BankError> {
        let slug = config.organization_slug.trim();
        let secret = config.secret_key.trim();
        if !is_header_safe(slug) || slug.contains(':') {
            return Err(invalid_config(
                "the organization slug is letters, digits and hyphens, as Qonto shows it",
            ));
        }
        if !is_header_safe(secret) {
            return Err(invalid_config(
                "the secret key is the one Qonto shows, with no space in it",
            ));
        }
        let https = base_url.starts_with("https://");
        let trust = |pins: &[String]| Trust::PinnedWithCa {
            pins: pins.to_vec(),
            extra_ca_pem: extra_ca_pem.clone(),
        };
        let (api, attachments) = if https {
            if config.api_pins.is_empty() {
                return Err(invalid_config("no certificate is pinned for the Qonto API"));
            }
            let attachments = match config.attachments_pins.is_empty() {
                true => None,
                false => Some(pinned_http::agent(base_url, Some(&trust(&config.attachments_pins)))?),
            };
            (
                pinned_http::agent(base_url, Some(&trust(&config.api_pins)))?,
                attachments,
            )
        } else {
            // Only reachable from `with_base_url`: a test server on this machine.
            if !config.api_pins.is_empty() || !config.attachments_pins.is_empty() {
                return Err(invalid_config(
                    "pins are set but the URL is http://: they would be ignored",
                ));
            }
            (
                pinned_http::agent(base_url, None)?,
                Some(pinned_http::agent(base_url, None)?),
            )
        };
        Ok(Arc::new(Self {
            base_url: base_url.to_string(),
            authorization: format!("{slug}:{secret}"),
            api,
            attachments,
            download_scheme: if https { "https://" } else { "http://" },
        }))
    }

    /// A GET of the API, as this organization.
    fn get(&self, path: &str) -> ureq::Request {
        self.api
            .agent
            .get(&format!("{}{}", self.base_url, path))
            .timeout(API_TIMEOUT)
            .set("Authorization", &self.authorization)
            // `bank.py`: "Otherwise would send default User-Agent, that does fail"
            .set("User-Agent", "")
    }

    fn api_host(&self) -> String {
        let authority = self.base_url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
        authority.to_string()
    }

    /// Turns anything but a 2xx into an error.
    fn check(
        agent: &PinnedAgent,
        which: PinnedHost,
        host: &str,
        result: Result<ureq::Response, ureq::Error>,
    ) -> Result<ureq::Response, BankError> {
        let response = match result {
            Ok(response) if (200..300).contains(&response.status()) => return Ok(response),
            // Redirects are not followed, so a 3xx comes back as a response.
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                if let Some(served) = agent.pin_mismatch() {
                    return Err(BankError::PinMismatch {
                        which,
                        host: host.to_string(),
                        served,
                    });
                }
                return Err(BankError::Network {
                    message: transport.to_string(),
                });
            }
        };
        let (status, reason) = (response.status(), response.status_text().to_string());
        let mut body = Vec::new();
        let _ = response.into_reader().take(HTTP_ERROR_BODY_MAX).read_to_end(&mut body);
        Err(BankError::Http {
            status,
            message: describe_http_error(status, &reason, &body),
        })
    }

    fn api_json<T: for<'de> Deserialize<'de>>(
        &self,
        result: Result<ureq::Response, ureq::Error>,
    ) -> Result<T, BankError> {
        let response = Self::check(&self.api, PinnedHost::Api, &self.api_host(), result)?;
        serde_json::from_reader(response.into_reader().take(JSON_BODY_MAX)).map_err(|e| unexpected(e.to_string()))
    }

    fn attachments_path(transaction_id: &str) -> Result<String, BankError> {
        validate_uuid(transaction_id)?;
        Ok(format!("/v2/transactions/{transaction_id}/attachments"))
    }

    /// SHA-256 of each of the attachments that a transaction has on Qonto right now.
    /// `bank.py`: `get_remote_hashes`. An attachment is only downloaded the first time that its id is seen.
    fn remote_hashes(&self, transaction_id: &str, directory: &str) -> Result<Vec<String>, BankError> {
        self.attachments(transaction_id.to_string())?
            .into_iter()
            .map(|attachment| {
                Ok(self
                    .download_attachment(attachment, directory.to_string(), None)?
                    .sha256)
            })
            .collect()
    }
}

#[uniffi::export]
impl BankClient {
    /// The client of the real API, at [`API_BASE_URL`]. Validates the configuration; connects to nothing.
    #[uniffi::constructor]
    pub fn new(config: BankConfig) -> Result<Arc<Self>, BankError> {
        Self::build(config, API_BASE_URL, None)
    }

    /// For tests only: a client of a mock server on this machine. Anything but a loopback address is
    /// refused, so that this cannot send the credentials anywhere else.
    /// Over http, no pin is given, and attachment URLs are http too.
    /// Over https, `extra_ca_pem` is the CA of the test server, on top of the public roots.
    #[uniffi::constructor]
    pub fn with_base_url(
        config: BankConfig,
        base_url: String,
        extra_ca_pem: Option<String>,
    ) -> Result<Arc<Self>, BankError> {
        if !is_loopback_url(&base_url) {
            return Err(invalid_config(
                "a base URL is for tests: http(s)://127.0.0.1, localhost or [::1], with a port and no path",
            ));
        }
        Self::build(config, &base_url, extra_ca_pem)
    }

    /// The bank accounts of the organization. `bank.py`: `bank` (`/v2/organization`).
    pub fn accounts(&self) -> Result<Vec<BankAccount>, BankError> {
        let response: OrganizationResponse = self.api_json(self.get("/v2/organization").call())?;
        Ok(response.organization.bank_accounts)
    }

    /// One page of the transactions of an account, 100 at most. Pages start at 1.
    pub fn transactions_page(
        &self,
        bank_account_id: String,
        filter: TransactionFilter,
        page: u32,
    ) -> Result<TransactionsPage, BankError> {
        let path = transactions_query(&bank_account_id, &filter, page)?;
        let response: TransactionsResponse = self.api_json(self.get(&path).call())?;
        // `bank.py`: a next page that is not a later page would loop forever, or skip some.
        let next_page = match response.meta.get("next_page") {
            None | Some(Value::Null) => None,
            Some(next) => match next.as_u64().and_then(|n| u32::try_from(n).ok()) {
                Some(n) if n > page => Some(n),
                _ => return Err(unexpected(format!("Unexpected next page after page {page}: {next}"))),
            },
        };
        Ok(TransactionsPage {
            transactions: response.transactions,
            next_page,
            total_count: response.meta.get("total_count").and_then(Value::as_u64),
        })
    }

    /// Every transaction of an account, page after page until the last one. `bank.py`: `bank transactions`.
    pub fn transactions(
        &self,
        bank_account_id: String,
        filter: TransactionFilter,
    ) -> Result<Vec<Transaction>, BankError> {
        let (mut transactions, mut page) = (Vec::new(), Some(1));
        while let Some(current) = page {
            let fetched = self.transactions_page(bank_account_id.clone(), filter.clone(), current)?;
            transactions.extend(fetched.transactions);
            page = fetched.next_page;
        }
        Ok(transactions)
    }

    /// What the API lists as the attachments of a transaction, by its id (the UUID).
    pub fn attachments(&self, transaction_id: String) -> Result<Vec<Attachment>, BankError> {
        let path = Self::attachments_path(&transaction_id)?;
        let response: AttachmentsResponse = self.api_json(self.get(&path).call())?;
        Ok(response.attachments)
    }

    /// Downloads an attachment into `directory`, streamed to disk, from its pre-signed URL: nothing but
    /// https, to a host that presents a pinned certificate, and without the credentials.
    /// The file appears there complete or not at all. One that is already there is not fetched again:
    /// an attachment does not change under the same id.
    pub fn download_attachment(
        &self,
        attachment: Attachment,
        directory: String,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<LocalAttachment, BankError> {
        let directory = PathBuf::from(directory);
        let name = cache_name(&attachment.id);
        let extensions = FILE_TYPES.iter().map(|(_, extension, _)| *extension);
        for extension in extensions.chain([UNKNOWN_EXTENSION]) {
            let cached = directory.join(format!("{name}.{extension}"));
            if cached.is_file() {
                return local_attachment(&cached).map_err(|e| io_error("cannot read the downloaded attachment", e));
            }
        }
        let url = match attachment.url {
            Some(url) if url.starts_with(self.download_scheme) => url,
            _ => return Err(unexpected("No download URL")),
        };
        let agent = self.attachments.as_ref().ok_or(BankError::NoAttachmentsPin)?;
        let host = url[self.download_scheme.len()..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        // No overall timeout: the size is not known. Each read has one (pinned_http::IO_TIMEOUT).
        let result = agent.agent.get(&url).set("User-Agent", "").call();
        let response = Self::check(agent, PinnedHost::Attachments, host, result)?;
        let total = response
            .header("Content-Length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        std::fs::create_dir_all(&directory).map_err(|e| io_error("cannot create the downloads directory", e))?;
        // Two downloads of the same attachment at once must not write the same file.
        let part_path = directory.join(format!(".{name}.{}.part", pinned_http::random_hex(8)));
        let downloaded = (|| {
            let mut part = HashingWriter::new(create_private(&part_path)?);
            let size = copy_with_progress(&mut response.into_reader(), &mut part, total, report(&progress))?;
            if total != 0 && size != total {
                return Err(std::io::Error::other(format!("got {size} bytes out of {total}")));
            }
            part.inner.sync_all()?;
            let file_type = file_type_of(&part.head);
            let path = directory.join(format!("{name}.{}", file_type.map_or(UNKNOWN_EXTENSION, |t| t.0)));
            std::fs::rename(&part_path, &path)?;
            Ok(LocalAttachment {
                path: path.to_string_lossy().into_owned(),
                size,
                sha256: hex(part.context.finish().as_ref()),
                mimetype: file_type.map(|t| t.1.to_string()),
            })
        })();
        downloaded.map_err(|e| {
            let _ = std::fs::remove_file(&part_path);
            io_error("download failed", e)
        })
    }

    /// Sends a local file (PDF, JPEG or PNG, by its first bytes: its name is not trusted) to a transaction
    /// as an attachment. `bank.py`: `bank justify`.
    ///
    /// Unless `force`, Qonto is first asked what the transaction already has, and a file that is already
    /// there is not sent again: what an answer that was lost, or another machine, left there. For that,
    /// its attachments are downloaded into `attachments_directory` (once each, see `download_attachment`).
    /// When this check cannot be done, nothing is sent.
    ///
    /// `force` sends without asking, as a new attachment whatever was sent before.
    pub fn attach_file(
        &self,
        transaction_id: String,
        path: String,
        force: bool,
        attachments_directory: String,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<AttachOutcome, BankError> {
        let url_path = Self::attachments_path(&transaction_id)?;
        let mut content = Vec::new();
        File::open(&path)
            .and_then(|file| file.take(UPLOAD_MAX + 1).read_to_end(&mut content))
            .map_err(|e| io_error("Could not read the file", e))?;
        if content.len() as u64 > UPLOAD_MAX {
            return Err(invalid_input(format!(
                "The file is larger than {} MB",
                UPLOAD_MAX / (1024 * 1024)
            )));
        }
        let (extension, mimetype) = file_type_of(&content).ok_or(BankError::UnsupportedFile)?;
        let sha256 = pinned_http::sha256_hex(&content);

        if !force {
            let already_there = self
                .remote_hashes(&transaction_id, &attachments_directory)
                .map_err(|e| BankError::NotChecked { message: e.to_string() })?
                .contains(&sha256);
            if already_there {
                return Ok(AttachOutcome::AlreadyThere { sha256 });
            }
        }
        // Past the 30 minutes that Qonto remembers a key, the check above is what prevents a second copy.
        // force is a new key: always a new attachment.
        let idempotency_key = match force {
            true => uuid4(),
            false => idempotency_key(&transaction_id, &sha256),
        };

        // `bank.py`: MultiPartForm. The name is ours: the one of the local file says nothing to Qonto.
        let boundary = pinned_http::random_hex(16);
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"file.{extension}\"\r\n\
             Content-Type: {mimetype}\r\n\r\n"
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let total = (head.len() + content.len() + tail.len()) as u64;
        let body = Cursor::new(head.into_bytes())
            .chain(Cursor::new(content))
            .chain(Cursor::new(tail.into_bytes()));
        // No overall timeout: a photo over a bad connection takes what it takes. Each write has one.
        let result = self
            .api
            .agent
            .post(&format!("{}{}", self.base_url, url_path))
            .set("Authorization", &self.authorization)
            .set("User-Agent", "")
            .set("X-Qonto-Idempotency-Key", &idempotency_key)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .set("Content-Length", &total.to_string())
            .send(ProgressReader::new(body, total, report(&progress)));
        let answer: Value = self.api_json(result)?;
        if answer != serde_json::json!({}) {
            let shown: String = printable(&answer.to_string()).chars().take(300).collect();
            return Err(unexpected(format!("the upload was answered with {shown}")));
        }
        Ok(AttachOutcome::Uploaded {
            sha256,
            idempotency_key,
        })
    }
}

/// [`API_BASE_URL`], for Swift.
#[uniffi::export]
pub fn api_base_url() -> String {
    API_BASE_URL.to_string()
}

/// [`DEFAULT_ATTACHMENTS_HOST`], for Swift.
#[uniffi::export]
pub fn default_attachments_host() -> String {
    DEFAULT_ATTACHMENTS_HOST.to_string()
}

/// Reads the certificate an https server presents, sending it nothing.
/// It is not trusted by this call: show the fingerprint, and have it compared with one read from
/// somewhere else (`tls-pin-get thirdparty.qonto.com:443` on a trusted machine) before pinning it.
#[uniffi::export]
pub fn fetch_fingerprint(url: String, extra_ca_pem: Option<String>) -> Result<CertificateFingerprint, BankError> {
    let found = pinned_http::fetch_fingerprint(&url, extra_ca_pem.as_deref())?;
    Ok(CertificateFingerprint {
        sha256: found.sha256,
        chain_valid: found.chain_valid,
    })
}

/// The pins in a text, one SHA-256 per line: what `tls-pin-get` prints, and what `bank.py` expects of its
/// pin commands. Uppercase, colons and blank lines are fine; anything else is an error, not a pin less.
/// Returned lowercase, without duplicates, in the order given.
#[uniffi::export]
pub fn parse_pins(text: String) -> Result<Vec<String>, BankError> {
    let mut pins = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let pin = pinned_http::normalize_pin(line)
            .map_err(|_| invalid_input(format!("Not a SHA-256: {}", printable(line))))?;
        if !pins.contains(&pin) {
            pins.push(pin);
        }
    }
    if pins.is_empty() {
        return Err(invalid_input("No pin: a SHA-256 is 64 hex characters"));
    }
    if pins.len() > pinned_http::MAX_PINS {
        return Err(invalid_input(format!("More than {} pins", pinned_http::MAX_PINS)));
    }
    Ok(pins)
}

/// What a file is, from its first bytes: `None` for what Qonto does not accept as an attachment.
#[uniffi::export]
pub fn detect_file_type(head: Vec<u8>) -> Option<FileType> {
    file_type_of(&head).map(|(extension, mimetype)| FileType {
        extension: extension.to_string(),
        mimetype: mimetype.to_string(),
    })
}

/// The message of an error, for people. (The Swift `localizedDescription` of a
/// generated error is its case name, not this.)
#[uniffi::export]
pub fn error_message(error: BankError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(slug: &str, secret: &str, api_pins: &[&str]) -> BankConfig {
        BankConfig {
            organization_slug: slug.into(),
            secret_key: secret.into(),
            api_pins: api_pins.iter().map(|p| p.to_string()).collect(),
            attachments_pins: vec![],
        }
    }

    #[test]
    fn the_real_api_is_https_and_pinned() {
        let pin = "ab".repeat(32);
        let client = BankClient::new(config(" my-org-1234 ", " s3cr3t\n", &[&pin])).unwrap();
        assert_eq!(client.base_url, "https://thirdparty.qonto.com");
        assert_eq!(client.authorization, "my-org-1234:s3cr3t");
        assert_eq!(client.api.pins, vec![pin.clone()]);
        assert!(client.attachments.is_none());
        assert_eq!(client.download_scheme, "https://");

        for (slug, secret, pins) in [
            ("my-org", "secret", vec![]),       // no unpinned https
            ("my-org", "secret", vec!["nope"]), // not a SHA-256
            ("", "secret", vec![pin.as_str()]),
            ("my-org", "", vec![pin.as_str()]),
            ("my:org", "secret", vec![pin.as_str()]),
            ("my org", "secret", vec![pin.as_str()]),
            ("my-org\r\nX-Evil: 1", "secret", vec![pin.as_str()]),
            ("my-org", "sec\r\nX-Evil: 1", vec![pin.as_str()]),
            ("my-org", "sécret", vec![pin.as_str()]),
        ] {
            let result = BankClient::new(config(slug, secret, &pins));
            assert!(
                matches!(result, Err(BankError::InvalidConfig { .. })),
                "{slug:?} {secret:?} {pins:?}"
            );
        }
    }

    #[test]
    fn a_base_url_is_this_machine_only() {
        let pin = "ab".repeat(32);
        for url in ["http://127.0.0.1:8080", "http://localhost:8080", "http://[::1]:8080"] {
            assert!(
                BankClient::with_base_url(config("o", "s", &[]), url.into(), None).is_ok(),
                "{url}"
            );
        }
        assert!(BankClient::with_base_url(config("o", "s", &[&pin]), "https://localhost:8443".into(), None).is_ok());
        for url in [
            "https://thirdparty.qonto.com",
            "http://example.com",
            "http://127.0.0.1.example.com:80",
            "http://localhost@example.com",
            "http://example.com/127.0.0.1",
            "http://127.0.0.1:8080/path",
            "http://10.0.0.2:8080",
            "ftp://127.0.0.1",
            "127.0.0.1:8080",
        ] {
            let result = BankClient::with_base_url(config("o", "s", &[&pin]), url.into(), None);
            assert!(matches!(result, Err(BankError::InvalidConfig { .. })), "{url}");
        }
        // Pins with http would be silently ignored, and https has none without.
        assert!(BankClient::with_base_url(config("o", "s", &[&pin]), "http://127.0.0.1:1".into(), None).is_err());
        assert!(BankClient::with_base_url(config("o", "s", &[]), "https://127.0.0.1:1".into(), None).is_err());
    }

    #[test]
    fn uuids() {
        assert!(validate_uuid("12345678-1234-1234-1234-123456789abc").is_ok());
        assert!(validate_uuid("12345678-1234-1234-1234-123456789ABC").is_ok());
        for bad in [
            "",
            "12345678-1234-1234-1234-123456789ab",
            "12345678-1234-1234-1234-123456789abcd",
            "12345678-1234-1234-1234-123456789abg",
            "123456781-234-1234-1234-123456789abc",
            "12345678-1234-1234-1234/../../../etc",
            "12345678-1234-1234-1234-12345678é9a",
            "../../../../../../../../../../../etc",
        ] {
            assert!(
                matches!(validate_uuid(bad), Err(BankError::InvalidInput { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn idempotency_key_is_the_one_of_the_cli() {
        // python3 -c 'from uuid import *; print(uuid5(NAMESPACE_URL, "bank:justify:" + id + ":" + "ab" * 32))'
        assert_eq!(
            idempotency_key("00000000-0000-0000-0000-000000000001", &"ab".repeat(32)),
            "c577539c-bcaa-5f17-8e72-2e80bf78f232"
        );
        let (a, b) = (uuid4(), uuid4());
        assert_ne!(a, b);
        assert!(validate_uuid(&a).is_ok());
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn file_types_are_what_the_content_is() {
        assert_eq!(file_type_of(b"%PDF-1.7\n"), Some(("pdf", "application/pdf")));
        assert_eq!(
            file_type_of(b"\xff\xd8\xff\xe0\x00\x10JFIF\x00"),
            Some(("jpg", "image/jpeg"))
        );
        assert_eq!(
            file_type_of(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"),
            Some(("png", "image/png"))
        );
        for other in [&b""[..], b"GIF89a", b"%PDF", b"PK\x03\x04", b" %PDF-1.7"] {
            assert_eq!(file_type_of(other), None);
        }
        assert_eq!(detect_file_type(b"%PDF-1.7".to_vec()).unwrap().extension, "pdf");
        assert_eq!(detect_file_type(b"ftypheic".to_vec()), None);
    }

    #[test]
    fn transactions_query_is_the_one_of_the_cli() {
        let account = "12345678-1234-1234-1234-123456789abc";
        let all = TransactionFilter::default();
        assert_eq!(
            transactions_query(account, &all, 1).unwrap(),
            "/v2/transactions?bank_account_id=12345678-1234-1234-1234-123456789abc&per_page=100&page=1"
        );
        let filter = TransactionFilter {
            with_attachments: Some(false),
            emitted_at_from: Some("2023-01-01T00:00:00Z".into()),
            emitted_at_to: Some("2024-12-31T23:59:59Z".into()),
        };
        assert_eq!(
            transactions_query(account, &filter, 3).unwrap(),
            "/v2/transactions?with_attachments=false&emitted_at_from=2023-01-01T00:00:00Z\
             &emitted_at_to=2024-12-31T23:59:59Z&bank_account_id=12345678-1234-1234-1234-123456789abc\
             &per_page=100&page=3"
        );

        let bad = |from: Option<&str>, to: Option<&str>| {
            let filter = TransactionFilter {
                with_attachments: None,
                emitted_at_from: from.map(String::from),
                emitted_at_to: to.map(String::from),
            };
            matches!(
                transactions_query(account, &filter, 1),
                Err(BankError::InvalidInput { .. })
            )
        };
        assert!(bad(Some("2024"), None));
        assert!(bad(Some("2024-01-01"), None));
        assert!(bad(Some("2024-01-01T00:00:00Z&bank_account_id=x"), None));
        assert!(bad(None, Some("2024-01-01T00:00:00+0")));
        assert!(bad(Some("2025-01-01T00:00:00Z"), Some("2024-12-31T23:59:59Z")));
        assert!(transactions_query("not-an-account", &all, 1).is_err());
        assert!(transactions_query(account, &all, 0).is_err());
    }

    #[test]
    fn http_errors_say_what_the_server_answered() {
        // The cases of test.py: NetworkErrorTest
        let body = br#"{"errors": [{"code": "unauthorized", "detail": "Invalid credentials"}, {"code": "other"}]}"#;
        assert_eq!(
            describe_http_error(401, "Unauthorized", body),
            "HTTP 401 Unauthorized: unauthorized: Invalid credentials; other"
        );
        let reason = "Internal Server Error";
        assert_eq!(describe_http_error(500, reason, b""), "HTTP 500 Internal Server Error");
        assert_eq!(
            describe_http_error(500, reason, b"<Error>\n  <Code>AccessDenied</Code>\x1b[31m\n</Error>"),
            "HTTP 500 Internal Server Error: <Error> <Code>AccessDenied</Code>\u{fffd}[31m </Error>"
        );
        assert_eq!(
            describe_http_error(500, reason, br#"["a list"]"#),
            r#"HTTP 500 Internal Server Error: ["a list"]"#
        );
        assert_eq!(
            describe_http_error(500, reason, &[b'x'; 5000]),
            format!("HTTP 500 Internal Server Error: {}", "x".repeat(300))
        );
        assert_eq!(
            describe_http_error(418, "", b"\xff teapot"),
            "HTTP 418: \u{fffd} teapot"
        );
    }

    #[test]
    fn pins_are_parsed_as_the_cli_expects_them() {
        let (a, b) = ("ab".repeat(32), "cd".repeat(32));
        assert_eq!(parse_pins(format!("{a}\n")).unwrap(), vec![a.clone()]);
        assert_eq!(parse_pins(format!("{a}\n{b}\n")).unwrap(), vec![a.clone(), b.clone()]);
        let colons = vec!["AB"; 32].join(":");
        assert_eq!(
            parse_pins(format!("\n  {colons}\r\n\n{b}\n{a}")).unwrap(),
            vec![a.clone(), b.clone()],
            "uppercase, colons, blank lines and a duplicate"
        );
        for bad in [
            "",
            "\n\n",
            "abcd",
            &format!("{a}\nnot a pin"),
            &format!("{a} {b}"),
            &"zz".repeat(32),
        ] {
            assert!(
                matches!(parse_pins(bad.to_string()), Err(BankError::InvalidInput { .. })),
                "{bad:?}"
            );
        }
        let many: Vec<String> = (0..=pinned_http::MAX_PINS).map(|i| format!("{i:064x}")).collect();
        assert!(parse_pins(many[..pinned_http::MAX_PINS].join("\n")).is_ok());
        assert!(parse_pins(many.join("\n")).is_err());
    }

    #[test]
    fn what_the_api_names_is_not_a_path() {
        assert_eq!(
            cache_name("0190a8a4-7e2b-7c1d-9f2e-3b1c5d6e7f80"),
            "0190a8a4-7e2b-7c1d-9f2e-3b1c5d6e7f80"
        );
        assert_eq!(cache_name("att-invoice"), "att-invoice");
        for hostile in ["", "../../etc/passwd", "a/b", ".hidden", "a b", &"a".repeat(65), "é"] {
            let name = cache_name(hostile);
            assert_eq!(name.len(), 64, "{hostile:?}");
            assert!(name.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn amounts_are_integers() {
        let transaction = |amount: &str| {
            serde_json::from_str::<Transaction>(&format!(
                r#"{{"transaction_id": "t", "id": "i", "label": null, "amount_cents": {amount},
                    "local_amount_cents": 1, "currency": "EUR", "side": "debit", "emitted_at": "2024"}}"#
            ))
        };
        let parsed = transaction("123456789012").unwrap();
        assert_eq!(parsed.amount_cents, 123_456_789_012);
        assert_eq!(parsed.label, "");
        assert_eq!(parsed.attachment_ids, Vec::<String>::new());
        assert!(transaction("12.5").is_err());
        assert!(transaction("\"1250\"").is_err());

        let account: BankAccount =
            serde_json::from_str(r#"{"id": "a", "balance": 12.34, "balance_cents": 12.34, "name": ["x"]}"#).unwrap();
        assert_eq!(account.balance_cents, None, "a float is not read as an amount");
        assert_eq!(account.name, None);
    }
}
