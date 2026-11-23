//! A `ureq` agent that pins the SHA-256 of the server's leaf certificate.
//!
//! Same model as the Python tools this comes from (`make_pinned_ssl_context`
//! in c4ffein/python-snippets): the pin is the SHA-256 of the DER leaf
//! certificate, checked during the handshake, on top of the usual
//! verification. There is no way to get an https agent without a pin.
//!
//! Two trust modes:
//! - [`Trust::PinnedWithCa`]: the pin, plus chain and hostname verification
//!   against the bundled Mozilla roots and an optional CA of your own.
//! - [`Trust::PinnedOnly`]: the pin alone, for a self-signed certificate.
//!   Still safe: the handshake signature is verified against the pinned
//!   certificate's key, so the peer has to hold its private key.
//!
//! Plain http needs no trust at all: [`agent`] with `None`.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// For one read or one write, not for a whole transfer: a big upload is fine, a stalled one is not.
pub const IO_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// Pin + chain and hostname verification (Mozilla roots, plus `extra_ca_pem` when given).
    PinnedWithCa {
        sha256: String,
        extra_ca_pem: Option<String>,
    },
    /// Pin only: for a self-signed certificate.
    PinnedOnly { sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The pin is not 64 hex characters.
    InvalidPin(String),
    /// `extra_ca_pem` holds no usable certificate.
    InvalidCa(String),
    /// An https URL with no trust, or a trust with an http URL (it would be silently ignored).
    SchemeMismatch(String),
    InvalidUrl(String),
    Connect(String),
    Tls(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidPin(m) => write!(f, "invalid pin: {m}"),
            Error::InvalidCa(m) => write!(f, "invalid CA: {m}"),
            Error::SchemeMismatch(m) | Error::InvalidUrl(m) => f.write_str(m),
            Error::Connect(m) => write!(f, "connection failed: {m}"),
            Error::Tls(m) => write!(f, "TLS failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// The SHA-256 of the last leaf certificate a pinned agent was served, as hex.
/// After a failed request, compare it with the pin to tell a pin mismatch from
/// any other transport error.
#[derive(Debug, Clone, Default)]
pub struct SeenCertificate(Arc<Mutex<Option<String>>>);

impl SeenCertificate {
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set(&self, sha256: String) {
        *self.0.lock().unwrap() = Some(sha256);
    }
}

pub struct PinnedAgent {
    pub agent: ureq::Agent,
    /// The normalized pin, `None` over plain http.
    pub pin: Option<String>,
    pub seen: SeenCertificate,
}

/// Lowercases and validates a pin: 64 hex characters, `:` and spaces tolerated
/// (people paste `openssl x509 -fingerprint` output).
pub fn normalize_pin(pin: &str) -> Result<String, Error> {
    let cleaned: String = pin
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if cleaned.len() != 64 || !cleaned.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidPin(format!(
            "expected 64 hex characters, got {}",
            cleaned.len()
        )));
    }
    Ok(cleaned)
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    let mut out = String::with_capacity(64);
    for byte in digest.as_ref() {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

/// `bytes` random bytes from the system, as hex.
pub fn random_hex(bytes: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .expect("the system has no randomness");
    buf.iter().fold(String::with_capacity(bytes * 2), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn roots(extra_ca_pem: Option<&str>) -> Result<RootCertStore, Error> {
    let mut store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    if let Some(pem) = extra_ca_pem {
        let mut added = 0;
        for cert in CertificateDer::pem_slice_iter(pem.as_bytes()) {
            let cert = cert.map_err(|e| Error::InvalidCa(e.to_string()))?;
            store.add(cert).map_err(|e| Error::InvalidCa(e.to_string()))?;
            added += 1;
        }
        if added == 0 {
            return Err(Error::InvalidCa("no certificate found in the PEM".into()));
        }
    }
    Ok(store)
}

fn webpki_verifier(
    extra_ca_pem: Option<&str>,
    provider: &Arc<CryptoProvider>,
) -> Result<Arc<WebPkiServerVerifier>, Error> {
    WebPkiServerVerifier::builder_with_provider(Arc::new(roots(extra_ca_pem)?), provider.clone())
        .build()
        .map_err(|e| Error::InvalidCa(e.to_string()))
}

/// What the verifier does once it has hashed the leaf certificate.
#[derive(Debug)]
enum Mode {
    /// Refuse anything but this hash.
    Pin(String),
    /// Accept anything: only to read a fingerprint, never to send a request.
    Record,
}

#[derive(Debug)]
struct Verifier {
    mode: Mode,
    /// Chain + hostname verification, when wanted.
    chain: Option<Arc<WebPkiServerVerifier>>,
    /// In `Record` mode: did the chain verification pass?
    chain_ok: Arc<Mutex<Option<bool>>>,
    seen: SeenCertificate,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got = sha256_hex(end_entity.as_ref());
        self.seen.set(got.clone());
        let chain = self
            .chain
            .as_ref()
            .map(|v| v.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now));
        match &self.mode {
            Mode::Pin(pin) => {
                if &got != pin {
                    return Err(rustls::Error::General(format!(
                        "certificate does not match the pin (served {got})"
                    )));
                }
                chain.unwrap_or(Ok(ServerCertVerified::assertion()))
            }
            Mode::Record => {
                *self.chain_ok.lock().unwrap() = chain.map(|r| r.is_ok());
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn client_config(verifier: Verifier) -> Result<ClientConfig, Error> {
    Ok(ClientConfig::builder_with_provider(verifier.provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

fn is_https(url: &str) -> Result<bool, Error> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        Ok(true)
    } else if lower.starts_with("http://") {
        Ok(false)
    } else {
        Err(Error::InvalidUrl(format!(
            "the URL must start with http:// or https:// (got {url:?})"
        )))
    }
}

/// Builds the agent for `base_url`. Redirects are never followed: a redirect
/// could move a request (and its credentials) to a host the pin says nothing about.
///
/// - https requires a trust: there is no unpinned https.
/// - http refuses a trust: it would be silently ignored.
pub fn agent(base_url: &str, trust: Option<&Trust>) -> Result<PinnedAgent, Error> {
    let https = is_https(base_url)?;
    let builder = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(IO_TIMEOUT)
        .timeout_write(IO_TIMEOUT)
        .https_only(https);
    let seen = SeenCertificate::default();
    match (https, trust) {
        (false, None) => Ok(PinnedAgent {
            agent: builder.build(),
            pin: None,
            seen,
        }),
        (false, Some(_)) => Err(Error::SchemeMismatch(
            "a certificate pin is set but the URL is http://: switch to https:// or remove the pin".into(),
        )),
        (true, None) => Err(Error::SchemeMismatch(
            "an https:// URL needs a pinned certificate: fetch its fingerprint first".into(),
        )),
        (true, Some(trust)) => {
            let provider = provider();
            let (sha256, chain) = match trust {
                Trust::PinnedWithCa { sha256, extra_ca_pem } => {
                    (sha256, Some(webpki_verifier(extra_ca_pem.as_deref(), &provider)?))
                }
                Trust::PinnedOnly { sha256 } => (sha256, None),
            };
            let pin = normalize_pin(sha256)?;
            let config = client_config(Verifier {
                mode: Mode::Pin(pin.clone()),
                chain,
                chain_ok: Arc::default(),
                seen: seen.clone(),
                provider,
            })?;
            Ok(PinnedAgent {
                agent: builder.tls_config(Arc::new(config)).build(),
                pin: Some(pin),
                seen,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// SHA-256 of the DER leaf certificate, lowercase hex: what to pin.
    pub sha256: String,
    /// The chain and hostname verify against the Mozilla roots (plus the extra CA):
    /// `PinnedWithCa` will work. Otherwise only `PinnedOnly` will.
    pub chain_valid: bool,
}

/// Splits `https://host[:port][/...]` into host and port. https only.
pub fn https_host_port(url: &str) -> Result<(String, u16), Error> {
    if !is_https(url)? {
        return Err(Error::SchemeMismatch("only an https:// URL has a certificate".into()));
    }
    let rest = &url.trim()["https://".len()..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(Error::InvalidUrl("credentials in the URL are not supported".into()));
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        // [::1]:8443
        let (host, after) = v6
            .split_once(']')
            .ok_or_else(|| Error::InvalidUrl(format!("bad IPv6 host in {url:?}")))?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(Error::InvalidUrl(format!("no host in {url:?}")));
    }
    let port = match port {
        None | Some("") => 443,
        Some(p) => p
            .parse()
            .map_err(|_| Error::InvalidUrl(format!("bad port in {url:?}")))?,
    };
    Ok((host.to_string(), port))
}

/// Connects to an https URL and reports the leaf certificate it is served,
/// without sending any request. The certificate is NOT trusted by this call:
/// the fingerprint is to be compared with one read on the server itself
/// (`companion fingerprint`, `openssl x509 -fingerprint -sha256`) before it is pinned.
pub fn fetch_fingerprint(url: &str, extra_ca_pem: Option<&str>) -> Result<Fingerprint, Error> {
    let (host, port) = https_host_port(url)?;
    let provider = provider();
    let seen = SeenCertificate::default();
    let chain_ok: Arc<Mutex<Option<bool>>> = Arc::default();
    let config = client_config(Verifier {
        mode: Mode::Record,
        chain: Some(webpki_verifier(extra_ca_pem, &provider)?),
        chain_ok: chain_ok.clone(),
        seen: seen.clone(),
        provider,
    })?;
    let server_name =
        ServerName::try_from(host.clone()).map_err(|e| Error::InvalidUrl(format!("bad host {host:?}: {e}")))?;
    let mut conn = ClientConnection::new(Arc::new(config), server_name).map_err(|e| Error::Tls(e.to_string()))?;

    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Error::Connect(e.to_string()))?;
    let mut last = Error::Connect(format!("{host} resolves to no address"));
    let mut sock = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                sock = Some(s);
                break;
            }
            Err(e) => last = Error::Connect(e.to_string()),
        }
    }
    let mut sock = sock.ok_or(last)?;
    sock.set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| Error::Connect(e.to_string()))?;
    sock.set_write_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| Error::Connect(e.to_string()))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| Error::Tls(e.to_string()))?;
    }
    conn.send_close_notify();
    let _ = conn.complete_io(&mut sock);

    let sha256 = seen
        .get()
        .ok_or_else(|| Error::Tls("the server sent no certificate".into()))?;
    let chain_valid = chain_ok.lock().unwrap().unwrap_or(false);
    Ok(Fingerprint { sha256, chain_valid })
}

/// Wraps a reader and reports how many bytes went through, out of `total`.
pub struct ProgressReader<R, F: FnMut(u64, u64)> {
    inner: R,
    sent: u64,
    total: u64,
    report: F,
}

impl<R: Read, F: FnMut(u64, u64)> ProgressReader<R, F> {
    pub fn new(inner: R, total: u64, report: F) -> Self {
        Self {
            inner,
            sent: 0,
            total,
            report,
        }
    }
}

impl<R: Read, F: FnMut(u64, u64)> Read for ProgressReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.sent += n as u64;
            (self.report)(self.sent, self.total);
        }
        Ok(n)
    }
}

/// Copies `reader` to `writer`, reporting progress out of `total` (0 when unknown).
pub fn copy_with_progress(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    total: u64,
    mut report: impl FnMut(u64, u64),
) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(done);
        }
        writer.write_all(&buf[..n])?;
        done += n as u64;
        report(done, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_normalized() {
        let hex = "AB".repeat(32);
        assert_eq!(normalize_pin(&hex).unwrap(), "ab".repeat(32));
        let colons = vec!["AB"; 32].join(":");
        assert_eq!(normalize_pin(&format!(" {colons}\n")).unwrap(), "ab".repeat(32));
        assert!(normalize_pin("abcd").is_err());
        assert!(normalize_pin(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn host_and_port() {
        assert_eq!(https_host_port("https://a.example").unwrap(), ("a.example".into(), 443));
        assert_eq!(
            https_host_port("https://a.example:8443/x?y").unwrap(),
            ("a.example".into(), 8443)
        );
        assert_eq!(https_host_port("https://[::1]:8443/").unwrap(), ("::1".into(), 8443));
        assert_eq!(https_host_port("https://[fe80::1]").unwrap(), ("fe80::1".into(), 443));
        assert!(https_host_port("http://a.example").is_err());
        assert!(https_host_port("https://user:pw@a.example").is_err());
        assert!(https_host_port("https://:8443").is_err());
        assert!(https_host_port("https://a.example:port").is_err());
    }

    #[test]
    fn scheme_and_trust_must_agree() {
        let pin = Trust::PinnedOnly {
            sha256: "ab".repeat(32),
        };
        assert!(agent("http://a.example", None).is_ok());
        assert!(matches!(
            agent("https://a.example", None),
            Err(Error::SchemeMismatch(_))
        ));
        assert!(matches!(
            agent("http://a.example", Some(&pin)),
            Err(Error::SchemeMismatch(_))
        ));
        assert!(agent("https://a.example", Some(&pin)).is_ok());
        assert!(matches!(agent("ftp://a.example", None), Err(Error::InvalidUrl(_))));
        let bad = Trust::PinnedOnly { sha256: "nope".into() };
        assert!(matches!(
            agent("https://a.example", Some(&bad)),
            Err(Error::InvalidPin(_))
        ));
        let bad_ca = Trust::PinnedWithCa {
            sha256: "ab".repeat(32),
            extra_ca_pem: Some("not pem".into()),
        };
        assert!(matches!(
            agent("https://a.example", Some(&bad_ca)),
            Err(Error::InvalidCa(_))
        ));
    }
}
