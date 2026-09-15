//! Regression guard for #1314: the remote TUI must not re-enable the root
//! crate's local-execution features through `swink-agent-tui`.

use std::process::Command;

fn root_features_for(extra_args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "-p",
            "swink-agent-tui-remote",
            "-e",
            "normal,features",
            "-i",
            "swink-agent",
            "--prefix",
            "none",
        ])
        .args(extra_args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("cargo tree output is UTF-8")
}

#[test]
fn remote_tui_does_not_enable_root_local_features() {
    for args in [&[][..], &["--no-default-features"][..]] {
        let tree = root_features_for(args);
        for feature in ["builtin-tools", "transfer", "default"] {
            let needle = format!("swink-agent feature \"{feature}\"");
            assert!(
                !tree.lines().any(|line| line.starts_with(&needle)),
                "swink-agent-tui-remote ({args:?}) activates root `{feature}`:\n{tree}"
            );
        }
    }
}
