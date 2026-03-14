use std::sync::Arc;

use swink_agent::{ModelConnection, ProviderKind, model_catalog};
use thiserror::Error;

use crate::{LocalModel, LocalStreamFn, ModelConfig};

pub const DEFAULT_LOCAL_PRESET_ID: &str = "smollm3_3b";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LocalPresetError {
    #[error("Missing local default preset local.{preset_id} in the model catalog")]
    MissingDefaultPreset { preset_id: &'static str },
    #[error("local.{preset_id} is not a local preset")]
    NotLocalPreset { preset_id: &'static str },
    #[error("local.{preset_id} is missing repo_id in the model catalog")]
    MissingRepoId { preset_id: &'static str },
    #[error("local.{preset_id} is missing filename in the model catalog")]
    MissingFilename { preset_id: &'static str },
}

pub fn default_local_connection() -> Result<ModelConnection, LocalPresetError> {
    let preset = model_catalog()
        .preset("local", DEFAULT_LOCAL_PRESET_ID)
        .ok_or(LocalPresetError::MissingDefaultPreset {
            preset_id: DEFAULT_LOCAL_PRESET_ID,
        })?;
    if preset.provider_kind != ProviderKind::Local {
        return Err(LocalPresetError::NotLocalPreset {
            preset_id: DEFAULT_LOCAL_PRESET_ID,
        });
    }

    let model_spec = preset.model_spec();
    let repo_id = preset.repo_id.ok_or(LocalPresetError::MissingRepoId {
        preset_id: DEFAULT_LOCAL_PRESET_ID,
    })?;
    let filename = preset.filename.ok_or(LocalPresetError::MissingFilename {
        preset_id: DEFAULT_LOCAL_PRESET_ID,
    })?;

    let model = LocalModel::new(ModelConfig {
        repo_id: std::env::var("LOCAL_MODEL_REPO").unwrap_or(repo_id),
        filename: std::env::var("LOCAL_MODEL_FILE").unwrap_or(filename),
        ..ModelConfig::default()
    });
    Ok(ModelConnection::new(
        model_spec,
        Arc::new(LocalStreamFn::new(Arc::new(model))),
    ))
}

#[cfg(test)]
mod tests {
    use swink_agent::{ModelSpec, model_catalog};

    use super::*;

    #[test]
    fn default_local_connection_uses_catalog_model_spec() {
        let connection = default_local_connection().unwrap();
        let preset = model_catalog()
            .preset("local", DEFAULT_LOCAL_PRESET_ID)
            .unwrap();
        assert_eq!(connection.model_spec(), &preset.model_spec());
    }

    #[test]
    fn default_local_connection_does_not_require_api_key() {
        let connection = default_local_connection().unwrap();
        assert_eq!(
            connection.model_spec(),
            &ModelSpec::new("local", "SmolLM3-3B-Q4_K_M")
        );
    }
}
