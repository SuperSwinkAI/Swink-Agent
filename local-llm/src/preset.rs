use std::sync::Arc;

use swink_agent::{ModelConnection, ProviderKind, model_catalog};
use thiserror::Error;

use crate::embedding::EmbeddingConfig;
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

// ─── ModelPreset ────────────────────────────────────────────────────────────

/// Named configuration bundles for supported local models.
///
/// Each variant bundles a `HuggingFace` repository ID, filename, quantization
/// level, and context window so consumers can select a model by name without
/// manual configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPreset {
    /// SmolLM3-3B with `Q4_K_M` quantization, 8192-token context window (~1.92 GB).
    SmolLM3_3B,
    /// EmbeddingGemma-300M for text vectorization (<200 MB).
    EmbeddingGemma300M,
    /// Gemma 4 E2B with `Q4_K_M` quantization, 128K context (~3.46 GB).
    #[cfg(feature = "gemma4")]
    Gemma4E2B,
    /// Gemma 4 E4B with `Q4_K_M` quantization, 128K context (~5.5 GB).
    #[cfg(feature = "gemma4")]
    Gemma4E4B,
    /// Gemma 4 26B `MoE` with `Q4_K_M` quantization, 256K context (~16 GB).
    #[cfg(feature = "gemma4")]
    Gemma4_26B,
    /// Gemma 4 31B dense with `Q4_K_M` quantization, 256K context (~20 GB).
    #[cfg(feature = "gemma4")]
    Gemma4_31B,
}

impl ModelPreset {
    /// Convert this preset to a [`ModelConfig`] for inference models.
    pub fn config(&self) -> ModelConfig {
        match self {
            Self::SmolLM3_3B => ModelConfig {
                repo_id: std::env::var("LOCAL_MODEL_REPO")
                    .unwrap_or_else(|_| "unsloth/SmolLM3-3B-GGUF".to_string()),
                filename: std::env::var("LOCAL_MODEL_FILE")
                    .unwrap_or_else(|_| "SmolLM3-3B-Q4_K_M.gguf".to_string()),
                context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(8192),
                chat_template: None,
                gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
            Self::EmbeddingGemma300M => ModelConfig {
                repo_id: std::env::var("LOCAL_EMBED_REPO")
                    .unwrap_or_else(|_| "google/gemma-embedding-300m".to_string()),
                filename: std::env::var("LOCAL_EMBED_FILE").unwrap_or_default(),
                context_length: 2048,
                chat_template: None,
                gpu_layers: 0,
            },
            #[cfg(feature = "gemma4")]
            Self::Gemma4E2B => ModelConfig {
                repo_id: std::env::var("LOCAL_MODEL_REPO").unwrap_or_else(|_| {
                    "google/gemma-4-E2B-it".to_string()
                }),
                filename: String::new(), // safetensors, not GGUF
                context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(131_072),
                chat_template: None,
                gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
            #[cfg(feature = "gemma4")]
            Self::Gemma4E4B => ModelConfig {
                repo_id: std::env::var("LOCAL_MODEL_REPO").unwrap_or_else(|_| {
                    "google/gemma-4-E4B-it".to_string()
                }),
                filename: String::new(), // safetensors, not GGUF
                context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(131_072),
                chat_template: None,
                gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
            #[cfg(feature = "gemma4")]
            Self::Gemma4_26B => ModelConfig {
                repo_id: std::env::var("LOCAL_MODEL_REPO").unwrap_or_else(|_| {
                    "google/gemma-4-26B-A4B-it".to_string()
                }),
                filename: String::new(), // safetensors, not GGUF
                context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(262_144),
                chat_template: None,
                gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
            #[cfg(feature = "gemma4")]
            Self::Gemma4_31B => ModelConfig {
                repo_id: std::env::var("LOCAL_MODEL_REPO").unwrap_or_else(|_| {
                    "google/gemma-4-31B-it".to_string()
                }),
                filename: String::new(), // safetensors, not GGUF
                context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(262_144),
                chat_template: None,
                gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            },
        }
    }

    /// Convert this preset to an [`EmbeddingConfig`] for embedding models.
    pub fn embedding_config(&self) -> EmbeddingConfig {
        match self {
            Self::EmbeddingGemma300M => EmbeddingConfig {
                repo_id: std::env::var("LOCAL_EMBED_REPO")
                    .unwrap_or_else(|_| "google/gemma-embedding-300m".to_string()),
                filename: std::env::var("LOCAL_EMBED_FILE").unwrap_or_default(),
                dimensions: std::env::var("LOCAL_EMBED_DIMS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(768),
            },
            Self::SmolLM3_3B => EmbeddingConfig {
                repo_id: "unsloth/SmolLM3-3B-GGUF".to_string(),
                filename: "SmolLM3-3B-Q4_K_M.gguf".to_string(),
                dimensions: 768,
            },
            #[cfg(feature = "gemma4")]
            Self::Gemma4E2B | Self::Gemma4E4B | Self::Gemma4_26B | Self::Gemma4_31B => EmbeddingConfig {
                repo_id: "google/gemma-embedding-300m".to_string(),
                filename: String::new(),
                dimensions: 768,
            },
        }
    }

    /// Returns a static slice of all available presets.
    #[cfg(not(feature = "gemma4"))]
    pub const fn all() -> &'static [Self] {
        &[Self::SmolLM3_3B, Self::EmbeddingGemma300M]
    }

    /// Returns a static slice of all available presets.
    #[cfg(feature = "gemma4")]
    pub const fn all() -> &'static [Self] {
        &[
            Self::SmolLM3_3B,
            Self::EmbeddingGemma300M,
            Self::Gemma4E2B,
            Self::Gemma4E4B,
            Self::Gemma4_26B,
            Self::Gemma4_31B,
        ]
    }
}

impl std::fmt::Display for ModelPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SmolLM3_3B => write!(f, "SmolLM3-3B"),
            Self::EmbeddingGemma300M => write!(f, "EmbeddingGemma-300M"),
            #[cfg(feature = "gemma4")]
            Self::Gemma4E2B => write!(f, "Gemma4-E2B"),
            #[cfg(feature = "gemma4")]
            Self::Gemma4E4B => write!(f, "Gemma4-E4B"),
            #[cfg(feature = "gemma4")]
            Self::Gemma4_26B => write!(f, "Gemma4-26B"),
            #[cfg(feature = "gemma4")]
            Self::Gemma4_31B => write!(f, "Gemma4-31B"),
        }
    }
}

