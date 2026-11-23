//! The pinned https path, end to end: the mock of the Qonto API behind TLS, with a certificate for
//! `localhost` signed by a throwaway CA. The client is the one of the real API in every way but its
//! base URL and that CA: pin, chain and hostname are all verified, for the API and for the storage.

mod mock;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use bank_core::{BankClient, BankConfig, BankError, CertificateFingerprint, PinnedHost, TransactionFilter};
use mock::{Mock, State, MAIN_ACCOUNT, PDF, SECRET, SLUG};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

struct TlsMock {
    mock: Mock,
    /// The same server, by an address its certificate does not cover.
    url_by_ip: String,
    leaf_sha256: String,
    ca_pem: String,
}

fn serve() -> TlsMock {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::from_params(&ca_params, &ca_key);
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &issuer)
        .unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], key)
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("https://localhost:{port}");
    let state = Arc::new(Mutex::new(State::sample()));
    mock::serve_on(listener, state.clone(), url.clone(), Some(Arc::new(config)));
    TlsMock {
        mock: Mock { url, state },
        url_by_ip: format!("https://127.0.0.1:{port}"),
        leaf_sha256: pinned_http::sha256_hex(leaf.der()),
        ca_pem: ca_cert.pem(),
    }
}

fn client(server: &TlsMock, url: &str, api_pins: &[&str], attachments_pins: &[&str]) -> Arc<BankClient> {
    let config = BankConfig {
        organization_slug: SLUG.into(),
        secret_key: SECRET.into(),
        api_pins: api_pins.iter().map(|p| p.to_string()).collect(),
        attachments_pins: attachments_pins.iter().map(|p| p.to_string()).collect(),
    };
    BankClient::with_base_url(config, url.into(), Some(server.ca_pem.clone())).unwrap()
}

