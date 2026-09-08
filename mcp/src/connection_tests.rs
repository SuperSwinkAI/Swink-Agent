//! Tests for `connection`.
#![cfg(test)]

use std::collections::HashMap;
use std::process::Stdio;

use super::build_stdio_command;

#[tokio::test]
async fn stdio_command_clears_inherited_environment_before_applying_configured_values() {
    let inherited_key = std::env::vars()
        .map(|(key, _)| key)
        .find(|key| key != "COMSPEC" && !key.starts_with('='))
        .expect("current process should expose at least one inherited env var");
    let configured_key = "SWINK_MCP_STDIO_ONLY";
    let configured_value = "configured-value";
    let env = HashMap::from([(configured_key.to_string(), configured_value.to_string())]);

    let (command, args) = env_probe_command();
    let mut cmd = build_stdio_command(&command, &args, &env);
    cmd.stdout(Stdio::piped());

    let output = cmd.output().await.expect("spawn env probe");
    assert!(
        output.status.success(),
        "env probe should exit successfully: {output:?}"
    );

    let stdout = String::from_utf8(output.stdout).expect("env probe output should be UTF-8");
    let configured_line = format!("{configured_key}={configured_value}");
    let inherited_prefix = format!("{inherited_key}=");

    assert!(
        stdout.lines().any(|line| line == configured_line),
        "configured env var should be present in child env: {stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with(&inherited_prefix)),
        "inherited env var should be absent after env_clear(): {stdout}"
    );
}

#[cfg(windows)]
fn env_probe_command() -> (String, Vec<String>) {
    let command =
        std::env::var("COMSPEC").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".into());
    (command, vec!["/d".into(), "/c".into(), "set".into()])
}

#[cfg(not(windows))]
fn env_probe_command() -> (String, Vec<String>) {
    use std::path::Path;

    let command = if Path::new("/usr/bin/env").exists() {
        "/usr/bin/env".to_string()
    } else {
        "/bin/env".to_string()
    };
    (command, Vec::new())
}
