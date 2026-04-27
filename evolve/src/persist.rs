use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::diagnose::TargetComponent;
use crate::evaluate::CandidateResult;
use crate::gate::{AcceptanceResult, AcceptanceVerdict};
use crate::mutate::Candidate;
use crate::types::BaselineSnapshot;

/// One line of the cycle JSONL manifest, covering a single evaluated candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub cycle_id: u32,
    pub timestamp: String,
    pub target_component: String,
    pub original_value: String,
    pub mutated_value: String,
    pub strategy: String,
    pub baseline_score: f64,
    pub candidate_score: f64,
    pub verdict: String,
    pub rejection_reason: Option<String>,
}

/// Writes cycle outputs to a versioned directory under `output_root`.
pub struct CyclePersister {
    output_root: PathBuf,
}

impl CyclePersister {
    pub fn new(output_root: impl Into<PathBuf>) -> Self {
        Self { output_root: output_root.into() }
    }

    /// Persist a single cycle to `<output_root>/cycle-{N:04}-{timestamp}/`.
    ///
    /// Returns the path of the created cycle directory.
    pub fn persist(
        &self,
        cycle_number: u32,
        acceptance_result: &AcceptanceResult,
        baseline: &BaselineSnapshot,
    ) -> std::io::Result<PathBuf> {
        let now = chrono::Utc::now();
        let ts = now.format("%Y-%m-%dT%H-%M-%SZ").to_string();
        let dir = self.output_root.join(format!("cycle-{:04}-{}", cycle_number, ts));
        std::fs::create_dir_all(&dir)?;

        let now_iso = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut entries: Vec<ManifestEntry> = Vec::new();

        for (candidate, result) in &acceptance_result.applied {
            entries.push(build_entry(cycle_number, &now_iso, candidate, result, baseline, "Accepted", None));
            write_config(&dir, candidate)?;
        }
        for (candidate, result) in &acceptance_result.accepted_not_applied {
            entries.push(build_entry(cycle_number, &now_iso, candidate, result, baseline, "AcceptedNotApplied", None));
        }
        for (candidate, result, verdict) in &acceptance_result.rejected {
            let reason = rejection_reason_string(verdict);
            entries.push(build_entry(cycle_number, &now_iso, candidate, result, baseline, "Rejected", Some(reason)));
        }

        let manifest: String = entries
            .iter()
            .map(|e| serde_json::to_string(e).expect("ManifestEntry is always serializable") + "\n")
            .collect();
        std::fs::write(dir.join("manifest.jsonl"), manifest)?;

        Ok(dir)
    }

    /// Load all cycle manifests under `output_root`, sorted by cycle number.
    pub fn load_history(output_root: impl Into<PathBuf>) -> Vec<(u32, Vec<ManifestEntry>)> {
        let root = output_root.into();
        let Ok(read_dir) = std::fs::read_dir(&root) else { return Vec::new() };

        let mut dirs: Vec<(u32, PathBuf)> = read_dir
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                let rest = s.strip_prefix("cycle-")?;
                let num: u32 = rest.split('-').next()?.parse().ok()?;
                Some((num, e.path()))
            })
            .collect();

        dirs.sort_by_key(|(n, _)| *n);

        dirs.into_iter()
            .filter_map(|(num, dir)| {
                let content = std::fs::read_to_string(dir.join("manifest.jsonl")).ok()?;
                let entries: Vec<ManifestEntry> = content
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect();
                Some((num, entries))
            })
            .collect()
    }
}

fn build_entry(
    cycle_id: u32,
    timestamp: &str,
    candidate: &Candidate,
    result: &CandidateResult,
    baseline: &BaselineSnapshot,
    verdict: &str,
    rejection_reason: Option<String>,
) -> ManifestEntry {
    ManifestEntry {
        cycle_id,
        timestamp: timestamp.to_string(),
        target_component: component_str(&candidate.component),
        original_value: candidate.original_value.clone(),
        mutated_value: candidate.mutated_value.clone(),
        strategy: candidate.strategy.clone(),
        baseline_score: baseline.aggregate_score,
        candidate_score: result.aggregate_score,
        verdict: verdict.to_string(),
        rejection_reason,
    }
}

fn write_config(dir: &Path, candidate: &Candidate) -> std::io::Result<()> {
    match &candidate.component {
        TargetComponent::FullPrompt | TargetComponent::PromptSection { .. } => {
            std::fs::write(dir.join("system-prompt.md"), &candidate.mutated_value)?;
        }
        TargetComponent::ToolDescription { tool_name } => {
            std::fs::write(
                dir.join(format!("tool-{}.json", tool_name)),
                &candidate.mutated_value,
            )?;
        }
    }
    Ok(())
}

fn component_str(c: &TargetComponent) -> String {
    match c {
        TargetComponent::FullPrompt => "FullPrompt".to_string(),
        TargetComponent::PromptSection { name, .. } => {
            format!("PromptSection({})", name.as_deref().unwrap_or("unnamed"))
        }
        TargetComponent::ToolDescription { tool_name } => {
            format!("ToolDescription({})", tool_name)
        }
    }
}

fn rejection_reason_string(verdict: &AcceptanceVerdict) -> String {
    match verdict {
        AcceptanceVerdict::BelowThreshold { improvement, threshold } => {
            format!("BelowThreshold(improvement={improvement:.4}, threshold={threshold:.4})")
        }
        AcceptanceVerdict::P1Regression { case_id } => format!("P1Regression(case_id={case_id})"),
        AcceptanceVerdict::NoImprovement => "NoImprovement".to_string(),
        AcceptanceVerdict::Accepted => "Accepted".to_string(),
        AcceptanceVerdict::AcceptedNotApplied => "AcceptedNotApplied".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_entry_jsonl_roundtrip() {
        let entry = ManifestEntry {
            cycle_id: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            target_component: "PromptSection(Persona)".to_string(),
            original_value: "You are a helpful assistant.".to_string(),
            mutated_value: "You must be a helpful assistant.".to_string(),
            strategy: "template".to_string(),
            baseline_score: 0.7,
            candidate_score: 0.82,
            verdict: "Accepted".to_string(),
            rejection_reason: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: ManifestEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.cycle_id, entry.cycle_id);
        assert_eq!(restored.verdict, entry.verdict);
        assert_eq!(restored.candidate_score, entry.candidate_score);
        assert_eq!(restored.rejection_reason, entry.rejection_reason);
    }
}
