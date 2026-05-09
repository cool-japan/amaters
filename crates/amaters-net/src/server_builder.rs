//! Builder for [`crate::server::AqlServiceImpl`].
//!
//! Extracted from `server.rs` to keep that module under the workspace's
//! per-file size policy.  All fields are private; configuration flows
//! through fluent `with_*` methods (every `with_*` returns `Self` so calls
//! chain).  Read-back accessors (`logging_verbosity()`, `metrics_addr()`,
//! …) exist for [`crate::config::NetConfig::apply_to`] and tests.

use std::sync::Arc;

use amaters_core::traits::StorageEngine;

use crate::error::NetResult;
use crate::server::AqlServiceImpl;

/// Server builder for creating AQL service instances.
///
/// Configuration is layered via fluent setters.  Defaults match
/// `AqlServiceImpl::new(storage)` exactly; nothing is spawned until
/// [`Self::build`] runs.
pub struct AqlServerBuilder<S: StorageEngine> {
    storage: Arc<S>,
    /// Optional logging verbosity for the `LoggingLayer`.
    logging_verbosity: Option<crate::logging_layer::LogVerbosity>,
    /// Optional slow-request threshold (ms) for the `LoggingLayer`.
    slow_threshold_ms: Option<u64>,
    /// Optional address for the Prometheus metrics HTTP server.
    metrics_addr: Option<std::net::SocketAddr>,
    /// Optional gRPC bind address.  Recorded only — actual bind happens in
    /// the caller's tonic `Server::bind` (out of this crate's scope).
    bind_addr: Option<std::net::SocketAddr>,
    /// Optional rate-limit QPS for callers that wire a `RateLimiter`.
    rate_limit_qps: Option<f64>,
    /// Optional path to a JWT secret for bearer-token auth.
    jwt_secret_path: Option<std::path::PathBuf>,
    /// Optional store of swappable rustls server config.  When `Some`, callers
    /// build a [`crate::tls_acceptor::LiveTlsAcceptor`] from this store and
    /// hand the resulting stream to tonic via `serve_with_incoming`.
    #[cfg(feature = "mtls")]
    tls_config_store: Option<Arc<arc_swap::ArcSwap<rustls::ServerConfig>>>,
    /// Shared metrics registry — exposed so callers can wire `MetricsLayer`.
    metrics: Arc<crate::metrics_layer::NetMetrics>,
}

