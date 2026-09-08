//! YAML loading support for eval sets (requires `yaml` feature).

use std::fs;
use std::path::Path;

use crate::error::EvalError;
use crate::types::{EvalSet, validate_eval_set};

/// Load an [`EvalSet`] from a YAML file.
///
/// All [`ResponseCriteria`](crate::ResponseCriteria) variants except `Custom`
/// are supported — `Custom` requires programmatic construction.
///
/// Case-load validation (FR-015): duplicate names within an individual case's
/// `expected_environment_state` list are rejected with
/// [`EvalError::InvalidCase`] pointing at the offending name.
pub fn load_eval_set_yaml(path: &Path) -> Result<EvalSet, EvalError> {
    let contents = fs::read_to_string(path)?;
    let set: EvalSet = serde_yaml_ng::from_str(&contents)?;
    validate_eval_set(&set)?;
    Ok(set)
}

#[cfg(test)]
#[path = "yaml_tests.rs"]
mod tests;
