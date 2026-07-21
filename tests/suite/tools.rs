use serde_json::json;
use tokio_util::sync::CancellationToken;

use swink_agent::ContentBlock;
use swink_agent::{AgentTool, BashTool, ReadFileTool, WriteFileTool};

// Cross-platform `sleep N seconds` command string for BashTool tests.
fn sleep_command(seconds: u32) -> String {
    if cfg!(windows) {
        // `ping -n K 127.0.0.1` waits ~(K-1) seconds. Add 1 for the target duration.
        format!("ping -n {} 127.0.0.1 > NUL", seconds + 1)
    } else {
        format!("sleep {seconds}")
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// BashTool
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn bash_tool_metadata() {
    let tool = BashTool::new();
    assert_eq!(tool.name(), "bash");
    assert_eq!(tool.label(), "Bash");
    assert!(!tool.description().is_empty());

    let schema = tool.parameters_schema();
    let required = schema["required"]
        .as_array()
        .expect("required should be an array");
    assert!(
        required
            .iter()
            .filter_map(|v| v.as_str())
            .any(|x| x == "command"),
        "schema must require 'command'"
    );
}

#[tokio::test]
async fn bash_echo_success() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_1",
            json!({"command": "echo hello"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 0"),
        "expected exit code 0, got: {text}"
    );
    assert!(
        text.contains("hello"),
        "expected 'hello' in output, got: {text}"
    );
}

// Regression for #202: BashTool used to hardcode `sh -c`, which does not exist
// on Windows. This test spawns a command via the platform shell — on Windows
// that means `cmd /C`, on Unix `sh -c`. Failure here indicates the dispatch
// regressed.
#[tokio::test]
async fn bash_uses_platform_shell() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_platform",
            json!({"command": "echo platform-ok"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    assert!(
        !result.is_error,
        "spawning platform shell must succeed, got: {:?}",
        result.content
    );
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 0") && text.contains("platform-ok"),
        "expected successful platform-shell dispatch, got: {text}"
    );
}

// ─── #1197: environment inheritance contract ────────────────────────────
//
// Orchestrators (e.g. AgentShore) inject per-agent identity env overlays
// (GH_TOKEN, GIT_AUTHOR_*, GIT_SSH_COMMAND, …) on the host process and
// require every bash-tool subprocess to see them unmodified. Precedent for
// the risk: Codex's shell tool stripped inherited env by default
// (shell_environment_policy), which orchestrators discovered in production.
// These tests pin the guarantee documented on `BashTool`.

/// A pre-existing host variable round-trips into the subprocess with its
/// exact value — inheritance is not just present but unmodified.
#[tokio::test]
async fn bash_inherits_host_path_unmodified() {
    let host_path = std::env::var("PATH").expect("test host must have PATH");

    let command = if cfg!(windows) {
        "echo %PATH%".to_string()
    } else {
        r#"printf '%s' "$PATH""#.to_string()
    };

    let tool = BashTool::new();
    let result = tool
        .execute(
            "tc_env_path",
            json!({ "command": command }),
            CancellationToken::new(),
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 0"),
        "expected success, got: {text}"
    );
    assert!(
        text.contains(&host_path),
        "subprocess PATH must match the host value exactly.\nhost: {host_path}\ngot: {text}"
    );
}

/// Every host variable — well-known or not — reaches the subprocess with its
/// exact value. This is the allowlist tripwire: a policy that passes
/// well-known vars (PATH, HOME) but strips unknown ones (CARGO_*, NEXTEST_*,
/// arbitrary user/orchestrator vars) would pass the PATH test above and fail
/// here. The shell may *add* vars (PWD, SHLVL, _); it must never drop or
/// rewrite an inherited one.
#[tokio::test]
async fn bash_inherits_entire_host_environment() {
    let command = if cfg!(windows) { "set" } else { "env" };

    let tool = BashTool::new();
    let result = tool
        .execute(
            "tc_env_full",
            json!({ "command": command }),
            CancellationToken::new(),
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 0"),
        "expected success, got: {text}"
    );

    let missing: Vec<String> = std::env::vars()
        .filter(|(key, value)| {
            // Line-based comparison can't represent multi-line values, and
            // `_`/`SHLVL`/`PWD` are legitimately rewritten by the shell.
            // On macOS, SIP strips `DYLD_*` from the environment when
            // exec-ing protected binaries such as `/bin/zsh`, below any
            // control of ours (`cargo test` sets DYLD_FALLBACK_LIBRARY_PATH,
            // so the suite only trips this under plain `cargo test`).
            !key.is_empty()
                && !value.contains('\n')
                && !value.contains('\r')
                && key != "_"
                && key != "SHLVL"
                && key != "PWD"
                && key != "OLDPWD"
                && !(cfg!(target_os = "macos") && key.starts_with("DYLD_"))
                && !text.contains(&format!("{key}={value}"))
        })
        .map(|(key, _)| key)
        .collect();

    assert!(
        missing.is_empty(),
        "host env vars missing or modified in the bash subprocess \
         (inheritance contract #1197 violated): {missing:?}"
    );
}

#[tokio::test]
async fn bash_exit_code_nonzero() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_2",
            json!({"command": "exit 42"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 42"),
        "expected exit code 42, got: {text}"
    );
}

#[tokio::test]
async fn bash_stderr_captured() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_3",
            json!({"command": "echo err >&2"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Stderr:"),
        "expected Stderr section, got: {text}"
    );
    assert!(
        text.contains("err"),
        "expected 'err' in stderr, got: {text}"
    );
}

#[tokio::test]
async fn bash_invalid_params() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_4",
            json!({}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("invalid parameters") || text.contains("error"),
        "expected error for missing command, got: {text}"
    );
}

#[tokio::test]
async fn bash_cancellation() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    token.cancel();
    let result = tool
        .execute(
            "tc_5",
            json!({"command": "echo should not run"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("cancelled"),
        "expected cancelled, got: {text}"
    );
}

#[tokio::test]
async fn bash_timeout() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_6",
            json!({"command": sleep_command(30), "timeout_ms": 100}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("timed out"),
        "expected timeout error, got: {text}"
    );
}

// Unix-only: uses `cat` and `>&2` which are not available in `cmd /C`.
#[cfg(unix)]
#[tokio::test]
async fn bash_output_truncation() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    // Generate output larger than MAX_OUTPUT_BYTES (100 * 1024 = 102400).
    // Split output across stdout (55KB) and stderr (55KB), each within the OS
    // pipe buffer limit, but combined (110KB) exceeding MAX_OUTPUT_BYTES.
    // This avoids a deadlock where the child blocks on a full pipe buffer
    // before exiting (stdout/stderr are read after child.wait()).
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let stdout_file = dir.path().join("stdout.txt");
    let stderr_file = dir.path().join("stderr.txt");
    std::fs::write(&stdout_file, "A".repeat(55_000)).expect("write stdout file");
    std::fs::write(&stderr_file, "B".repeat(55_000)).expect("write stderr file");
    let cmd = format!(
        "cat {} && cat {} >&2",
        stdout_file.display(),
        stderr_file.display()
    );
    let result = tool
        .execute(
            "tc_7",
            json!({"command": cmd}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("[truncated]"),
        "expected truncation marker, got length: {}",
        text.len()
    );
}

// Uses Unix-only utilities (`head`, `tr`, `/dev/zero`); skipped on Windows
// where the platform shell is `cmd /C` and cannot interpret them.
#[cfg(unix)]
#[tokio::test]
async fn bash_large_stdout_does_not_deadlock() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tool.execute(
            "tc_8",
            json!({"command": "head -c 200000 /dev/zero | tr '\\000' A"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        ),
    )
    .await
    .expect("bash tool should not deadlock on large stdout");

    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 0"),
        "expected success, got: {text}"
    );
}

