//! Benchmark stub server — minimal in-memory HashMap server for benchmarks only.
//!
//! Does NOT implement FHE, auth, or streaming. Not suitable for correctness
//! tests. Used exclusively by `client_bench.rs` to measure raw SDK throughput
//! against a local in-process gRPC endpoint.
//!
//! The stub is backed by the existing `amaters_net` gRPC service layer wired to
//! an `amaters_core::storage::MemoryStorage` backend. This gives us a real
//! tonic server without reimplementing the proto trait by hand.

use amaters_core::storage::MemoryStorage;
use amaters_net::server::AqlServerBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Minimal in-process stub server for benchmarking.
///
/// Binds to an OS-assigned port. Drop the handle to stop the server.
pub struct StubServer {
    /// Bound socket address (usable immediately after `start()` returns).
    addr: SocketAddr,
    /// Background Tokio task handle — dropped when `StubServer` is dropped.
    _task: tokio::task::JoinHandle<()>,
}

impl StubServer {
    /// Start the stub server on a random OS-assigned port.
    ///
    /// Returns the bound [`SocketAddr`] so the caller can build an SDK client.
    ///
    /// # Errors
    ///
    /// Returns an error if binding the OS port or spawning the server task fails.
    pub async fn start() -> anyhow::Result<Self> {
        // Bind to `127.0.0.1:0` to get an OS-assigned ephemeral port.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // Build the gRPC service backed by an in-memory storage engine.
        let storage = Arc::new(MemoryStorage::new());
        let grpc_service = AqlServerBuilder::new(storage).build_grpc_service();

        // Convert the Tokio listener into a stream that tonic can use.
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        let task = tokio::spawn(async move {
            let result = tonic::transport::Server::builder()
                .add_service(grpc_service)
                .serve_with_incoming(incoming)
                .await;

            if let Err(e) = result {
                // Bench server shutting down on drop is expected; only log real errors.
                if !e.to_string().contains("accept") {
                    eprintln!("[stub_server] tonic serve error: {e}");
                }
            }
        });

        Ok(Self { addr, _task: task })
    }

    /// Returns the address the stub server is listening on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns a `http://` URI string suitable for `AmateRSClient::connect`.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }
}
