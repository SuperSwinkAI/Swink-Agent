//! Sandboxed execution evaluator (T080–T083, behind `evaluator-sandbox`).
//!
//! Wraps a child process with POSIX `rlimit`s + (on Linux) a fresh network
//! namespace so extracted code can be executed under deterministic resource
//! bounds without spinning up a container. Windows builds ship a stub that
//! surfaces [`EvaluatorError::UnsupportedPlatform`] at evaluation time per
//! FR-017.
//!
//! The public surface is stable across platforms:
//!
//! * [`SandboxLimits`] — resource caps (wall-clock / CPU / RSS / FDs / network).
//!   Default values are pinned by FR-017.
//! * [`SandboxOutcome`] — structured return type from [`run_sandboxed`]
//!   capturing success, stderr, and which limit (if any) was exceeded.
//! * [`run_sandboxed`] — lower-level primitive used by the integration tests
//!   (T083) to exercise each limit in isolation.
//! * [`SandboxedExecutionEvaluator`] + [`SandboxRunner`] — [`crate::Evaluator`]
//!   binding that extracts a code block, dispatches to a [`SandboxRunner`]
//!   (default: shell) to build the child `Command`, and folds the
//!   [`SandboxOutcome`] into an [`crate::EvalMetricResult`].
//!
//! ## Unsafe scope
//!
//! Per FR-049, unsafe is denied workspace-wide and narrowed further at the
//! `swink-agent-eval` crate root. The single authorised carve-out is the
//! `cfg(target_family = "unix")` [`posix`] submodule, which relaxes to
//! `#![allow(unsafe_code)]` — every `unsafe` block inside it carries a
//! `// SAFETY:` comment explaining the invariant being upheld. Nothing in this
//! parent module uses `unsafe`.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::evaluator::Evaluator;
use crate::evaluators::EvaluatorError;
use crate::evaluators::code::extractor::CodeExtractor;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

#[cfg(target_family = "unix")]
pub(crate) mod posix;

/// Resource limits enforced on the child process (T080 / FR-017).
///
/// Defaults are pinned by FR-017:
///
/// | Limit             | Default    | Rationale                                |
/// |-------------------|------------|------------------------------------------|
/// | `wall_clock`      | 120 s      | Real-time deadline enforced by parent.  |
/// | `cpu`             | 60 s       | `RLIMIT_CPU` seconds.                    |
/// | `memory_bytes`    | 1 GiB      | `RLIMIT_AS` address space cap.           |
/// | `max_open_files`  | 256        | `RLIMIT_NOFILE` hard + soft.             |
/// | `allow_network`   | `false`    | Linux: `unshare(CLONE_NEWNET)`.          |
///
/// On macOS `unshare` is unavailable and the network-off invariant degrades to
/// "child has no configured provider" — documented as a known limitation in
/// `specs/043-evals-adv-features/research.md` §R-006.
#[derive(Debug, Clone)]
pub struct SandboxLimits {
    /// Real-time deadline. The parent SIGKILLs the child on expiry.
    pub wall_clock: Duration,
    /// CPU seconds via `RLIMIT_CPU`. Child receives SIGXCPU on expiry.
    pub cpu: Duration,
    /// Virtual address space ceiling via `RLIMIT_AS`.
    pub memory_bytes: u64,
    /// File-descriptor ceiling via `RLIMIT_NOFILE`.
    pub max_open_files: u64,
    /// Whether the child may open external network connections.
    pub allow_network: bool,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(120),
            cpu: Duration::from_secs(60),
            memory_bytes: 1024 * 1024 * 1024,
            max_open_files: 256,
            allow_network: false,
        }
    }
}

/// Structured outcome of [`run_sandboxed`].
#[derive(Debug, Clone)]
pub struct SandboxOutcome {
    /// The child exited with status 0 and no limit was exceeded.
    pub success: bool,
    /// Raw exit code, if the child exited normally.
    pub exit_code: Option<i32>,
    /// Terminating signal number, if the child was signalled.
    pub signal: Option<i32>,
    /// Captured stderr (trimmed and truncated to the first few lines).
    pub stderr: String,
    /// Which limit (if any) was exceeded.
    pub limit_exceeded: Option<String>,
}

impl SandboxOutcome {
    /// Short label describing the outcome for reporter consumption.
    #[must_use]
    pub fn summary(&self) -> String {
        match &self.limit_exceeded {
            Some(limit) => format!("sandbox limit exceeded: {limit}"),
            None if self.success => "ok".to_string(),
            None => {
                let detail = self
                    .stderr
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .take(8)
                    .collect::<Vec<_>>()
                    .join("\n");
                if detail.is_empty() {
                    match (self.exit_code, self.signal) {
                        (Some(code), _) => format!("exit status {code}"),
                        (_, Some(sig)) => format!("signal {sig}"),
                        _ => "non-zero exit".to_string(),
                    }
                } else {
                    detail
                }
            }
        }
    }
}

