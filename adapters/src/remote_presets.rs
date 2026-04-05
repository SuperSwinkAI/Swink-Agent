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
        provider_key: &'static str,
        preset_id: &'static str,
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

#[must_use]
pub fn remote_presets(provider_key: Option<&str>) -> Vec<CatalogPreset> {
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
            provider_key: "unknown",
            preset_id: "unknown",
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
    let catalog_preset = preset(model_id).ok_or_else(|| {
        RemoteModelConnectionError::UnknownModelId {
            model_id: model_id.to_string(),
        }
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

    #[test]
    fn grouped_remote_presets_are_loaded_from_catalog() {
        let all = remote_presets(None);
        assert!(!all.is_empty(), "catalog should have remote presets");
    }

    #[test]
    fn preset_finds_by_model_id() {
        let sonnet = preset("claude-sonnet-4-6").expect("sonnet preset should exist");
        assert_eq!(sonnet.provider_key, "anthropic");
        assert_eq!(sonnet.preset_id, "sonnet_46");

        let gpt = preset("gpt-4o").expect("gpt-4o preset should exist");
        assert_eq!(gpt.provider_key, "openai");

        assert!(preset("nonexistent-model-xyz").is_none());
    }

    #[test]
    fn preset_key_resolves_via_catalog() {
        let key = RemotePresetKey::new("anthropic", "sonnet_46");
        let catalog_preset = required_catalog_preset(key).unwrap();
        assert_eq!(catalog_preset.model_id, "claude-sonnet-4-6");
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
        let err = build_connection_from_preset(&preset, None, None).unwrap_err();
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
    fn every_remote_provider_has_at_least_one_preset() {
        let catalog = model_catalog();
        for provider in &catalog.providers {
            if provider.kind == ProviderKind::Remote {
                let presets = remote_presets(Some(&provider.key));
                assert!(
                    !presets.is_empty(),
                    "remote provider '{}' should have presets",
                    provider.key
                );
            }
        }
    }

    #[test]
    fn all_remote_presets_resolvable_via_key() {
        for p in remote_presets(None) {
            let key = RemotePresetKey::new(
                Box::leak(p.provider_key.clone().into_boxed_str()),
                Box::leak(p.preset_id.clone().into_boxed_str()),
            );
            let resolved = required_catalog_preset(key).unwrap();
            assert_eq!(resolved.model_id, p.model_id);
        }
    }

    #[test]
    fn preset_finds_representative_models_across_providers() {
        // Spot-check one model per remote provider to verify catalog coverage.
        let checks = [
            ("claude-sonnet-4-6", "anthropic"),
            ("gpt-4o", "openai"),
            ("gemini-3-flash-preview", "google"),
            ("mistral-large-latest", "mistral"),
        ];
        for (model_id, expected_provider) in checks {
            let p = preset(model_id).unwrap_or_else(|| {
                panic!("preset for model_id '{model_id}' should exist in catalog")
            });
            assert_eq!(p.provider_key, expected_provider);
        }
    }
}
