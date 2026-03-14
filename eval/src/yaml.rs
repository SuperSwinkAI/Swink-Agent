//! YAML loading support for eval sets (requires `yaml` feature).

use std::fs;
use std::path::Path;

use crate::error::EvalError;
use crate::types::EvalSet;

/// Load an [`EvalSet`] from a YAML file.
///
/// All [`ResponseCriteria`](crate::ResponseCriteria) variants except `Custom`
/// are supported — `Custom` requires programmatic construction.
pub fn load_eval_set_yaml(path: &Path) -> Result<EvalSet, EvalError> {
    let contents = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&contents)?)
}
