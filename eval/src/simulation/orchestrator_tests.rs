//! Tests for `orchestrator`.
#![cfg(test)]

use super::*;

#[test]
fn simulation_error_wraps_schema_variant() {
    let err: SimulationError = ToolSimulationError::SchemaValidation("boom".into()).into();
    assert!(matches!(err, SimulationError::SchemaValidation(_)));
}
