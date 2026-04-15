#![forbid(unsafe_code)]

mod catalog;
mod report;
mod verifier;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "Developer tasks for the Swink Agent workspace"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Verify that every catalog preset `model_id` exists in the provider's live API
    VerifyCatalog {
        /// Only verify a specific provider key (e.g. "anthropic")
        #[arg(long)]
        provider: Option<String>,
        /// Write a markdown summary to `$GITHUB_STEP_SUMMARY` (auto-detected when env var is set)
        #[arg(long)]
        github: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::VerifyCatalog { provider, github } => {
            run_verify_catalog(provider.as_deref(), github).await;
        }
    }
}

async fn run_verify_catalog(provider: Option<&str>, github: bool) {
    let write_summary = github || std::env::var("GITHUB_STEP_SUMMARY").is_ok();
    let tasks = catalog::build_verify_tasks(provider);
    let rows = verifier::verify_all(tasks).await;
    report::print_table(&rows);
    if write_summary && let Err(e) = report::write_github_summary(&rows) {
        eprintln!("warning: failed to write GitHub summary: {e}");
    }
    let has_fail = rows
        .iter()
        .any(|row| matches!(row.result, verifier::PresetResult::Fail { .. }));
    let has_error = rows
        .iter()
        .any(|row| matches!(row.result, verifier::PresetResult::NetworkError { .. }));
    if has_fail {
        std::process::exit(1);
    } else if has_error {
        std::process::exit(2);
    }
}
