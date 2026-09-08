use std::sync::Arc;

#[cfg(feature = "gemini")]
use swink_agent::ApiVersion;
use swink_agent::{CatalogPreset, ModelConnection, ProviderKind, StreamFn, model_catalog};
use thiserror::Error;

#[cfg(feature = "anthropic")]
use crate::AnthropicStreamFn;
#[cfg(feature = "bedrock")]
use crate::BedrockStreamFn;
#[cfg(feature = "codex")]
use crate::CodexStreamFn;
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

#[non_exhaustive]
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

#[non_exhaustive]
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
    #[error("{provider_key} is misconfigured: {detail}")]
    ProviderConfigError {
        provider_key: String,
        detail: String,
    },
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
        "codex" => cfg!(feature = "codex"),
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

/// Builds a [`ModelConnection`] for a preset key using an explicitly provided
/// credential instead of reading it from the process environment.
///
/// This is useful for embedders that manage secrets in an external store and
/// want to avoid process-global environment mutation.
pub fn build_remote_connection_with_credential(
    key: RemotePresetKey,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<ModelConnection, RemoteModelConnectionError> {
    let preset = required_catalog_preset(key)?;
    build_connection_from_preset(&preset, api_key, base_url)
}

#[allow(unreachable_code, unused_variables)]
// One arm per provider; the length is the dispatch table, not logic.
#[allow(clippy::too_many_lines)]
pub fn build_connection_from_preset(
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

    // Bedrock signs with SigV4 and codex resolves an OAuth grant per request:
    // neither carries an API key, so neither may demand one here.
    let api_key = if matches!(provider_key, "bedrock" | "codex") {
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
        "openai" => {
            // The catalog's `openai` provider is OpenAI proper (Responses), but
            // OPENAI_BASE_URL may point at an OpenAI-compatible server that
            // only speaks Chat Completions; OPENAI_API is the knob for that.
            let wire = crate::OpenAiWire::from_env().map_err(|e| {
                RemoteModelConnectionError::ProviderConfigError {
                    provider_key: "openai".to_string(),
                    detail: e.to_string(),
                }
            })?;
            Arc::new(OpenAiStreamFn::new_for_wire(
                wire,
                resolved_base_url()?,
                &api_key,
            ))
        }
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
        #[cfg(feature = "codex")]
        "codex" => {
            let mut codex = CodexStreamFn::from_env().map_err(|e| {
                RemoteModelConnectionError::ProviderConfigError {
                    provider_key: "codex".to_string(),
                    detail: e.to_string(),
                }
            })?;
            if let Some(url) = base_url {
                codex = codex.with_base_url(url);
            }
            Arc::new(codex)
        }
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
/// (e.g. `"claude-sonnet-4-6"`, `"gpt-5.4"`).
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
#[path = "remote_presets_tests.rs"]
mod tests;
