//! TUI configuration loaded from `~/.config/agent-harness/tui.toml`.

use serde::Deserialize;

/// TUI configuration.
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
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            show_thinking: true,
            auto_scroll: true,
            tick_rate_ms: 100,
            default_model: "not connected".to_string(),
            theme: "default".to_string(),
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

        let config_path = config_dir.join("agent-harness").join("tui.toml");

        let Ok(contents) = std::fs::read_to_string(&config_path) else {
            return Self::default();
        };

        toml::from_str(&contents).unwrap_or_default()
    }
}
