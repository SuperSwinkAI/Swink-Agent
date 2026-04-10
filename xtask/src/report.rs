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
    let mut md = String::from("## Catalog Verification\n\n");
    md.push_str("| Provider | Preset | Model ID | Result |\n");
    md.push_str("|---|---|---|---|\n");
    for row in rows {
        let key = &row.task.provider_key;
        let preset = &row.task.preset_display;
        let model = &row.task.model_id;
        let result = result_label(&row.result);
        writeln!(md, "| {key} | {preset} | {model} | {result} |")
            .map_err(std::io::Error::other)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&summary_path)?;
    file.write_all(md.as_bytes())?;
    Ok(())
}
