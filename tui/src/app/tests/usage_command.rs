//! `/usage` command and host-registered command dispatch (issue #1084).

use swink_agent::{AgentEvent, AssistantMessage, Cost, StopReason, Usage};

use super::super::*;
use crate::config::TuiConfig;
use crate::extensions::{CustomCommandOutcome, TuiExtensions};

fn type_command(app: &mut App, text: &str) {
    for c in text.chars() {
        app.input.insert_char(c);
    }
    app.submit_input();
}

fn last_message(app: &App) -> &DisplayMessage {
    app.messages.last().expect("a message should be pushed")
}

fn stubbed_turn(model_id: &str, input: u64, output: u64, cost: f64) -> AgentEvent {
    AgentEvent::MessageEnd {
        message: AssistantMessage {
            content: vec![],
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

#[test]
fn slash_usage_reports_the_per_turn_breakdown() {
    let mut app = App::new(TuiConfig::default());
    app.handle_agent_event(stubbed_turn("claude-sonnet-4-6", 1_000, 200, 0.01));
    app.handle_agent_event(stubbed_turn("claude-sonnet-4-6", 2_000, 300, 0.02));

    type_command(&mut app, "/usage");

    let message = last_message(&app);
    assert_eq!(message.role, MessageRole::System);
    assert!(message.content.contains("Usage — 2 turns"), "{message:?}");
    assert!(message.content.contains("$0.0300"), "{message:?}");
}

#[test]
fn slash_usage_before_any_turn_reports_no_usage() {
    let mut app = App::new(TuiConfig::default());
    type_command(&mut app, "/usage");
    assert!(
        last_message(&app).content.contains("No usage recorded yet"),
        "{:?}",
        last_message(&app)
    );
}

#[test]
fn slash_usage_is_not_forwarded_to_the_agent() {
    let mut app = App::new(TuiConfig::default());
    type_command(&mut app, "/usage");

    // Only the system feedback message; the command never became a user turn.
    assert_eq!(app.messages.len(), 1);
    assert_eq!(last_message(&app).role, MessageRole::System);
}

#[test]
fn reset_clears_the_per_turn_breakdown() {
    let mut app = App::new(TuiConfig::default());
    app.handle_agent_event(stubbed_turn("model-a", 1_000, 200, 0.01));
    assert_eq!(app.turn_usage.len(), 1);

    app.reset_session_state();

    assert!(app.turn_usage.is_empty());
    assert!((app.total_cost).abs() < 1e-9);
}

#[test]
fn host_command_runs_and_pushes_its_feedback() {
    let extensions = TuiExtensions::new().with_command("spend", |app: &App, _args: &str| {
        CustomCommandOutcome::Feedback(format!("spent ${:.4}", app.total_cost))
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);
    app.handle_agent_event(stubbed_turn("model-a", 10, 5, 0.5));

    type_command(&mut app, "/spend");

    assert_eq!(last_message(&app).content, "spent $0.5000");
    assert_eq!(last_message(&app).role, MessageRole::System);
}

#[test]
fn host_command_receives_its_arguments() {
    let extensions = TuiExtensions::new().with_command("echo", |_app: &App, args: &str| {
        CustomCommandOutcome::Feedback(format!("echo:{args}"))
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "/echo one two");

    assert_eq!(last_message(&app).content, "echo:one two");
}

#[test]
fn host_command_works_under_the_hash_sigil_too() {
    let extensions = TuiExtensions::new().with_command("stats", |_app: &App, _args: &str| {
        CustomCommandOutcome::Feedback("stats".to_string())
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "#stats");

    assert_eq!(last_message(&app).content, "stats");
}

#[test]
fn host_command_shadows_a_built_in() {
    let extensions = TuiExtensions::new().with_command("usage", |_app: &App, _args: &str| {
        CustomCommandOutcome::Feedback("host usage".to_string())
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "/usage");

    assert_eq!(last_message(&app).content, "host usage");
}

#[test]
fn host_command_declining_falls_through_to_the_built_in() {
    let extensions = TuiExtensions::new().with_command("usage", |_app: &App, _args: &str| {
        CustomCommandOutcome::NotHandled
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "/usage");

    assert!(
        last_message(&app).content.contains("No usage recorded yet"),
        "{:?}",
        last_message(&app)
    );
}

#[test]
fn unregistered_command_still_reaches_the_built_in_table() {
    let extensions = TuiExtensions::new().with_command("other", |_app: &App, _args: &str| {
        CustomCommandOutcome::Feedback("other".to_string())
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "/nonexistent");

    assert!(
        last_message(&app).content.contains("Unknown command"),
        "{:?}",
        last_message(&app)
    );
}

/// Host handlers must never see secret-bearing input: the `#key` classification
/// runs first and returns before dispatch.
#[test]
fn host_command_never_sees_key_input() {
    let extensions = TuiExtensions::new().with_command("key", |_app: &App, args: &str| {
        CustomCommandOutcome::Feedback(format!("leaked:{args}"))
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "#key openai sk-leak-sentinel");

    let rendered = last_message(&app).content.clone();
    assert!(
        !rendered.contains("sk-leak-sentinel"),
        "secret reached a host command handler: {rendered}"
    );
    assert!(!rendered.starts_with("leaked:"), "{rendered}");
}

/// Plain prompts must not be mistaken for host commands.
#[test]
fn plain_text_is_not_dispatched_to_host_commands() {
    let extensions = TuiExtensions::new().with_command("hello", |_app: &App, _args: &str| {
        CustomCommandOutcome::Feedback("host ran".to_string())
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "hello there");

    assert_eq!(last_message(&app).role, MessageRole::User);
    assert_eq!(last_message(&app).content, "hello there");
}
