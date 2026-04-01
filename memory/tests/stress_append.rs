//! Stress tests for JSONL append performance and correctness.

mod common;

use std::time::Instant;

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

    let (_, loaded) = store.load("stress_500").unwrap();
    assert_eq!(loaded.len(), 501); // 1 seed + 500 appended

    // Verify ordering: first message is the seed
    let first_text = match &loaded[0] {
        swink_agent::LlmMessage::User(u) => match &u.content[0] {
            swink_agent::ContentBlock::Text { text } => text.clone(),
            _ => panic!("unexpected content block"),
        },
        _ => panic!("expected user message"),
    };
    assert_eq!(first_text, "seed message");

    // Verify last message
    let last_text = match &loaded[500] {
        swink_agent::LlmMessage::Assistant(a) => match &a.content[0] {
            swink_agent::ContentBlock::Text { text } => text.clone(),
            _ => panic!("unexpected content block"),
        },
        _ => panic!("expected assistant message"),
    };
    assert_eq!(last_text, "assistant message 499");

    // Guard against catastrophic regression — 10s is generous for 500 appends
    assert!(
        elapsed.as_secs() < 10,
        "sequential append took {elapsed:?}, expected < 10s (possible O(n^2) regression)"
    );
}

#[test]
fn slow_path_triggered_by_title_change() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("slow_path", "short");
    let seed = vec![user_message("first"), assistant_message("second")];
    store.save("slow_path", &meta, &seed).unwrap();

    // Append a few messages with the original (short) title — fast path
    for i in 0..5 {
        store
            .append("slow_path", &[user_message(&format!("before title change {i}"))])
            .unwrap();
    }

    // Change the title to something longer, triggering the slow path on next append.
    // We do this by saving the full session with the new meta + all existing messages.
    let (_, existing) = store.load("slow_path").unwrap();
    let new_meta = sample_meta(
        "slow_path",
        "a much longer title that changes meta line byte length",
    );
    store.save("slow_path", &new_meta, &existing).unwrap();

    // Append more messages after the title change — slow path on the first one
    // (meta line length differs from what's on disk after the save, but subsequent
    // appends will hit fast path again since meta byte length stabilizes).
    for i in 0..5 {
        store
            .append("slow_path", &[assistant_message(&format!("after title change {i}"))])
            .unwrap();
    }

    let (loaded_meta, loaded) = store.load("slow_path").unwrap();

    // 2 seed + 5 before + 5 after = 12
    assert_eq!(loaded.len(), 12);
    assert_eq!(
        loaded_meta.title,
        "a much longer title that changes meta line byte length"
    );

    // Verify ordering: seed messages first, then before-change, then after-change
    let text_at = |idx: usize| -> String {
        match &loaded[idx] {
            swink_agent::LlmMessage::User(u) => match &u.content[0] {
                swink_agent::ContentBlock::Text { text } => text.clone(),
                other => panic!("unexpected content block at {idx}: {other:?}"),
            },
            swink_agent::LlmMessage::Assistant(a) => match &a.content[0] {
                swink_agent::ContentBlock::Text { text } => text.clone(),
                other => panic!("unexpected content block at {idx}: {other:?}"),
            },
            other @ swink_agent::LlmMessage::ToolResult(_) => {
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
fn append_performance_no_quadratic_regression() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("perf_guard", "Performance guard");
    store.save("perf_guard", &meta, &[]).unwrap();

    // First 100 appends
    let t1 = Instant::now();
    for i in 0..100 {
        store
            .append("perf_guard", &[user_message(&format!("msg {i}"))])
            .unwrap();
    }
    let first_100 = t1.elapsed();

    // Next 100 appends (messages 100–199)
    let t2 = Instant::now();
    for i in 100..200 {
        store
            .append("perf_guard", &[user_message(&format!("msg {i}"))])
            .unwrap();
    }
    let last_100 = t2.elapsed();

    let (_, loaded) = store.load("perf_guard").unwrap();
    assert_eq!(loaded.len(), 200);

    // The last 100 appends operate on a larger file, so some slowdown is expected
    // due to I/O. But O(n^2) would show a dramatic ratio. Allow up to 3x.
    let ratio = last_100.as_secs_f64() / first_100.as_secs_f64().max(0.001);
    assert!(
        ratio < 3.0,
        "last 100 appends took {last_100:?} vs first 100 {first_100:?} (ratio {ratio:.1}x) — possible O(n^2) regression"
    );
}
