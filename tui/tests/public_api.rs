use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swink_agent::testing::SimpleMockStreamFn;
use swink_agent::{
    Agent, AgentEvent, AgentOptions, AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason,
    Usage,
};
use swink_agent_tui::{
    App, CustomCommandOutcome, InProcessTransport, MessageRole, PathCandidate, TuiConfig,
    TuiExtensions, TuiTransport, UserInput, parse_mentions,
};

#[test]
fn tui_reexports_remain_consumable() {
    let _: fn(TuiConfig) -> App = App::new;
}

/// A stubbed assistant response, priced as the agent loop would have priced it.
fn stubbed_turn(model_id: &str, input: u64, output: u64, cost: f64) -> AgentEvent {
    AgentEvent::MessageEnd {
        message: AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "stub".to_string(),
            }],
            provider: "anthropic".to_string(),
            model_id: model_id.to_string(),
            usage: Usage {
                input,
                output,
                ..Usage::default()
            },
            cost: Cost {
                total: cost,
                ..Cost::default()
            },
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        },
    }
}

/// Issue #1084 §3: a downstream crate must be able to feed an `App` a stubbed
/// turn and assert on the resulting counters. `handle_agent_event` used to be
/// `pub(super)`, so `App` was constructible from outside the crate but could
/// never be advanced — this test would not have compiled.
#[test]
fn app_state_is_observable_from_outside_the_crate() {
    let mut app = App::new(TuiConfig::default());

    app.handle_agent_event(stubbed_turn("claude-sonnet-4-6", 1_200, 340, 0.0042));

    assert_eq!(app.total_input_tokens, 1_200);
    assert_eq!(app.total_output_tokens, 340);
    assert!((app.total_cost - 0.0042).abs() < 1e-9);
    assert_eq!(app.model_name, "claude-sonnet-4-6");
}

/// The per-turn breakdown behind `/usage` is public, so a host can render its
/// own view of it.
#[test]
fn per_turn_usage_is_observable_from_outside_the_crate() {
    let mut app = App::new(TuiConfig::default());

    app.handle_agent_event(stubbed_turn("model-a", 100, 20, 0.01));
    app.handle_agent_event(stubbed_turn("model-b", 200, 30, 0.02));

    assert_eq!(app.turn_usage.len(), 2);
    assert_eq!(app.turn_usage[0].model_id, "model-a");
    assert_eq!(app.turn_usage[1].input_tokens, 200);
    assert!((app.turn_usage[1].cost - 0.02).abs() < 1e-9);
    assert!((app.total_cost - 0.03).abs() < 1e-9);
}

/// Issue #1084 §2: a host must be able to register commands without forking the
/// crate. The registry itself is exercised in-crate; this asserts the seam is
/// reachable and composes with `App` from outside.
#[test]
fn host_commands_are_registrable_from_outside_the_crate() {
    let extensions = TuiExtensions::new().with_command("spend", |app: &App, _args: &str| {
        CustomCommandOutcome::Feedback(format!(
            "{} turn(s), ${:.4}",
            app.turn_usage.len(),
            app.total_cost
        ))
    });
    assert_eq!(extensions.command_names().collect::<Vec<_>>(), ["spend"]);

    let app = App::new(TuiConfig::default()).with_extensions(extensions);
    assert_eq!(app.turn_usage.len(), 0);
}

async fn recv_transport_event(transport: &mut InProcessTransport) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(3), transport.recv())
        .await
        .expect("transport should forward the agent event stream")
        .expect("agent stream should not close before AgentEnd")
}

async fn collect_turn_reply(transport: &mut InProcessTransport) -> String {
    let mut reply = String::new();

    loop {
        match recv_transport_event(transport).await {
            AgentEvent::MessageEnd { message } => {
                for block in &message.content {
                    if let ContentBlock::Text { text } = block {
                        reply.push_str(text);
                    }
                }
            }
            AgentEvent::AgentEnd { .. } => break,
            _ => {}
        }
    }

    reply
}

