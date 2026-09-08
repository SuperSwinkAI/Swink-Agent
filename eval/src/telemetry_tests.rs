//! Tests for `telemetry`.
#![cfg(test)]

use super::*;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

fn fresh_provider() -> (SdkTracerProvider, InMemorySpanExporter) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    (provider, exporter)
}

#[test]
fn builder_uses_injected_tracer() {
    let (provider, exporter) = fresh_provider();
    let telemetry = EvalsTelemetry::builder()
        .with_tracer_provider(&provider)
        .build();
    // Emit a span via the configured tracer to confirm it flows through.
    let mut span = telemetry.tracer().start("selftest");
    span.end();
    provider.force_flush().expect("flush ok");
    let spans = exporter.get_finished_spans().expect("get spans");
    assert!(spans.iter().any(|s| s.name == "selftest"));
}

#[test]
fn verdict_str_rendering() {
    assert_eq!(verdict_str(Verdict::Pass), "pass");
    assert_eq!(verdict_str(Verdict::Fail), "fail");
}
