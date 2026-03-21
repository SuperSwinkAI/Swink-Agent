use swink_agent::{AuthMode, ProviderCatalog, ProviderKind, model_catalog};

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

pub fn build_verify_tasks(provider_filter: Option<&str>) -> Vec<VerifyTask> {
    let catalog = model_catalog();
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
    tasks
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