impl<S: StorageEngine + Send + Sync + 'static> AqlServerBuilder<S> {
    /// Create a new server builder with the given storage engine.
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            logging_verbosity: None,
            slow_threshold_ms: None,
            metrics_addr: None,
            bind_addr: None,
            rate_limit_qps: None,
            jwt_secret_path: None,
            #[cfg(feature = "mtls")]
            tls_config_store: None,
            metrics: crate::metrics_layer::NetMetrics::new(),
        }
    }

    /// Configure request/response logging verbosity.
    ///
    /// Returns `self` for chaining.  The stored verbosity can be retrieved via
    /// [`Self::logging_verbosity`] so callers can apply a [`LoggingLayer`]
    /// around the tonic service.
    ///
    /// [`LoggingLayer`]: crate::logging_layer::LoggingLayer
    pub fn with_logging(mut self, verbosity: crate::logging_layer::LogVerbosity) -> Self {
        self.logging_verbosity = Some(verbosity);
        self
    }

    /// Return the configured logging verbosity (if any).
    pub fn logging_verbosity(&self) -> Option<crate::logging_layer::LogVerbosity> {
        self.logging_verbosity
    }

    /// Configure the slow-request threshold (ms) for the `LoggingLayer`.
    pub fn with_slow_threshold_ms(mut self, ms: u64) -> Self {
        self.slow_threshold_ms = Some(ms);
        self
    }

    /// Return the configured slow-request threshold (if any).
    pub fn slow_threshold_ms(&self) -> Option<u64> {
        self.slow_threshold_ms
    }

    /// Set the gRPC server bind address.  Recorded for callers that wire a
    /// tonic `Server::bind`; this builder does not itself spawn a tonic
    /// server.
    pub fn with_bind_addr(mut self, addr: std::net::SocketAddr) -> Self {
        self.bind_addr = Some(addr);
        self
    }

    /// Return the configured gRPC bind address (if any).
    pub fn bind_addr(&self) -> Option<std::net::SocketAddr> {
        self.bind_addr
    }

    /// Configure the steady-state QPS for the rate limiter.  Recorded for
    /// callers that wire a [`crate::rate_limiter::RateLimiter`].
    pub fn with_rate_limit_qps(mut self, qps: f64) -> Self {
        self.rate_limit_qps = Some(qps);
        self
    }

    /// Return the configured rate-limit QPS (if any).
    pub fn rate_limit_qps(&self) -> Option<f64> {
        self.rate_limit_qps
    }

    /// Configure the JWT secret path used by the bearer-token auth middleware.
    pub fn with_jwt_secret_path(mut self, path: std::path::PathBuf) -> Self {
        self.jwt_secret_path = Some(path);
        self
    }

    /// Return the configured JWT secret path (if any).
    pub fn jwt_secret_path(&self) -> Option<&std::path::Path> {
        self.jwt_secret_path.as_deref()
    }

    /// Install initial TLS credentials for live cert rotation.
    ///
    /// Builds a `rustls::ServerConfig` from `creds`, wraps it in an
    /// [`arc_swap::ArcSwap`], and stores the handle on the builder.  Callers
    /// retrieve the store via [`Self::tls_config_store`] and pass it to a
    /// [`crate::tls_acceptor::LiveTlsAcceptor`] so each new TLS handshake
    /// reads the latest cert.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::NetError::TlsError`] if the credentials cannot
    /// be parsed into a `rustls::ServerConfig`.
    #[cfg(feature = "mtls")]
    pub fn with_tls_creds(
        mut self,
        creds: &crate::tls_acceptor::TlsCredsRef<'_>,
    ) -> NetResult<Self> {
        let config = crate::tls_acceptor::build_rustls_config(creds)?;
        self.tls_config_store = Some(Arc::new(arc_swap::ArcSwap::from_pointee(config)));
        Ok(self)
    }

    /// Return a clone of the current TLS config store (if installed).
    ///
    /// Callers feed this into [`crate::tls_acceptor::LiveTlsAcceptor::new`]
    /// to enable per-connection cert pickup; the same store can later be
    /// updated atomically via `store.store(Arc::new(new_config))`.
    #[cfg(feature = "mtls")]
    pub fn tls_config_store(&self) -> Option<Arc<arc_swap::ArcSwap<rustls::ServerConfig>>> {
        self.tls_config_store.as_ref().map(Arc::clone)
    }

    /// Set the `SocketAddr` on which the Prometheus metrics HTTP server will
    /// listen.  When set, [`Self::build`] spawns a background task serving
    /// `GET /metrics`.
    ///
    /// The metrics server runs on a separate port from gRPC so that scrape
    /// traffic never reaches the tonic transport.
    pub fn with_metrics_addr(mut self, addr: std::net::SocketAddr) -> Self {
        self.metrics_addr = Some(addr);
        self
    }

    /// Return the configured metrics HTTP address (if any).
    pub fn metrics_addr(&self) -> Option<std::net::SocketAddr> {
        self.metrics_addr
    }

    /// Return a clone of the shared [`crate::metrics_layer::NetMetrics`]
    /// registry.
    ///
    /// Use this to apply [`crate::metrics_layer::MetricsLayer`] around the
    /// tonic service so that gRPC request metrics flow into the same registry
    /// that the HTTP endpoint serves.
    pub fn metrics(&self) -> Arc<crate::metrics_layer::NetMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Build the service implementation.
    ///
    /// If [`Self::with_metrics_addr`] was called, also spawns the Prometheus
    /// HTTP server as a background tokio task.  The returned handle is
    /// discarded here; the task runs until the process exits or the tokio
    /// runtime shuts down.
    pub fn build(self) -> AqlServiceImpl<S> {
        if let Some(addr) = self.metrics_addr {
            crate::metrics_layer::spawn_metrics_server(addr, Arc::clone(&self.metrics));
        }
        AqlServiceImpl::new(self.storage)
    }

    /// Build a tonic-ready gRPC service (wrapped in `AqlServiceServer`).
    ///
    /// When the `compression` feature is enabled the server is configured to
    /// accept and send gzip-compressed messages.
    pub fn build_grpc_service(
        self,
    ) -> crate::proto::aql::aql_service_server::AqlServiceServer<
        crate::grpc_service::AqlGrpcService<S>,
    > {
        use crate::grpc_service::AqlGrpcService;
        use crate::proto::aql::aql_service_server::AqlServiceServer;

        let service_impl = Arc::new(AqlServiceImpl::new(self.storage));
        let grpc_service = AqlGrpcService::new(service_impl);

        #[allow(unused_mut)]
        let mut server = AqlServiceServer::new(grpc_service);

        #[cfg(feature = "compression")]
        {
            server = server
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
                .send_compressed(tonic::codec::CompressionEncoding::Gzip);
        }

        server
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaters_core::storage::MemoryStorage;

    /// `build_grpc_service` compiles and produces a server regardless of the
    /// `compression` feature.
    #[tokio::test]
    async fn test_build_grpc_service_compression_feature_gate() {
        let storage = Arc::new(MemoryStorage::new());
        let builder = AqlServerBuilder::new(storage);
        let _server = builder.build_grpc_service();
    }
}
