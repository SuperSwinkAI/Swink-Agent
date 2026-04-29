use std::{str::FromStr, sync::Arc};

use swink_agent::{CatalogPreset, ModelConnection, ProviderKind, model_catalog};
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
    #[error("local.{preset_id} is missing context_window_tokens in the model catalog")]
    MissingContextWindow { preset_id: &'static str },
    #[error("local.{preset_id} has invalid context_window_tokens in the model catalog")]
    InvalidContextWindow { preset_id: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatPresetDefaults {
    repo_id: String,
    filename: String,
    context_length: usize,
}

impl ChatPresetDefaults {
    fn into_config(self) -> ModelConfig {
        ModelConfig {
            repo_id: env_or("LOCAL_MODEL_REPO", self.repo_id),
            filename: env_or("LOCAL_MODEL_FILE", self.filename),
            context_length: env_parse_or("LOCAL_CONTEXT_LENGTH", self.context_length),
            chat_template: None,
            gpu_layers: env_parse_or("LOCAL_GPU_LAYERS", 0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmbeddingPresetDefaults {
    repo_id: String,
    filename: String,
    context_length: usize,
    dimensions: usize,
}

impl EmbeddingPresetDefaults {
    fn into_model_config(self) -> ModelConfig {
        ModelConfig {
            repo_id: env_or("LOCAL_EMBED_REPO", self.repo_id),
            filename: env_or("LOCAL_EMBED_FILE", self.filename),
            context_length: self.context_length,
            chat_template: None,
            gpu_layers: 0,
        }
    }

    fn into_embedding_config(self) -> EmbeddingConfig {
        EmbeddingConfig {
            repo_id: env_or("LOCAL_EMBED_REPO", self.repo_id),
            filename: env_or("LOCAL_EMBED_FILE", self.filename),
            dimensions: env_parse_or("LOCAL_EMBED_DIMS", self.dimensions),
        }
    }
}

fn env_or(key: &str, default: String) -> String {
    std::env::var(key).unwrap_or(default)
}

fn env_parse_or<T>(key: &str, default: T) -> T
where
    T: Copy + FromStr,
{
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn built_in_chat_preset_defaults() -> ChatPresetDefaults {
    ChatPresetDefaults {
        repo_id: "unsloth/SmolLM3-3B-GGUF".to_string(),
        filename: "SmolLM3-3B-Q4_K_M.gguf".to_string(),
        context_length: 8192,
    }
}

fn chat_preset_defaults_from_catalog(
    preset: CatalogPreset,
) -> Result<ChatPresetDefaults, LocalPresetError> {
    chat_preset_defaults_from_parts(
        preset.repo_id,
        preset.filename,
        preset.context_window_tokens,
    )
}

fn chat_preset_defaults_from_parts(
    repo_id: Option<String>,
    filename: Option<String>,
    context_window_tokens: Option<u64>,
) -> Result<ChatPresetDefaults, LocalPresetError> {
    let repo_id = repo_id.ok_or(LocalPresetError::MissingRepoId {
        preset_id: DEFAULT_LOCAL_PRESET_ID,
    })?;
    let filename = filename.ok_or(LocalPresetError::MissingFilename {
        preset_id: DEFAULT_LOCAL_PRESET_ID,
    })?;
    let context_length = context_window_tokens
        .ok_or(LocalPresetError::MissingContextWindow {
            preset_id: DEFAULT_LOCAL_PRESET_ID,
        })
        .and_then(|tokens| {
            usize::try_from(tokens).map_err(|_| LocalPresetError::InvalidContextWindow {
                preset_id: DEFAULT_LOCAL_PRESET_ID,
            })
        })?;

    Ok(ChatPresetDefaults {
        repo_id,
        filename,
        context_length,
    })
}

fn default_chat_preset_defaults() -> Result<ChatPresetDefaults, LocalPresetError> {
    let preset = model_catalog()
        .preset("local", DEFAULT_LOCAL_PRESET_ID)
        .ok_or(LocalPresetError::MissingDefaultPreset {
            preset_id: DEFAULT_LOCAL_PRESET_ID,
        })?;

    chat_preset_defaults_from_catalog(preset)
}

fn default_embedding_preset_defaults() -> EmbeddingPresetDefaults {
    EmbeddingPresetDefaults {
        repo_id: "unsloth/embeddinggemma-300m-GGUF".to_string(),
        filename: "embeddinggemma-300m-Q8_0.gguf".to_string(),
        context_length: 2048,
        dimensions: 768,
    }
}

#[cfg(feature = "gemma4")]
fn gemma4_config(repo_id: &str, filename: &str, context_length: usize) -> ModelConfig {
    ModelConfig {
        repo_id: env_or("LOCAL_MODEL_REPO", repo_id.to_string()),
        filename: env_or("LOCAL_MODEL_FILE", filename.to_string()),
        context_length: env_parse_or("LOCAL_CONTEXT_LENGTH", context_length),
        chat_template: None,
        gpu_layers: env_parse_or("LOCAL_GPU_LAYERS", 0),
    }
}

pub(crate) fn default_chat_model_config() -> ModelConfig {
    default_chat_preset_defaults()
        .unwrap_or_else(|_| built_in_chat_preset_defaults())
        .into_config()
}

pub(crate) fn default_embedding_model_config() -> ModelConfig {
    default_embedding_preset_defaults().into_model_config()
}

pub(crate) fn default_embedding_config() -> EmbeddingConfig {
    default_embedding_preset_defaults().into_embedding_config()
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
    pub fn try_config(&self) -> Result<ModelConfig, LocalPresetError> {
        Ok(match self {
            Self::SmolLM3_3B => default_chat_preset_defaults()?.into_config(),
            Self::EmbeddingGemma300M => default_embedding_model_config(),
            #[cfg(feature = "gemma4")]
            Self::Gemma4E2B => gemma4_config(
                "bartowski/google_gemma-4-E2B-it-GGUF",
                "google_gemma-4-E2B-it-Q4_K_M.gguf",
                131_072,
            ),
            #[cfg(feature = "gemma4")]
            Self::Gemma4E4B => gemma4_config(
                "bartowski/google_gemma-4-E4B-it-GGUF",
                "google_gemma-4-E4B-it-Q4_K_M.gguf",
                131_072,
            ),
            #[cfg(feature = "gemma4")]
            Self::Gemma4_26B => gemma4_config(
                "bartowski/google_gemma-4-26B-A4B-it-GGUF",
                "google_gemma-4-26B-A4B-it-Q4_K_M.gguf",
                262_144,
            ),
            #[cfg(feature = "gemma4")]
            Self::Gemma4_31B => gemma4_config(
                "bartowski/google_gemma-4-31B-it-GGUF",
                "google_gemma-4-31B-it-Q4_K_M.gguf",
                262_144,
            ),
        })
    }

    /// Convert this preset to a [`ModelConfig`] for inference models.
    #[must_use]
    pub fn config(&self) -> ModelConfig {
        self.try_config()
            .unwrap_or_else(|_| built_in_chat_preset_defaults().into_config())
    }

    /// Convert this preset to an [`EmbeddingConfig`] for embedding models.
    pub fn embedding_config(&self) -> EmbeddingConfig {
        match self {
            Self::EmbeddingGemma300M => default_embedding_config(),
            Self::SmolLM3_3B => {
                let defaults = default_chat_preset_defaults()
                    .unwrap_or_else(|_| built_in_chat_preset_defaults());
                EmbeddingConfig {
                    repo_id: defaults.repo_id,
                    filename: defaults.filename,
                    dimensions: 768,
                }
            }
            #[cfg(feature = "gemma4")]
            Self::Gemma4E2B | Self::Gemma4E4B | Self::Gemma4_26B | Self::Gemma4_31B => {
                let defaults = default_embedding_preset_defaults();
                EmbeddingConfig {
                    repo_id: defaults.repo_id,
                    filename: defaults.filename,
                    dimensions: defaults.dimensions,
                }
            }
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
    preset.repo_id.ok_or(LocalPresetError::MissingRepoId {
        preset_id: DEFAULT_LOCAL_PRESET_ID,
    })?;
    preset.filename.ok_or(LocalPresetError::MissingFilename {
        preset_id: DEFAULT_LOCAL_PRESET_ID,
    })?;

    let model = LocalModel::new(default_chat_preset_defaults()?.into_config());
    Ok(ModelConnection::new(
        model_spec,
        Arc::new(LocalStreamFn::new(Arc::new(model))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_local_connection_succeeds() {
        let result = default_local_connection();
        assert!(result.is_ok(), "default_local_connection should succeed");
    }

    #[test]
    fn try_config_returns_default_chat_config() {
        let config = ModelPreset::SmolLM3_3B
            .try_config()
            .unwrap_or_else(|err| panic!("default chat preset should be valid: {err}"));

        assert!(config.repo_id.contains("SmolLM3"));
        assert!(config.filename.contains("SmolLM3"));
        assert_eq!(config.context_length, 8192);
    }

    #[test]
    fn chat_preset_defaults_report_missing_repo_id() {
        let result =
            chat_preset_defaults_from_parts(None, Some("model.gguf".to_string()), Some(8192));

        assert_eq!(
            result,
            Err(LocalPresetError::MissingRepoId {
                preset_id: DEFAULT_LOCAL_PRESET_ID
            })
        );
    }

    #[test]
    fn chat_preset_defaults_report_missing_filename() {
        let result =
            chat_preset_defaults_from_parts(Some("owner/repo".to_string()), None, Some(8192));

        assert_eq!(
            result,
            Err(LocalPresetError::MissingFilename {
                preset_id: DEFAULT_LOCAL_PRESET_ID
            })
        );
    }

    #[test]
    fn chat_preset_defaults_report_missing_context_window() {
        let result = chat_preset_defaults_from_parts(
            Some("owner/repo".to_string()),
            Some("model.gguf".to_string()),
            None,
        );

        assert_eq!(
            result,
            Err(LocalPresetError::MissingContextWindow {
                preset_id: DEFAULT_LOCAL_PRESET_ID
            })
        );
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

    // ── Phase 5 (US3) tests ───────────────────────────────────────────────

    #[test]
    fn default_preset_remains_smollm3() {
        assert_eq!(DEFAULT_LOCAL_PRESET_ID, "smollm3_3b");
    }

    #[test]
    fn smollm3_preset_still_available() {
        let config = ModelPreset::SmolLM3_3B.config();
        assert!(config.repo_id.contains("SmolLM3"));
        assert_eq!(config.context_length, 8192);
    }

    #[test]
    fn smollm3_default_config_matches_preset_config() {
        assert_eq!(ModelConfig::default(), ModelPreset::SmolLM3_3B.config());
    }

    #[test]
    fn embedding_defaults_match_preset_config() {
        assert_eq!(
            EmbeddingConfig::default(),
            ModelPreset::EmbeddingGemma300M.embedding_config()
        );
    }

    #[test]
    fn embedding_model_config_matches_embedding_defaults() {
        let model_config = ModelPreset::EmbeddingGemma300M.config();
        let embedding_config = EmbeddingConfig::default();
        assert_eq!(model_config.repo_id, embedding_config.repo_id);
        assert_eq!(model_config.filename, embedding_config.filename);
        assert_eq!(model_config.context_length, 2048);
        assert_eq!(model_config.gpu_layers, 0);
        assert!(model_config.chat_template.is_none());
    }

    #[cfg(feature = "gemma4")]
    mod gemma4_tests {
        use super::*;

        #[test]
        fn gemma4_e2b_preset_config_defaults() {
            let config = ModelPreset::Gemma4E2B.config();
            assert!(config.repo_id.contains("gemma-4-E2B"));
            assert!(config.filename.contains(".gguf"));
            assert_eq!(config.context_length, 131_072);
            assert!(config.chat_template.is_none());
        }

        #[test]
        fn gemma4_e4b_preset_config_defaults() {
            let config = ModelPreset::Gemma4E4B.config();
            assert!(config.repo_id.contains("gemma-4-E4B"));
            assert!(config.filename.contains(".gguf"));
            assert_eq!(config.context_length, 131_072);
        }

        #[test]
        fn gemma4_26b_preset_config_defaults() {
            let config = ModelPreset::Gemma4_26B.config();
            assert!(config.repo_id.contains("gemma-4-26B"));
            assert!(config.filename.contains(".gguf"));
            assert_eq!(config.context_length, 262_144);
        }

        #[test]
        fn gemma4_e2b_env_override() {
            let config = ModelPreset::Gemma4E2B.config();
            assert!(config.repo_id.contains("gemma-4-E2B"));
        }

        #[test]
        fn gemma4_31b_preset_config_defaults() {
            let config = ModelPreset::Gemma4_31B.config();
            assert!(config.repo_id.contains("gemma-4-31B"));
            assert!(config.filename.contains(".gguf"));
            assert_eq!(config.context_length, 262_144);
            assert!(config.chat_template.is_none());
        }

        #[test]
        fn gemma4_e2b_selectable_via_preset() {
            let config = ModelPreset::Gemma4E2B.config();
            assert!(config.is_gemma4());
            assert_eq!(config.context_length, 131_072);
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
