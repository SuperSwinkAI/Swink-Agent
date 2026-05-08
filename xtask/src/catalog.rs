use swink_agent::{AuthMode, ProviderCatalog, ProviderKind, model_catalog};

#[derive(Debug, PartialEq, Eq)]
pub struct UnknownProviderFilter {
    pub provider_key: String,
    pub valid_provider_keys: Vec<String>,
}

pub struct VerifyTask {
    pub provider_key: String,
    pub preset_id: String,
    pub preset_display: String,
    pub model_id: String,
    pub endpoint: ProviderEndpoint,
}

#[derive(Clone)]
pub enum ProviderEndpoint {
    Anthropic { base_url: String, api_key: String },
    OpenAiCompat { base_url: String, api_key: String },
    Google { base_url: String, api_key: String },
    Skipped { reason: &'static str },
}

pub fn build_verify_tasks(
    provider_filter: Option<&str>,
) -> Result<Vec<VerifyTask>, UnknownProviderFilter> {
    let catalog = model_catalog();
    let valid_provider_keys: Vec<String> = catalog
        .providers
        .iter()
        .map(|provider| provider.key.clone())
        .collect();

    if let Some(filter) = provider_filter
        && !catalog
            .providers
            .iter()
            .any(|provider| provider.key == filter)
    {
        return Err(UnknownProviderFilter {
            provider_key: filter.to_owned(),
            valid_provider_keys,
        });
    }

    let mut tasks = Vec::new();
    for provider in &catalog.providers {
        if provider_filter.is_some_and(|f| f != provider.key) {
            continue;
        }
        let endpoint = resolve_endpoint(provider);
        for preset in &provider.presets {
            tasks.push(VerifyTask {
                provider_key: provider.key.clone(),
                preset_id: preset.id.clone(),
                preset_display: preset.display_name.clone(),
                model_id: preset.model_id.clone(),
                endpoint: endpoint.clone(),
            });
        }
    }
    Ok(tasks)
}

fn resolve_endpoint(provider: &ProviderCatalog) -> ProviderEndpoint {
    resolve_endpoint_with_env(provider, |env_var| std::env::var(env_var).ok())
}

fn resolve_endpoint_with_env(
    provider: &ProviderCatalog,
    env_var_value: impl Fn(&str) -> Option<String>,
) -> ProviderEndpoint {
    if provider.kind == ProviderKind::Local {
        return ProviderEndpoint::Skipped {
            reason: "local provider",
        };
    }
    if provider.requires_base_url {
        return ProviderEndpoint::Skipped {
            reason: "requires_base_url",
        };
    }
    if provider.auth_mode == Some(AuthMode::AwsSigv4) {
        return ProviderEndpoint::Skipped {
            reason: "aws_sigv4 auth",
        };
    }
    let api_key = match &provider.credential_env_var {
        Some(env_var) => match env_var_value(env_var) {
            Some(key) if !key.trim().is_empty() => key,
            _ => {
                return ProviderEndpoint::Skipped {
                    reason: "missing credential",
                };
            }
        },
        None => {
            return ProviderEndpoint::Skipped {
                reason: "no credential_env_var",
            };
        }
    };
    let base_url = provider.default_base_url.clone().unwrap_or_default();
    match provider.key.as_str() {
        "anthropic" => ProviderEndpoint::Anthropic { base_url, api_key },
        "google" => ProviderEndpoint::Google { base_url, api_key },
        _ => ProviderEndpoint::OpenAiCompat { base_url, api_key },
    }
}

#[cfg(test)]
mod tests {
    use swink_agent::{AuthMode, ProviderCatalog, ProviderKind};

    fn provider(key: &str) -> ProviderCatalog {
        ProviderCatalog {
            key: key.to_owned(),
            display_name: key.to_owned(),
            kind: ProviderKind::Remote,
            auth_mode: Some(AuthMode::Bearer),
            credential_env_var: None,
            base_url_env_var: None,
            default_base_url: Some(format!("https://api.{key}.example")),
            requires_base_url: false,
            region_env_var: None,
            presets: Vec::new(),
        }
    }

