use std::sync::Arc;

#[cfg(feature = "gemini")]
use swink_agent::ApiVersion;
use swink_agent::{CatalogPreset, ModelConnection, ProviderKind, StreamFn, model_catalog};
use thiserror::Error;

#[cfg(feature = "anthropic")]
use crate::AnthropicStreamFn;
#[cfg(feature = "bedrock")]
use crate::BedrockStreamFn;
#[cfg(feature = "gemini")]
use crate::GeminiStreamFn;
#[cfg(feature = "mistral")]
use crate::MistralStreamFn;
#[cfg(feature = "openai")]
use crate::OpenAiStreamFn;
#[cfg(feature = "xai")]
use crate::XAiStreamFn;
#[cfg(feature = "azure")]
use crate::{AzureAuth, AzureStreamFn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemotePresetKey {
    pub provider_key: &'static str,
    pub preset_id: &'static str,
}

impl RemotePresetKey {
    #[must_use]
    pub const fn new(provider_key: &'static str, preset_id: &'static str) -> Self {
        Self {
            provider_key,
            preset_id,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteModelConnectionError {
    #[error("Unknown remote preset {provider_key}.{preset_id}")]
    UnknownPreset {
        provider_key: &'static str,
        preset_id: &'static str,
    },
    #[error("No remote preset found for model_id \"{model_id}\"")]
    UnknownModelId { model_id: String },
    #[error("{provider_key}.{preset_id} is not a remote preset")]
    NotRemotePreset {
        provider_key: String,
        preset_id: String,
    },
    #[error(
        "Missing {env_var} for {preset}. Set it in your environment or .env before launching the example."
    )]
    MissingCredential { preset: String, env_var: String },
    #[error(
        "Missing {env_var} for {preset}. Set it in your environment or .env before launching the example."
    )]
    MissingBaseUrl { preset: String, env_var: String },
    #[error(
        "Missing {env_var} for {preset}. Set it in your environment or .env before launching the example."
    )]
    MissingRegion { preset: String, env_var: String },
    #[error(
        "Missing AWS_ACCESS_KEY_ID or AWS_SECRET_ACCESS_KEY for {preset}. Set AWS credentials in your environment or .env before launching the example."
    )]
    MissingAwsCredentials { preset: String },
    #[error("Unsupported provider \"{provider_key}\" — no adapter feature enabled")]
    UnsupportedProvider { provider_key: String },
}

/// Returns `true` if the adapter for the given provider key is compiled in.
///
/// Uses `#[cfg(feature = "...")]` checks so the answer is a compile-time
/// constant for each provider. Provider keys that don't map to any adapter
/// feature (e.g. `"local"`) always return `false`.
#[must_use]
#[allow(clippy::match_like_matches_macro)] // arms evaluate different cfg! flags, not a set membership check
pub fn is_provider_compiled(provider_key: &str) -> bool {
    match provider_key {
        "anthropic" => cfg!(feature = "anthropic"),
        "openai" => cfg!(feature = "openai"),
        "google" => cfg!(feature = "gemini"),
        "azure" => cfg!(feature = "azure"),
        "xai" => cfg!(feature = "xai"),
        "mistral" => cfg!(feature = "mistral"),
        "bedrock" => cfg!(feature = "bedrock"),
        _ => false,
    }
}

/// Returns remote presets filtered to only those whose provider adapter is
/// compiled in. Use [`all_remote_presets`] to enumerate the full catalog
/// regardless of compiled adapter support.
#[must_use]
pub fn remote_presets(provider_key: Option<&str>) -> Vec<CatalogPreset> {
    all_remote_presets(provider_key)
        .into_iter()
        .filter(|p| is_provider_compiled(&p.provider_key))
        .collect()
}

/// Returns all remote presets from the catalog, regardless of feature flags.
///
/// Useful for discovery UIs that want to show available models even when the
/// corresponding adapter is not compiled in.
#[must_use]
pub fn all_remote_presets(provider_key: Option<&str>) -> Vec<CatalogPreset> {
    let catalog = model_catalog();
    catalog
        .providers
        .iter()
        .filter(|provider| provider.kind == ProviderKind::Remote)
        .filter(|provider| provider_key.is_none_or(|key| provider.key == key))
        .flat_map(|provider| {
            provider
                .presets
                .iter()
                .filter_map(|preset| catalog.preset(&provider.key, &preset.id))
        })
        .collect()
}

pub fn build_remote_connection(
    key: RemotePresetKey,
) -> Result<ModelConnection, RemoteModelConnectionError> {
    let preset = required_catalog_preset(key)?;
    build_connection_from_preset(
        &preset,
        preset
            .credential_env_var
            .as_deref()
            .and_then(|env_var| std::env::var(env_var).ok()),
        preset
            .base_url_env_var
            .as_deref()
            .and_then(|env_var| std::env::var(env_var).ok())
            .as_deref(),
    )
}

