use std::collections::BTreeMap;
use std::process::Command;

use serde_json::Value;

fn adapters_package_metadata() -> Value {
    workspace_package_metadata("swink-agent-adapters")
}

/// `cargo metadata --no-deps` lists every workspace member, so sibling crates'
/// manifests are reachable from here without extra dev-dependencies.
fn workspace_package_metadata(name: &str) -> Value {
    let manifest_path = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output()
        .expect("cargo metadata should run for adapters manifest");

    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata should emit valid JSON");

    metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"].as_str() == Some(name))
        })
        .cloned()
        .unwrap_or_else(|| panic!("{name} package metadata should be present"))
}

#[test]
fn azure_feature_owns_auth_dependency() {
    let package = adapters_package_metadata();
    let dependencies = package["dependencies"]
        .as_array()
        .expect("package dependencies should be an array");
    let auth_dependency = dependencies
        .iter()
        .find(|dependency| dependency["name"].as_str() == Some("swink-agent-auth"))
        .expect("swink-agent-auth dependency should be present");

    assert_eq!(
        auth_dependency["optional"].as_bool(),
        Some(true),
        "swink-agent-auth should stay optional so non-Azure builds do not pull it in"
    );

    let azure_feature = package["features"]["azure"]
        .as_array()
        .expect("azure feature should be declared");
    assert!(
        azure_feature
            .iter()
            .any(|entry| entry.as_str() == Some("dep:swink-agent-auth")),
        "azure feature should own swink-agent-auth"
    );
}

#[test]
fn default_profile_enables_no_provider_features() {
    let package = adapters_package_metadata();
    let default_feature = package["features"]["default"]
        .as_array()
        .expect("default feature should be declared");

    assert!(
        default_feature.is_empty(),
        "swink-agent-adapters should not enable providers by default"
    );
}

/// Feature -> implied features (excluding `dep:` entries), sorted.
fn feature_implications(package: &Value) -> BTreeMap<String, Vec<String>> {
    package["features"]
        .as_object()
        .expect("features should be an object")
        .iter()
        .map(|(name, implies)| {
            let entries = implies
                .as_array()
                .expect("feature entries should be an array");
            (
                name.clone(),
                sorted(entries.iter().filter_map(Value::as_str)),
            )
        })
        .collect()
}

fn sorted<'a>(entries: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut entries: Vec<String> = entries
        .filter(|entry| !entry.starts_with("dep:"))
        .map(str::to_owned)
        .collect();
    entries.sort();
    entries
}

fn contract(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(name, implies)| ((*name).to_owned(), sorted(implies.iter().copied())))
        .collect()
}

/// Feature names and implications are a semver surface documented in
/// `specs/033-workspace-feature-gates/contracts/feature-surface.md` (#1313).
/// If this fails, update that contract alongside the manifest.
#[test]
fn adapters_features_match_feature_surface_contract() {
    let actual = feature_implications(&adapters_package_metadata());
    let expected = contract(&[
        ("default", &[]),
        (
            "all",
            &[
                "anthropic",
                "openai",
                "ollama",
                "gemini",
                "proxy",
                "azure",
                "bedrock",
                "mistral",
                "xai",
                "responses",
                "codex",
            ],
        ),
        ("full", &["all"]),
        ("anthropic", &[]),
        ("openai-compat", &[]),
        ("openai", &["openai-compat", "responses"]),
        ("responses", &[]),
        ("codex", &["responses"]),
        ("ollama", &[]),
        ("gemini", &[]),
        ("proxy", &[]),
        ("azure", &[]),
        ("bedrock", &[]),
        ("mistral", &[]),
        ("xai", &["openai-compat"]),
        ("__no_default_features_sentinel", &[]),
    ]);
    assert_eq!(actual, expected);
}

/// See `adapters_features_match_feature_surface_contract`.
#[test]
fn local_llm_features_match_feature_surface_contract() {
    let actual = feature_implications(&workspace_package_metadata("swink-agent-local-llm"));
    let expected = contract(&[
        ("gemma4", &[]),
        ("metal", &["llama-cpp-2/metal"]),
        ("cuda", &["llama-cpp-2/cuda"]),
        ("cudnn", &["cuda"]),
        ("vulkan", &["llama-cpp-2/vulkan"]),
    ]);
    assert_eq!(actual, expected);
}
