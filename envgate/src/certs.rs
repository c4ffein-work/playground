//! In-memory certificate store, consulted on every TLS handshake.
//!
//! Certificates are swapped atomically, so a renewal takes effect on the next
//! handshake without a restart and without blocking handshakes in progress.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Default)]
pub struct CertStore {
    certs: ArcSwap<HashMap<String, Arc<CertifiedKey>>>,
}

impl std::fmt::Debug for CertStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let certs = self.certs.load();
        let mut hosts: Vec<_> = certs.keys().collect();
        hosts.sort();
        f.debug_struct("CertStore").field("hosts", &hosts).finish()
    }
}

impl CertStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs (or replaces) the certificate served for `host`, which may be
    /// an exact name or a `*.example.com` wildcard.
    pub fn set(&self, host: &str, key: Arc<CertifiedKey>) {
        let host = host.to_ascii_lowercase();
        self.certs.rcu(|current| {
            let mut next = HashMap::clone(current);
            next.insert(host.clone(), key.clone());
            next
        });
    }

    pub fn get(&self, server_name: &str) -> Option<Arc<CertifiedKey>> {
        let name = server_name.to_ascii_lowercase();
        let certs = self.certs.load();
        if let Some(found) = certs.get(&name) {
            return Some(found.clone());
        }
        let (_, parent) = name.split_once('.')?;
        certs.get(&format!("*.{parent}")).cloned()
    }
}

impl ResolvesServerCert for CertStore {
    /// Clients that send no SNI, or ask for a host that is not configured, get
    /// no certificate and the handshake fails: there is nothing honest to show
    /// them.
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.get(hello.server_name()?)
    }
}

pub fn certified_key_from_pem(chain_pem: &[u8], key_pem: &[u8]) -> Result<CertifiedKey, BoxError> {
    let chain = CertificateDer::pem_slice_iter(chain_pem).collect::<Result<Vec<_>, _>>()?;
    if chain.is_empty() {
        return Err("no certificate found in PEM data".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem)?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)?;
    let certified = CertifiedKey::new(chain, signing_key);
    certified.keys_match()?;
    Ok(certified)
}

pub fn certified_key_from_files(cert: &Path, key: &Path) -> Result<CertifiedKey, BoxError> {
    let chain_pem = std::fs::read(cert).map_err(|e| format!("{}: {e}", cert.display()))?;
    let key_pem = std::fs::read(key).map_err(|e| format!("{}: {e}", key.display()))?;
    certified_key_from_pem(&chain_pem, &key_pem)
        .map_err(|e| format!("{} / {}: {e}", cert.display(), key.display()).into())
}

pub fn self_signed(host: &str) -> Result<CertifiedKey, BoxError> {
    let generated = rcgen::generate_simple_self_signed(vec![host.to_string()])?;
    certified_key_from_pem(
        generated.cert.pem().as_bytes(),
        generated.signing_key.serialize_pem().as_bytes(),
    )
}

/// Validity window of the leaf certificate, as Unix timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validity {
    pub not_before: i64,
    pub not_after: i64,
}

impl Validity {
    pub fn of(key: &CertifiedKey) -> Result<Validity, BoxError> {
        let leaf = key.cert.first().ok_or("empty certificate chain")?;
        let (_, parsed) = x509_parser::parse_x509_certificate(leaf.as_ref())
            .map_err(|e| format!("cannot parse certificate: {e}"))?;
        Ok(Validity {
            not_before: parsed.validity().not_before.timestamp(),
            not_after: parsed.validity().not_after.timestamp(),
        })
    }

    /// Renew once two thirds of the lifetime has passed. This is the usual
    /// rule and, unlike a fixed "30 days before expiry", it also works for
    /// short-lived certificates.
    pub fn renew_at(&self) -> i64 {
        self.not_after - (self.not_after - self.not_before) / 3
    }
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Writes a file so that readers never see a partial one, and so that private
/// keys are never readable by other users, even briefly.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_beats_wildcard_and_unknown_hosts_get_nothing() {
        let store = CertStore::new();
        let exact = Arc::new(self_signed("a.example.com").unwrap());
        let wild = Arc::new(self_signed("*.example.com").unwrap());
        store.set("A.example.com", exact.clone());
        store.set("*.example.com", wild.clone());

        assert!(Arc::ptr_eq(&store.get("a.EXAMPLE.com").unwrap(), &exact));
        assert!(Arc::ptr_eq(&store.get("b.example.com").unwrap(), &wild));
        // A wildcard covers exactly one label.
        assert!(store.get("x.b.example.com").is_none());
        assert!(store.get("example.com").is_none());
        assert!(store.get("other.test").is_none());
    }

    #[test]
    fn replacing_a_certificate_takes_effect() {
        let store = CertStore::new();
        store.set("a.test", Arc::new(self_signed("a.test").unwrap()));
        let second = Arc::new(self_signed("a.test").unwrap());
        store.set("a.test", second.clone());
        assert!(Arc::ptr_eq(&store.get("a.test").unwrap(), &second));
    }

    #[test]
    fn mismatched_key_is_rejected() {
        let a = rcgen::generate_simple_self_signed(vec!["a.test".to_string()]).unwrap();
        let b = rcgen::generate_simple_self_signed(vec!["b.test".to_string()]).unwrap();
        let result = certified_key_from_pem(
            a.cert.pem().as_bytes(),
            b.signing_key.serialize_pem().as_bytes(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn renewal_is_due_after_two_thirds_of_the_lifetime() {
        let v = Validity {
            not_before: 0,
            not_after: 90,
        };
        assert_eq!(v.renew_at(), 60);
        let parsed = Validity::of(&self_signed("a.test").unwrap()).unwrap();
        assert!(parsed.not_after > unix_now());
    }

    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("envgate-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("key.pem");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
