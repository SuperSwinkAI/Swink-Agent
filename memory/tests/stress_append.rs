//! Stress tests for JSONL append performance and correctness.

mod common;

use std::time::Instant;

use swink_agent::{AgentMessage, LlmMessage};
use swink_agent_memory::{JsonlSessionStore, SessionStore};

use common::{assistant_message, sample_meta, user_message};

#[test]
fn sequential_append_500_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("stress_500", "Stress test");
    let seed = vec![user_message("seed message")];
    store.save("stress_500", &meta, &seed).unwrap();

    let start = Instant::now();

    for i in 0..500 {
        let msg = if i % 2 == 0 {
            user_message(&format!("user message {i}"))
        } else {
            assistant_message(&format!("assistant message {i}"))
        };
        store.append("stress_500", &[msg]).unwrap();
    }

    let elapsed = start.elapsed();

    let (_, loaded) = store.load("stress_500", None).unwrap();
    assert_eq!(loaded.len(), 501); // 1 seed + 500 appended

    // Verify ordering: first message is the seed
    let first_text = match &loaded[0] {
        AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
            swink_agent::ContentBlock::Text { text } => text.clone(),
            _ => panic!("unexpected content block"),
        },
        _ => panic!("expected user message"),
    };
    assert_eq!(first_text, "seed message");

    // Verify last message
    let last_text = match &loaded[500] {
        AgentMessage::Llm(LlmMessage::Assistant(a)) => match &a.content[0] {
            swink_agent::ContentBlock::Text { text } => text.clone(),
            _ => panic!("unexpected content block"),
        },
        _ => panic!("expected assistant message"),
    };
    assert_eq!(last_text, "assistant message 499");

    // Guard against catastrophic regression — 10s is generous even though
    // append now rewrites through an atomic temp file for crash safety.
    assert!(
        elapsed.as_secs() < 10,
        "sequential append took {elapsed:?}, expected < 10s (possible O(n^2) regression)"
    );
}

#[test]
fn append_remains_correct_after_title_change() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("slow_path", "short");
    let seed = vec![user_message("first"), assistant_message("second")];
    store.save("slow_path", &meta, &seed).unwrap();

    for i in 0..5 {
        store
            .append(
                "slow_path",
                &[user_message(&format!("before title change {i}"))],
            )
            .unwrap();
    }

    // Change the title by saving the full session with new metadata.
    let (mut loaded_meta, existing) = store.load("slow_path", None).unwrap();
    loaded_meta.title = "a much longer title that changes meta line byte length".to_string();
    store.save("slow_path", &loaded_meta, &existing).unwrap();

    for i in 0..5 {
        store
            .append(
                "slow_path",
                &[assistant_message(&format!("after title change {i}"))],
            )
            .unwrap();
    }

    let (loaded_meta, loaded) = store.load("slow_path", None).unwrap();

    // 2 seed + 5 before + 5 after = 12
    assert_eq!(loaded.len(), 12);
    assert_eq!(
        loaded_meta.title,
        "a much longer title that changes meta line byte length"
    );

    // Verify ordering: seed messages first, then before-change, then after-change
    let text_at = |idx: usize| -> String {
        match &loaded[idx] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                swink_agent::ContentBlock::Text { text } => text.clone(),
                other => panic!("unexpected content block at {idx}: {other:?}"),
            },
            AgentMessage::Llm(LlmMessage::Assistant(a)) => match &a.content[0] {
                swink_agent::ContentBlock::Text { text } => text.clone(),
                other => panic!("unexpected content block at {idx}: {other:?}"),
            },
            other => {
                panic!("unexpected message type at {idx}: {other:?}")
            }
        }
    };

    assert_eq!(text_at(0), "first");
    assert_eq!(text_at(1), "second");
    assert_eq!(text_at(2), "before title change 0");
    assert_eq!(text_at(6), "before title change 4");
    assert_eq!(text_at(7), "after title change 0");
    assert_eq!(text_at(11), "after title change 4");
}

#[test]
fn append_200_messages_completes_in_reasonable_time() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("perf_guard", "Performance guard");
    store.save("perf_guard", &meta, &[]).unwrap();

    let start = Instant::now();
    for i in 0..200 {
        store
            .append("perf_guard", &[user_message(&format!("msg {i}"))])
            .unwrap();
    }
    let elapsed = start.elapsed();

    let (_, loaded) = store.load("perf_guard", None).unwrap();
    assert_eq!(loaded.len(), 200);

    assert!(
        elapsed.as_secs() < 10,
        "200 append operations took {elapsed:?}, expected < 10s"
    );
}
