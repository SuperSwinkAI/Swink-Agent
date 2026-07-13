//! Feature-gated OpenTelemetry integration.
//!
//! When the `otel` feature is enabled, this module provides a convenience
//! function to initialize a `tracing` layer that bridges spans to an
//! OpenTelemetry OTLP exporter. The agent loop already emits `tracing` spans
//! (`agent.run`, `agent.turn`, `agent.llm_call`, `agent.tool`), so enabling
//! this layer is all that's needed to export them to an `OTel`-compatible backend.

use std::error::Error;
use std::fmt;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::Layer;

/// Error returned when [`init_otel_layer`] fails to build the OTLP exporter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelInitError {
    message: String,
}

impl OtelInitError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for OtelInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for OtelInitError {}

/// Configuration for the convenience `OTel` initialization helper.
#[derive(Debug, Clone)]
pub struct OtelInitConfig {
    /// Service name reported to the `OTel` backend.
    pub service_name: String,
    /// OTLP gRPC endpoint. Defaults to `"http://localhost:4317"`.
    pub endpoint: Option<String>,
}

impl Default for OtelInitConfig {
    fn default() -> Self {
        Self {
            service_name: "swink-agent".to_string(),
            endpoint: None,
        }
    }
}

/// Initialize a `tracing` [`Layer`] that exports spans to an OTLP gRPC
/// endpoint via `tracing-opentelemetry`.
///
/// Compose the returned layer into a `tracing_subscriber::Registry`:
///
/// ```ignore
/// use tracing_subscriber::prelude::*;
/// use swink_agent::otel::{OtelInitConfig, init_otel_layer};
///
/// let otel_layer = init_otel_layer(OtelInitConfig::default()).expect("otel init");
/// tracing_subscriber::registry()
///     .with(otel_layer)
///     .init();
/// ```
///
/// # Errors
///
/// Returns [`OtelInitError`] if the OTLP gRPC exporter fails to build (e.g.
/// an invalid endpoint configuration).
pub fn init_otel_layer<S>(config: OtelInitConfig) -> Result<impl Layer<S>, OtelInitError>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    let endpoint = config
        .endpoint
        .unwrap_or_else(|| "http://localhost:4317".to_string());

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|err| OtelInitError::new(format!("failed to build OTLP exporter: {err}")))?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name(config.service_name)
                .build(),
        )
        .build();

    let tracer = provider.tracer("swink-agent");

    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

// ─── Send + Sync assertion ──────────────────────────────────────────────────

const fn _assert_send_sync() {
    const fn assert<T: Send + Sync>() {}
    assert::<OtelInitConfig>();
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otel_init_config_defaults() {
        let config = OtelInitConfig::default();
        assert_eq!(config.service_name, "swink-agent");
        assert!(config.endpoint.is_none());
    }
}
