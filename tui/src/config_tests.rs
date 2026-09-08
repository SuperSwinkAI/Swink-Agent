//! Tests for `config`.
#![cfg(test)]

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
fn default_pricing_is_empty() {
    assert!(TuiConfig::default().pricing.is_empty());
}

#[test]
fn from_toml_parses_pricing_section() {
    let toml = r#"
            default_model = "my-local-llama"

            [pricing."my-local-llama"]
            input_per_million = 0.10
            output_per_million = 0.40
        "#;
    let config = TuiConfig::from_toml(toml);
    assert_eq!(config.default_model, "my-local-llama");
    assert_eq!(config.pricing.len(), 1);

    let rates = config
        .pricing
        .get("my-local-llama")
        .expect("rates declared");
    assert!((rates.input_per_million - 0.10).abs() < 1e-9);
    assert!((rates.output_per_million - 0.40).abs() < 1e-9);
}

#[test]
fn from_toml_parses_multiple_pricing_entries() {
    let toml = r#"
            [pricing."model-a"]
            input_per_million = 1.0

            [pricing."model-b"]
            input_per_million = 2.0
            cache_read_per_million = 0.25
        "#;
    let config = TuiConfig::from_toml(toml);
    assert_eq!(config.pricing.len(), 2);
    assert!(
        (config
            .pricing
            .get("model-b")
            .unwrap()
            .cache_read_per_million
            - 0.25)
            .abs()
            < 1e-9
    );
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
