//! Integration tests for `SandboxedExecutionEvaluator` (T083).
//!
//! These tests exercise the POSIX sandbox primitive directly via
//! [`run_sandboxed`] with `/bin/sh` snippets. Every scenario exercises one
//! [`SandboxLimits`] field:
//!
//! | Scenario             | Limit under test        |
//! |----------------------|-------------------------|
//! | `wall_clock_timeout` | `wall_clock`            |
//! | `cpu_limit_enforced` | `cpu`                   |
//! | `memory_bomb_caught` | `memory_bytes` (RLIMIT_AS) |
//! | `fd_bomb_caught`     | `max_open_files`        |
//! | `network_egress_blocked` | `allow_network`    |
//!
//! Tests are Unix-only (`cfg(target_family = "unix")`); Windows CI skips
//! the whole module. Each test keeps its snippet self-contained — no
//! external binaries are assumed beyond `/bin/sh`.
//!
//! Memory, FD, and network classifiers rely on stderr heuristics
//! documented in `specs/043-evals-adv-features/research.md` §R-006 —
//! the assertions below therefore accept either the specific named limit
//! or a recognisable failure mode surfaced via stderr.

#![cfg(all(target_family = "unix", feature = "evaluator-sandbox"))]

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use swink_agent_eval::{
    CodeExtractor, CodeExtractorStrategy, EvaluatorError, SandboxLimits,
    SandboxedExecutionEvaluator, run_sandboxed,
};

fn shell(script: &str) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(script);
    cmd
}

fn baseline_limits() -> SandboxLimits {
    // Defaults are the FR-017 caps (120 s wall / 60 s CPU / 1 GiB / 256 FDs /
    // no network). Individual tests tighten only the field they exercise so
    // the remaining fields don't accidentally fire first.
    SandboxLimits::default()
}

#[test]
fn default_limits_match_fr_017() {
    let limits = SandboxLimits::default();
    assert_eq!(limits.wall_clock, Duration::from_secs(120));
    assert_eq!(limits.cpu, Duration::from_secs(60));
    assert_eq!(limits.memory_bytes, 1024 * 1024 * 1024);
    assert_eq!(limits.max_open_files, 256);
    assert!(!limits.allow_network);
}

#[test]
fn trivial_command_succeeds() {
    let limits = baseline_limits();
    let outcome = run_sandboxed(shell("echo hello"), &limits).expect("trivial exec succeeds");
    assert!(outcome.success, "expected success, got {outcome:?}");
    assert_eq!(outcome.exit_code, Some(0));
    assert!(outcome.limit_exceeded.is_none());
}

