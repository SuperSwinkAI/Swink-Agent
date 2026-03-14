use std::sync::Arc;

use swink_agent::{
    ApiVersion, CatalogPreset, ModelConnection, ProviderKind, StreamFn, model_catalog,
};
use thiserror::Error;

use crate::{
    AnthropicStreamFn, AzureStreamFn, BedrockStreamFn, GeminiStreamFn, MistralStreamFn,
    OpenAiStreamFn, XAiStreamFn,
};

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

pub mod remote_preset_keys {
    use super::RemotePresetKey;

    pub mod anthropic {
        use super::RemotePresetKey;

        pub const OPUS_46: RemotePresetKey = RemotePresetKey::new("anthropic", "opus_46");
        pub const SONNET_46: RemotePresetKey = RemotePresetKey::new("anthropic", "sonnet_46");
        pub const HAIKU_45: RemotePresetKey = RemotePresetKey::new("anthropic", "haiku_45");
    }

    pub mod openai {
        use super::RemotePresetKey;

        pub const GPT_5_4: RemotePresetKey = RemotePresetKey::new("openai", "gpt_5_4");
        pub const GPT_5_2: RemotePresetKey = RemotePresetKey::new("openai", "gpt_5_2");
        pub const GPT_5_1: RemotePresetKey = RemotePresetKey::new("openai", "gpt_5_1");
        pub const GPT_5_3_CHAT_LATEST: RemotePresetKey =
            RemotePresetKey::new("openai", "gpt_5_3_chat_latest");
        pub const GPT_5_2_CHAT_LATEST: RemotePresetKey =
            RemotePresetKey::new("openai", "gpt_5_2_chat_latest");
        pub const GPT_5_1_CHAT_LATEST: RemotePresetKey =
            RemotePresetKey::new("openai", "gpt_5_1_chat_latest");
    }

    pub mod google {
        use super::RemotePresetKey;

        pub const GEMINI_3_1_PRO: RemotePresetKey =
            RemotePresetKey::new("google", "gemini_3_1_pro");
        pub const GEMINI_3_1_DEEP_THINK: RemotePresetKey =
            RemotePresetKey::new("google", "gemini_3_1_deep_think");
        pub const GEMINI_3_FLASH: RemotePresetKey =
            RemotePresetKey::new("google", "gemini_3_flash");
        pub const GEMINI_3_1_FLASH_LITE: RemotePresetKey =
            RemotePresetKey::new("google", "gemini_3_1_flash_lite");
    }

    pub mod azure {
        use super::RemotePresetKey;

        pub const GPT_5_4: RemotePresetKey = RemotePresetKey::new("azure", "gpt_5_4");
        pub const GPT_5_2: RemotePresetKey = RemotePresetKey::new("azure", "gpt_5_2");
        pub const PHI_4: RemotePresetKey = RemotePresetKey::new("azure", "phi_4");
    }

    pub mod xai {
        use super::RemotePresetKey;

        pub const GROK_3: RemotePresetKey = RemotePresetKey::new("xai", "grok_3");
        pub const GROK_3_FAST: RemotePresetKey = RemotePresetKey::new("xai", "grok_3_fast");
    }

    pub mod mistral {
        use super::RemotePresetKey;

        pub const MISTRAL_MEDIUM: RemotePresetKey =
            RemotePresetKey::new("mistral", "mistral_medium");
        pub const MISTRAL_SMALL: RemotePresetKey = RemotePresetKey::new("mistral", "mistral_small");
        pub const CODESTRAL: RemotePresetKey = RemotePresetKey::new("mistral", "codestral");
    }

    pub mod bedrock {
        use super::RemotePresetKey;

