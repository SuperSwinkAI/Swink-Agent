//! Tests for `otel`.
#![cfg(test)]

use super::*;

#[test]
fn otel_init_config_defaults() {
    let config = OtelInitConfig::default();
    assert_eq!(config.service_name, "swink-agent");
    assert!(config.endpoint.is_none());
}
