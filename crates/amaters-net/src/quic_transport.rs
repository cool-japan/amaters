//! QUIC transport layer using quinn 0.11
//!
//! This module provides a QUIC-based transport using the quinn crate,
//! with rustls for TLS 1.3 and rcgen for self-signed certificate generation.

use crate::error::{NetError, NetResult};
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::sync::Arc;

/// Configuration for a QUIC server endpoint
pub struct QuicServerConfig {
    /// Address to bind the server to
    pub bind_addr: SocketAddr,
    /// DER-encoded certificate
    pub cert_der: Vec<u8>,
    /// DER-encoded private key
    pub key_der: Vec<u8>,
    /// Maximum number of concurrent bidirectional streams per connection
    pub max_concurrent_bidi_streams: u32,
    /// Maximum number of concurrent unidirectional streams per connection
    pub max_concurrent_uni_streams: u32,
    /// Whether to enable session tickets for 0-RTT resumption
    pub session_tickets: bool,
}

impl Default for QuicServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().expect("valid addr"),
            cert_der: Vec::new(),
            key_der: Vec::new(),
            max_concurrent_bidi_streams: 100,
            max_concurrent_uni_streams: 0,
            session_tickets: true,
        }
    }
}

/// A QUIC server endpoint that accepts incoming connections
pub struct QuicServer {
    endpoint: Endpoint,
    #[allow(dead_code)]
    config: QuicServerConfig,
}

impl QuicServer {
    /// Create a new QUIC server with the given configuration
    pub fn new(config: QuicServerConfig) -> NetResult<Self> {
        let cert = CertificateDer::from(config.cert_der.clone());
        let key = PrivateKeyDer::try_from(config.key_der.clone())
            .map_err(|e| NetError::ServerInternal(format!("Invalid private key: {}", e)))?;

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls_server_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| NetError::ServerInternal(format!("TLS protocol version error: {}", e)))?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(|e| NetError::ServerInternal(format!("TLS config error: {}", e)))?;

        // Set ALPN protocol identifiers: "amaters" for direct AQL-over-QUIC transport.
        // Clients must negotiate the same ALPN to complete the handshake.
        tls_server_config.alpn_protocols = vec![b"amaters".to_vec()];

        let quinn_server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls_server_config).map_err(|e| {
                NetError::ServerInternal(format!("QUIC server config error: {}", e))
            })?,
        ));

        let endpoint =
            quinn::Endpoint::server(quinn_server_config, config.bind_addr).map_err(|e| {
                NetError::ServerInternal(format!("Failed to create server endpoint: {}", e))
            })?;

        Ok(Self { endpoint, config })
    }

    /// Accept the next incoming QUIC connection
    pub async fn accept_connection(&self) -> NetResult<Connection> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| NetError::ServerInternal("Server endpoint closed".to_string()))?;
        let connection = incoming
            .await
            .map_err(|e| NetError::ServerInternal(format!("Connection handshake error: {}", e)))?;
        Ok(connection)
    }

    /// Return the local socket address the server is bound to
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }
}

/// Configuration for a QUIC client
pub struct QuicClientConfig {
    /// Remote server address to connect to
    pub server_addr: SocketAddr,
    /// Server name for TLS SNI and certificate verification
    pub server_name: String,
    /// DER-encoded CA certificate for server verification.
    /// If `None`, certificate verification is skipped (useful for tests with self-signed certs).
    pub ca_cert_der: Option<Vec<u8>>,
}

/// A QUIC client that connects to a remote server
pub struct QuicClient {
    endpoint: Endpoint,
    connection: Option<Connection>,
    config: QuicClientConfig,
}

