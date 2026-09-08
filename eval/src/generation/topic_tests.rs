//! Tests for `topic`.
#![cfg(test)]

use super::*;

#[test]
fn distribution_is_even_when_divisible() {
    let slots = distribute(vec!["a".into(), "b".into(), "c".into(), "d".into()], 20);
    assert_eq!(slots.len(), 4);
    for slot in slots {
        assert_eq!(slot.case_count, 5);
    }
}

#[test]
fn distribution_spreads_remainder_across_leading_slots() {
    let slots = distribute(vec!["a".into(), "b".into(), "c".into()], 10);
    assert_eq!(slots.len(), 3);
    let counts: Vec<u32> = slots.iter().map(|s| s.case_count).collect();
    assert_eq!(counts, vec![4, 3, 3]);
}

#[test]
fn parse_truncates_and_pads() {
    let parsed = parse_topic_list(r#"["one","two","three"]"#, 2);
    assert_eq!(parsed, vec!["one".to_string(), "two".to_string()]);
    let parsed = parse_topic_list(r#"["one"]"#, 3);
    assert_eq!(
        parsed,
        vec![
            "one".to_string(),
            "topic-2".to_string(),
            "topic-3".to_string()
        ]
    );
}

#[test]
fn parse_falls_back_when_body_is_not_a_list() {
    let parsed = parse_topic_list("not json", 2);
    assert_eq!(parsed, vec!["topic-1".to_string(), "topic-2".to_string()]);
}
