#![forbid(unsafe_code)]
//! Swink Agent TUI — interactive terminal interface for LLM agents.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use swink_agent::{
    AgentOptions, CatalogPreset, ModelConnection, ModelConnections, ModelSpec, StreamFn,
};
use swink_agent_adapters::{
    AnthropicStreamFn, OllamaStreamFn, OpenAiStreamFn, ProxyStreamFn, remote_presets,
};
#[cfg(feature = "local")]
use swink_agent_local_llm::default_local_connection;

use swink_agent_tui::{
    TuiConfig, TuiError, credentials, launch, resolve_system_prompt, restore_terminal,
    setup_terminal, wizard,
};

type AppResult<T> = Result<T, TuiError>;

fn main() -> AppResult<()> {
    if !std::io::stdout().is_terminal() {
        eprintln!("Error: swink-agent-tui requires an interactive terminal (TTY).");
        eprintln!("Cannot run in a non-interactive environment (e.g., piped input/output).");
        std::process::exit(1);
    }

    dotenvy::dotenv().ok();

    // Initialize file-based tracing (TUI owns stdout, so we log to a file).
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("swink-agent")
        .join("logs");
    let file_appender = tracing_appender::rolling::daily(log_dir, "swink-agent.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("swink_agent=info".parse().unwrap()),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    // Install panic hook that restores terminal before printing panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal);
    restore_terminal()?;
    result
}

fn run(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> AppResult<()> {
    // Skip the setup wizard when the `local` feature already provides a usable
    // first-run path with no credentials.
    if should_run_setup_wizard() {
        let mut wiz = wizard::SetupWizard::new();
        if !wiz.run(terminal)? {
            return Ok(()); // User chose to quit from wizard
        }
    }

    let config = TuiConfig::load();
    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let system_prompt = resolve_system_prompt(None, &config);

        launch(config, terminal, create_options(system_prompt))
            .await
            .map_err(|e| TuiError::Other(e.to_string().into()))
    })
}

/// Build agent options using catalog-driven model construction.
///
/// Providers are checked in priority order:
/// 1. **Proxy** — `LLM_BASE_URL` set (custom SSE endpoint)
/// 2. **`OpenAI`** — `OPENAI_API_KEY` env or keychain
/// 3. **`Anthropic`** — `ANTHROPIC_API_KEY` env or keychain
/// 4. **Local** — bundled `swink-agent-local-llm` preset when built with `local`
/// 5. **Ollama** — local Ollama instance (default fallback)
///
/// Model IDs and base URLs are resolved from the shared model catalog.
/// Provider-specific env var overrides (`OPENAI_MODEL`, `ANTHROPIC_MODEL`, etc.)
/// are still respected for backward compatibility.
fn create_options(system_prompt: String) -> AgentOptions {
    AgentOptions::from_connections(system_prompt, resolve_connections())
}

/// Resolve model connections by trying providers in priority order.
fn resolve_connections() -> ModelConnections {
    if let Some(conns) = try_proxy() {
        return conns;
    }
    if let Some(conns) = try_catalog_provider("openai", "OPENAI_MODEL") {
        return conns;
    }
    if let Some(conns) = try_catalog_provider("anthropic", "ANTHROPIC_MODEL") {
        return conns;
    }
    if let Some(conns) = try_local() {
        return conns;
    }

    ollama_connections()
}

fn should_run_setup_wizard() -> bool {
    should_run_setup_wizard_with(credentials::any_key_configured())
}

const fn should_run_setup_wizard_with(any_key_configured: bool) -> bool {
    !any_key_configured && !cfg!(feature = "local")
}

