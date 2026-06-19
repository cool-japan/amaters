//! OpenTelemetry distributed tracing initialisation for AmateRS.
//!
//! Call [`init_tracing`] once at startup (inside amaters-server's main).
//! The returned [`TelemetryGuard`] must be kept alive for the duration of
//! the process; dropping it flushes and shuts down the exporter pipeline.
//!
//! # Feature gate
//! This module is only compiled when the `telemetry` Cargo feature is enabled.
//! Without the feature, the crate uses the existing `tracing` spans (which
//! still flow into any `tracing_subscriber` the application installs).
//!
//! # Example
//! ```rust,ignore
//! let _guard = amaters_core::telemetry::init_tracing(TelemetryConfig::default())?;
//! ```

#[cfg(feature = "telemetry")]
pub use telemetry_impl::*;

#[cfg(feature = "telemetry")]
mod telemetry_impl {
    use std::time::Duration;

    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::{KeyValue, global};
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::{
        Resource,
        trace::{Sampler, SdkTracerProvider},
    };
    use opentelemetry_semantic_conventions::attribute;
    use tracing_opentelemetry::OpenTelemetryLayer;
    use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};

    use crate::error::{AmateRSError, ErrorContext, Result};

    // -------------------------------------------------------------------------
    // Configuration
    // -------------------------------------------------------------------------

    /// Configuration for the OpenTelemetry tracing pipeline.
    ///
    /// Controls the OTLP exporter endpoint, service identity recorded in the
    /// `Resource`, and the tail-based sampling ratio.
    ///
    /// Batch exporter tuning is driven by the standard OpenTelemetry environment
    /// variables (`OTEL_BSP_MAX_EXPORT_BATCH_SIZE`, `OTEL_BSP_SCHEDULE_DELAY`,
    /// etc.) so that operators can tune without recompiling.
    #[derive(Debug, Clone)]
    pub struct TelemetryConfig {
        /// gRPC endpoint for the OTLP exporter (e.g. `"http://localhost:4317"`)
        pub endpoint: String,
        /// Logical service name recorded in every span.
        pub service_name: String,
        /// Service version string (defaults to `CARGO_PKG_VERSION`).
        pub service_version: String,
        /// Fraction of traces to sample, in `[0.0, 1.0]`.
        ///
        /// `1.0` means 100% sampling (always sample); `0.0` means never sample.
        pub sample_ratio: f64,
        /// Maximum number of spans to send in a single export request.
        ///
        /// Maps to `OTEL_BSP_MAX_EXPORT_BATCH_SIZE` if not overridden by env.
        /// The SDK default is 512.
        pub max_export_batch_size: usize,
        /// Delay between two consecutive batch export attempts.
        ///
        /// Maps to `OTEL_BSP_SCHEDULE_DELAY` if not overridden by env.
        pub scheduled_delay: Duration,
    }

    impl Default for TelemetryConfig {
        fn default() -> Self {
            Self {
                endpoint: "http://localhost:4317".to_string(),
                service_name: "amaters".to_string(),
                service_version: env!("CARGO_PKG_VERSION").to_string(),
                sample_ratio: 1.0,
                max_export_batch_size: 512,
                scheduled_delay: Duration::from_secs(5),
            }
        }
    }

    // -------------------------------------------------------------------------
    // Guard
    // -------------------------------------------------------------------------

    /// RAII guard that holds the [`SdkTracerProvider`].
    ///
    /// When this guard is dropped the span exporter pipeline is flushed and
    /// shut down gracefully.  Keep the guard alive (e.g. bound to `_guard` in
    /// `main`) for the duration of the process.
    pub struct TelemetryGuard(SdkTracerProvider);

    impl Drop for TelemetryGuard {
        fn drop(&mut self) {
            if let Err(err) = self.0.shutdown() {
                tracing::warn!(
                    error = %err,
                    "OpenTelemetry tracer provider shutdown returned an error; \
                     some spans may not have been flushed"
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // Initialisation
    // -------------------------------------------------------------------------

    /// Initialise the OpenTelemetry + `tracing` pipeline.
    ///
    /// Sets up:
    /// 1. An OTLP gRPC span exporter pointed at `config.endpoint`.
    /// 2. A batching `SdkTracerProvider` with `SdkTracerProvider::builder()`.
    /// 3. A `tracing_subscriber` [`Registry`] layered with:
    ///    - [`EnvFilter`] defaulting to `"info"` (override via `RUST_LOG`).
    ///    - A pretty-printing `fmt` layer writing to `stderr`.
    ///    - The [`OpenTelemetryLayer`] that exports spans to the OTLP pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The OTLP exporter cannot be constructed (e.g. bad TLS config).
    /// - The `tracing` global subscriber is already set.
    ///
    /// # Notes on batch exporter eagerness
    ///
    /// The OTLP batch exporter does **not** fail immediately when the endpoint
    /// is unreachable; it queues spans and retries in the background.  This
    /// means `init_tracing` succeeds even when the collector is offline.
    pub fn init_tracing(config: TelemetryConfig) -> Result<TelemetryGuard> {
        // -- 1. Apply batch knobs via env vars (SDK honours these) ----------
        //       Only set if the user has not already set them.
        let batch_size_str = config.max_export_batch_size.to_string();
        let scheduled_delay_ms = config.scheduled_delay.as_millis().to_string();
        if std::env::var("OTEL_BSP_MAX_EXPORT_BATCH_SIZE").is_err() {
            // Safety: single-threaded init; the SDK reads this before spawning
            // the background task, so this races with nothing in practice.
            // We intentionally call set_var in a controlled init context.
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("OTEL_BSP_MAX_EXPORT_BATCH_SIZE", &batch_size_str);
            }
        }
        if std::env::var("OTEL_BSP_SCHEDULE_DELAY").is_err() {
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("OTEL_BSP_SCHEDULE_DELAY", &scheduled_delay_ms);
            }
        }

        // -- 2. Build the OTLP gRPC span exporter ---------------------------
        let exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&config.endpoint)
            .build()
            .map_err(|err| {
                AmateRSError::ConfigError(ErrorContext::new(format!(
                    "Failed to build OTLP span exporter targeting '{}': {}",
                    config.endpoint, err
                )))
            })?;

        // -- 3. Build the resource describing this service ------------------
        let resource = Resource::builder()
            .with_attribute(KeyValue::new(
                attribute::SERVICE_NAME,
                config.service_name.clone(),
            ))
            .with_attribute(KeyValue::new(
                attribute::SERVICE_VERSION,
                config.service_version.clone(),
            ))
            .build();

        // -- 4. Choose a sampler based on the configured ratio --------------
        let sampler = if (config.sample_ratio - 1.0_f64).abs() < f64::EPSILON {
            Sampler::AlwaysOn
        } else if config.sample_ratio <= 0.0 {
            Sampler::AlwaysOff
        } else {
            Sampler::TraceIdRatioBased(config.sample_ratio)
        };

        // -- 5. Build the SdkTracerProvider ---------------------------------
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .with_sampler(sampler)
            .build();

        // -- 6. Register the provider globally ------------------------------
        global::set_tracer_provider(provider.clone());
        let tracer = provider.tracer("amaters");

        // -- 7. Build and install the tracing_subscriber --------------------
        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        let otel_layer = OpenTelemetryLayer::new(tracer);

        Registry::default()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(otel_layer)
            .try_init()
            .map_err(|err| {
                AmateRSError::ConfigError(ErrorContext::new(format!(
                    "Failed to install global tracing subscriber: {}",
                    err
                )))
            })?;

        tracing::info!(
            service.name = %config.service_name,
            service.version = %config.service_version,
            endpoint = %config.endpoint,
            sample_ratio = %config.sample_ratio,
            "OpenTelemetry tracing pipeline initialised"
        );

        Ok(TelemetryGuard(provider))
    }

    // -------------------------------------------------------------------------
    // Shutdown helper
    // -------------------------------------------------------------------------

    /// Explicitly flush and shut down a tracer provider.
    ///
    /// Prefer relying on [`TelemetryGuard`]'s `Drop` impl instead, which
    /// calls `SdkTracerProvider::shutdown()` automatically when the guard
    /// goes out of scope.
    ///
    /// Note: `opentelemetry::global::shutdown_tracer_provider()` was removed
    /// in opentelemetry 0.32.  Shutdown must be performed on the concrete
    /// `SdkTracerProvider` instance held by [`TelemetryGuard`].
    pub fn shutdown_tracer(provider: SdkTracerProvider) {
        if let Err(err) = provider.shutdown() {
            tracing::warn!(
                error = %err,
                "OpenTelemetry tracer provider shutdown returned an error; \
                 some spans may not have been flushed"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_telemetry_config_defaults() {
            let cfg = TelemetryConfig::default();

            assert_eq!(cfg.endpoint, "http://localhost:4317");
            assert_eq!(cfg.service_name, "amaters");
            // service_version is CARGO_PKG_VERSION — just verify it's non-empty.
            assert!(
                !cfg.service_version.is_empty(),
                "service_version must not be empty"
            );
            // sample_ratio 1.0 == always sample.
            assert!(
                (cfg.sample_ratio - 1.0_f64).abs() < f64::EPSILON,
                "default sample_ratio should be 1.0, got {}",
                cfg.sample_ratio
            );
            assert_eq!(cfg.max_export_batch_size, 512);
            assert_eq!(cfg.scheduled_delay, Duration::from_secs(5));
        }

        #[test]
        fn test_telemetry_config_custom() {
            let cfg = TelemetryConfig {
                endpoint: "http://otel-collector:4317".to_string(),
                service_name: "amaters-test".to_string(),
                service_version: "99.0.0".to_string(),
                sample_ratio: 0.5,
                max_export_batch_size: 256,
                scheduled_delay: Duration::from_secs(10),
            };

            assert_eq!(cfg.endpoint, "http://otel-collector:4317");
            assert_eq!(cfg.service_name, "amaters-test");
            assert_eq!(cfg.service_version, "99.0.0");
            assert!((cfg.sample_ratio - 0.5_f64).abs() < f64::EPSILON);
            assert_eq!(cfg.max_export_batch_size, 256);
            assert_eq!(cfg.scheduled_delay, Duration::from_secs(10));
        }

        #[test]
        fn test_sampler_selection_logic() {
            // This tests the sampler selection logic embedded in init_tracing
            // by mirroring it here.
            let always_on_ratio = 1.0_f64;
            let always_off_ratio = 0.0_f64;
            let partial_ratio = 0.5_f64;

            let sampler_always_on = if (always_on_ratio - 1.0_f64).abs() < f64::EPSILON {
                "AlwaysOn"
            } else if always_on_ratio <= 0.0 {
                "AlwaysOff"
            } else {
                "TraceIdRatioBased"
            };
            assert_eq!(sampler_always_on, "AlwaysOn");

            let sampler_always_off = if (always_off_ratio - 1.0_f64).abs() < f64::EPSILON {
                "AlwaysOn"
            } else if always_off_ratio <= 0.0 {
                "AlwaysOff"
            } else {
                "TraceIdRatioBased"
            };
            assert_eq!(sampler_always_off, "AlwaysOff");

            let sampler_partial = if (partial_ratio - 1.0_f64).abs() < f64::EPSILON {
                "AlwaysOn"
            } else if partial_ratio <= 0.0 {
                "AlwaysOff"
            } else {
                "TraceIdRatioBased"
            };
            assert_eq!(sampler_partial, "TraceIdRatioBased");
        }

        /// Verify that init_tracing does not fail eagerly when the OTLP
        /// endpoint is unreachable.
        ///
        /// The OTLP batch exporter is designed to queue spans and export them
        /// asynchronously; it does not reject the pipeline construction if the
        /// collector is offline.  This test exercises that property by pointing
        /// at a localhost port that is not listening.
        ///
        /// Note: because installing a global tracing subscriber is a one-shot
        /// operation, this test is marked `#[ignore]` to avoid interfering with
        /// other tests in the same process.  Run it in isolation with:
        ///
        /// ```text
        /// cargo test -p amaters-core --features telemetry \
        ///     -- telemetry_impl::tests::test_init_tracing_without_exporter --ignored
        /// ```
        #[ignore = "installs a global subscriber — run in isolation"]
        #[tokio::test]
        async fn test_init_tracing_without_exporter() {
            let cfg = TelemetryConfig {
                endpoint: "http://127.0.0.1:1".to_string(), // port 1 — unreachable
                service_name: "amaters-test-isolated".to_string(),
                service_version: "0.0.0-test".to_string(),
                sample_ratio: 1.0,
                max_export_batch_size: 512,
                scheduled_delay: Duration::from_secs(5),
            };

            // init_tracing must succeed even when the endpoint is unreachable.
            let guard = init_tracing(cfg)
                .expect("init_tracing must not fail when the OTLP endpoint is unreachable");

            // Emit a test span to exercise the exporter code path.
            tracing::info!(span_kind = "test", "telemetry init_tracing smoke test");

            // Explicitly drop the guard so the pipeline is shut down and any
            // pending background tasks are stopped before the test returns.
            drop(guard);
        }
    }
}