#[allow(unreachable_code, unused_variables)]
fn build_connection_from_preset(
    preset: &CatalogPreset,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<ModelConnection, RemoteModelConnectionError> {
    if preset.provider_kind != ProviderKind::Remote {
        return Err(RemoteModelConnectionError::NotRemotePreset {
            provider_key: preset.provider_key.clone(),
            preset_id: preset.preset_id.clone(),
        });
    }

    let provider_key = preset.provider_key.as_str();

    let api_key = if provider_key == "bedrock" {
        String::new()
    } else {
        let env_var = preset.credential_env_var.clone().ok_or_else(|| {
            RemoteModelConnectionError::UnsupportedProvider {
                provider_key: provider_key.to_string(),
            }
        })?;
        match api_key {
            Some(value) if !value.trim().is_empty() => value,
            _ => {
                return Err(RemoteModelConnectionError::MissingCredential {
                    preset: preset.display_name.clone(),
                    env_var,
                });
            }
        }
    };

    let resolved_base_url = || {
        base_url
            .map(str::to_string)
            .or_else(|| preset.default_base_url.clone())
            .ok_or_else(|| RemoteModelConnectionError::MissingBaseUrl {
                preset: preset.display_name.clone(),
                env_var: preset
                    .base_url_env_var
                    .clone()
                    .unwrap_or_else(|| "BASE_URL".to_string()),
            })
    };
    let stream_fn: Arc<dyn StreamFn> = match provider_key {
        #[cfg(feature = "anthropic")]
        "anthropic" => Arc::new(AnthropicStreamFn::new(resolved_base_url()?, &api_key)),
        #[cfg(feature = "openai")]
        "openai" => Arc::new(OpenAiStreamFn::new(resolved_base_url()?, &api_key)),
        #[cfg(feature = "gemini")]
        "google" => Arc::new(GeminiStreamFn::new(
            resolved_base_url()?,
            &api_key,
            preset.api_version.clone().unwrap_or(ApiVersion::V1beta),
        )),
        #[cfg(feature = "azure")]
        #[allow(clippy::redundant_clone)]
        // Clone needed when multiple adapter features enabled
        "azure" => Arc::new(AzureStreamFn::new(
            resolved_base_url()?,
            AzureAuth::ApiKey(api_key.clone()),
        )),
        #[cfg(feature = "xai")]
        "xai" => Arc::new(XAiStreamFn::new(resolved_base_url()?, &api_key)),
        #[cfg(feature = "mistral")]
        "mistral" => Arc::new(MistralStreamFn::new(resolved_base_url()?, &api_key)),
        #[cfg(feature = "bedrock")]
        "bedrock" => {
            let region_env_var = preset
                .region_env_var
                .clone()
                .unwrap_or_else(|| "AWS_REGION".to_string());
            let region = std::env::var(&region_env_var).map_err(|_| {
                RemoteModelConnectionError::MissingRegion {
                    preset: preset.display_name.clone(),
                    env_var: region_env_var,
                }
            })?;
            let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").map_err(|_| {
                RemoteModelConnectionError::MissingAwsCredentials {
                    preset: preset.display_name.clone(),
                }
            })?;
            let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| {
                RemoteModelConnectionError::MissingAwsCredentials {
                    preset: preset.display_name.clone(),
                }
            })?;
            let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
            Arc::new(BedrockStreamFn::new(
                region,
                access_key_id,
                secret_access_key,
                session_token,
            ))
        }
        _ => {
            return Err(RemoteModelConnectionError::UnsupportedProvider {
                provider_key: provider_key.to_string(),
            });
        }
    };
    Ok(ModelConnection::new(preset.model_spec(), stream_fn))
}

/// Looks up a remote preset by its `model_id` (e.g. `"claude-sonnet-4-6"`).
///
/// This is the primary entry point for finding a preset — callers write
/// `preset("claude-sonnet-4-6")` instead of constructing a `RemotePresetKey`
/// and looking up the catalog manually.
#[must_use]
pub fn preset(model_id: &str) -> Option<CatalogPreset> {
    remote_presets(None)
        .into_iter()
        .find(|p| p.model_id == model_id)
}