// Unix-only: uses `head`, `tr`, `/dev/zero`, `&`, and `wait`.
#[cfg(unix)]
#[tokio::test]
async fn bash_large_stdout_and_stderr_do_not_deadlock() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tool.execute(
            "tc_9",
            json!({"command": "(head -c 150000 /dev/zero | tr '\\000' A) & (head -c 150000 /dev/zero | tr '\\000' B >&2) & wait"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        ),
    )
    .await
    .expect("bash tool should not deadlock on large stdout/stderr");

    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Exit code: 0"),
        "expected success, got: {text}"
    );
    assert!(
        text.contains("Stdout:"),
        "expected stdout in result, got: {text}"
    );
    assert!(
        text.contains("Stderr:"),
        "expected stderr in result, got: {text}"
    );
}

// Unix-only: `yes` is not available on Windows `cmd`.
#[cfg(unix)]
#[tokio::test]
async fn bash_noisy_timeout_does_not_deadlock() {
    let tool = BashTool::new();
    let token = CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tool.execute(
            "tc_10",
            json!({"command": "yes X", "timeout_ms": 100}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        ),
    )
    .await
    .expect("bash tool timeout should not deadlock under active output");

    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("timed out"),
        "expected timeout error, got: {text}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// ReadFileTool
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn read_file_metadata() {
    let tool = ReadFileTool::new();
    assert_eq!(tool.name(), "read_file");
    assert_eq!(tool.label(), "Read File");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn read_file_success() {
    let tool = ReadFileTool::new();
    let token = CancellationToken::new();

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let file_path = dir.path().join("hello.txt");
    std::fs::write(&file_path, "hello world").expect("failed to write temp file");

    let result = tool
        .execute(
            "tc_1",
            json!({"path": file_path.to_str().unwrap()}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert_eq!(text, "hello world");
}

#[tokio::test]
async fn read_file_not_found() {
    let tool = ReadFileTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_2",
            json!({"path": "/tmp/nonexistent_swink_agent_test_file_xyz"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("failed to read file"),
        "expected read error, got: {text}"
    );
}

#[tokio::test]
async fn read_file_invalid_params() {
    let tool = ReadFileTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_3",
            json!({}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("invalid parameters"),
        "expected invalid parameters error, got: {text}"
    );
}

#[tokio::test]
async fn read_file_cancellation() {
    let tool = ReadFileTool::new();
    let token = CancellationToken::new();
    token.cancel();
    let result = tool
        .execute(
            "tc_4",
            json!({"path": "/tmp/anything"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("cancelled"),
        "expected cancelled, got: {text}"
    );
}

#[tokio::test]
async fn read_file_truncation() {
    let tool = ReadFileTool::new();
    let token = CancellationToken::new();

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let file_path = dir.path().join("big.txt");
    // Write a file larger than MAX_OUTPUT_BYTES (100 * 1024 = 102400).
    let big_content = "A".repeat(110_000);
    std::fs::write(&file_path, &big_content).expect("failed to write big file");

    let result = tool
        .execute(
            "tc_5",
            json!({"path": file_path.to_str().unwrap()}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("[truncated]"),
        "expected truncation marker, got length: {}",
        text.len()
    );
    // Verify the output is smaller than the input.
    assert!(
        text.len() < big_content.len(),
        "truncated output should be smaller than original"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// WriteFileTool
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn write_file_metadata() {
    let tool = WriteFileTool::new();
    assert_eq!(tool.name(), "write_file");
    assert_eq!(tool.label(), "Write File");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn write_file_success() {
    let tool = WriteFileTool::new();
    let token = CancellationToken::new();

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let file_path = dir.path().join("output.txt");

    let result = tool
        .execute(
            "tc_1",
            json!({"path": file_path.to_str().unwrap(), "content": "written by test"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Successfully wrote"),
        "expected success message, got: {text}"
    );

    let on_disk = std::fs::read_to_string(&file_path).expect("failed to read written file");
    assert_eq!(on_disk, "written by test");
}

#[tokio::test]
async fn write_file_creates_dirs() {
    let tool = WriteFileTool::new();
    let token = CancellationToken::new();

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let nested_path = dir.path().join("a").join("b").join("c").join("deep.txt");

    let result = tool
        .execute(
            "tc_2",
            json!({"path": nested_path.to_str().unwrap(), "content": "deep content"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("Successfully wrote"),
        "expected success message, got: {text}"
    );

    let on_disk = std::fs::read_to_string(&nested_path).expect("failed to read nested file");
    assert_eq!(on_disk, "deep content");
}

#[tokio::test]
async fn write_file_invalid_params() {
    let tool = WriteFileTool::new();
    let token = CancellationToken::new();
    let result = tool
        .execute(
            "tc_3",
            json!({}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("invalid parameters"),
        "expected invalid parameters error, got: {text}"
    );
}

#[tokio::test]
async fn write_file_cancellation() {
    let tool = WriteFileTool::new();
    let token = CancellationToken::new();
    token.cancel();
    let result = tool
        .execute(
            "tc_4",
            json!({"path": "/tmp/anything", "content": "nope"}),
            token,
            None,
            std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
            None,
        )
        .await;
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("cancelled"),
        "expected cancelled, got: {text}"
    );
}