#[test]
fn wall_clock_timeout_cancels_child() {
    let mut limits = baseline_limits();
    limits.wall_clock = Duration::from_millis(200);
    // Keep CPU headroom so wall_clock is what fires.
    limits.cpu = Duration::from_secs(60);

    let start = Instant::now();
    let err = run_sandboxed(shell("sleep 5"), &limits).expect_err("wall clock must trip");
    let elapsed = start.elapsed();

    match err {
        EvaluatorError::SandboxLimitExceeded { limit } => {
            assert_eq!(limit, "wall_clock");
        }
        other => panic!("expected SandboxLimitExceeded(wall_clock), got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(3),
        "wall clock enforcement too slow: {elapsed:?}"
    );
}

#[test]
fn cpu_limit_enforced_via_sigxcpu() {
    let mut limits = baseline_limits();
    limits.cpu = Duration::from_secs(1);
    // Give ourselves plenty of wall-clock so the CPU limit fires first.
    limits.wall_clock = Duration::from_secs(30);

    // Tight infinite loop in shell arithmetic burns CPU quickly.
    let err = run_sandboxed(shell("i=0; while :; do i=$((i+1)); done"), &limits)
        .expect_err("cpu limit must trip");

    match err {
        EvaluatorError::SandboxLimitExceeded { limit } => {
            // Accept either cpu (SIGXCPU on Linux/macOS) or wall_clock as a
            // fallback on platforms where SIGXCPU is delivered only after
            // the hard limit and the sleep-based parent timer catches it
            // first. Both indicate the sandbox bounded the runaway.
            assert!(
                limit == "cpu" || limit == "wall_clock",
                "expected cpu or wall_clock, got {limit}"
            );
        }
        other => panic!("expected SandboxLimitExceeded, got {other:?}"),
    }
}

#[test]
fn memory_bomb_caught() {
    let mut limits = baseline_limits();
    limits.memory_bytes = 64 * 1024 * 1024; // 64 MiB — easy to blow past.
    limits.wall_clock = Duration::from_secs(15);

    // `yes | head -c` cheaply forces a large buffer write; on some shells
    // the simpler approach is to loop `printf` and append to a string var.
    // We use `/bin/sh`-portable syntax: append to a variable in a loop.
    let script = r#"
        s=""
        i=0
        while [ $i -lt 400000 ]; do
            s="${s}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            i=$((i+1))
        done
        echo done
    "#;
    let result = run_sandboxed(shell(script), &limits);

    match result {
        Err(EvaluatorError::SandboxLimitExceeded { limit }) => {
            // `memory` is the ideal classification; `wall_clock` or `cpu` as
            // fallbacks still prove the bound was enforced (the child was
            // killed before it could print "done").
            assert!(
                ["memory", "wall_clock", "cpu"].contains(&limit.as_str()),
                "unexpected limit: {limit}"
            );
        }
        Ok(outcome) => {
            // Some shells handle string growth with RLIMIT_AS gracefully and
            // return non-zero without a signal. Accept that too — the child
            // did NOT finish successfully, which is the invariant the
            // sandbox must uphold.
            assert!(
                !outcome.success,
                "memory bomb should have failed or been caught: {outcome:?}"
            );
            assert!(
                !outcome.stderr.is_empty() || outcome.exit_code != Some(0),
                "memory bomb produced no diagnostic and exit 0: {outcome:?}"
            );
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn fd_bomb_caught() {
    let mut limits = baseline_limits();
    limits.max_open_files = 16; // minimal — `/bin/sh` may hold a few FDs open.
    limits.wall_clock = Duration::from_secs(15);

    // Open a fresh FD each iteration, keeping the previous ones open. Once
    // the cap is hit, the `exec` builtin fails with EMFILE and the shell
    // exits non-zero. We redirect at increasing FD numbers so each `exec`
    // truly allocates rather than reassigning the same slot. `set -e`
    // propagates the failure; `exec` writes a diagnostic like "Too many
    // open files" to the shell's stderr which the classifier picks up.
    let script = r#"
        set -e
        i=3
        while [ $i -lt 512 ]; do
            eval "exec ${i}< /dev/null"
            i=$((i+1))
        done
    "#;

    let result = run_sandboxed(shell(script), &limits);
    match result {
        Err(EvaluatorError::SandboxLimitExceeded { limit }) => {
            assert_eq!(limit, "fds");
        }
        Ok(outcome) => {
            // If the limit didn't classify cleanly, we at minimum require
            // the child to have failed — never silently succeed past cap.
            assert!(!outcome.success, "fd bomb should have failed: {outcome:?}");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn network_egress_blocked() {
    let mut limits = baseline_limits();
    limits.allow_network = false;
    limits.wall_clock = Duration::from_secs(10);

    // Try to resolve+connect to a canonical address. On Linux with
    // CAP_SYS_ADMIN we unshare the netns and the connect fails outright. On
    // macOS we rely on the absence of a provider being configured and on
    // the stderr classifier catching name resolution / connection errors.
    let script = r#"
        if command -v getent >/dev/null 2>&1; then
            getent ahosts example.com >/dev/null 2>&1 && exit 0
        fi
        # Fall back to /dev/tcp in bash-compat shells; sh may not support it
        # but it's harmless — the failure also surfaces on stderr.
        (exec 3<>/dev/tcp/example.com/80) 2>&1 && exit 0
        echo "Network is unreachable" >&2
        exit 1
    "#;

    let result = run_sandboxed(shell(script), &limits);
    match result {
        Err(EvaluatorError::SandboxLimitExceeded { limit }) => {
            assert_eq!(limit, "network");
        }
        Ok(outcome) => {
            // If the sandbox couldn't hard-block (e.g. macOS without a
            // container), the child should at least have failed. We can't
            // guarantee a network outcome in every CI environment, so we
            // treat a non-zero exit as acceptable evidence the connect
            // path wasn't established.
            assert!(
                !outcome.success || outcome.limit_exceeded.is_some(),
                "network-off sandbox let connect succeed: {outcome:?}"
            );
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn evaluator_returns_none_when_no_code_extracted() {
    use swink_agent::{Cost, ModelSpec, StopReason, Usage};
    use swink_agent_eval::{EvalCase, Evaluator, Invocation};

    let extractor = Arc::new(CodeExtractor::new(CodeExtractorStrategy::MarkdownFence {
        language: Some("rust".into()),
    }));
    let evaluator = SandboxedExecutionEvaluator::new(extractor);
    let case = EvalCase {
        id: "case".into(),
        name: "Case".into(),
        description: None,
        system_prompt: "s".into(),
        user_messages: vec!["hi".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    };
    let invocation = Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: Some("no fenced code here".into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "m"),
    };

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}
