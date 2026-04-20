use std::fs;
use std::path::PathBuf;

use toml::Value;

const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "adapters/Cargo.toml",
    "artifacts/Cargo.toml",
    "auth/Cargo.toml",
    "eval/Cargo.toml",
    "local-llm/Cargo.toml",
    "mcp/Cargo.toml",
    "memory/Cargo.toml",
    "patterns/Cargo.toml",
    "plugins/web/Cargo.toml",
    "policies/Cargo.toml",
    "tui/Cargo.toml",
    "xtask/Cargo.toml",
];

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_manifest(relative_path: &str) -> Value {
    let manifest_path = manifest_root().join(relative_path);
    let raw = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    toml::from_str::<Value>(&raw)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()))
}

fn tokio_features(section: Option<&Value>) -> Vec<String> {
    section
        .and_then(Value::as_table)
        .and_then(|table| table.get("tokio"))
        .and_then(Value::as_table)
        .and_then(|tokio| tokio.get("features"))
        .and_then(Value::as_array)
        .map(|features| {
            features
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn workspace_tokio_baseline_avoids_full_profile() {
    let manifest = load_manifest("Cargo.toml");
    let tokio_features = manifest
        .get("workspace")
        .and_then(Value::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Value::as_table)
        .and_then(|dependencies| dependencies.get("tokio"))
        .and_then(Value::as_table)
        .and_then(|tokio| tokio.get("features"));

    assert!(
        tokio_features.is_none(),
        "workspace tokio should stay feature-minimal; crate-specific features belong in member manifests"
    );
}

#[test]
fn production_tokio_dependencies_do_not_use_full_or_test_util() {
    for manifest_path in MANIFESTS {
        let manifest = load_manifest(manifest_path);
        let features = tokio_features(manifest.get("dependencies"));

        assert!(
            !features
                .iter()
                .any(|feature| feature == "full" || feature == "test-util"),
            "{manifest_path} should not enable broad tokio production features: {features:?}"
        );
    }
}
