//! Persistence for eval sets and results.
//!
//! Provides the [`EvalStore`] trait and a filesystem-backed implementation
//! ([`FsEvalStore`]) that stores data as JSON files.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::EvalError;
use crate::types::{EvalSet, EvalSetResult};

/// Persistence interface for eval sets and results.
///
/// Implementations handle storage and retrieval of eval definitions and
/// scored results, enabling historical comparison and trending.
pub trait EvalStore: Send + Sync {
    /// Save an eval set definition.
    fn save_set(&self, set: &EvalSet) -> Result<(), EvalError>;

    /// Load an eval set by ID.
    fn load_set(&self, id: &str) -> Result<EvalSet, EvalError>;

    /// Save an eval set result.
    fn save_result(&self, result: &EvalSetResult) -> Result<(), EvalError>;

    /// Load a specific result by eval set ID and timestamp.
    fn load_result(&self, eval_set_id: &str, timestamp: u64) -> Result<EvalSetResult, EvalError>;

    /// List all result timestamps for an eval set, sorted ascending.
    fn list_results(&self, eval_set_id: &str) -> Result<Vec<u64>, EvalError>;
}

/// Filesystem-backed eval store using JSON files.
///
/// Directory layout:
/// ```text
/// {dir}/sets/{id}.json
/// {dir}/results/{eval_set_id}/{timestamp}.json
/// ```
pub struct FsEvalStore {
    dir: PathBuf,
}

impl FsEvalStore {
    /// Create a new store rooted at the given directory.
    ///
    /// The directory and subdirectories are created on first write.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn sets_dir(&self) -> PathBuf {
        self.dir.join("sets")
    }

    fn results_dir(&self, eval_set_id: &str) -> PathBuf {
        self.dir.join("results").join(eval_set_id)
    }

    fn set_path(&self, id: &str) -> PathBuf {
        self.sets_dir().join(format!("{id}.json"))
    }

    #[cfg(feature = "yaml")]
    fn set_path_yaml(&self, id: &str) -> PathBuf {
        self.sets_dir().join(format!("{id}.yaml"))
    }

    #[cfg(feature = "yaml")]
    fn set_path_yml(&self, id: &str) -> PathBuf {
        self.sets_dir().join(format!("{id}.yml"))
    }

    fn result_path(&self, eval_set_id: &str, timestamp: u64) -> PathBuf {
        self.results_dir(eval_set_id)
            .join(format!("{timestamp}.json"))
    }

    fn ensure_dir(path: &Path) -> Result<(), EvalError> {
        if !path.exists() {
            fs::create_dir_all(path)?;
        }
        Ok(())
    }
}

impl EvalStore for FsEvalStore {
    fn save_set(&self, set: &EvalSet) -> Result<(), EvalError> {
        Self::ensure_dir(&self.sets_dir())?;
        let json = serde_json::to_string_pretty(set)?;
        fs::write(self.set_path(&set.id), json)?;
        Ok(())
    }

    fn load_set(&self, id: &str) -> Result<EvalSet, EvalError> {
        #[cfg(feature = "yaml")]
        {
            for path in [self.set_path_yaml(id), self.set_path_yml(id)] {
                if path.exists() {
                    let contents = fs::read_to_string(path)?;
                    return Ok(serde_yaml::from_str(&contents)?);
                }
            }
        }

        let path = self.set_path(id);
        if !path.exists() {
            return Err(EvalError::SetNotFound { id: id.to_string() });
        }
        let json = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }

    fn save_result(&self, result: &EvalSetResult) -> Result<(), EvalError> {
        Self::ensure_dir(&self.results_dir(&result.eval_set_id))?;
        let json = serde_json::to_string_pretty(result)?;
        fs::write(
            self.result_path(&result.eval_set_id, result.timestamp),
            json,
        )?;
        Ok(())
    }

    fn load_result(&self, eval_set_id: &str, timestamp: u64) -> Result<EvalSetResult, EvalError> {
        let path = self.result_path(eval_set_id, timestamp);
        if !path.exists() {
            return Err(EvalError::SetNotFound {
                id: format!("{eval_set_id}/{timestamp}"),
            });
        }
        let json = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }

    fn list_results(&self, eval_set_id: &str) -> Result<Vec<u64>, EvalError> {
        let dir = self.results_dir(eval_set_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut timestamps: Vec<u64> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .collect();

        timestamps.sort_unstable();
        Ok(timestamps)
    }
}
