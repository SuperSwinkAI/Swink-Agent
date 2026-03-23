//! TUI configuration loaded from `~/.config/swink-agent/tui.toml`.

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
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Override for external editor command (defaults to `$EDITOR` / `$VISUAL` / `vi`).
    pub editor_command: Option<String>,
    /// Color mode: `"custom"`, `"mono-white"`, or `"mono-black"`.
    pub color_mode: String,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = TuiConfig::default();
        assert!(config.show_thinking);
        assert!(config.auto_scroll);
        assert_eq!(config.tick_rate_ms, 33);
        assert_eq!(config.default_model, "not connected");
        assert_eq!(config.theme, "default");
    }

    #[test]
    fn from_toml_full_override() {
        let toml = r#"
            show_thinking = false
            auto_scroll = false
            tick_rate_ms = 200
            default_model = "gpt-4o"
            theme = "dark"
            color_mode = "mono-white"
        "#;
        let config = TuiConfig::from_toml(toml);
        assert!(!config.show_thinking);
        assert!(!config.auto_scroll);
        assert_eq!(config.tick_rate_ms, 200);
        assert_eq!(config.default_model, "gpt-4o");
        assert_eq!(config.theme, "dark");
        assert_eq!(config.color_mode, "mono-white");
    }

    #[test]
    fn from_toml_partial_override_uses_defaults() {
        let toml = r"
            show_thinking = false
        ";
        let config = TuiConfig::from_toml(toml);
        assert!(!config.show_thinking);
        // Other fields should be defaults
        assert!(config.auto_scroll);
        assert_eq!(config.tick_rate_ms, 33);
        assert_eq!(config.default_model, "not connected");
        assert_eq!(config.theme, "default");
    }

    #[test]
    fn from_toml_empty_string_uses_defaults() {
        let config = TuiConfig::from_toml("");
        assert!(config.show_thinking);
        assert!(config.auto_scroll);
        assert_eq!(config.tick_rate_ms, 33);
    }

    #[test]
    fn from_toml_invalid_falls_back_to_defaults() {
        let config = TuiConfig::from_toml("this is not valid toml {{{{");
        assert!(config.show_thinking);
        assert_eq!(config.tick_rate_ms, 33);
    }

    #[test]
    fn from_toml_editor_command() {
        let toml = r#"editor_command = "nano""#;
        let config = TuiConfig::from_toml(toml);
        assert_eq!(config.editor_command.as_deref(), Some("nano"));
    }

    #[test]
    fn from_toml_unknown_fields_ignored() {
        let toml = r#"
            show_thinking = false
            unknown_field = "hello"
        "#;
        let config = TuiConfig::from_toml(toml);
        assert!(!config.show_thinking);
    }
}
