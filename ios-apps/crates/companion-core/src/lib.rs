//! Client for a companion server (<https://github.com/c4ffein/companion>),
//! exposed to Swift through UniFFI.
//!
//! Every call blocks until the server answers: call them off the main thread.
//! The client holds no state but its configuration, and is safe to share.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pinned_http::{copy_with_progress, PinnedAgent, ProgressReader, Trust};
use serde::Deserialize;

uniffi::setup_scaffolding!();

/// For the calls that move no file.
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// How an https server is trusted. There is no unpinned https.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TlsTrust {
    /// The pinned certificate, which must also verify (chain and hostname)
    /// against the public roots, or against `extra_ca_pem` when given.
    PinnedWithCa { sha256: String, extra_ca_pem: Option<String> },
    /// The pinned certificate alone: for a self-signed one.
    PinnedOnly { sha256: String },
}

impl From<&TlsTrust> for Trust {
    fn from(trust: &TlsTrust) -> Self {
        match trust.clone() {
            TlsTrust::PinnedWithCa { sha256, extra_ca_pem } => Trust::PinnedWithCa { sha256, extra_ca_pem },
            TlsTrust::PinnedOnly { sha256 } => Trust::PinnedOnly { sha256 },
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerConfig {
    /// `http://host:port` or `https://host[:port]`.
    pub url: String,
    pub client_id: String,
    pub client_secret: String,
    /// Required with https, refused with http.
    pub trust: Option<TlsTrust>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct RemoteFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mimetype: String,
    /// ISO 8601, in the server's local time, with no zone.
    pub uploaded: String,
    #[serde(default)]
    pub uploaded_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct UploadedFile {
    pub id: String,
    pub filename: String,
    pub size: u64,
}

/// What every client is asked to show. `file_id` is `None` when nothing is presented.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct PreviewState {
    pub file_id: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mimetype: Option<String>,
    /// Milliseconds, strictly increasing: changes whenever the preview is set.
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct Pad {
    pub content: String,
    /// Milliseconds, strictly increasing: changes whenever the pad is written.
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CertificateFingerprint {
    /// SHA-256 of the leaf certificate, lowercase hex: what to pin.
    pub sha256: String,
    /// `PinnedWithCa` will work. When false, only `PinnedOnly` will.
    pub chain_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CompanionError {
    #[error("{message}")]
    InvalidConfig { message: String },
    #[error("{message}")]
    Network { message: String },
    #[error("the server's certificate is not the pinned one (pinned {pinned}, served {served})")]
    PinMismatch { pinned: String, served: String },
    #[error("invalid credentials")]
    Unauthorized,
    #[error("admin access required")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("a token rotation is in progress for this client: complete it first")]
    RotationInProgress,
    #[error("too large: {message}")]
    TooLarge { message: String },
    #[error("rate limit exceeded, try again in a minute")]
    RateLimited,
    #[error("the server answered {status}: {message}")]
    Server { status: u16, message: String },
    #[error("{message}")]
    Io { message: String },
}

impl From<pinned_http::Error> for CompanionError {
    fn from(e: pinned_http::Error) -> Self {
        match e {
            pinned_http::Error::Connect(_) | pinned_http::Error::Tls(_) => Self::Network { message: e.to_string() },
            _ => Self::InvalidConfig { message: e.to_string() },
        }
    }
}

fn io_error(context: &str, e: std::io::Error) -> CompanionError {
    CompanionError::Io { message: format!("{context}: {e}") }
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

#[derive(uniffi::Object)]
pub struct CompanionClient {
    base_url: String,
    /// `Bearer <client_id>:<client_secret>`
    authorization: String,
    http: PinnedAgent,
}

fn invalid(message: &str) -> CompanionError {
    CompanionError::InvalidConfig { message: message.to_string() }
}

/// The server's own rule for path parameters and client ids.
fn is_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// What the server will see as the filename: it reads it from a quoted header
/// value, up to the end of the line, so quotes and line breaks cannot go in.
fn header_safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || c == '"' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('\'');
    if cleaned.is_empty() { "file".to_string() } else { cleaned.to_string() }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

#[derive(Deserialize)]
struct TimestampBody {
    timestamp: u64,
}

impl CompanionClient {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        self.http.agent.request(method, &self.url(path)).set("Authorization", &self.authorization)
    }

    /// Turns anything but a 2xx into an error.
    fn check(&self, result: Result<ureq::Response, ureq::Error>) -> Result<ureq::Response, CompanionError> {
        let (status, response) = match result {
            Ok(response) if (200..300).contains(&response.status()) => return Ok(response),
            // Redirects are not followed, so a 3xx comes back as a response.
            Ok(response) => (response.status(), response),
            Err(ureq::Error::Status(status, response)) => (status, response),
            Err(ureq::Error::Transport(transport)) => {
                if let (Some(pinned), Some(served)) = (&self.http.pin, self.http.seen.get()) {
                    if *pinned != served {
                        return Err(CompanionError::PinMismatch { pinned: pinned.clone(), served });
                    }
                }
                return Err(CompanionError::Network { message: transport.to_string() });
            }
        };
        let mut body = String::new();
        let _ = response.into_reader().take(64 * 1024).read_to_string(&mut body);
        let message = match serde_json::from_str::<ErrorBody>(&body) {
            Ok(parsed) => parsed.error,
            Err(_) if (300..400).contains(&status) => "unexpected redirect: is the URL the server's own?".into(),
            Err(_) => "not a companion server answer".into(),
        };
        Err(match status {
            401 => CompanionError::Unauthorized,
            403 => CompanionError::Forbidden,
            404 => CompanionError::NotFound,
            409 => CompanionError::RotationInProgress,
            413 => CompanionError::TooLarge { message },
            429 => CompanionError::RateLimited,
            _ => CompanionError::Server { status, message },
        })
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, result: Result<ureq::Response, ureq::Error>) -> Result<T, CompanionError> {
        let response = self.check(result)?;
        serde_json::from_reader(response.into_reader()).map_err(|e| CompanionError::Server {
            status: 200,
            message: format!("unexpected answer: {e}"),
        })
    }

    /// Posts JSON to an endpoint that answers with a new timestamp.
    fn post_json(&self, path: &str, body: serde_json::Value) -> Result<u64, CompanionError> {
        let result = self
            .request("POST", path)
            .timeout(API_TIMEOUT)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        Ok(self.json::<TimestampBody>(result)?.timestamp)
    }

    fn upload(
        &self,
        filename: &str,
        content: impl Read,
        size: u64,
        progress: &Option<Arc<dyn ProgressListener>>,
    ) -> Result<UploadedFile, CompanionError> {
        // The server splits the body on the boundary: 128 random bits cannot be in the file.
        let boundary = format!("companion-{}", pinned_http::random_hex(16));
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n",
            header_safe_filename(filename)
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let total = head.len() as u64 + size + tail.len() as u64;
        // `take`: a file that grows meanwhile must not outrun the announced length.
        let body = Cursor::new(head.into_bytes()).chain(content.take(size)).chain(Cursor::new(tail.into_bytes()));
        let result = self
            .request("POST", "/api/upload")
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .set("Content-Length", &total.to_string())
            .send(ProgressReader::new(body, total, report(progress)));
        self.json(result)
    }
}

#[uniffi::export]
impl CompanionClient {
    /// Validates the configuration. Connects to nothing.
    #[uniffi::constructor]
    pub fn new(config: ServerConfig) -> Result<Arc<Self>, CompanionError> {
        let base_url = config.url.trim().trim_end_matches('/').to_string();
        let client_id = config.client_id.trim();
        let client_secret = config.client_secret.trim();
        if !is_id(client_id) || client_id.len() > 64 {
            return Err(invalid("the client id is 1 to 64 letters, digits and hyphens"));
        }
        if client_secret.is_empty() || !client_secret.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(invalid("the client secret is lowercase hex"));
        }
        let after_scheme = base_url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
        if after_scheme.is_empty() || after_scheme.contains(['/', '?', '#', '@']) || base_url.contains(char::is_whitespace) {
            return Err(invalid("the server URL is http(s)://host[:port], with no path"));
        }
        let http = pinned_http::agent(&base_url, config.trust.as_ref().map(Trust::from).as_ref())?;
        Ok(Arc::new(Self { base_url, authorization: format!("Bearer {client_id}:{client_secret}"), http }))
    }

    /// Every file on the server, oldest first.
    pub fn list_files(&self) -> Result<Vec<RemoteFile>, CompanionError> {
        self.json(self.request("GET", "/api/files").timeout(API_TIMEOUT).call())
    }

    /// Uploads the file at `path`, streamed from disk. `filename` is the name
    /// to give it on the server, the name of the file itself when `None`.
    pub fn upload_file(
        &self,
        path: String,
        filename: Option<String>,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<UploadedFile, CompanionError> {
        let file = File::open(&path).map_err(|e| io_error("cannot open the file", e))?;
        let metadata = file.metadata().map_err(|e| io_error("cannot read the file", e))?;
        if !metadata.is_file() {
            return Err(CompanionError::Io { message: "not a file".into() });
        }
        let filename = filename.unwrap_or_else(|| {
            Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        });
        self.upload(&filename, file, metadata.len(), &progress)
    }

    /// Uploads bytes already in memory (a pasted text, a small image).
    pub fn upload_bytes(&self, filename: String, content: Vec<u8>) -> Result<UploadedFile, CompanionError> {
        let size = content.len() as u64;
        self.upload(&filename, Cursor::new(content), size, &None)
    }

    /// Downloads a file to `dest_path`, streamed to disk: the file appears there
    /// complete or not at all (it is written beside it as `.part`, then renamed).
    /// Returns its size.
    pub fn download_file(
        &self,
        file_id: String,
        dest_path: String,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<u64, CompanionError> {
        if !is_id(&file_id) {
            return Err(CompanionError::NotFound);
        }
        let response = self.check(self.request("GET", &format!("/download/{file_id}")).call())?;
        let total = response.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let part_path = format!("{dest_path}.part");
        let written = (|| {
            let mut part = File::create(&part_path)?;
            let written = copy_with_progress(&mut response.into_reader(), &mut part, total, report(&progress))?;
            if total != 0 && written != total {
                return Err(std::io::Error::other(format!("got {written} bytes out of {total}")));
            }
            part.sync_all()?;
            std::fs::rename(&part_path, &dest_path)?;
            Ok(written)
        })();
        written.map_err(|e| {
            let _ = std::fs::remove_file(&part_path);
            io_error("download failed", e)
        })
    }

    /// What every client is asked to show right now.
    pub fn preview_current(&self) -> Result<PreviewState, CompanionError> {
        self.json(self.request("GET", "/api/preview/current").timeout(API_TIMEOUT).call())
    }

    /// Asks every client to show this file. Returns the new preview timestamp.
    pub fn set_preview(&self, file_id: String) -> Result<u64, CompanionError> {
        self.post_json("/api/preview/set", serde_json::json!({ "file_id": file_id }))
    }

    pub fn get_pad(&self) -> Result<Pad, CompanionError> {
        self.json(self.request("GET", "/api/pad").timeout(API_TIMEOUT).call())
    }

    /// Replaces the pad (last write wins). Returns the new pad timestamp.
    pub fn set_pad(&self, content: String) -> Result<u64, CompanionError> {
        self.post_json("/api/pad", serde_json::json!({ "content": content }))
    }
}

/// Reads the certificate an https server presents, sending it nothing.
/// It is not trusted by this call: show the fingerprint, and have it compared
/// with the one read on the server (`companion fingerprint`) before pinning it.
#[uniffi::export]
pub fn fetch_fingerprint(url: String, extra_ca_pem: Option<String>) -> Result<CertificateFingerprint, CompanionError> {
    let found = pinned_http::fetch_fingerprint(&url, extra_ca_pem.as_deref())?;
    Ok(CertificateFingerprint { sha256: found.sha256, chain_valid: found.chain_valid })
}

/// The message of an error, for people. (The Swift `localizedDescription` of a
/// generated error is its case name, not this.)
#[uniffi::export]
pub fn error_message(error: CompanionError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(url: &str, id: &str, secret: &str) -> ServerConfig {
        ServerConfig { url: url.into(), client_id: id.into(), client_secret: secret.into(), trust: None }
    }

    #[test]
    fn config_is_validated() {
        let ok = CompanionClient::new(config(" http://10.0.0.2:8080/ ", " phone-1 ", " abc123\n")).unwrap();
        assert_eq!(ok.base_url, "http://10.0.0.2:8080");
        assert_eq!(ok.authorization, "Bearer phone-1:abc123");

        for (url, id, secret) in [
            ("http://h:1", "", "ab"),
            ("http://h:1", "a b", "ab"),
            ("http://h:1", "a\r\nX-Evil: 1", "ab"),
            ("http://h:1", "a", ""),
            ("http://h:1", "a", "ABCD"),
            ("http://h:1", "a", "ab\r\nX-Evil: 1"),
            ("http://h:1/path", "a", "ab"),
            ("http://user@h:1", "a", "ab"),
            ("http://", "a", "ab"),
            ("h:1", "a", "ab"),
            ("https://h:1", "a", "ab"), // https with no pin
        ] {
            let result = CompanionClient::new(config(url, id, secret));
            assert!(matches!(result, Err(CompanionError::InvalidConfig { .. })), "{url:?} {id:?} {secret:?}");
        }
    }

    #[test]
    fn filenames_cannot_break_the_multipart_header() {
        assert_eq!(header_safe_filename("a.pdf"), "a.pdf");
        assert_eq!(header_safe_filename("été 2026.jpg"), "été 2026.jpg");
        assert_eq!(header_safe_filename("a\"b\r\nc.txt"), "a_b__c.txt");
        assert_eq!(header_safe_filename("'quoted'"), "quoted");
        assert_eq!(header_safe_filename("  "), "file");
        assert_eq!(header_safe_filename(""), "file");
    }
}