/// Execute `command` under the configured [`SandboxLimits`] (T081).
///
/// On Unix this installs `rlimit`s via a `pre_exec` hook inside the
/// [`posix`] submodule and enforces wall-clock by SIGKILL-ing the child after
/// the deadline. On Windows this returns
/// [`EvaluatorError::UnsupportedPlatform`] (T082) without spawning.
///
/// When a limit is exceeded the returned [`SandboxOutcome`] has
/// `limit_exceeded = Some(<name>)` and callers may synthesise
/// [`EvaluatorError::SandboxLimitExceeded`] from it; the lower-level
/// [`SandboxOutcome`] shape is preserved so callers that want to inspect
/// stderr can do so before mapping to the typed error.
pub fn run_sandboxed(
    command: Command,
    limits: &SandboxLimits,
) -> Result<SandboxOutcome, EvaluatorError> {
    #[cfg(target_family = "unix")]
    {
        posix::run_sandboxed_unix(command, limits)
    }
    #[cfg(target_family = "windows")]
    {
        // Touch the parameters to silence the unused-warning on stub builds.
        let _ = (command, limits);
        Err(EvaluatorError::UnsupportedPlatform {
            reason: "SandboxedExecutionEvaluator is Unix-only (Linux/macOS); \
                     FR-017 defines Windows as unsupported for this evaluator"
                .to_string(),
        })
    }
}

/// Builds the child `Command` for a [`SandboxedExecutionEvaluator`].
///
/// Implementors are responsible for writing any auxiliary files into
/// `workdir` and returning a `Command` whose `current_dir` is inside
/// `workdir`. The evaluator guarantees `workdir` lives for the duration of
/// the child process and is deleted afterwards.
pub trait SandboxRunner: Send + Sync {
    /// Assemble the `Command` to execute `code` inside `workdir`.
    fn command(&self, code: &str, workdir: &std::path::Path) -> std::io::Result<Command>;
}

/// Default [`SandboxRunner`]: runs the extracted snippet verbatim via
/// `/bin/sh -c`. Intended for smoke tests and shell-style snippets.
///
/// Most real deployments will want to plug a custom runner in (e.g. scaffold
/// a Rust crate and `cargo run`), but the shell runner keeps the evaluator
/// useful out of the box and — crucially — self-contained for tests (no
/// compilers, no `cc`).
#[derive(Debug, Default, Clone)]
pub struct ShellRunner;

impl SandboxRunner for ShellRunner {
    fn command(&self, code: &str, workdir: &std::path::Path) -> std::io::Result<Command> {
        let script = workdir.join("snippet.sh");
        std::fs::write(&script, code)?;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(script);
        cmd.current_dir(workdir);
        Ok(cmd)
    }
}

/// Sandboxed execution evaluator (T081 — evaluator wiring).
///
/// Extracts a code block via the configured [`CodeExtractor`], writes it
/// into a fresh tempdir, and invokes a [`SandboxRunner`] to produce the
/// child `Command`. The child runs under [`SandboxLimits`]; the outcome is
/// folded into an [`EvalMetricResult`].
///
/// When no code is extractable this evaluator returns `None` to match the
/// FR-020 "criterion not set" semantics used by the sibling
/// [`crate::CargoCheckEvaluator`].
pub struct SandboxedExecutionEvaluator {
    name: &'static str,
    extractor: Arc<CodeExtractor>,
    limits: SandboxLimits,
    runner: Arc<dyn SandboxRunner>,
}

impl SandboxedExecutionEvaluator {
    /// Construct an evaluator with the default shell runner + default limits.
    #[must_use]
    pub fn new(extractor: Arc<CodeExtractor>) -> Self {
        Self {
            name: "sandboxed_execution",
            extractor,
            limits: SandboxLimits::default(),
            runner: Arc::new(ShellRunner),
        }
    }

    /// Override the reported evaluator name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Override the resource limits.
    #[must_use]
    pub fn with_limits(mut self, limits: SandboxLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Override the runner used to build the child `Command`.
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn SandboxRunner>) -> Self {
        self.runner = runner;
        self
    }
}

impl Evaluator for SandboxedExecutionEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let response = invocation.final_response.as_ref()?;
        let code = crate::evaluators::block_on(self.extractor.extract(response))?;

        let tempdir = match tempfile::TempDir::new() {
            Ok(dir) => dir,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name.to_string(),
                    score: Score::fail(),
                    details: Some(format!("tempdir creation failed: {err}")),
                });
            }
        };

        let command = match self.runner.command(&code, tempdir.path()) {
            Ok(cmd) => cmd,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name.to_string(),
                    score: Score::fail(),
                    details: Some(format!("runner failed: {err}")),
                });
            }
        };

        let (score, details) = match run_sandboxed(command, &self.limits) {
            Ok(outcome) => {
                let score = if outcome.success {
                    Score::pass()
                } else {
                    Score::fail()
                };
                (score, outcome.summary())
            }
            Err(EvaluatorError::UnsupportedPlatform { reason }) => {
                (Score::fail(), format!("unsupported platform: {reason}"))
            }
            Err(EvaluatorError::SandboxLimitExceeded { limit }) => {
                (Score::fail(), format!("sandbox limit exceeded: {limit}"))
            }
            Err(err) => (Score::fail(), err.into_metric_details()),
        };

        Some(EvalMetricResult {
            evaluator_name: self.name.to_string(),
            score,
            details: Some(details),
        })
    }
}