        pub const ANTHROPIC_CLAUDE_SONNET_45: RemotePresetKey =
            RemotePresetKey::new("bedrock", "anthropic_claude_sonnet_45");
        pub const META_LLAMA_4_MAVERICK: RemotePresetKey =
            RemotePresetKey::new("bedrock", "meta_llama_4_maverick");
        pub const MISTRAL_PIXTRAL_LARGE: RemotePresetKey =
            RemotePresetKey::new("bedrock", "mistral_pixtral_large");
        pub const AMAZON_NOVA_PRO: RemotePresetKey =
            RemotePresetKey::new("bedrock", "amazon_nova_pro");
        pub const AI21_JAMBA_1_5_LARGE: RemotePresetKey =
            RemotePresetKey::new("bedrock", "ai21_jamba_1_5_large");
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteModelConnectionError {
    #[error("Unknown remote preset {provider_key}.{preset_id}")]
    UnknownPreset {
        provider_key: &'static str,
        preset_id: &'static str,
    },
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
    build_remote_connection_from_values(
        key,
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

fn build_remote_connection_from_values(
    key: RemotePresetKey,
    api_key: Option<String>,
    base_url: Option<&str>,
) -> Result<ModelConnection, RemoteModelConnectionError> {
    let preset = required_catalog_preset(key)?;
    if preset.provider_kind != ProviderKind::Remote {
        return Err(RemoteModelConnectionError::NotRemotePreset {
            provider_key: key.provider_key,
            preset_id: key.preset_id,
        });
    }

    let api_key =
        if key.provider_key == "bedrock" {
            String::new()
        } else {
            let env_var = preset.credential_env_var.clone().ok_or(
                RemoteModelConnectionError::UnknownPreset {
                    provider_key: key.provider_key,
                    preset_id: key.preset_id,
                },
            )?;
            match api_key {
                Some(value) if !value.trim().is_empty() => value,
                _ => {
                    return Err(RemoteModelConnectionError::MissingCredential {
                        preset: preset.display_name,
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
    let stream_fn: Arc<dyn StreamFn> = match key.provider_key {
        "anthropic" => Arc::new(AnthropicStreamFn::new(resolved_base_url()?, &api_key)),
        "openai" => Arc::new(OpenAiStreamFn::new(resolved_base_url()?, &api_key)),
        "google" => Arc::new(GeminiStreamFn::new(
            resolved_base_url()?,
            &api_key,
            preset.api_version.clone().unwrap_or(ApiVersion::V1beta),
        )),
        "azure" => Arc::new(AzureStreamFn::new(resolved_base_url()?, &api_key)),
        "xai" => Arc::new(XAiStreamFn::new(resolved_base_url()?, &api_key)),
        "mistral" => Arc::new(MistralStreamFn::new(resolved_base_url()?, &api_key)),
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
            return Err(RemoteModelConnectionError::UnknownPreset {
                provider_key: key.provider_key,
                preset_id: key.preset_id,
            });
        }
    };
    Ok(ModelConnection::new(preset.model_spec(), stream_fn))
}

/// Looks up a remote preset by its `model_id` (e.g. `"claude-sonnet-4-6"`).
///
/// This is a convenience shorthand so callers can write
/// `preset("claude-sonnet-4-6")` instead of navigating
/// `remote_preset_keys::anthropic::SONNET_46` and the catalog manually.
#[must_use]
pub fn preset(model_id: &str) -> Option<CatalogPreset> {
    remote_presets(None)
        .into_iter()
        .find(|p| p.model_id == model_id)
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
        let anthropic = remote_presets(Some("anthropic"));
        let ids: Vec<_> = anthropic
            .iter()
            .map(|preset| preset.preset_id.as_str())
            .collect();
        assert_eq!(anthropic.len(), 3);
        assert!(ids.contains(&"opus_46"));
        assert!(ids.contains(&"sonnet_46"));
        assert!(ids.contains(&"haiku_45"));

        let openai = remote_presets(Some("openai"));
        let ids: Vec<_> = openai
            .iter()
            .map(|preset| preset.preset_id.as_str())
            .collect();
        assert_eq!(openai.len(), 6);
        assert!(ids.contains(&"gpt_5_4"));
        assert!(ids.contains(&"gpt_5_2"));
        assert!(ids.contains(&"gpt_5_1"));
        assert!(ids.contains(&"gpt_5_3_chat_latest"));
        assert!(ids.contains(&"gpt_5_2_chat_latest"));
        assert!(ids.contains(&"gpt_5_1_chat_latest"));

        let google = remote_presets(Some("google"));
        let ids: Vec<_> = google
            .iter()
            .map(|preset| preset.preset_id.as_str())
            .collect();
        assert_eq!(google.len(), 4);
        assert!(ids.contains(&"gemini_3_1_pro"));
        assert!(ids.contains(&"gemini_3_1_deep_think"));
        assert!(ids.contains(&"gemini_3_flash"));
        assert!(ids.contains(&"gemini_3_1_flash_lite"));

        let azure = remote_presets(Some("azure"));
        assert_eq!(azure.len(), 3);

        let xai = remote_presets(Some("xai"));
        assert_eq!(xai.len(), 2);

        let mistral = remote_presets(Some("mistral"));
        assert_eq!(mistral.len(), 3);

        let bedrock = remote_presets(Some("bedrock"));
        assert_eq!(bedrock.len(), 5);
    }

    #[test]
    fn remote_connection_uses_catalog_model_spec() {
        let connection = build_remote_connection_from_values(
            remote_preset_keys::anthropic::SONNET_46,
            Some("anthropic-test-key".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(
            connection.model_spec(),
            &required_catalog_preset(remote_preset_keys::anthropic::SONNET_46)
                .unwrap()
                .model_spec()
        );
    }

    #[test]
    fn added_anthropic_four_five_and_four_six_presets_map_to_catalog_models() {
        let opus = required_catalog_preset(remote_preset_keys::anthropic::OPUS_46).unwrap();
        assert_eq!(opus.model_id, "claude-opus-4-6");

        let sonnet = required_catalog_preset(remote_preset_keys::anthropic::SONNET_46).unwrap();
        assert_eq!(sonnet.model_id, "claude-sonnet-4-6");

        let haiku = required_catalog_preset(remote_preset_keys::anthropic::HAIKU_45).unwrap();
        assert_eq!(haiku.model_id, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn remote_preset_requires_key() {
        let Err(error) =
            build_remote_connection_from_values(remote_preset_keys::openai::GPT_5_2, None, None)
        else {
            panic!("expected missing credential error");
        };
        assert_eq!(
            error,
            RemoteModelConnectionError::MissingCredential {
                preset: "OpenAI GPT-5.2".to_string(),
                env_var: "OPENAI_API_KEY".to_string(),
            }
        );
    }

    #[test]
    fn added_openai_presets_map_to_catalog_models() {
        let gpt_54 = required_catalog_preset(remote_preset_keys::openai::GPT_5_4).unwrap();
        assert_eq!(gpt_54.model_id, "gpt-5.4");

        let gpt_51 = required_catalog_preset(remote_preset_keys::openai::GPT_5_1).unwrap();
        assert_eq!(gpt_51.model_id, "gpt-5.1");

        let gpt_53_chat =
            required_catalog_preset(remote_preset_keys::openai::GPT_5_3_CHAT_LATEST).unwrap();
        assert_eq!(gpt_53_chat.model_id, "gpt-5.3-chat-latest");

        let gpt_52_chat =
            required_catalog_preset(remote_preset_keys::openai::GPT_5_2_CHAT_LATEST).unwrap();
        assert_eq!(gpt_52_chat.model_id, "gpt-5.2-chat-latest");

        let gpt_51_chat =
            required_catalog_preset(remote_preset_keys::openai::GPT_5_1_CHAT_LATEST).unwrap();
        assert_eq!(gpt_51_chat.model_id, "gpt-5.1-chat-latest");
    }

    #[test]
    fn google_presets_map_to_catalog_models() {
        let gemini_31_pro =
            required_catalog_preset(remote_preset_keys::google::GEMINI_3_1_PRO).unwrap();
        assert_eq!(gemini_31_pro.model_id, "gemini-3.1-pro-preview");

        let gemini_31_deep_think =
            required_catalog_preset(remote_preset_keys::google::GEMINI_3_1_DEEP_THINK).unwrap();
        assert_eq!(
            gemini_31_deep_think.model_id,
            "gemini-3.1-deep-think-preview"
        );

        let gemini_3_flash =
            required_catalog_preset(remote_preset_keys::google::GEMINI_3_FLASH).unwrap();
        assert_eq!(gemini_3_flash.model_id, "gemini-3-flash-preview");

        let gemini_31_flash_lite =
            required_catalog_preset(remote_preset_keys::google::GEMINI_3_1_FLASH_LITE).unwrap();
        assert_eq!(
            gemini_31_flash_lite.model_id,
            "gemini-3.1-flash-lite-preview"
        );
    }

    #[test]
    fn preset_finds_by_model_id() {
        let sonnet = preset("claude-sonnet-4-6").expect("sonnet preset should exist");
        assert_eq!(sonnet.provider_key, "anthropic");
        assert_eq!(sonnet.preset_id, "sonnet_46");

        let gpt = preset("gpt-5.4").expect("gpt-5.4 preset should exist");
        assert_eq!(gpt.provider_key, "openai");

        assert!(preset("nonexistent-model-xyz").is_none());
    }

    #[test]
    fn added_provider_presets_map_to_catalog_models() {
        assert_eq!(
            required_catalog_preset(remote_preset_keys::azure::GPT_5_4)
                .unwrap()
                .model_id,
            "gpt-5.4"
        );
        assert_eq!(
            required_catalog_preset(remote_preset_keys::xai::GROK_3)
                .unwrap()
                .model_id,
            "grok-3"
        );
        assert_eq!(
            required_catalog_preset(remote_preset_keys::mistral::CODESTRAL)
                .unwrap()
                .model_id,
            "codestral-latest"
        );
        assert_eq!(
            required_catalog_preset(remote_preset_keys::bedrock::AMAZON_NOVA_PRO)
                .unwrap()
                .group
                .as_deref(),
            Some("amazon")
        );
    }
}
