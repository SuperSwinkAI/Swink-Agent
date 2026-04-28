//! US6 + US8: persistence and history tests.

use swink_agent::Cost;

use swink_agent_evolve::{
    AcceptanceResult, AcceptanceVerdict, BaselineSnapshot, Candidate, CandidateResult,
    CyclePersister, ManifestEntry, OptimizationTarget, TargetComponent,
};

fn make_candidate(mutated: &str) -> Candidate {
    Candidate::new(
        TargetComponent::FullPrompt,
        "original".to_string(),
        mutated.to_string(),
        "test".to_string(),
    )
}

fn make_candidate_result(candidate: Candidate, score: f64) -> CandidateResult {
    CandidateResult {
        candidate,
        results: vec![],
        aggregate_score: score,
        cost: Cost::default(),
    }
}

fn make_baseline(aggregate: f64) -> BaselineSnapshot {
    BaselineSnapshot {
        target: OptimizationTarget::new("sys", vec![]),
        results: vec![],
        aggregate_score: aggregate,
        cost: Cost::default(),
    }
}

fn make_acceptance(
    accepted: Vec<(Candidate, CandidateResult)>,
    rejected: Vec<(Candidate, CandidateResult, AcceptanceVerdict)>,
) -> AcceptanceResult {
    AcceptanceResult {
        applied: accepted,
        accepted_not_applied: vec![],
        rejected,
    }
}

#[test]
fn manifest_contains_all_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let baseline = make_baseline(0.6);

    let ca = make_candidate("v2");
    let ca_result = make_candidate_result(ca.clone(), 0.7);

    let cb = make_candidate("v3");
    let cb_result = make_candidate_result(cb.clone(), 0.55);

    let acceptance = make_acceptance(
        vec![(ca, ca_result)],
        vec![(cb, cb_result, AcceptanceVerdict::NoImprovement)],
    );

    let persister = CyclePersister::new(tmp.path());
    let dir = persister.persist(1, &acceptance, &baseline).unwrap();

    let manifest_content = std::fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
    let entries: Vec<ManifestEntry> = manifest_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry.cycle_id, 1);
        assert!(!entry.timestamp.is_empty());
        assert!(!entry.target_component.is_empty());
        assert!(!entry.original_value.is_empty());
        assert!(!entry.mutated_value.is_empty());
        assert!(!entry.strategy.is_empty());
        assert!(!entry.verdict.is_empty());
    }
    // Accepted entry has no rejection reason; rejected has one
    let accepted = entries.iter().find(|e| e.verdict == "Accepted").unwrap();
    assert!(accepted.rejection_reason.is_none());
    let rejected = entries.iter().find(|e| e.verdict == "Rejected").unwrap();
    assert!(rejected.rejection_reason.is_some());
}

#[test]
fn output_directory_versioned() {
    let tmp = tempfile::tempdir().unwrap();
    let baseline = make_baseline(0.5);

    let c1 = make_candidate("v1");
    let c1r = make_candidate_result(c1.clone(), 0.6);
    let accept1 = make_acceptance(vec![(c1, c1r)], vec![]);
    let persister = CyclePersister::new(tmp.path());
    persister.persist(1, &accept1, &baseline).unwrap();

    let c2 = make_candidate("v2");
    let c2r = make_candidate_result(c2.clone(), 0.7);
    let accept2 = make_acceptance(vec![(c2, c2r)], vec![]);
    persister.persist(2, &accept2, &baseline).unwrap();

    let dirs: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(
        dirs.iter().any(|d| d.starts_with("cycle-0001-")),
        "cycle-0001-* directory not found; dirs: {:?}",
        dirs
    );
    assert!(
        dirs.iter().any(|d| d.starts_with("cycle-0002-")),
        "cycle-0002-* directory not found; dirs: {:?}",
        dirs
    );
}

#[test]
fn no_config_written_when_all_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let baseline = make_baseline(0.6);

    let c = make_candidate("v2");
    let cr = make_candidate_result(c.clone(), 0.55);
    let acceptance = make_acceptance(vec![], vec![(c, cr, AcceptanceVerdict::NoImprovement)]);

    let persister = CyclePersister::new(tmp.path());
    let dir = persister.persist(1, &acceptance, &baseline).unwrap();

    // manifest.jsonl exists
    assert!(
        dir.join("manifest.jsonl").exists(),
        "manifest.jsonl should exist"
    );
    // system-prompt.md should NOT exist (no accepted candidates)
    assert!(
        !dir.join("system-prompt.md").exists(),
        "system-prompt.md should not be written when all candidates are rejected"
    );
}

#[test]
fn manifest_jsonl_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let baseline = make_baseline(0.5);

    let c = make_candidate("improved prompt");
    let cr = make_candidate_result(c.clone(), 0.7);
    let acceptance = make_acceptance(vec![(c, cr)], vec![]);

    let persister = CyclePersister::new(tmp.path());
    let dir = persister.persist(1, &acceptance, &baseline).unwrap();

    let content = std::fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
    let entries: Vec<ManifestEntry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].mutated_value, "improved prompt");
    assert_eq!(entries[0].verdict, "Accepted");
    assert!((entries[0].baseline_score - 0.5).abs() < 1e-10);
    assert!((entries[0].candidate_score - 0.7).abs() < 1e-10);
}

// ─── US8: history ──────────────────────────────────────────────────────────

#[test]
fn load_manifests_ordered_by_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let baseline = make_baseline(0.5);
    let persister = CyclePersister::new(tmp.path());

    // Write 3 cycles in reverse order to ensure sort is working
    for cycle in [3u32, 1, 2] {
        let c = make_candidate(&format!("v{cycle}"));
        let cr = make_candidate_result(c.clone(), 0.5 + cycle as f64 * 0.05);
        let acceptance = make_acceptance(vec![(c, cr)], vec![]);
        persister.persist(cycle, &acceptance, &baseline).unwrap();
    }

    let history = CyclePersister::load_history(tmp.path());
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].0, 1);
    assert_eq!(history[1].0, 2);
    assert_eq!(history[2].0, 3);
    for (_, entries) in &history {
        assert_eq!(entries.len(), 1);
    }
}
