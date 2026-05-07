use std::fmt::Write as FmtWrite;
use std::io::Write;

use crate::verifier::{PresetResult, VerifyRow};

pub fn print_table(rows: &[VerifyRow]) {
    if rows.is_empty() {
        println!("No presets to verify.");
        return;
    }
    let result_strs: Vec<String> = rows.iter().map(|r| result_label(&r.result)).collect();
    let w1 = rows
        .iter()
        .map(|r| r.task.provider_key.len())
        .max()
        .unwrap_or(0)
        .max("Provider".len());
    let w2 = rows
        .iter()
        .map(|r| r.task.preset_id.len())
        .max()
        .unwrap_or(0)
        .max("Preset".len());
    let w3 = rows
        .iter()
        .map(|r| r.task.model_id.len())
        .max()
        .unwrap_or(0)
        .max("Model ID".len());
    let w4 = result_strs
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Result".len());
    println!(
        "{:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}",
        "Provider", "Preset", "Model ID", "Result"
    );
    println!(
        "{}  {}  {}  {}",
        "─".repeat(w1),
        "─".repeat(w2),
        "─".repeat(w3),
        "─".repeat(w4)
    );
    for (row, rs) in rows.iter().zip(&result_strs) {
        println!(
            "{:<w1$}  {:<w2$}  {:<w3$}  {rs}",
            row.task.provider_key, row.task.preset_id, row.task.model_id,
        );
    }
    let counts = ResultCounts::from_rows(rows);
    println!(
        "\nSummary: {} passed, {} failed, {} skipped, {} network errors",
        counts.passed, counts.failed, counts.skipped, counts.network_errors
    );
}

fn result_label(result: &PresetResult) -> String {
    match result {
        PresetResult::Pass => "PASS".to_owned(),
        PresetResult::Fail { available_count } => {
            format!("FAIL ({available_count} models available)")
        }
        PresetResult::Skipped { reason } => format!("SKIPPED ({reason})"),
        PresetResult::NetworkError { error } => format!("ERROR ({error})"),
    }
}

pub fn write_github_summary(rows: &[VerifyRow]) -> std::io::Result<()> {
    let Ok(summary_path) = std::env::var("GITHUB_STEP_SUMMARY") else {
        eprintln!("GITHUB_STEP_SUMMARY not set; skipping summary write");
        return Ok(());
    };
    let md = github_summary_markdown(rows)?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&summary_path)?;
    file.write_all(md.as_bytes())?;
    Ok(())
}

fn github_summary_markdown(rows: &[VerifyRow]) -> std::io::Result<String> {
    let mut md = String::from("## Catalog Verification\n\n");
    let counts = ResultCounts::from_rows(rows);
    writeln!(
        md,
        "**Summary:** {} passed, {} failed, {} skipped, {} network errors\n",
        counts.passed, counts.failed, counts.skipped, counts.network_errors
    )
    .map_err(std::io::Error::other)?;
    md.push_str("| Provider | Preset | Model ID | Result |\n");
    md.push_str("|---|---|---|---|\n");
    for row in rows {
        let key = escape_table_cell(&row.task.provider_key);
        let preset = escape_table_cell(&row.task.preset_display);
        let model = escape_table_cell(&row.task.model_id);
        let result = escape_table_cell(&result_label(&row.result));
        writeln!(md, "| {key} | {preset} | {model} | {result} |").map_err(std::io::Error::other)?;
    }
    Ok(md)
}

fn escape_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br>")
}

struct ResultCounts {
    passed: usize,
    failed: usize,
    skipped: usize,
    network_errors: usize,
}

impl ResultCounts {
    fn from_rows(rows: &[VerifyRow]) -> Self {
        let mut counts = Self {
            passed: 0,
            failed: 0,
            skipped: 0,
            network_errors: 0,
        };
        for row in rows {
            match row.result {
                PresetResult::Pass => counts.passed += 1,
                PresetResult::Fail { .. } => counts.failed += 1,
                PresetResult::Skipped { .. } => counts.skipped += 1,
                PresetResult::NetworkError { .. } => counts.network_errors += 1,
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use crate::catalog::{ProviderEndpoint, VerifyTask};
    use crate::verifier::{PresetResult, VerifyRow};

    fn row(provider_key: &str, preset_display: &str, result: PresetResult) -> VerifyRow {
        VerifyRow {
            task: VerifyTask {
                provider_key: provider_key.to_owned(),
                preset_id: preset_display.to_lowercase().replace(' ', "-"),
                preset_display: preset_display.to_owned(),
                model_id: format!("{provider_key}-model"),
                endpoint: ProviderEndpoint::Skipped { reason: "test" },
            },
            result,
        }
    }

    #[test]
    fn result_counts_classify_each_outcome() {
        let rows = vec![
            row("anthropic", "Claude", PresetResult::Pass),
            row(
                "openai",
                "GPT",
                PresetResult::Fail {
                    available_count: 12,
                },
            ),
            row(
                "google",
                "Gemini",
                PresetResult::Skipped {
                    reason: "missing credential",
                },
            ),
            row(
                "proxy",
                "Proxy",
                PresetResult::NetworkError {
                    error: "timeout".to_owned(),
                },
            ),
        ];

        let counts = super::ResultCounts::from_rows(&rows);

        assert_eq!(counts.passed, 1);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.network_errors, 1);
    }

    #[test]
    fn github_summary_includes_counts_and_result_labels() {
        let rows = vec![
            row("anthropic", "Claude", PresetResult::Pass),
            row("openai", "GPT", PresetResult::Fail { available_count: 2 }),
            row(
                "google",
                "Gemini",
                PresetResult::Skipped {
                    reason: "missing credential",
                },
            ),
            row(
                "proxy",
                "Proxy",
                PresetResult::NetworkError {
                    error: "timeout".to_owned(),
                },
            ),
        ];

        let markdown = super::github_summary_markdown(&rows).expect("summary builds");

        assert!(markdown.contains("**Summary:** 1 passed, 1 failed, 1 skipped, 1 network errors"));
        assert!(markdown.contains("| anthropic | Claude | anthropic-model | PASS |"));
        assert!(markdown.contains("| openai | GPT | openai-model | FAIL (2 models available) |"));
        assert!(
            markdown.contains("| google | Gemini | google-model | SKIPPED (missing credential) |")
        );
        assert!(markdown.contains("| proxy | Proxy | proxy-model | ERROR (timeout) |"));
    }

    #[test]
    fn github_summary_escapes_table_cell_boundaries() {
        let mut row = row(
            "custom|provider",
            "Preset\nName",
            PresetResult::NetworkError {
                error: "bad | response".to_owned(),
            },
        );
        row.task.model_id = "model|id".to_owned();

        let markdown = super::github_summary_markdown(&[row]).expect("summary builds");

        assert!(markdown.contains(
            "| custom\\|provider | Preset<br>Name | model\\|id | ERROR (bad \\| response) |"
        ));
    }
}
