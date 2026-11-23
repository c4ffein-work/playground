//! The trust modes, against a local TLS server with a throwaway CA.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use pinned_http::{agent, fetch_fingerprint, sha256_hex, Fingerprint, Trust};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

struct Server {
    url: String,
    /// The same server, by an address its certificate does not cover.
    url_by_ip: String,
    leaf_sha256: String,
    ca_pem: String,
}

/// Serves `ok` to every request, with a certificate for `localhost` signed by a new CA.
fn serve() -> Server {
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
    let leaf_sha256 = sha256_hex(leaf.der());

    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], key)
        .unwrap();
    let config = Arc::new(config);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(sock) = sock else { continue };
            let config = config.clone();
            std::thread::spawn(move || {
                let Ok(conn) = ServerConnection::new(config) else {
                    return;
                };
                let mut tls = StreamOwned::new(conn, sock);
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf) {
                        Ok(0) | Err(_) => return, // refused handshake, or a fingerprint probe
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });

    Server {
        url: format!("https://localhost:{port}"),
        url_by_ip: format!("https://127.0.0.1:{port}"),
        leaf_sha256,
        ca_pem: ca_cert.pem(),
    }
}

fn get(url: &str, trust: &Trust) -> Result<String, String> {
    let pinned = agent(url, Some(trust)).map_err(|e| e.to_string())?;
    match pinned.agent.get(url).call() {
        Ok(response) => Ok(response.into_string().unwrap()),
        Err(e) => Err(format!("{e} (served {:?})", pinned.seen.get())),
    }
}

#[test]
fn fingerprint_is_the_leaf_sha256() {
    let s = serve();
    assert_eq!(
        fetch_fingerprint(&s.url, None).unwrap(),
        Fingerprint {
            sha256: s.leaf_sha256.clone(),
            chain_valid: false
        },
        "an unknown CA: the fingerprint is read, and reported as not verifiable"
    );
    assert_eq!(
        fetch_fingerprint(&s.url, Some(&s.ca_pem)).unwrap(),
        Fingerprint {
            sha256: s.leaf_sha256.clone(),
            chain_valid: true
        }
    );
    assert_eq!(
        fetch_fingerprint(&s.url_by_ip, Some(&s.ca_pem)).unwrap(),
        Fingerprint {
            sha256: s.leaf_sha256.clone(),
            chain_valid: false
        },
        "the right CA but the wrong name is not a valid chain"
    );
}

#[test]
fn pinned_only_accepts_the_pinned_certificate() {
    let s = serve();
    let trust = Trust::PinnedOnly {
        pins: vec![s.leaf_sha256.clone()],
    };
    assert_eq!(get(&s.url, &trust).unwrap(), "ok");
    // The pin identifies the certificate: the name it was reached by does not matter.
    assert_eq!(get(&s.url_by_ip, &trust).unwrap(), "ok");
    // Uppercase with colons, as openssl prints it.
    let openssl_style = s
        .leaf_sha256
        .to_uppercase()
        .as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(":");
    assert_eq!(
        get(
            &s.url,
            &Trust::PinnedOnly {
                pins: vec![openssl_style]
            }
        )
        .unwrap(),
        "ok"
    );
}

#[test]
fn a_wrong_pin_is_refused_and_the_served_one_is_reported() {
    let s = serve();
    for trust in [
        Trust::PinnedOnly {
            pins: vec!["ab".repeat(32)],
        },
        Trust::PinnedWithCa {
            pins: vec!["ab".repeat(32)],
            extra_ca_pem: Some(s.ca_pem.clone()),
        },
    ] {
        let pinned = agent(&s.url, Some(&trust)).unwrap();
        assert!(pinned.agent.get(&s.url).call().is_err());
        assert_eq!(pinned.seen.get().as_deref(), Some(s.leaf_sha256.as_str()));
        assert_eq!(pinned.pin_mismatch().as_deref(), Some(s.leaf_sha256.as_str()));
    }
}

#[test]
fn pinned_with_ca_also_verifies_chain_and_name() {
    let s = serve();
    let sha256 = s.leaf_sha256.clone();

    let with_ca = Trust::PinnedWithCa {
        pins: vec![sha256.clone()],
        extra_ca_pem: Some(s.ca_pem.clone()),
    };
    assert_eq!(get(&s.url, &with_ca).unwrap(), "ok");

    let err = get(&s.url_by_ip, &with_ca).unwrap_err();
    assert!(
        err.contains("not valid for name"),
        "right pin and CA, wrong name: {err}"
    );

    let public_roots_only = Trust::PinnedWithCa {
        pins: vec![sha256],
        extra_ca_pem: None,
    };
    let err = get(&s.url, &public_roots_only).unwrap_err();
    assert!(err.contains("UnknownIssuer"), "right pin, unknown CA: {err}");
}

#[test]
fn any_of_the_pins_is_accepted() {
    // During a rotation: the certificate to come, and the one served today.
    let s = serve();
    let trust = Trust::PinnedOnly {
        pins: vec!["cd".repeat(32), s.leaf_sha256.clone()],
    };
    let pinned = agent(&s.url, Some(&trust)).unwrap();
    assert_eq!(pinned.agent.get(&s.url).call().unwrap().into_string().unwrap(), "ok");
    assert_eq!(pinned.pin_mismatch(), None);
}

#[test]
fn web_agent_verifies_against_public_roots_only() {
    let s = serve();
    let err = pinned_http::web_agent(0)
        .unwrap()
        .get(&s.url)
        .call()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("UnknownIssuer"),
        "a private CA is not a public site: {err}"
    );
}

#[test]
fn redirects_are_not_followed() {
    // A plain http server that redirects everything to another host.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { continue };
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://elsewhere.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    let url = format!("http://127.0.0.1:{port}");
    let response = agent(&url, None).unwrap().agent.get(&url).call().unwrap();
    assert_eq!(response.status(), 302, "the redirect is handed back, not followed");
}
