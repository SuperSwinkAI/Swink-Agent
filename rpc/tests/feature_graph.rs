use std::fs;

use toml::Value;

fn manifest() -> Value {
    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let raw = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    toml::from_str::<Value>(&raw)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()))
}

#[test]
fn rpc_core_dependency_keeps_default_features_disabled() {
    let manifest = manifest();
    let dependency = manifest
        .get("dependencies")
        .and_then(Value::as_table)
        .and_then(|dependencies| dependencies.get("swink-agent"))
        .and_then(Value::as_table)
        .expect("swink-agent dependency should be declared as a table");

    assert_eq!(
        dependency.get("default-features").and_then(Value::as_bool),
        Some(false),
        "swink-agent-rpc should not pull core default features into protocol/client/server consumers"
    );
}

#[test]
fn daemon_cli_feature_opts_into_builtin_tools_explicitly() {
    let manifest = manifest();
    let cli_features = manifest
        .get("features")
        .and_then(Value::as_table)
        .and_then(|features| features.get("cli"))
        .and_then(Value::as_array)
        .expect("cli feature should be declared as an array");

    assert!(
        cli_features
            .iter()
            .filter_map(Value::as_str)
            .any(|feature| feature == "swink-agent/builtin-tools"),
        "swink-agentd calls AgentOptions::with_default_tools(), so cli must opt into core builtin-tools explicitly"
    );
}