fn scratch() -> String {
    let dir = std::env::temp_dir().join(format!("bank-core-tls-{}", pinned_http::random_hex(8)));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

#[test]
fn everything_works_over_pinned_https() {
    let server = serve();
    let pin = server.leaf_sha256.as_str();
    let client = client(&server, &server.mock.url, &[pin], &[pin]);

    assert_eq!(client.accounts().unwrap()[0].id, MAIN_ACCOUNT);
    let transactions = client
        .transactions(MAIN_ACCOUNT.into(), TransactionFilter::default())
        .unwrap();
    assert_eq!(transactions.len(), 5);

    let dir = scratch();
    let attachment = client.attachments(mock::transaction_uuid(4)).unwrap().remove(0);
    assert!(attachment.url.as_deref().unwrap().starts_with("https://localhost:"));
    let local = client.download_attachment(attachment, dir.clone(), None).unwrap();
    assert_eq!(std::fs::read(&local.path).unwrap(), PDF);

    let receipt = format!("{dir}/receipt");
    std::fs::write(&receipt, mock::JPG).unwrap();
    client
        .attach_file(mock::transaction_uuid(5), receipt, false, dir.clone(), None)
        .unwrap();
    assert_eq!(server.mock.state().attachments_of(5)[0].content, mock::JPG);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_certificate_that_is_not_pinned_gets_nothing() {
    let server = serve();
    let wrong = "ab".repeat(32);
    let error = client(&server, &server.mock.url, &[&wrong], &[])
        .accounts()
        .unwrap_err();
    assert_eq!(
        error,
        BankError::PinMismatch {
            which: PinnedHost::Api,
            host: server.mock.url["https://".len()..].to_string(),
            served: server.leaf_sha256.clone(),
        }
    );
    assert!(bank_core::error_message(error).contains(&server.leaf_sha256));
    assert!(
        server.mock.state().requests.is_empty(),
        "the handshake is refused: no request, no credentials"
    );
}

#[test]
fn any_of_the_pins_is_accepted() {
    // During a rotation: the certificate to come, and the one served today.
    let server = serve();
    let (next, current) = ("cd".repeat(32), server.leaf_sha256.to_uppercase());
    let client = client(&server, &server.mock.url, &[&next, &current], &[]);
    assert_eq!(client.accounts().unwrap().len(), 2);
}

#[test]
fn the_pin_is_not_enough_chain_and_name_are_verified_too() {
    let server = serve();
    let pin = server.leaf_sha256.as_str();
    // The right certificate, reached by a name it is not for.
    let error = client(&server, &server.url_by_ip, &[pin], &[]).accounts().unwrap_err();
    assert!(
        matches!(&error, BankError::Network { message } if message.contains("not valid for name")),
        "{error:?}"
    );
    // The right certificate, signed by a CA that is not trusted: the public roots only.
    let config = BankConfig {
        organization_slug: SLUG.into(),
        secret_key: SECRET.into(),
        api_pins: vec![pin.into()],
        attachments_pins: vec![],
    };
    let public_roots_only = BankClient::with_base_url(config, server.mock.url.clone(), None).unwrap();
    let error = public_roots_only.accounts().unwrap_err();
    assert!(
        matches!(&error, BankError::Network { message } if message.contains("UnknownIssuer")),
        "{error:?}"
    );
    assert!(server.mock.state().requests.is_empty());
}

#[test]
fn the_storage_has_its_own_pins() {
    let server = serve();
    let pin = server.leaf_sha256.as_str();
    let wrong = "ab".repeat(32);
    let dir = scratch();
    let receipt = format!("{dir}/receipt.pdf");
    std::fs::write(&receipt, b"%PDF-1.4 another invoice").unwrap();
    let transaction = mock::transaction_uuid(4);

    // No pin for the storage: the API works, nothing is downloaded.
    let without = client(&server, &server.mock.url, &[pin], &[]);
    let attachment = without.attachments(transaction.clone()).unwrap().remove(0);
    let error = without
        .download_attachment(attachment.clone(), dir.clone(), None)
        .unwrap_err();
    assert_eq!(error, BankError::NoAttachmentsPin);
    // So what the transaction has cannot be checked: nothing is sent, unless forced.
    let error = without
        .attach_file(transaction.clone(), receipt.clone(), false, dir.clone(), None)
        .unwrap_err();
    assert!(matches!(error, BankError::NotChecked { .. }), "{error:?}");
    assert_eq!(server.mock.state().paths("POST").len(), 0);

    // A pin that is not the one of the storage: said as such, with what was served.
    let mismatched = client(&server, &server.mock.url, &[pin], &[&wrong]);
    let error = mismatched
        .download_attachment(attachment, dir.clone(), None)
        .unwrap_err();
    assert_eq!(
        error,
        BankError::PinMismatch {
            which: PinnedHost::Attachments,
            host: server.mock.url["https://".len()..].to_string(),
            served: server.leaf_sha256.clone(),
        }
    );
    assert!(!server.mock.state().requests.iter().any(|r| r.path.starts_with("/s3/")));

    // An attachment URL that is not https is not fetched, whatever the API says.
    let mut downgraded = mismatched.attachments(transaction).unwrap().remove(0);
    downgraded.url = Some(downgraded.url.unwrap().replace("https://", "http://"));
    let error = mismatched
        .download_attachment(downgraded, dir.clone(), None)
        .unwrap_err();
    assert_eq!(bank_core::error_message(error), "Unexpected answer: No download URL");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_served_certificate_can_be_read_before_it_is_pinned() {
    let server = serve();
    assert_eq!(
        bank_core::fetch_fingerprint(server.mock.url.clone(), Some(server.ca_pem.clone())).unwrap(),
        CertificateFingerprint {
            sha256: server.leaf_sha256.clone(),
            chain_valid: true
        }
    );
    assert_eq!(
        bank_core::fetch_fingerprint(server.mock.url.clone(), None).unwrap(),
        CertificateFingerprint {
            sha256: server.leaf_sha256.clone(),
            chain_valid: false
        },
        "not signed by a public CA: read, and reported as not usable"
    );
    assert!(matches!(
        bank_core::fetch_fingerprint("http://localhost:1".into(), None),
        Err(BankError::InvalidConfig { .. })
    ));
    assert!(
        server.mock.state().requests.is_empty(),
        "reading a certificate sends nothing"
    );
}
