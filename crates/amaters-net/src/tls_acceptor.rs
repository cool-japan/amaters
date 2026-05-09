//! Live-rotating TLS acceptor.
//!
//! [`LiveTlsAcceptor`] wraps a [`tokio::net::TcpListener`] and an
//! `Arc<ArcSwap<rustls::ServerConfig>>`.  On every accepted TCP connection
//! it loads the *current* `ServerConfig` and performs a TLS handshake
//! against it; new handshakes after a swap pick up the new cert
//! automatically while in-flight connections continue on whatever cert
//! they negotiated.
//!
//! This is the wire-level counterpart to
//! `amaters_server::hot_reload::spawn_tls_reloader`: the reloader writes
//! into the `ArcSwap`, the acceptor reads from it.
//!
//! # Wiring example
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use arc_swap::ArcSwap;
//! use tokio::net::TcpListener;
//! use amaters_net::tls_acceptor::{LiveTlsAcceptor, TlsCredsRef, build_rustls_config};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let creds = TlsCredsRef {
//!     cert_pem: include_bytes!("../tests/fixtures/cert.pem"),
//!     key_pem:  include_bytes!("../tests/fixtures/key.pem"),
//! };
//! let initial = build_rustls_config(&creds)?;
//! let store = Arc::new(ArcSwap::from_pointee(initial));
//!
//! let listener = TcpListener::bind("127.0.0.1:50051").await?;
//! let acceptor = LiveTlsAcceptor::new(listener, Arc::clone(&store));
//! let stream = acceptor.into_stream();
//!
//! // tonic: hand the stream to Server::serve_with_incoming
//! // tonic::transport::Server::builder()
//! //     .add_service(svc)
//! //     .serve_with_incoming(stream)
//! //     .await?;
//! # Ok::<(), anyhow::Error>(())
//! # }
//! ```

#![cfg(feature = "mtls")]

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use futures::Stream;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tracing::warn;

use crate::error::{NetError, NetResult};

/// Borrowed view of TLS PEM credentials (cert + private key).
///
/// Avoids forcing callers to allocate or move PEM bytes when they already
/// have them on hand.  The `'a` lifetime covers both buffers.
#[derive(Debug, Clone, Copy)]
pub struct TlsCredsRef<'a> {
    /// PEM-encoded certificate chain.
    pub cert_pem: &'a [u8],
    /// PEM-encoded private key (PKCS#8 / RSA / EC, sniffed automatically).
    pub key_pem: &'a [u8],
}

impl<'a> TlsCredsRef<'a> {
    /// Build a borrowed view from raw byte slices.
    pub fn new(cert_pem: &'a [u8], key_pem: &'a [u8]) -> Self {
        Self { cert_pem, key_pem }
    }
}

// ---------------------------------------------------------------------------
// build_rustls_config
// ---------------------------------------------------------------------------

/// Build a `rustls::ServerConfig` from PEM-encoded cert + key.
///
/// The private key is sniffed in PKCS#8 → RSA → EC order; the first format
/// that parses successfully wins.  All cert chain entries are loaded.
///
/// # Crypto provider
///
/// Rustls 0.23 requires a process-wide `CryptoProvider`.  This function
/// installs the bundled `ring` provider on first call (`install_default`
/// is a no-op once a provider has been set), so callers do not need to
/// configure rustls themselves.  If the application has already installed
/// a different provider, it is preserved.
///
/// # Errors
///
/// Returns [`NetError::TlsError`] if the cert chain is empty or unparseable,
/// the private key cannot be parsed in any supported format, or the rustls
/// builder rejects the resulting key/cert pair (mismatched key type, etc.).
pub fn build_rustls_config(creds: &TlsCredsRef<'_>) -> NetResult<ServerConfig> {
    // Ensure a crypto provider is installed.  `install_default` returns Err
    // if a provider is already present — we treat that as a no-op.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Parse certificate chain.
    let mut cert_reader = std::io::Cursor::new(creds.cert_pem);
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| NetError::TlsError(format!("Failed to parse cert PEM: {e}")))?;
    if cert_chain.is_empty() {
        return Err(NetError::TlsError(
            "Cert PEM contained no certificates".to_string(),
        ));
    }

    // Parse private key — try PKCS#8, RSA, EC in order.
    let key_der = parse_private_key(creds.key_pem)?;

    // Build the rustls server config.  Default cipher suites + TLS 1.2/1.3.
    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key_der)
        .map_err(|e| NetError::TlsError(format!("rustls rejected cert/key: {e}")))?;

    Ok(cfg)
}

