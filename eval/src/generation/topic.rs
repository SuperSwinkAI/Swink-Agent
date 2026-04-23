//! Topic planner for diverse experiment generation.
//!
//! Given a natural-language context + task + desired topic count, the planner
//! consults a [`JudgeClient`] to produce a `Vec<TopicSlot>` that partitions
//! the desired case count evenly across topics.
//!
//! Gated by the `generation` feature.

#![forbid(unsafe_code)]

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::judge::{JudgeClient, JudgeError};

/// One topic plus the number of cases to generate under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicSlot {
    /// Free-form topic label.
    pub topic: String,
    /// Number of cases the generator should produce for this topic.
    pub case_count: u32,
}

/// Plans diverse topics for an upcoming generation request.
pub struct TopicPlanner {
    judge: Arc<dyn JudgeClient>,
    model_id: String,
}

impl TopicPlanner {
    /// Build a planner bound to the given judge client.
    #[must_use]
    pub fn new(judge: Arc<dyn JudgeClient>, model_id: impl Into<String>) -> Self {
        Self {
            judge,
            model_id: model_id.into(),
        }
    }

    /// Model identifier forwarded to the judge.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Produce `num_topics` topics covering `context`/`task` and partition
    /// `desired_count` evenly across them.
    ///
    /// The function issues exactly one judge call — the `reason` field MUST
    /// contain a JSON array of topic strings. Extra/missing entries are
    /// reconciled by truncation / padding with fallback labels.
    pub async fn plan(
        &self,
        context: &str,
        task: &str,
        num_topics: u32,
        desired_count: u32,
    ) -> Result<Vec<TopicSlot>, JudgeError> {
        if num_topics == 0 {
            return Ok(Vec::new());
        }
        let prompt = format!(
            "Plan {num_topics} diverse, distinct topics covering this testing task.\n\
Context: {context}\nTask: {task}\n\
Respond with a JSON array of {num_topics} short topic strings."
        );
        let verdict = self.judge.judge(&prompt).await?;
        let topics = match verdict.reason.as_deref() {
            Some(body) => parse_topic_list(body, num_topics),
            None => fallback_topics(num_topics),
        };
        Ok(distribute(topics, desired_count))
    }
}

impl std::fmt::Debug for TopicPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopicPlanner")
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

fn parse_topic_list(body: &str, num_topics: u32) -> Vec<String> {
    match serde_json::from_str::<Vec<String>>(body.trim()) {
        Ok(mut parsed) => {
            parsed.truncate(num_topics as usize);
            while parsed.len() < num_topics as usize {
                parsed.push(format!("topic-{}", parsed.len() + 1));
            }
            parsed
        }
        Err(_) => fallback_topics(num_topics),
    }
}

fn fallback_topics(num_topics: u32) -> Vec<String> {
    (0..num_topics)
        .map(|i| format!("topic-{}", i + 1))
        .collect()
}

/// Evenly distribute `desired_count` cases across `topics`, preserving order.
#[must_use]
pub fn distribute(topics: Vec<String>, desired_count: u32) -> Vec<TopicSlot> {
    if topics.is_empty() || desired_count == 0 {
        return Vec::new();
    }
    let n = u32::try_from(topics.len()).unwrap_or(u32::MAX);
    let base = desired_count / n;
    let remainder = desired_count % n;
    topics
        .into_iter()
        .enumerate()
        .map(|(idx, topic)| {
            let idx = u32::try_from(idx).unwrap_or(u32::MAX);
            TopicSlot {
                topic,
                case_count: base + u32::from(idx < remainder),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
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
}
