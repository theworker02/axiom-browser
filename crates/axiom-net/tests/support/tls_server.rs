//! Local HTTPS server for `localhost`, signed by a fresh test CA. Shared by the `h2`
//! integration tests and the `netbench` example; nothing here touches the internet.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axiom_net::{FixedDnsResolver, NetworkService, NetworkServiceConfig};
use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::TlsAcceptor;

pub struct TlsServer {
    pub addr: SocketAddr,
    pub ca_pem: String,
    pub connections: Arc<AtomicUsize>,
    _runtime: tokio::runtime::Runtime,
}

impl TlsServer {
    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{}", self.addr.port(), path)
    }
}

/// Replies with the HTTP version the server saw, so the client's protocol claim can be
/// cross-checked.
pub fn spawn_tls_server() -> TlsServer {
    spawn_tls_server_with(false)
}

/// `expired_leaf`: the leaf certificate's validity ended in 2001.
pub fn spawn_tls_server_with(expired_leaf: bool) -> TlsServer {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "Axiom Test CA");
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    if expired_leaf {
        leaf_params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        leaf_params.not_after = rcgen::date_time_ymd(2001, 1, 1);
    }
    let leaf = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();

    let chain: Vec<CertificateDer<'static>> = vec![leaf.der().clone(), ca_cert.der().clone()];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let conns = Arc::clone(&connections);
    runtime.spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            conns.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
                let svc = service_fn(|req: Request<hyper::body::Incoming>| async move {
                    let body = format!("{:?}", req.version());
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body))))
                });
                let io = TokioIo::new(tls);
                if h2 {
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(io, svc)
                        .await;
                } else {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                }
            });
        }
    });
    TlsServer {
        addr,
        ca_pem: ca_cert.pem(),
        connections,
        _runtime: runtime,
    }
}

/// A client that resolves `localhost` to loopback and (optionally) trusts the test CA.
pub fn client(server: &TlsServer, trust_ca: bool, http1_only: bool) -> NetworkService {
    let dns = FixedDnsResolver::new().with("localhost", IpAddr::V4(Ipv4Addr::LOCALHOST));
    let config = NetworkServiceConfig {
        extra_root_certs_pem: if trust_ca {
            vec![server.ca_pem.clone().into_bytes()]
        } else {
            Vec::new()
        },
        http1_only,
        ..NetworkServiceConfig::default()
    };
    NetworkService::try_new(config, Arc::new(dns)).unwrap()
}