/// Sniff a private key in PKCS#8 → RSA → EC order.
fn parse_private_key(key_pem: &[u8]) -> NetResult<PrivateKeyDer<'static>> {
    let mut cursor = std::io::Cursor::new(key_pem);
    if let Some(key) = rustls_pemfile::pkcs8_private_keys(&mut cursor)
        .next()
        .transpose()
        .map_err(|e| NetError::TlsError(format!("PKCS#8 parse error: {e}")))?
    {
        return Ok(PrivateKeyDer::Pkcs8(key));
    }

    let mut cursor = std::io::Cursor::new(key_pem);
    if let Some(key) = rustls_pemfile::rsa_private_keys(&mut cursor)
        .next()
        .transpose()
        .map_err(|e| NetError::TlsError(format!("RSA parse error: {e}")))?
    {
        return Ok(PrivateKeyDer::Pkcs1(key));
    }

    let mut cursor = std::io::Cursor::new(key_pem);
    if let Some(key) = rustls_pemfile::ec_private_keys(&mut cursor)
        .next()
        .transpose()
        .map_err(|e| NetError::TlsError(format!("EC parse error: {e}")))?
    {
        return Ok(PrivateKeyDer::Sec1(key));
    }

    Err(NetError::TlsError(
        "No valid private key in PEM (tried PKCS#8, RSA, EC)".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// LiveTlsAcceptor
// ---------------------------------------------------------------------------

/// TCP-level acceptor that performs a per-connection rustls handshake
/// against the *current* server config in an [`ArcSwap`].
///
/// Cheap to clone (`store` is `Arc`).  Hand to a tonic transport via
/// [`Self::into_stream`].
pub struct LiveTlsAcceptor {
    listener: TcpListener,
    store: Arc<ArcSwap<ServerConfig>>,
}

impl LiveTlsAcceptor {
    /// Create a new acceptor bound to `listener` reading from `store`.
    pub fn new(listener: TcpListener, store: Arc<ArcSwap<ServerConfig>>) -> Self {
        Self { listener, store }
    }

    /// Return the locally-bound socket address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Borrow the underlying ArcSwap store, e.g. so callers can swap a new
    /// `ServerConfig` in.
    pub fn store(&self) -> Arc<ArcSwap<ServerConfig>> {
        Arc::clone(&self.store)
    }

    /// Accept a single connection and run the TLS handshake.
    ///
    /// The current `ServerConfig` is loaded from the [`ArcSwap`] at accept
    /// time, so a swap that completes between two `accept()` calls applies
    /// to the second connection.
    ///
    /// # Errors
    ///
    /// Bubbles up `tokio::io::Error` from either the TCP accept or the TLS
    /// handshake.
    pub async fn accept(
        &self,
    ) -> io::Result<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr)> {
        let (tcp, peer) = self.listener.accept().await?;
        let cfg = Arc::clone(&self.store.load());
        let acceptor = TlsAcceptor::from(cfg);
        let tls = acceptor.accept(tcp).await?;
        Ok((tls, peer))
    }

    /// Convert the acceptor into a `Stream` of TLS streams for tonic's
    /// `serve_with_incoming` family.
    ///
    /// Handshake errors are logged at WARN and skipped — the stream itself
    /// stays open so a single malformed client never tears down the listener.
    pub fn into_stream(
        self,
    ) -> impl Stream<Item = io::Result<tokio_rustls::server::TlsStream<TcpStream>>> {
        async_stream::stream! {
            loop {
                match self.accept().await {
                    Ok((tls, _peer)) => yield Ok(tls),
                    Err(e) => {
                        warn!("LiveTlsAcceptor: accept/handshake failed: {e}");
                        // Continue serving — do not propagate per-connection errors.
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::SelfSignedGenerator;
    use rustls::pki_types::ServerName;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsConnector;

    /// Generate a fresh self-signed cert pair as PEM bytes for testing.
    ///
    /// `SelfSignedGenerator::new` pre-populates the SAN list with `"localhost"`,
    /// so we add `san` and `"127.0.0.1"` on top — the per-cert lookup helpers
    /// (`cert_has_san`) match by membership rather than ordering.
    fn pem_pair_with_san(san: &str) -> (Vec<u8>, Vec<u8>) {
        let generator = SelfSignedGenerator::new(san)
            .with_san(san)
            .with_san("127.0.0.1");
        let (cert_der, key_der) = generator.generate().expect("generate cert");

        // PEM-encode both.
        let cert_pem = pem_encode("CERTIFICATE", cert_der.as_ref());
        let key_pem = match key_der {
            PrivateKeyDer::Pkcs8(k) => pem_encode("PRIVATE KEY", k.secret_pkcs8_der()),
            PrivateKeyDer::Pkcs1(k) => pem_encode("RSA PRIVATE KEY", k.secret_pkcs1_der()),
            PrivateKeyDer::Sec1(k) => pem_encode("EC PRIVATE KEY", k.secret_sec1_der()),
            _ => panic!("unexpected key kind"),
        };
        (cert_pem, key_pem)
    }

    /// Minimal PEM encoder using base64 standard alphabet (RFC 7468).
    fn pem_encode(label: &str, der: &[u8]) -> Vec<u8> {
        let mut out = format!("-----BEGIN {label}-----\n").into_bytes();
        let b64 = base64_encode(der);
        // 64-char wrapping per RFC 7468.
        for chunk in b64.as_bytes().chunks(64) {
            out.extend_from_slice(chunk);
            out.push(b'\n');
        }
        out.extend_from_slice(format!("-----END {label}-----\n").as_bytes());
        out
    }

    /// Tiny base64 encoder — alphabet, padding, no line breaks.
    fn base64_encode(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        let mut i = 0;
        while i + 3 <= data.len() {
            let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
            i += 3;
        }
        let rem = data.len() - i;
        if rem == 1 {
            let n = (data[i] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        } else if rem == 2 {
            let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        out
    }

    /// Build a rustls client config that trusts a single self-signed cert.
    fn client_config_trusting(cert_pem: &[u8]) -> rustls::ClientConfig {
        let mut roots = rustls::RootCertStore::empty();
        let mut reader = std::io::Cursor::new(cert_pem);
        for cert in rustls_pemfile::certs(&mut reader) {
            let cert = cert.expect("parse cert");
            roots.add(cert).expect("add to root store");
        }
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    }

    /// Return `true` if `der` is an X.509 cert that carries `expected` as a
    /// SAN DNS name (case-sensitive, exact match).  Used by the per-connection
    /// tests to assert which cert version was negotiated, regardless of the
    /// order the `localhost` baseline SAN was inserted.
    fn cert_has_san(der: &[u8], expected: &str) -> bool {
        use x509_parser::prelude::*;
        let (_, cert) = match X509Certificate::from_der(der) {
            Ok(v) => v,
            Err(_) => return false,
        };
        if let Ok(Some(san_ext)) = cert.subject_alternative_name() {
            for name in &san_ext.value.general_names {
                if let GeneralName::DNSName(s) = name {
                    if *s == expected {
                        return true;
                    }
                }
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // build_rustls_config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_rustls_config_from_creds() {
        let (cert_pem, key_pem) = pem_pair_with_san("server.test");
        let creds = TlsCredsRef::new(&cert_pem, &key_pem);
        let cfg = build_rustls_config(&creds).expect("build rustls config");
        // Sanity: the resulting ServerConfig is non-empty (has the cert).
        // We can't introspect cert chain length from the public API but a
        // successful build is the contract.
        let _ = cfg;
    }

    #[test]
    fn test_build_rustls_config_invalid_cert_errors() {
        let creds = TlsCredsRef::new(
            b"-----BEGIN GARBAGE-----\nnope\n-----END GARBAGE-----\n",
            b"",
        );
        let err = build_rustls_config(&creds).expect_err("should fail");
        assert!(matches!(err, NetError::TlsError(_)), "got {err:?}");
    }

    #[test]
    fn test_build_rustls_config_empty_cert_errors() {
        let creds = TlsCredsRef::new(b"", b"");
        let err = build_rustls_config(&creds).expect_err("should fail");
        assert!(matches!(err, NetError::TlsError(_)), "got {err:?}");
    }

    // -----------------------------------------------------------------------
    // LiveTlsAcceptor tests
    // -----------------------------------------------------------------------

    /// Spawn an accept loop that echoes any bytes the client writes for the
    /// lifetime of the connection — used by the per-connection-cert tests to
    /// prove that an old TLS connection still does real I/O after the
    /// server-side `ServerConfig` has been swapped for a different cert.
    async fn spawn_echo_acceptor(
        store: Arc<ArcSwap<ServerConfig>>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let acceptor = LiveTlsAcceptor::new(listener, store);
        let handle = tokio::spawn(async move {
            loop {
                match acceptor.accept().await {
                    Ok((mut tls, _peer)) => {
                        tokio::spawn(async move {
                            // Echo every chunk the client sends until EOF/error.
                            // The held-connection test relies on this to prove
                            // the original session is still alive after a
                            // server-side cert swap.
                            let mut buf = [0u8; 64];
                            loop {
                                match tls.read(&mut buf).await {
                                    Ok(0) | Err(_) => return,
                                    Ok(n) => {
                                        if tls.write_all(&buf[..n]).await.is_err() {
                                            return;
                                        }
                                        if tls.flush().await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        });
                    }
                    Err(_) => return,
                }
            }
        });
        (addr, handle)
    }

    /// Connect a TLS client to `addr` trusting `cert_pem`, with `sni` as the
    /// SNI server name.
    async fn connect_client(
        addr: SocketAddr,
        cert_pem: &[u8],
        sni: &str,
    ) -> tokio_rustls::client::TlsStream<TcpStream> {
        let tcp = TcpStream::connect(addr).await.expect("client connect");
        let cfg = Arc::new(client_config_trusting(cert_pem));
        let connector = TlsConnector::from(cfg);
        let server_name = ServerName::try_from(sni.to_string()).expect("server name");
        connector
            .connect(server_name, tcp)
            .await
            .expect("tls handshake")
    }

    #[tokio::test]
    async fn test_live_tls_acceptor_serves_initial_cert() {
        let (cert, key) = pem_pair_with_san("v1.test");
        let creds = TlsCredsRef::new(&cert, &key);
        let cfg = build_rustls_config(&creds).expect("rustls cfg");
        let store = Arc::new(ArcSwap::from_pointee(cfg));
        let (addr, handle) = spawn_echo_acceptor(Arc::clone(&store)).await;

        let mut client = connect_client(addr, &cert, "v1.test").await;
        client.write_all(b"ping").await.expect("write");
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.expect("read echo");
        assert_eq!(&buf, b"ping");

        let (_io, conn) = client.get_ref();
        let peer_certs = conn.peer_certificates().expect("peer certs");
        assert_eq!(peer_certs.len(), 1);
        assert!(
            cert_has_san(peer_certs[0].as_ref(), "v1.test"),
            "expected v1.test SAN in initial cert"
        );

        drop(client);
        handle.abort();
    }

    #[tokio::test]
    async fn test_live_tls_acceptor_swap_changes_cert_for_new_connection() {
        let (cert_v1, key_v1) = pem_pair_with_san("v1.test");
        let (cert_v2, key_v2) = pem_pair_with_san("v2.test");

        let cfg_v1 = build_rustls_config(&TlsCredsRef::new(&cert_v1, &key_v1)).expect("v1");
        let store = Arc::new(ArcSwap::from_pointee(cfg_v1));
        let (addr, handle) = spawn_echo_acceptor(Arc::clone(&store)).await;

        // First connection picks up v1.
        let mut a = connect_client(addr, &cert_v1, "v1.test").await;
        a.write_all(b"a").await.expect("write");
        let mut buf = [0u8; 1];
        a.read_exact(&mut buf).await.expect("read");
        let (_io, conn_a) = a.get_ref();
        let cert_a = conn_a.peer_certificates().expect("certs")[0].clone();
        assert!(
            cert_has_san(cert_a.as_ref(), "v1.test"),
            "expected v1.test SAN before swap"
        );

        // Swap to v2.
        let cfg_v2 = build_rustls_config(&TlsCredsRef::new(&cert_v2, &key_v2)).expect("v2");
        store.store(Arc::new(cfg_v2));
        // Brief yield so the swap is visible to the next accept.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Second connection picks up v2.
        let mut b = connect_client(addr, &cert_v2, "v2.test").await;
        b.write_all(b"b").await.expect("write");
        b.read_exact(&mut buf).await.expect("read");
        let (_io, conn_b) = b.get_ref();
        let cert_b = conn_b.peer_certificates().expect("certs")[0].clone();
        assert!(
            cert_has_san(cert_b.as_ref(), "v2.test"),
            "expected v2.test SAN after swap"
        );
        // And the post-swap cert must NOT advertise v1.test.
        assert!(
            !cert_has_san(cert_b.as_ref(), "v1.test"),
            "v2 cert should not advertise v1 SAN"
        );

        drop(a);
        drop(b);
        handle.abort();
    }

    #[tokio::test]
    async fn test_live_tls_acceptor_existing_connection_continues_on_old_cert() {
        let (cert_v1, key_v1) = pem_pair_with_san("v1.test");
        let (cert_v2, key_v2) = pem_pair_with_san("v2.test");

        let cfg_v1 = build_rustls_config(&TlsCredsRef::new(&cert_v1, &key_v1)).expect("v1");
        let store = Arc::new(ArcSwap::from_pointee(cfg_v1));
        let (addr, handle) = spawn_echo_acceptor(Arc::clone(&store)).await;

        // Open client A on v1, run the echo cycle to verify the connection works,
        // and HOLD it.
        let mut client_a = connect_client(addr, &cert_v1, "v1.test").await;
        client_a.write_all(b"hold").await.expect("write");
        let mut buf = [0u8; 4];
        client_a.read_exact(&mut buf).await.expect("read echo v1");
        assert_eq!(&buf, b"hold");
        let (_io, conn_a) = client_a.get_ref();
        let cert_a = conn_a.peer_certificates().expect("certs")[0].clone();
        assert!(
            cert_has_san(cert_a.as_ref(), "v1.test"),
            "expected v1.test SAN before swap"
        );

        // Server-side: swap to v2.
        let cfg_v2 = build_rustls_config(&TlsCredsRef::new(&cert_v2, &key_v2)).expect("v2");
        store.store(Arc::new(cfg_v2));

        // Open client B on v2 — proves the new cert is now in use.
        let mut client_b = connect_client(addr, &cert_v2, "v2.test").await;
        client_b.write_all(b"new!").await.expect("write");
        client_b.read_exact(&mut buf).await.expect("read echo v2");
        assert_eq!(&buf, b"new!");
        let (_io, conn_b) = client_b.get_ref();
        let cert_b = conn_b.peer_certificates().expect("certs")[0].clone();
        assert!(
            cert_has_san(cert_b.as_ref(), "v2.test"),
            "expected v2.test SAN on new connection"
        );

        // ── Liveness proof: do a SECOND request/response cycle through
        // ── client A *after* the swap and after client B has done I/O.
        // ── This is the real test: per-connection isolation means A's
        // ── original session is still alive on its negotiated v1 cert.
        let mut buf2 = [0u8; 5];
        client_a
            .write_all(b"alive")
            .await
            .expect("post-swap write through held v1 connection");
        client_a
            .read_exact(&mut buf2)
            .await
            .expect("post-swap read through held v1 connection");
        assert_eq!(
            &buf2, b"alive",
            "held v1 connection must still echo through after server-side swap"
        );

        // Cached cert metadata must still match v1 (the negotiated identity
        // never mutates inside a single session).
        let (_io, conn_a_after) = client_a.get_ref();
        let cert_a_after = conn_a_after.peer_certificates().expect("certs")[0].clone();
        assert!(
            cert_has_san(cert_a_after.as_ref(), "v1.test"),
            "held connection should still report v1.test"
        );
        assert!(
            !cert_has_san(cert_a_after.as_ref(), "v2.test"),
            "held connection should not advertise v2 SAN"
        );

        drop(client_a);
        drop(client_b);
        handle.abort();
    }
}