/// Custom TLS certificate verifier that skips verification.
///
/// Only intended for use in tests with self-signed certificates.
#[derive(Debug)]
struct SkipVerification(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for SkipVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl QuicClient {
    /// Create a new QUIC client with the given configuration
    pub fn new(config: QuicClientConfig) -> NetResult<Self> {
        let mut tls_client_config = if let Some(ref ca_cert_der) = config.ca_cert_der {
            // Use provided CA certificate for verification
            let mut root_store = rustls::RootCertStore::empty();
            root_store
                .add(CertificateDer::from(ca_cert_der.clone()))
                .map_err(|e| NetError::ServerInternal(format!("Invalid CA certificate: {}", e)))?;
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            rustls::ClientConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| {
                    NetError::ServerInternal(format!("TLS protocol version error: {}", e))
                })?
                .with_root_certificates(root_store)
                .with_no_client_auth()
        } else {
            // Skip certificate verification (for self-signed certs in tests)
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            rustls::ClientConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| {
                    NetError::ServerInternal(format!("TLS protocol version error: {}", e))
                })?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(SkipVerification(provider)))
                .with_no_client_auth()
        };

        // Match server ALPN negotiation
        tls_client_config.alpn_protocols = vec![b"amaters".to_vec()];

        let quinn_client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls_client_config).map_err(|e| {
                NetError::ServerInternal(format!("QUIC client config error: {}", e))
            })?,
        ));

        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().expect("valid bind addr"))
            .map_err(|e| {
            NetError::ServerInternal(format!("Failed to create client endpoint: {}", e))
        })?;
        endpoint.set_default_client_config(quinn_client_config);

        Ok(Self {
            endpoint,
            connection: None,
            config,
        })
    }

    /// Connect to the remote server
    pub async fn connect(&mut self) -> NetResult<&Connection> {
        let connecting = self
            .endpoint
            .connect(self.config.server_addr, &self.config.server_name)
            .map_err(|e| {
                NetError::ServerInternal(format!("Failed to initiate connection: {}", e))
            })?;

        let connection = connecting
            .await
            .map_err(|e| NetError::ServerInternal(format!("Connection failed: {}", e)))?;

        self.connection = Some(connection);
        Ok(self.connection.as_ref().expect("just set"))
    }

    /// Open a new bidirectional stream on the current connection
    pub async fn open_bidi_stream(&self) -> NetResult<(SendStream, RecvStream)> {
        let conn = self.connection.as_ref().ok_or_else(|| {
            NetError::ServerInternal("Not connected — call connect() first".to_string())
        })?;
        conn.open_bi()
            .await
            .map_err(|e| NetError::ServerInternal(format!("Failed to open bidi stream: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::generate_simple_self_signed;

    fn make_test_cert() -> (Vec<u8>, Vec<u8>) {
        let certified_key =
            generate_simple_self_signed(vec!["localhost".to_string()]).expect("cert gen failed");
        let cert_der = certified_key.cert.der().to_vec();
        let key_der = certified_key.signing_key.serialize_der();
        (cert_der, key_der)
    }

    #[tokio::test]
    async fn test_quic_server_start() {
        let (cert_der, key_der) = make_test_cert();
        let config = QuicServerConfig {
            bind_addr: "127.0.0.1:0".parse().expect("valid loopback addr"),
            cert_der,
            key_der,
            ..Default::default()
        };
        let server = QuicServer::new(config).expect("server creation failed");
        let addr = server.local_addr().expect("local_addr failed");
        assert_ne!(addr.port(), 0);
    }

    #[tokio::test]
    async fn test_quic_client_connect() {
        let (cert_der, key_der) = make_test_cert();
        let server_config = QuicServerConfig {
            bind_addr: "127.0.0.1:0".parse().expect("valid loopback addr"),
            cert_der: cert_der.clone(),
            key_der,
            ..Default::default()
        };
        let server = QuicServer::new(server_config).expect("server creation failed");
        let server_addr = server.local_addr().expect("local_addr failed");

        let accept_task = tokio::spawn(async move { server.accept_connection().await });

        let mut client = QuicClient::new(QuicClientConfig {
            server_addr,
            server_name: "localhost".to_string(),
            ca_cert_der: None, // skip verification in test
        })
        .expect("client creation failed");

        client.connect().await.expect("connect failed");
        accept_task
            .await
            .expect("join failed")
            .expect("accept failed");
    }

    #[tokio::test]
    async fn test_quic_bidi_stream() {
        let (cert_der, key_der) = make_test_cert();
        let server_config = QuicServerConfig {
            bind_addr: "127.0.0.1:0".parse().expect("valid loopback addr"),
            cert_der: cert_der.clone(),
            key_der,
            ..Default::default()
        };
        let server = QuicServer::new(server_config).expect("server creation");
        let server_addr = server.local_addr().expect("local_addr");

        let server_task = tokio::spawn(async move {
            let conn = server.accept_connection().await.expect("accept");
            let (mut send, mut recv) = conn.accept_bi().await.expect("accept_bi");
            // Read all client data (client sends 10 bytes then finishes)
            let mut buf = vec![0u8; 1024];
            let mut total = 0usize;
            while let Some(n) = recv.read(&mut buf[total..]).await.expect("read") {
                total += n;
            }
            // Echo back exactly what we received
            send.write_all(&buf[..total]).await.expect("echo");
            send.finish().expect("finish");
            // Flush: wait until the send stream is fully acknowledged by the peer
            // before the server connection drops (which would abort unacknowledged data).
            tokio::time::timeout(std::time::Duration::from_secs(5), conn.closed())
                .await
                .ok(); // ignore timeout — client closed first or we timed out
        });

        let mut client = QuicClient::new(QuicClientConfig {
            server_addr,
            server_name: "localhost".to_string(),
            ca_cert_der: None,
        })
        .expect("client");
        client.connect().await.expect("connect");

        let (mut send, mut recv) = client.open_bidi_stream().await.expect("open_bidi");
        let msg = b"hello quic";
        send.write_all(msg).await.expect("write");
        send.finish().expect("finish");

        // Read the echo response
        let mut buf = vec![0u8; 1024];
        let mut total = 0usize;
        while let Some(n) = recv.read(&mut buf[total..]).await.expect("read") {
            total += n;
        }
        assert_eq!(&buf[..total], msg);

        server_task.await.expect("server task");
    }
}
