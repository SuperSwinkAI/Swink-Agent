//! Tests for `types`.
#![cfg(test)]

use std::sync::Arc;

use super::ContextMessages;
use crate::types::{AgentMessage, ContentBlock, LlmMessage, UserMessage};

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

fn text_of(message: &LlmMessage) -> &str {
    match message {
        LlmMessage::User(u) => match &u.content[0] {
            ContentBlock::Text { text } => text,
            other => panic!("expected text content, got {other:?}"),
        },
        other => panic!("expected user message, got {other:?}"),
    }
}

#[test]
fn snapshot_reuses_arcs_for_untouched_prefix() {
    let mut context = ContextMessages::new(vec![user_msg("a"), user_msg("b")]);
    let first = context.snapshot_llm();

    context.push(user_msg("c"));
    let second = context.snapshot_llm();

    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 3);
    assert!(
        Arc::ptr_eq(&first[0], &second[0]) && Arc::ptr_eq(&first[1], &second[1]),
        "untouched prefix must be shared, not deep-copied"
    );
    assert_eq!(text_of(&second[2]), "c");
}

#[test]
fn set_invalidates_mirror_from_replaced_index() {
    let mut context = ContextMessages::new(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
    let first = context.snapshot_llm();

    context.set(1, user_msg("b2"));
    let second = context.snapshot_llm();

    assert!(
        Arc::ptr_eq(&first[0], &second[0]),
        "prefix before the replaced index stays shared"
    );
    assert!(
        !Arc::ptr_eq(&first[1], &second[1]),
        "the replaced message must be re-cloned"
    );
    assert_eq!(text_of(&second[1]), "b2");
    assert_eq!(text_of(&second[2]), "c");
}

#[test]
fn make_mut_invalidates_whole_mirror_and_reflects_mutation() {
    let mut context = ContextMessages::new(vec![user_msg("a"), user_msg("b")]);
    let first = context.snapshot_llm();

    if let AgentMessage::Llm(LlmMessage::User(u)) = &mut context.make_mut()[0] {
        u.content = vec![ContentBlock::Text {
            text: "a2".to_string(),
        }];
    }
    let second = context.snapshot_llm();

    assert!(
        !Arc::ptr_eq(&first[0], &second[0]),
        "bulk mutation must invalidate the mirror"
    );
    assert_eq!(text_of(&second[0]), "a2");
    assert_eq!(text_of(&second[1]), "b");
}

#[test]
fn take_vec_empties_history_and_mirror() {
    let mut context = ContextMessages::new(vec![user_msg("a")]);
    let _ = context.snapshot_llm();

    let taken = context.take_vec();

    assert_eq!(taken.len(), 1);
    assert!(context.as_slice().is_empty());
    assert!(context.snapshot_llm().is_empty());
}
