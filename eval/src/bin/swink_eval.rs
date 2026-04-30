//! `swink-eval` CLI entry point (spec 043 T152-T155).
//!
//! Three subcommands per contracts/public-api.md §Binary target:
//!
//! ```text
//! swink-eval run --set <path> [--out <path>] [--parallelism <n>] [--reporter <fmt>]
//! swink-eval report --result <path> --format <fmt>
//! swink-eval gate   --result <path> --gate-config <path>
//! ```
//!
//! Exit codes:
//! * `0` — success (run passed; gate passed; report rendered)
//! * `1` — eval run completed but gate failed
//! * `2` — configuration error (missing file, invalid args)
//! * `3` — runtime error (cancelled, IO error)
//!
//! The binary is fully feature-gated behind `cli`; without the feature
//! the target is not built and users get the usual cargo feature error.

#![cfg(feature = "cli")]
#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "html-report")]
use swink_agent_eval::HtmlReporter;
use swink_agent_eval::{
    ConsoleReporter, EvalError, EvalSet, EvalSetResult, GateConfig, JsonReporter, MarkdownReporter,
    Reporter, ReporterOutput, check_gate, decode_result_json,
};

const EXIT_OK: u8 = 0;
const EXIT_GATE_FAILED: u8 = 1;
const EXIT_CONFIG: u8 = 2;
const EXIT_RUNTIME: u8 = 3;

#[derive(Debug, Parser)]
#[command(
    name = "swink-eval",
    version,
    about = "Run, render, and gate swink-agent eval sets"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Execute an eval set against the built-in null factory and render
    /// the results.
    Run {
        /// Path to the YAML or JSON eval-set file.
        #[arg(long)]
        set: PathBuf,
        /// Optional path to persist the raw `EvalSetResult` JSON.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Parallelism (number of cases in flight). Defaults to 1.
        #[arg(long, default_value_t = 1)]
        parallelism: usize,
        /// Reporter used for stdout rendering.
        #[arg(long, default_value_t = Format::Console)]
        reporter: Format,
    },
    /// Re-render a previously persisted result through a different reporter.
    /// No re-execution is performed.
    Report {
        /// Path to the `EvalSetResult` JSON.
        #[arg(long)]
        result: PathBuf,
        /// Reporter format to render in.
        #[arg(long, default_value_t = Format::Console)]
        format: Format,
    },
    /// Check a persisted result against a gate configuration. No stdout
    /// output — the gate decision is communicated through the exit code.
    Gate {
        /// Path to the `EvalSetResult` JSON.
        #[arg(long)]
        result: PathBuf,
        /// Path to the `GateConfig` JSON.
        #[arg(long = "gate-config")]
        gate_config: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
#[value(rename_all = "lower")]
enum Format {
    Console,
    Json,
    Md,
    Html,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Console => f.write_str("console"),
            Self::Json => f.write_str("json"),
            Self::Md => f.write_str("md"),
            Self::Html => f.write_str("html"),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("swink-eval: tokio runtime init failed: {err}");
            return ExitCode::from(EXIT_RUNTIME);
        }
    };

    let code = rt.block_on(async {
        match cli.command {
            Command::Run {
                set,
                out,
                parallelism,
                reporter,
            } => run_subcommand(&set, out.as_deref(), parallelism, reporter),
            Command::Report { result, format } => report_subcommand(&result, format).await,
            Command::Gate {
                result,
                gate_config,
            } => gate_subcommand(&result, &gate_config).await,
        }
    });
    ExitCode::from(code)
}

fn run_subcommand(set_path: &Path, out: Option<&Path>, parallelism: usize, reporter: Format) -> u8 {
    let set = match load_eval_set(set_path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!(
                "swink-eval run: loading eval set `{}`: {err}",
                set_path.display()
            );
            return EXIT_CONFIG;
        }
    };
    let _ = (&set, out, parallelism, reporter);
    eprintln!(
        "swink-eval run: real agent and evaluator configuration is required; \
         this binary does not provide a default null execution path"
    );
    EXIT_CONFIG
}

#[allow(clippy::unused_async)]
async fn report_subcommand(result_path: &Path, format: Format) -> u8 {
    let result = match load_result(result_path) {
        Ok(r) => r,
        Err(err) => {
            eprintln!(
                "swink-eval report: loading `{}`: {err}",
                result_path.display()
            );
            return EXIT_CONFIG;
        }
    };
    emit_report(&result, format)
}

#[allow(clippy::unused_async)]
async fn gate_subcommand(result_path: &Path, gate_config_path: &Path) -> u8 {
    let result = match load_result(result_path) {
        Ok(r) => r,
        Err(err) => {
            eprintln!(
                "swink-eval gate: loading result `{}`: {err}",
                result_path.display()
            );
            return EXIT_CONFIG;
        }
    };
    let config = match load_gate_config(gate_config_path) {
        Ok(c) => c,
        Err(err) => {
            eprintln!(
                "swink-eval gate: loading gate config `{}`: {err}",
                gate_config_path.display()
            );
            return EXIT_CONFIG;
        }
    };
    let verdict = check_gate(&result, &config);
    if verdict.passed {
        EXIT_OK
    } else {
        EXIT_GATE_FAILED
    }
}

fn emit_report(result: &EvalSetResult, format: Format) -> u8 {
    let rendered: Result<ReporterOutput, _> = match format {
        Format::Console => ConsoleReporter::new().render(result),
        Format::Json => JsonReporter::new().render(result),
        Format::Md => MarkdownReporter::new().render(result),
        Format::Html => render_html(result),
    };
    match rendered {
        Ok(ReporterOutput::Stdout(text)) => {
            println!("{text}");
            EXIT_OK
        }
        Ok(ReporterOutput::Artifact { bytes, .. }) => {
            // Stream artifact bytes to stdout so `> file.json` works.
            use std::io::Write;
            match std::io::stdout().write_all(&bytes) {
                Ok(()) => EXIT_OK,
                Err(err) => {
                    eprintln!("swink-eval: stdout write: {err}");
                    EXIT_RUNTIME
                }
            }
        }
        Ok(ReporterOutput::Remote {
            backend,
            identifier,
        }) => {
            println!("pushed to {backend}: {identifier}");
            EXIT_OK
        }
        Err(err) => {
            eprintln!("swink-eval: render error: {err}");
            EXIT_RUNTIME
        }
    }
}

#[cfg(feature = "html-report")]
fn render_html(result: &EvalSetResult) -> Result<ReporterOutput, swink_agent_eval::ReporterError> {
    HtmlReporter::new().render(result)
}

#[cfg(not(feature = "html-report"))]
fn render_html(_result: &EvalSetResult) -> Result<ReporterOutput, swink_agent_eval::ReporterError> {
    Err(swink_agent_eval::ReporterError::Format(
        "html reporter requires the `html-report` cargo feature".into(),
    ))
}

fn load_eval_set(path: &Path) -> Result<EvalSet, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        #[cfg(feature = "yaml")]
        "yaml" | "yml" => {
            swink_agent_eval::load_eval_set_yaml(path).map_err(|err: EvalError| err.to_string())
        }
        "json" => {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            serde_json::from_slice::<EvalSet>(&bytes).map_err(|e| e.to_string())
        }
        other => Err(format!("unsupported eval set extension: .{other}")),
    }
}

fn load_result(path: &Path) -> Result<EvalSetResult, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    decode_result_json(&bytes).map_err(|e| e.to_string())
}

fn load_gate_config(path: &Path) -> Result<GateConfig, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice::<GateConfig>(&bytes).map_err(|e| e.to_string())
}
