use std::process::Command;

use serde_json::Value;

fn adapters_package_metadata() -> Value {
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
                .find(|package| package["name"].as_str() == Some("swink-agent-adapters"))
        })
        .cloned()
        .expect("swink-agent-adapters package metadata should be present")
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
