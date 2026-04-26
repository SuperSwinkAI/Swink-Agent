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
        Some(env_var) => match std::env::var(env_var) {
            Ok(key) if !key.trim().is_empty() => key,
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
}
