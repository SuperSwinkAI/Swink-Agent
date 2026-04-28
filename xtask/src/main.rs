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
        /// Exit successfully even when providers are skipped
        #[arg(long)]
        allow_skipped: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::VerifyCatalog {
            provider,
            github,
            allow_skipped,
        } => {
            run_verify_catalog(provider.as_deref(), github, allow_skipped).await;
        }
    }
}

async fn run_verify_catalog(provider: Option<&str>, github: bool, allow_skipped: bool) {
    let write_summary = github || std::env::var("GITHUB_STEP_SUMMARY").is_ok();
    let tasks = match catalog::build_verify_tasks(provider) {
        Ok(tasks) => tasks,
        Err(error) => {
            eprintln!(
                "unknown provider filter '{}'; valid provider keys: {}",
                error.provider_key,
                error.valid_provider_keys.join(", ")
            );
            std::process::exit(invalid_provider_exit_code());
        }
    };
    let rows = verifier::verify_all(tasks).await;
    report::print_table(&rows);
    if write_summary && let Err(e) = report::write_github_summary(&rows) {
        eprintln!("warning: failed to write GitHub summary: {e}");
    }
    let exit_code = verify_catalog_exit_code(&rows, allow_skipped);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn invalid_provider_exit_code() -> i32 {
    4
}

fn verify_catalog_exit_code(rows: &[verifier::VerifyRow], allow_skipped: bool) -> i32 {
    let has_fail = rows
        .iter()
        .any(|row| matches!(row.result, verifier::PresetResult::Fail { .. }));
    let has_error = rows
        .iter()
        .any(|row| matches!(row.result, verifier::PresetResult::NetworkError { .. }));
    let has_skipped = rows
        .iter()
        .any(|row| matches!(row.result, verifier::PresetResult::Skipped { .. }));
    if has_fail {
        1
    } else if has_error {
        2
    } else if has_skipped && !allow_skipped {
        3
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use crate::catalog::{ProviderEndpoint, VerifyTask};
    use crate::verifier::{PresetResult, VerifyRow};

    fn row(result: PresetResult) -> VerifyRow {
        VerifyRow {
            task: VerifyTask {
                provider_key: "provider".to_owned(),
                preset_id: "preset".to_owned(),
                preset_display: "Preset".to_owned(),
                model_id: "model".to_owned(),
                endpoint: ProviderEndpoint::Skipped { reason: "test" },
            },
            result,
        }
    }

    #[test]
    fn skipped_rows_fail_by_default() {
        let rows = vec![row(PresetResult::Skipped {
            reason: "missing credential",
        })];

        assert_eq!(super::verify_catalog_exit_code(&rows, false), 3);
    }

    #[test]
    fn allow_skipped_keeps_skipped_rows_successful() {
        let rows = vec![row(PresetResult::Skipped {
            reason: "missing credential",
        })];

        assert_eq!(super::verify_catalog_exit_code(&rows, true), 0);
    }

    #[test]
    fn failures_take_precedence_over_skipped_rows() {
        let rows = vec![
            row(PresetResult::Skipped {
                reason: "missing credential",
            }),
            row(PresetResult::Fail { available_count: 0 }),
        ];

        assert_eq!(super::verify_catalog_exit_code(&rows, false), 1);
    }

    #[test]
    fn network_errors_take_precedence_over_skipped_rows() {
        let rows = vec![
            row(PresetResult::Skipped {
                reason: "missing credential",
            }),
            row(PresetResult::NetworkError {
                error: "timeout".to_owned(),
            }),
        ];

        assert_eq!(super::verify_catalog_exit_code(&rows, false), 2);
    }

    #[test]
    fn unknown_provider_filter_exits_with_dedicated_code() {
        assert_eq!(super::invalid_provider_exit_code(), 4);
    }
}