#[tokio::test]
async fn in_process_transport_spawn_drives_agent_events() {
    let stream = Arc::new(SimpleMockStreamFn::from_text("transport reply"));
    let options = AgentOptions::new_simple("system", ModelSpec::new("mock", "test"), stream);
    let agent = Agent::new(options);
    let mut transport = InProcessTransport::spawn(agent);

    transport
        .send(UserInput::new("hello from tui"))
        .await
        .expect("transport should accept user input");

    let mut saw_start = false;
    let mut saw_reply = false;

    loop {
        let event = recv_transport_event(&mut transport).await;

        match event {
            AgentEvent::AgentStart => saw_start = true,
            AgentEvent::MessageEnd { message } => {
                saw_reply = message.content.iter().any(|block| {
                    matches!(block, ContentBlock::Text { text } if text == "transport reply")
                });
            }
            AgentEvent::AgentEnd { .. } => break,
            _ => {}
        }
    }

    assert!(saw_start, "transport should forward AgentStart");
    assert!(saw_reply, "transport should forward the assistant reply");
}

#[tokio::test]
async fn in_process_transport_processes_queued_inputs_in_order() {
    let stream = Arc::new(SimpleMockStreamFn::from_text("queued reply"));
    let options = AgentOptions::new_simple("system", ModelSpec::new("mock", "test"), stream);
    let agent = Agent::new(options);
    let mut transport = InProcessTransport::spawn(agent);

    transport
        .send(UserInput::new("first queued prompt"))
        .await
        .expect("first input should be accepted");
    transport
        .send(UserInput::new("second queued prompt"))
        .await
        .expect("second input should be accepted");

    assert_eq!(collect_turn_reply(&mut transport).await, "queued reply");
    assert_eq!(collect_turn_reply(&mut transport).await, "queued reply");
}

// ─── @path file mentions (issue #1093) ───────────────────────────────────

/// The `@path` seam has to be usable from outside the crate: a downstream host
/// registers a completion provider and a resolver, and drives the popup — all
/// through the public API, with no `pub(crate)` reach-in.
#[test]
fn a_host_can_drive_path_completion_through_the_public_api() {
    let extensions = TuiExtensions::new().with_path_completions(|query| {
        ["src/lib.rs", "src/main.rs"]
            .into_iter()
            .filter(|path| path.contains(query))
            .map(|path| PathCandidate::new(path).with_detail("rust"))
            .collect()
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    // The host's provider is consulted as the mention is typed.
    for ch in "@src/".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    let completion = app
        .path_completion
        .as_ref()
        .expect("popup should be open on a public App");
    assert_eq!(completion.candidates.len(), 2);
    assert_eq!(
        completion.selected_candidate().map(|c| c.path.as_str()),
        Some("src/lib.rs")
    );

    // ...and the host can select and accept through the same public surface.
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.input.lines(), ["@src/main.rs "]);
    assert!(app.path_completion.is_none());
}

/// `parse_mentions` is public so a host's resolver can reuse the TUI's parsing
/// rather than re-deriving what counts as a mention.
#[test]
fn parse_mentions_is_reusable_by_a_host_resolver() {
    let mentions = parse_mentions("diff @a/one.rs against @b/two.rs");
    let paths: Vec<&str> = mentions.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["a/one.rs", "b/two.rs"]);
    assert_eq!(
        &"diff @a/one.rs against @b/two.rs"[mentions[0].start..mentions[0].end],
        "@a/one.rs"
    );
}

/// The whole point of the seam: the host reads the files, and only at submit.
#[tokio::test]
async fn a_host_resolver_expands_mentions_only_on_submit() {
    let resolved = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&resolved);

    let extensions = TuiExtensions::new()
        .with_path_completions(|_| vec![PathCandidate::new("notes.md")])
        .with_mention_resolver(move |text, mentions| {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut out = text.to_string();
            for mention in mentions.iter().rev() {
                out.replace_range(mention.start..mention.end, "EXPANDED");
            }
            Some(out)
        });

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);
    app.set_agent(Agent::new(AgentOptions::new_simple(
        "system",
        ModelSpec::new("mock", "test"),
        Arc::new(SimpleMockStreamFn::from_text("hi")),
    )));

    for ch in "read @notes.md".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(
        resolved.load(Ordering::SeqCst),
        0,
        "typing a mention must not read any files"
    );

    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        resolved.load(Ordering::SeqCst),
        1,
        "submitting resolves exactly once"
    );

    // The transcript still shows what the user typed, not the expansion.
    let displayed = app
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .expect("user message should be displayed");
    assert_eq!(displayed.content, "read @notes.md");
}
