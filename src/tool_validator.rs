//! Custom validation hook invoked before tool execution.
//!
//! Runs after JSON Schema validation passes. Allows application-specific
//! constraints (e.g., file path allowlists, argument sanitization).

use serde_json::Value;

/// Custom validation hook invoked before tool execution.
///
/// Runs after JSON Schema validation passes. Allows application-specific
/// constraints (e.g., file path allowlists, argument sanitization).
pub trait ToolValidator: Send + Sync {
    /// Validate tool arguments. Return `Ok(())` to proceed, `Err(message)` to reject.
    fn validate(&self, tool_name: &str, arguments: &Value) -> Result<(), String>;
}

/// Blanket impl for closures.
impl<F: Fn(&str, &Value) -> Result<(), String> + Send + Sync> ToolValidator for F {
    fn validate(&self, tool_name: &str, arguments: &Value) -> Result<(), String> {
        self(tool_name, arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closure_as_validator() {
        let validator = |_name: &str, _args: &Value| -> Result<(), String> { Ok(()) };
        assert!(validator.validate("test", &json!({})).is_ok());
    }

    #[test]
    fn validator_rejects_invalid() {
        let validator = |name: &str, _args: &Value| -> Result<(), String> {
            if name == "dangerous" {
                Err("tool not allowed".to_string())
            } else {
                Ok(())
            }
        };
        assert!(validator.validate("safe", &json!({})).is_ok());
        assert!(validator.validate("dangerous", &json!({})).is_err());
    }
}