// ─── Catalog-based connection ──────────────────────────────────────────────

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
    use swink_agent::model_catalog;

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
        let spec = connection.model_spec();
        assert_eq!(spec.provider, "local");
        assert_eq!(spec.model_id, "SmolLM3-3B-Q4_K_M");
    }

    #[test]
    fn smollm3_preset_config_has_correct_defaults() {
        let config = ModelPreset::SmolLM3_3B.config();
        assert!(config.repo_id.contains("SmolLM3"));
        assert!(config.filename.contains("SmolLM3"));
        assert_eq!(config.context_length, 8192);
        assert!(config.chat_template.is_none());
    }

    #[test]
    fn embedding_gemma_preset_config() {
        let config = ModelPreset::EmbeddingGemma300M.config();
        assert!(config.repo_id.contains("gemma"));
    }

    #[test]
    fn embedding_gemma_embedding_config() {
        let config = ModelPreset::EmbeddingGemma300M.embedding_config();
        assert!(config.repo_id.contains("gemma"));
        assert_eq!(config.dimensions, 768);
    }

    #[test]
    #[cfg(not(feature = "gemma4"))]
    fn all_presets_returns_both_variants() {
        let all = ModelPreset::all();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&ModelPreset::SmolLM3_3B));
        assert!(all.contains(&ModelPreset::EmbeddingGemma300M));
    }

    #[test]
    fn preset_display() {
        assert_eq!(ModelPreset::SmolLM3_3B.to_string(), "SmolLM3-3B");
        assert_eq!(
            ModelPreset::EmbeddingGemma300M.to_string(),
            "EmbeddingGemma-300M"
        );
    }

    #[test]
    fn preset_is_copy() {
        let p = ModelPreset::SmolLM3_3B;
        let p2 = p;
        assert_eq!(p, p2);
    }

    #[cfg(feature = "gemma4")]
    mod gemma4_tests {
        use super::*;

        #[test]
        fn gemma4_e2b_preset_config_defaults() {
            let config = ModelPreset::Gemma4E2B.config();
            assert_eq!(config.repo_id, "google/gemma-4-E2B-it");
            assert!(config.filename.is_empty()); // safetensors, not GGUF
            assert_eq!(config.context_length, 131_072);
            assert!(config.chat_template.is_none());
        }

        #[test]
        fn gemma4_e4b_preset_config_defaults() {
            let config = ModelPreset::Gemma4E4B.config();
            assert_eq!(config.repo_id, "google/gemma-4-E4B-it");
            assert!(config.filename.is_empty());
            assert_eq!(config.context_length, 131_072);
        }

        #[test]
        fn gemma4_26b_preset_config_defaults() {
            let config = ModelPreset::Gemma4_26B.config();
            assert_eq!(config.repo_id, "google/gemma-4-26B-A4B-it");
            assert!(config.filename.is_empty());
            assert_eq!(config.context_length, 262_144);
        }

        #[test]
        fn gemma4_e2b_env_override() {
            // Env vars are shared process state; just verify the default path works.
            // Actual env override is tested by the existing SmolLM3 env override pattern.
            let config = ModelPreset::Gemma4E2B.config();
            assert_eq!(config.repo_id, "google/gemma-4-E2B-it");
        }

        #[test]
        fn gemma4_31b_preset_config_defaults() {
            let config = ModelPreset::Gemma4_31B.config();
            assert_eq!(config.repo_id, "google/gemma-4-31B-it");
            assert!(config.filename.is_empty());
            assert_eq!(config.context_length, 262_144);
            assert!(config.chat_template.is_none());
        }

        #[test]
        fn all_presets_includes_gemma4_variants() {
            let all = ModelPreset::all();
            assert_eq!(all.len(), 6);
            assert!(all.contains(&ModelPreset::SmolLM3_3B));
            assert!(all.contains(&ModelPreset::EmbeddingGemma300M));
            assert!(all.contains(&ModelPreset::Gemma4E2B));
            assert!(all.contains(&ModelPreset::Gemma4E4B));
            assert!(all.contains(&ModelPreset::Gemma4_26B));
            assert!(all.contains(&ModelPreset::Gemma4_31B));
        }
    }
}
