use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use swink_agent::sliding_window;
use swink_agent::types::{AgentMessage, ContentBlock, LlmMessage, UserMessage};

fn make_messages(n: usize, text: &str) -> Vec<AgentMessage> {
    (0..n)
        .map(|i| {
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: format!("{text} {i}"),
                }],
                timestamp: 0,
                cache_hint: None,
            }))
        })
        .collect()
}

fn bench_compact_no_op(c: &mut Criterion) {
    // Budget large enough that no messages are ever dropped.
    let compactor = sliding_window(1_000_000, 1_000_000, 10);

    c.bench_function("compact_sliding_window/no_op_100_msgs", |b| {
        b.iter_batched(
            || make_messages(100, "message number with some realistic content"),
            |mut msgs| compactor(&mut msgs, false),
            BatchSize::SmallInput,
        );
    });
}

fn bench_compact_heavy(c: &mut Criterion) {
    // Budget small enough to force compaction on every call.
    let compactor = sliding_window(500, 500, 10);

    c.bench_function("compact_sliding_window/heavy_500_msgs", |b| {
        b.iter_batched(
            || {
                make_messages(
                    500,
                    "message number with some realistic content that is long enough to matter",
                )
            },
            |mut msgs| compactor(&mut msgs, false),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_compact_no_op, bench_compact_heavy);
criterion_main!(benches);
