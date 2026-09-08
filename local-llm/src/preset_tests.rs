//! Tests for `preset`.
#![cfg(test)]

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
    let result = chat_preset_defaults_from_parts(None, Some("model.gguf".to_string()), Some(8192));

    assert_eq!(
        result,
        Err(LocalPresetError::MissingRepoId {
            preset_id: DEFAULT_LOCAL_PRESET_ID
        })
    );
}

#[test]
fn chat_preset_defaults_report_missing_filename() {
    let result = chat_preset_defaults_from_parts(Some("owner/repo".to_string()), None, Some(8192));

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