    #[test]
    fn unknown_provider_filter_reports_valid_keys() {
        let result = super::build_verify_tasks(Some("does-not-exist"));
        let Err(error) = result else {
            panic!("unknown provider should fail");
        };

        assert_eq!(error.provider_key, "does-not-exist");
        assert!(
            error
                .valid_provider_keys
                .iter()
                .any(|key| key == "anthropic"),
            "expected anthropic in valid key list"
        );
        assert!(
            error.valid_provider_keys.iter().any(|key| key == "openai"),
            "expected openai in valid key list"
        );
    }

    #[test]
    fn known_provider_filter_builds_tasks() {
        let tasks = super::build_verify_tasks(Some("anthropic"))
            .expect("known provider should build tasks");

        assert!(
            !tasks.is_empty(),
            "expected anthropic provider to produce verification tasks"
        );
        assert!(tasks.iter().all(|task| task.provider_key == "anthropic"));
    }

    #[test]
    fn local_provider_endpoint_is_skipped_before_credentials() {
        let mut provider = provider("local");
        provider.kind = ProviderKind::Local;

        let endpoint = super::resolve_endpoint(&provider);

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Skipped {
                reason: "local provider"
            }
        ));
    }

    #[test]
    fn requires_base_url_endpoint_is_skipped_before_credentials() {
        let mut provider = provider("custom");
        provider.requires_base_url = true;

        let endpoint = super::resolve_endpoint(&provider);

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Skipped {
                reason: "requires_base_url"
            }
        ));
    }

    #[test]
    fn aws_sigv4_endpoint_is_skipped_before_credentials() {
        let mut provider = provider("bedrock");
        provider.auth_mode = Some(AuthMode::AwsSigv4);

        let endpoint = super::resolve_endpoint(&provider);

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Skipped {
                reason: "aws_sigv4 auth"
            }
        ));
    }

    #[test]
    fn provider_without_credential_env_var_is_skipped() {
        let endpoint = super::resolve_endpoint(&provider("openai"));

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Skipped {
                reason: "no credential_env_var"
            }
        ));
    }

    #[test]
    fn empty_credential_env_var_is_skipped() {
        let mut provider = provider("openai");
        provider.credential_env_var = Some("OPENAI_API_KEY".to_owned());

        let endpoint = super::resolve_endpoint_with_env(&provider, |_| Some("  ".to_owned()));

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Skipped {
                reason: "missing credential"
            }
        ));
    }

    #[test]
    fn anthropic_endpoint_uses_anthropic_variant() {
        let mut provider = provider("anthropic");
        provider.credential_env_var = Some("ANTHROPIC_API_KEY".to_owned());

        let endpoint =
            super::resolve_endpoint_with_env(&provider, |name| Some(format!("{name}-value")));

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Anthropic {
                base_url,
                api_key
            } if base_url == "https://api.anthropic.example"
                && api_key == "ANTHROPIC_API_KEY-value"
        ));
    }

    #[test]
    fn google_endpoint_uses_google_variant() {
        let mut provider = provider("google");
        provider.credential_env_var = Some("GOOGLE_API_KEY".to_owned());

        let endpoint =
            super::resolve_endpoint_with_env(&provider, |name| Some(format!("{name}-value")));

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::Google {
                base_url,
                api_key
            } if base_url == "https://api.google.example"
                && api_key == "GOOGLE_API_KEY-value"
        ));
    }

    #[test]
    fn bearer_remote_endpoint_uses_openai_compatible_variant() {
        let mut provider = provider("mistral");
        provider.credential_env_var = Some("MISTRAL_API_KEY".to_owned());

        let endpoint =
            super::resolve_endpoint_with_env(&provider, |name| Some(format!("{name}-value")));

        assert!(matches!(
            endpoint,
            super::ProviderEndpoint::OpenAiCompat {
                base_url,
                api_key
            } if base_url == "https://api.mistral.example"
                && api_key == "MISTRAL_API_KEY-value"
        ));
    }
}
