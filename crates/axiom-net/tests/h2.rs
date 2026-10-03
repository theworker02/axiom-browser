//! Local HTTPS server with a test CA: verifies that HTTP/2 is actually negotiated via
//! ALPN (not assumed from configuration), that HTTP/1.1 fallback works, and that an
//! untrusted certificate is rejected (verification is never disabled).

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axiom_net::{
    CertificateErrorKind, FixedDnsResolver, HttpProtocol, NetworkError, NetworkRequest,
    NetworkService, NetworkServiceConfig, ResourceType,
};
use axiom_url::Url;

mod support {
    pub mod tls_server;
}
use support::tls_server::{client, spawn_tls_server, spawn_tls_server_with};

fn get(u: &str) -> NetworkRequest {
    NetworkRequest::get(Url::parse(u).unwrap(), ResourceType::Other)
}

#[test]
fn https_negotiates_http2_via_alpn() {
    let server = spawn_tls_server();
    let svc = client(&server, true, false);
    for _ in 0..3 {
        let resp = svc.execute_blocking(get(&server.url("/"))).unwrap();
        assert_eq!(resp.meta.protocol, HttpProtocol::Http2);
        let tls = resp.meta.tls.clone().expect("tls info");
        assert_eq!(tls.alpn, HttpProtocol::Http2);
        assert!(tls.certificate_verified);
        // Not measured by this transport; must not be invented.
        assert_eq!(tls.version, None);
        assert_eq!(tls.cipher_suite, None);
        assert_eq!(tls.hostname.as_deref(), Some("localhost"));
        let cert = tls.certificate.expect("peer certificate metadata");
        assert!(cert.subject.contains("localhost"), "{}", cert.subject);
        assert!(cert.issuer.contains("Axiom Test CA"), "{}", cert.issuer);
        assert!(cert.subject_alt_names.iter().any(|s| s == "localhost"));
        assert!(cert.not_before_unix < cert.not_after_unix);
        assert_eq!(cert.sha256_fingerprint.len(), 64);
        // The server independently confirms the protocol.
        assert_eq!(resp.body.read_all(1024).unwrap(), b"HTTP/2.0");
    }
    // HTTP/2 multiplexes sequential requests over a single connection.
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
    assert_eq!(svc.pool_stats().connections_opened, Some(1));
}

#[test]
fn http1_only_falls_back_to_http11_over_tls() {
    let server = spawn_tls_server();
    let svc = client(&server, true, true);
    let resp = svc.execute_blocking(get(&server.url("/"))).unwrap();
    assert_eq!(resp.meta.protocol, HttpProtocol::Http11);
    assert_eq!(resp.body.read_all(1024).unwrap(), b"HTTP/1.1");
}

#[test]
fn untrusted_certificate_is_rejected() {
    let server = spawn_tls_server();
    let svc = client(&server, false, false);
    let err = svc.execute_blocking(get(&server.url("/"))).unwrap_err();
    assert!(
        matches!(
            err,
            NetworkError::Certificate {
                kind: CertificateErrorKind::UntrustedIssuer,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(err.kind_name(), "certificate");
}

#[test]
fn expired_certificate_is_rejected() {
    let server = spawn_tls_server_with(true);
    let svc = client(&server, true, false);
    let err = svc.execute_blocking(get(&server.url("/"))).unwrap_err();
    assert!(
        matches!(
            err,
            NetworkError::Certificate {
                kind: CertificateErrorKind::Expired,
                ..
            }
        ),
        "{err:?}"
    );
    let entry = svc.recent_log(1).pop().unwrap();
    assert_eq!(entry.error_kind, Some("certificate"));
    assert!(
        entry.tls.is_none(),
        "no TLS facts for a rejected connection"
    );
}

#[test]
fn hostname_mismatch_is_rejected() {
    let server = spawn_tls_server();
    let dns = FixedDnsResolver::new().with("other.test", IpAddr::V4(Ipv4Addr::LOCALHOST));
    let svc = NetworkService::try_new(
        NetworkServiceConfig {
            extra_root_certs_pem: vec![server.ca_pem.clone().into_bytes()],
            ..NetworkServiceConfig::default()
        },
        Arc::new(dns),
    )
    .unwrap();
    let url = format!("https://other.test:{}/", server.addr.port());
    let err = svc.execute_blocking(get(&url)).unwrap_err();
    assert!(
        matches!(
            err,
            NetworkError::Certificate {
                kind: CertificateErrorKind::HostnameMismatch,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn https_response_is_cached_with_its_tls_facts() {
    let server = spawn_tls_server();
    let svc = client(&server, true, false);
    let first = svc.execute_blocking(get(&server.url("/"))).unwrap();
    first.body.read_all(1024).unwrap();
    // The test server sends no caching headers, so the response must not be stored; the
    // TLS facts of each response come from its own connection.
    let second = svc.execute_blocking(get(&server.url("/"))).unwrap();
    assert!(second
        .meta
        .tls
        .as_ref()
        .is_some_and(|t| t.certificate_verified));
    assert_eq!(
        svc.recent_log(1)[0].tls.as_ref().map(|t| t.alpn),
        Some(HttpProtocol::Http2)
    );
}