/// Build connections for proxy mode (highest priority, not in catalog).
fn try_proxy() -> Option<ModelConnections> {
    let base_url = std::env::var("LLM_BASE_URL").ok()?;
    let proxy_provider = credentials::providers()
        .into_iter()
        .find(|p| p.key_name == "proxy");
    let api_key = proxy_provider
        .as_ref()
        .and_then(credentials::credential)
        .unwrap_or_default();
    let model_id =
        std::env::var("LLM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
    let stream_fn: Arc<dyn StreamFn> = Arc::new(ProxyStreamFn::new(&base_url, &api_key));
    let model = ModelSpec::new("proxy", &model_id);

    Some(
        ModelConnections::builder()
            .primary(ModelConnection::new(model, stream_fn))
            .build(),
    )
}

/// Build connections for a catalog-backed remote provider.
///
/// Resolves credentials via the TUI keychain/env system, then uses the model
/// catalog to discover available models and default base URLs — eliminating
/// hardcoded model lists and URLs.
fn try_catalog_provider(provider_key: &str, model_env: &str) -> Option<ModelConnections> {
    let cred_provider = credentials::providers()
        .into_iter()
        .find(|p| p.key_name == provider_key)?;
    let api_key = credentials::credential(&cred_provider)?;

    let presets = remote_presets(Some(provider_key));
    if presets.is_empty() {
        return None;
    }

    // Resolve base URL: env override > catalog default
    let base_url_env = presets[0]
        .base_url_env_var
        .as_deref()
        .and_then(|var| std::env::var(var).ok());
    let base_url = base_url_env
        .as_deref()
        .or(presets[0].default_base_url.as_deref())?;

    let stream_fn: Arc<dyn StreamFn> = build_stream_fn(provider_key, base_url, &api_key)?;

    // Determine primary model: env override > first catalog preset
    let model_override = std::env::var(model_env).ok();
    let primary_model_id = model_override.as_deref().unwrap_or(&presets[0].model_id);

    // Find the catalog preset matching the primary model (for capabilities metadata)
    let primary_spec = presets
        .iter()
        .find(|p| p.model_id == primary_model_id)
        .map_or_else(
            || ModelSpec::new(provider_key, primary_model_id),
            CatalogPreset::model_spec,
        );

    let mut builder = ModelConnections::builder()
        .primary(ModelConnection::new(primary_spec, Arc::clone(&stream_fn)));

    // Add remaining catalog presets as fallbacks (excluding primary)
    for preset in &presets {
        if preset.model_id != primary_model_id {
            builder = builder.fallback(ModelConnection::new(
                preset.model_spec(),
                Arc::clone(&stream_fn),
            ));
        }
    }

    Some(builder.build())
}

#[cfg(feature = "local")]
fn try_local() -> Option<ModelConnections> {
    default_local_connection()
        .ok()
        .map(|connection| ModelConnections::builder().primary(connection).build())
}

#[cfg(not(feature = "local"))]
fn try_local() -> Option<ModelConnections> {
    None
}

/// Construct the appropriate `StreamFn` for a provider.
fn build_stream_fn(provider_key: &str, base_url: &str, api_key: &str) -> Option<Arc<dyn StreamFn>> {
    match provider_key {
        "openai" => Some(Arc::new(OpenAiStreamFn::new(base_url, api_key))),
        "anthropic" => Some(Arc::new(AnthropicStreamFn::new(base_url, api_key))),
        _ => None,
    }
}

/// Build connections for Ollama (lowest priority fallback).
fn ollama_connections() -> ModelConnections {
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model_id = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".to_string());
    let stream_fn: Arc<dyn StreamFn> = Arc::new(OllamaStreamFn::new(&host));
    let model = ModelSpec::new("ollama", &model_id);

    ModelConnections::builder()
        .primary(ModelConnection::new(model, stream_fn))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_stream_fn_returns_openai_for_openai_key() {
        let sfn = build_stream_fn("openai", "https://api.openai.com", "test-key");
        assert!(sfn.is_some(), "openai provider should produce a StreamFn");
    }

    #[test]
    fn build_stream_fn_returns_anthropic_for_anthropic_key() {
        let sfn = build_stream_fn("anthropic", "https://api.anthropic.com", "test-key");
        assert!(
            sfn.is_some(),
            "anthropic provider should produce a StreamFn"
        );
    }

    #[test]
    fn build_stream_fn_returns_none_for_unknown_provider() {
        let sfn = build_stream_fn("unknown_provider", "https://example.com", "key");
        assert!(sfn.is_none(), "unknown provider should return None");
    }

    #[test]
    fn catalog_presets_contain_expected_providers() {
        let anthropic_presets = remote_presets(Some("anthropic"));
        assert!(
            !anthropic_presets.is_empty(),
            "catalog should have anthropic presets"
        );
        assert!(
            anthropic_presets
                .iter()
                .any(|p| p.model_id.contains("claude")),
            "anthropic presets should contain claude models"
        );

        let openai_presets = remote_presets(Some("openai"));
        assert!(
            !openai_presets.is_empty(),
            "catalog should have openai presets"
        );
        assert!(
            openai_presets.iter().any(|p| p.model_id == "gpt-5.4"),
            "openai presets should contain gpt-5.4"
        );
    }

    #[test]
    fn catalog_presets_provide_model_specs_with_capabilities() {
        let presets = remote_presets(Some("anthropic"));
        let sonnet = presets
            .iter()
            .find(|p| p.model_id.contains("sonnet"))
            .expect("catalog should have a sonnet preset");
        let spec = sonnet.model_spec();
        assert_eq!(spec.provider, "anthropic");
        assert!(
            spec.capabilities
                .as_ref()
                .is_some_and(|c| c.supports_tool_use),
            "sonnet should support tool use"
        );
    }

    #[test]
    fn try_proxy_returns_none_without_env_var() {
        // LLM_BASE_URL is not normally set in test environments
        if std::env::var("LLM_BASE_URL").is_err() {
            assert!(try_proxy().is_none());
        }
    }

    #[cfg(not(feature = "local"))]
    #[test]
    fn setup_wizard_runs_when_no_keys_and_no_local_provider() {
        assert!(should_run_setup_wizard_with(false));
        assert!(!should_run_setup_wizard_with(true));
    }

    #[cfg(feature = "local")]
    #[test]
    fn setup_wizard_is_skipped_when_local_feature_is_enabled() {
        assert!(!should_run_setup_wizard_with(false));
        assert!(!should_run_setup_wizard_with(true));
    }

    #[cfg(feature = "local")]
    #[test]
    fn try_local_returns_local_connection_when_feature_enabled() {
        let connections = try_local().expect("local feature should provide a default connection");
        assert_eq!(connections.primary_model().provider, "local");
        assert!(
            !connections.primary_model().model_id.is_empty(),
            "local connection should expose a catalog-backed model id"
        );
    }
}