/// Builds a [`ModelConnection`] for a model identified by its `model_id`
/// (e.g. `"claude-sonnet-4-6"`, `"gpt-4o"`).
///
/// This is the simplest way to get a connection — it resolves the preset from
/// the catalog by `model_id`, reads credentials from the environment, and
/// constructs the appropriate provider-specific `StreamFn`.
///
/// # Errors
///
/// Returns [`RemoteModelConnectionError`] if the model is not found, is not a
/// remote preset, or required credentials are missing from the environment.
pub fn build_remote_connection_for_model(
    model_id: &str,
) -> Result<ModelConnection, RemoteModelConnectionError> {
    let catalog_preset =
        preset(model_id).ok_or_else(|| RemoteModelConnectionError::UnknownModelId {
            model_id: model_id.to_string(),
        })?;
    build_connection_from_preset(
        &catalog_preset,
        catalog_preset
            .credential_env_var
            .as_deref()
            .and_then(|env_var| std::env::var(env_var).ok()),
        catalog_preset
            .base_url_env_var
            .as_deref()
            .and_then(|env_var| std::env::var(env_var).ok())
            .as_deref(),
    )
}

fn required_catalog_preset(
    key: RemotePresetKey,
) -> Result<CatalogPreset, RemoteModelConnectionError> {
    model_catalog()
        .preset(key.provider_key, key.preset_id)
        .ok_or(RemoteModelConnectionError::UnknownPreset {
            provider_key: key.provider_key,
            preset_id: key.preset_id,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── all_remote_presets (unfiltered) ──────────────────────────────────

    #[test]
    fn all_remote_presets_are_loaded_from_catalog() {
        let all = all_remote_presets(None);
        assert!(!all.is_empty(), "catalog should have remote presets");
    }

    #[test]
    fn every_remote_provider_has_at_least_one_unfiltered_preset() {
        let catalog = model_catalog();
        for provider in &catalog.providers {
            if provider.kind == ProviderKind::Remote {
                let presets = all_remote_presets(Some(&provider.key));
                assert!(
                    !presets.is_empty(),
                    "remote provider '{}' should have presets in the catalog",
                    provider.key
                );
            }
        }
    }

    #[test]
    fn all_catalog_remote_presets_resolvable_by_provider_and_preset_id() {
        let catalog = model_catalog();
        for p in all_remote_presets(None) {
            let found = catalog
                .preset(&p.provider_key, &p.preset_id)
                .unwrap_or_else(|| {
                    panic!(
                        "catalog.preset('{}', '{}') must resolve for model_id '{}'",
                        p.provider_key, p.preset_id, p.model_id
                    )
                });
            assert_eq!(found.model_id, p.model_id);
        }
    }

    // ── is_provider_compiled ────────────────────────────────────────────

    #[test]
    fn is_provider_compiled_returns_false_for_unknown_provider() {
        assert!(!is_provider_compiled("nonexistent"));
        assert!(!is_provider_compiled("local"));
        assert!(!is_provider_compiled(""));
    }

    #[test]
    fn is_provider_compiled_matches_feature_gates() {
        // Each assertion matches the compile-time cfg for the corresponding feature.
        assert_eq!(
            is_provider_compiled("anthropic"),
            cfg!(feature = "anthropic")
        );
        assert_eq!(is_provider_compiled("openai"), cfg!(feature = "openai"));
        assert_eq!(is_provider_compiled("google"), cfg!(feature = "gemini"));
        assert_eq!(is_provider_compiled("azure"), cfg!(feature = "azure"));
        assert_eq!(is_provider_compiled("xai"), cfg!(feature = "xai"));
        assert_eq!(is_provider_compiled("mistral"), cfg!(feature = "mistral"));
        assert_eq!(is_provider_compiled("bedrock"), cfg!(feature = "bedrock"));
    }

    // ── remote_presets (filtered) ───────────────────────────────────────

    #[test]
    fn remote_presets_only_contains_compiled_providers() {
        for p in remote_presets(None) {
            assert!(
                is_provider_compiled(&p.provider_key),
                "remote_presets() returned preset '{}' for provider '{}' which is not compiled",
                p.preset_id,
                p.provider_key
            );
        }
    }

    #[test]
    fn remote_presets_subset_of_all_remote_presets() {
        let filtered = remote_presets(None);
        let all = all_remote_presets(None);
        assert!(
            filtered.len() <= all.len(),
            "filtered ({}) must be <= all ({})",
            filtered.len(),
            all.len()
        );
        // Every filtered preset must also appear in the unfiltered list.
        for p in &filtered {
            assert!(
                all.iter()
                    .any(|a| a.model_id == p.model_id && a.provider_key == p.provider_key),
                "filtered preset '{}.{}' not found in all_remote_presets",
                p.provider_key,
                p.preset_id
            );
        }
    }

    #[cfg(not(any(
        feature = "anthropic",
        feature = "openai",
        feature = "gemini",
        feature = "azure",
        feature = "xai",
        feature = "mistral",
        feature = "bedrock",
    )))]
    #[test]
    fn remote_presets_empty_when_no_adapters_compiled() {
        let presets = remote_presets(None);
        assert!(
            presets.is_empty(),
            "remote_presets() should be empty with no adapter features, got {} presets",
            presets.len()
        );
    }

    #[cfg(all(feature = "xai", not(feature = "openai")))]
    #[test]
    fn xai_feature_does_not_mark_openai_as_compiled() {
        assert!(is_provider_compiled("xai"));
        assert!(!is_provider_compiled("openai"));
        assert!(
            remote_presets(None)
                .into_iter()
                .all(|preset| preset.provider_key != "openai"),
            "xai-only builds must not expose OpenAI presets as compiled",
        );
    }

    // ── preset() (filtered) ────────────────────────────────────────────

    #[test]
    fn preset_only_finds_compiled_providers() {
        // Take every model_id from the full catalog and verify that preset()
        // only returns it when the provider is compiled.
        for p in all_remote_presets(None) {
            let result = preset(&p.model_id);
            if is_provider_compiled(&p.provider_key) {
                // May still be None if an earlier provider claimed this model_id.
                // That's fine — we just verify it doesn't return an uncompiled one.
                if let Some(found) = &result {
                    assert!(
                        is_provider_compiled(&found.provider_key),
                        "preset('{}') returned uncompiled provider '{}'",
                        p.model_id,
                        found.provider_key
                    );
                }
            }
        }
    }

    #[test]
    fn preset_returns_none_for_nonexistent_model() {
        assert!(preset("nonexistent-model-xyz").is_none());
    }

    // ── preset key resolution ──────────────────────────────────────────

    #[test]
    fn preset_key_resolves_via_catalog() {
        let key = RemotePresetKey::new("anthropic", "sonnet_46");
        let catalog_preset = required_catalog_preset(key).unwrap();
        assert_eq!(catalog_preset.model_id, "claude-sonnet-4-6");
    }

    // ── feature-gated connection tests ──────────────────────────────────

    #[cfg(feature = "anthropic")]
    #[test]
    fn preset_finds_anthropic_when_compiled() {
        let sonnet = preset("claude-sonnet-4-6").expect("sonnet preset should exist");
        assert_eq!(sonnet.provider_key, "anthropic");
        assert_eq!(sonnet.preset_id, "sonnet_46");
    }

    #[cfg(feature = "openai")]
    #[test]
    fn preset_finds_openai_when_compiled() {
        let gpt = preset("gpt-4o").expect("gpt-4o preset should exist");
        assert_eq!(gpt.provider_key, "openai");
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn remote_connection_uses_catalog_model_spec() {
        let key = RemotePresetKey::new("anthropic", "sonnet_46");
        let preset = required_catalog_preset(key).unwrap();
        let connection =
            build_connection_from_preset(&preset, Some("test-key".to_string()), None).unwrap();
        assert_eq!(connection.model_spec(), &preset.model_spec());
    }

    #[cfg(feature = "openai")]
    #[test]
    fn remote_preset_requires_key() {
        let preset = preset("gpt-4o").unwrap();
        let err = match build_connection_from_preset(&preset, None, None) {
            Ok(_) => panic!("expected missing credential error"),
            Err(err) => err,
        };
        assert_eq!(
            err,
            RemoteModelConnectionError::MissingCredential {
                preset: "OpenAI GPT-4o".to_string(),
                env_var: "OPENAI_API_KEY".to_string(),
            }
        );
    }

    #[test]
    fn build_remote_connection_for_model_rejects_unknown() {
        let result = build_remote_connection_for_model("nonexistent-xyz");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(
            err,
            RemoteModelConnectionError::UnknownModelId {
                model_id: "nonexistent-xyz".to_string(),
            }
        );
    }

    #[test]
    fn preset_by_model_id_returns_a_match_for_every_filtered_model_id() {
        let mut seen = std::collections::HashSet::new();
        for p in remote_presets(None) {
            if seen.insert(p.model_id.clone()) {
                assert!(
                    preset(&p.model_id).is_some(),
                    "preset('{}') must return Some for a compiled catalog model_id",
                    p.model_id
                );
            }
        }
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn preset_finds_representative_anthropic_model() {
        let p = preset("claude-sonnet-4-6").expect("anthropic preset should exist");
        assert_eq!(p.provider_key, "anthropic");
    }

    #[cfg(feature = "openai")]
    #[test]
    fn preset_finds_representative_openai_model() {
        let p = preset("gpt-4o").expect("openai preset should exist");
        assert_eq!(p.provider_key, "openai");
    }

    #[cfg(feature = "gemini")]
    #[test]
    fn preset_finds_representative_gemini_model() {
        let p = preset("gemini-3-flash-preview").expect("gemini preset should exist");
        assert_eq!(p.provider_key, "google");
    }

    #[cfg(feature = "mistral")]
    #[test]
    fn preset_finds_representative_mistral_model() {
        let p = preset("mistral-large-latest").expect("mistral preset should exist");
        assert_eq!(p.provider_key, "mistral");
    }
}
