//! Tests for `config`.
#![cfg(test)]

use super::*;

#[test]
fn structured_prompt_parsed_into_sections() {
    let prompt =
        "## Persona\nYou are helpful.\n\n## Rules\nBe concise.\n\n## Constraints\nNo markdown.";
    let target = OptimizationTarget::new(prompt, vec![]);
    assert_eq!(target.sections().len(), 3);
    assert_eq!(target.sections()[0].name.as_deref(), Some("Persona"));
    assert_eq!(target.sections()[1].name.as_deref(), Some("Rules"));
    assert_eq!(target.sections()[2].name.as_deref(), Some("Constraints"));
}

#[test]
fn unstructured_prompt_is_one_unnamed_section() {
    let prompt = "You are a helpful assistant that answers questions.";
    let target = OptimizationTarget::new(prompt, vec![]);
    assert_eq!(target.sections().len(), 1);
    assert_eq!(target.sections()[0].name, None);
    assert_eq!(target.sections()[0].content, prompt);
    assert_eq!(target.sections()[0].byte_range, 0..prompt.len());
}

#[test]
fn custom_delimiter_overrides_default() {
    let prompt = "### Alpha\nFirst content\n### Beta\nSecond content";
    let delim = Regex::new(r"(?m)^### (.+)$").unwrap();
    let target = OptimizationTarget::new(prompt, vec![]).with_section_delimiter(delim);
    assert_eq!(target.sections().len(), 2);
    assert_eq!(target.sections()[0].name.as_deref(), Some("Alpha"));
    assert_eq!(target.sections()[1].name.as_deref(), Some("Beta"));
}

#[test]
fn budget_tracks_spending() {
    let budget = CycleBudget::new(Cost::default().with_total(1.0));
    budget.record(Cost::default().with_total(0.3));
    budget.record(Cost::default().with_total(0.3));
    assert!(!budget.is_exhausted());
    budget.record(Cost::default().with_total(0.5));
    assert!(budget.is_exhausted());
}

#[test]
fn budget_exhausted_at_max() {
    let budget = CycleBudget::new(Cost::default().with_total(1.0));
    budget.record(Cost::default().with_total(1.0));
    assert!(budget.is_exhausted());
}
