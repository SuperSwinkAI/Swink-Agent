//! TUI configuration loaded from `~/.config/swink-agent/tui.toml`.

use serde::Deserialize;
use swink_agent::PricingTable;

/// TUI configuration.
///
/// Deserialized from TOML, so it holds data only. Host-supplied *code* — custom
/// commands and other extension points — goes through
/// [`TuiExtensions`](crate::TuiExtensions) instead.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// Whether to show thinking content.
    pub show_thinking: bool,
    /// Whether to auto-scroll on new messages.
    pub auto_scroll: bool,
    /// Tick rate in milliseconds.
    pub tick_rate_ms: u64,
    /// Default model identifier.
    pub default_model: String,
    /// Theme preset name.
    pub theme: String,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Override for external editor command (defaults to `$EDITOR` / `$VISUAL` / `vi`).
    pub editor_command: Option<String>,
    /// Color mode: `"custom"`, `"mono-white"`, or `"mono-black"`.
    pub color_mode: String,
    /// Operator-declared per-model rates, in USD per million tokens.
    ///
    /// The agent loop prices assistant messages from the compiled model
    /// catalog, which only covers models shipped with the crate — local
    /// endpoints and negotiated per-tier rates otherwise show `$0.0000` in the
    /// status bar and `/usage`. Declaring rates here fixes that, and takes
    /// precedence over the catalog for any model listed:
    ///
    /// ```toml
    /// [pricing."my-local-llama"]
    /// input_per_million = 0.10
    /// output_per_million = 0.40
    ///
    /// [pricing."claude-sonnet-4-6"]
    /// input_per_million = 1.50   # negotiated below the catalog's $3.00
    /// output_per_million = 7.50
    /// ```
    ///
    /// Rates are not applied automatically — the loop only sees them if the
    /// host passes them to the agent. [`apply_pricing`](Self::apply_pricing)
    /// does that; [`launch`](crate::launch) and
    /// [`launch_with_extensions`](crate::launch_with_extensions) call it for
    /// you.
    pub pricing: PricingTable,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            show_thinking: true,
            auto_scroll: true,
            tick_rate_ms: 33,
            default_model: "not connected".to_string(),
            theme: "default".to_string(),
            system_prompt: None,
            editor_command: None,
            color_mode: "custom".to_string(),
            pricing: PricingTable::new(),
        }
    }
}

impl TuiConfig {
    /// Load configuration from the default config path.
    /// Falls back to defaults if the file doesn't exist or is invalid.
    pub fn load() -> Self {
        let Some(config_dir) = dirs::config_dir() else {
            return Self::default();
        };

        let config_path = config_dir.join("swink-agent").join("tui.toml");

        let Ok(contents) = std::fs::read_to_string(&config_path) else {
            return Self::default();
        };

        toml::from_str(&contents).unwrap_or_default()
    }

    /// Parse configuration from a TOML string.
    ///
    /// Falls back to defaults for any missing or invalid fields.
    pub fn from_toml(toml_str: &str) -> Self {
        toml::from_str(toml_str).unwrap_or_default()
    }

    /// Attach this config's [`pricing`](Self::pricing) table to the agent
    /// options, so the loop prices assistant messages with the operator's rates
    /// before falling back to the compiled model catalog.
    ///
    /// A no-op when no rates are declared, which leaves any calculator the host
    /// already configured on `options` intact. When rates *are* declared they
    /// replace that calculator — config wins over code, because the operator
    /// editing `tui.toml` is the later authority.
    ///
    /// [`launch`](crate::launch) and
    /// [`launch_with_extensions`](crate::launch_with_extensions) call this, so
    /// only a host that builds its own [`Agent`](swink_agent::Agent) needs it
    /// directly.
    #[must_use]
    pub fn apply_pricing(&self, options: swink_agent::AgentOptions) -> swink_agent::AgentOptions {
        if self.pricing.is_empty() {
            return options;
        }
        options.with_pricing_table(self.pricing.clone())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
