//! Deferred host commands and the agent-swap seam (issue #1201).
//!
//! The seam exists so a downstream `/model <name>` can rebuild the agent from
//! its own provider/credential config and install it mid-session. These pin the
//! three properties that makes usable: the task runs off the *next* flush pass
//! (so its notice is on screen first), the swap keeps the session identity and
//! transcript, and it is refused where it would corrupt state.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::testing::text_events;
use swink_agent::{
    Agent, AgentMessage, ContentBlock, LlmMessage, ModelSpec, StreamFn, UserMessage,
};

use crate::config::TuiConfig;
use crate::extensions::{AgentSwap, CustomCommandOutcome, HostAction, TuiExtensions};

use super::super::*;
use super::helpers::*;

fn type_command(app: &mut App, text: &str) {
    for c in text.chars() {
        app.editor.input.insert_char(c);
    }
    app.submit_input();
}

fn last_message(app: &App) -> &DisplayMessage {
    app.view.messages.last().expect("a message should exist")
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage::new(vec![
        ContentBlock::Text {
            text: text.to_string(),
        },
    ])))
}

fn agent_history(app: &App) -> Vec<String> {
    app.agent_io
        .agent
        .as_ref()
        .expect("an agent should be installed")
        .state()
        .messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Llm(LlmMessage::User(user)) => {
                Some(ContentBlock::extract_text(&user.content))
            }
            _ => None,
        })
        .collect()
}

/// An agent whose model roster differs from the default test agent's, so a
/// swap is observable through `model_name` / `available_models`.
fn replacement_agent() -> Agent {
    let stream = Arc::new(ScriptedStreamFn::new(vec![text_events("from replacement")]));
    make_test_agent_with_models(
        ModelSpec::new("openai", "replacement-model"),
        stream as Arc<dyn StreamFn>,
        vec![(
            ModelSpec::new("openai", "replacement-alt"),
            Arc::new(ScriptedStreamFn::new(vec![text_events("alt")])) as Arc<dyn StreamFn>,
        )],
    )
}

fn app_with_swap_command(swap: impl Fn() -> AgentSwap + Send + Sync + 'static) -> App {
    let swap = Arc::new(swap);
    let extensions = TuiExtensions::new().with_command("model", move |_app: &App, _args: &str| {
        let swap = Arc::clone(&swap);
        CustomCommandOutcome::deferred_with_notice("Switching model…", move || {
            let swap = Arc::clone(&swap);
            async move { HostAction::ReplaceAgent(swap()) }
        })
    });
    App::new(TuiConfig::default()).with_extensions(extensions)
}

#[tokio::test]
async fn deferred_command_shows_its_notice_before_the_task_runs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let extensions = TuiExtensions::new().with_command("slow", move |_app: &App, _args: &str| {
        let seen = Arc::clone(&seen);
        CustomCommandOutcome::deferred_with_notice("working…", move || {
            let seen = Arc::clone(&seen);
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                HostAction::Feedback("finished".to_string())
            }
        })
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "/slow");

    // Dispatch queued the task but did not run it: the frame carrying the
    // notice is drawn before the event loop's next flush pass.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(last_message(&app).content, "working…");
    assert!(app.agent_io.pending_host_task.is_some());

    app.flush_host_task().await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(last_message(&app).content, "finished");
    assert!(app.agent_io.pending_host_task.is_none());
}

#[tokio::test]
async fn flushing_without_a_queued_task_is_a_no_op() {
    let mut app = App::new(TuiConfig::default());
    app.flush_host_task().await;
    assert!(app.view.messages.is_empty());
}

#[tokio::test]
async fn deferred_command_is_not_forwarded_to_the_agent() {
    let extensions = TuiExtensions::new().with_command("noop", |_app: &App, _args: &str| {
        CustomCommandOutcome::deferred(|| async { HostAction::Nothing })
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);
    app.set_agent(make_test_agent(Arc::new(ScriptedStreamFn::new(vec![
        text_events("unused"),
    ]))));

    type_command(&mut app, "/noop");
    app.flush_host_task().await;

    // No notice, no feedback, no user turn — the command's effects were
    // entirely on the host's side.
    assert!(app.view.messages.is_empty());
    assert_eq!(app.agent_io.status, AgentStatus::Idle);
}

#[tokio::test]
async fn a_second_deferred_command_replaces_the_first() {
    let extensions = TuiExtensions::new().with_command("queue", |_app: &App, args: &str| {
        let args = args.to_string();
        CustomCommandOutcome::deferred(move || {
            let args = args.clone();
            async move { HostAction::Feedback(format!("ran:{args}")) }
        })
    });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_command(&mut app, "/queue first");
    type_command(&mut app, "/queue second");
    app.flush_host_task().await;

    assert_eq!(last_message(&app).content, "ran:second");
    assert!(app.agent_io.pending_host_task.is_none());
}

#[tokio::test]
async fn agent_swap_adopts_the_replacement_model_and_roster() {
    let mut app = app_with_swap_command(|| {
        AgentSwap::new(replacement_agent()).with_feedback("Model: replacement-model")
    });
    app.set_agent(make_test_agent_with_models(
        ModelSpec::new("anthropic", "original-model"),
        Arc::new(ScriptedStreamFn::new(vec![text_events("original")])) as Arc<dyn StreamFn>,
        Vec::new(),
    ));
    let session_id = app.session.session_id.clone();
    assert_eq!(app.mode.model_name, "original-model");

    type_command(&mut app, "/model replacement");
    app.flush_host_task().await;

    assert_eq!(app.mode.model_name, "replacement-model");
    assert_eq!(app.available_models().len(), 2);
    assert_eq!(
        app.session.session_id, session_id,
        "a swap keeps the session identity"
    );
    assert_eq!(last_message(&app).content, "Model: replacement-model");
}

#[tokio::test]
async fn agent_swap_carries_the_conversation_across() {
    let mut app = app_with_swap_command(|| AgentSwap::new(replacement_agent()));
    app.set_agent(make_test_agent(Arc::new(ScriptedStreamFn::new(vec![
        text_events("original"),
    ]))));
    app.agent_io
        .agent
        .as_mut()
        .expect("agent installed")
        .set_messages(vec![user_message("earlier turn")]);
    app.push_system_message("visible transcript".to_string());

    type_command(&mut app, "/model replacement");
    app.flush_host_task().await;

    assert_eq!(agent_history(&app), ["earlier turn"]);
    assert!(
        app.view
            .messages
            .iter()
            .any(|m| m.content == "visible transcript"),
        "the on-screen transcript survives the swap"
    );
}

#[tokio::test]
async fn agent_swap_without_history_starts_from_the_replacement() {
    let mut app = app_with_swap_command(|| AgentSwap::new(replacement_agent()).without_history());
    app.set_agent(make_test_agent(Arc::new(ScriptedStreamFn::new(vec![
        text_events("original"),
    ]))));
    app.agent_io
        .agent
        .as_mut()
        .expect("agent installed")
        .set_messages(vec![user_message("earlier turn")]);

    type_command(&mut app, "/model replacement");
    app.flush_host_task().await;

    assert!(agent_history(&app).is_empty());
}

#[tokio::test]
async fn agent_swap_preserves_the_context_budget() {
    let mut app = app_with_swap_command(|| AgentSwap::new(replacement_agent()));
    app.set_agent(make_test_agent(Arc::new(ScriptedStreamFn::new(vec![
        text_events("original"),
    ]))));
    app.usage.context_budget = 250_000;

    type_command(&mut app, "/model replacement");
    app.flush_host_task().await;

    assert_eq!(app.usage.context_budget, 250_000);
}

#[tokio::test]
async fn agent_swap_clears_a_queued_model_cycle() {
    let mut app = app_with_swap_command(|| AgentSwap::new(replacement_agent()));
    app.set_agent(make_test_agent_with_models(
        ModelSpec::new("anthropic", "original-model"),
        Arc::new(ScriptedStreamFn::new(vec![text_events("original")])) as Arc<dyn StreamFn>,
        vec![(
            ModelSpec::new("anthropic", "original-alt"),
            Arc::new(ScriptedStreamFn::new(vec![text_events("alt")])) as Arc<dyn StreamFn>,
        )],
    ));
    app.cycle_model();
    assert!(app.mode.pending_model.is_some());

    type_command(&mut app, "/model replacement");
    app.flush_host_task().await;

    assert!(
        app.mode.pending_model.is_none(),
        "the queued cycle targeted the roster of the agent just replaced"
    );
    assert_eq!(app.mode.model_name, "replacement-model");
}

#[tokio::test]
async fn agent_swap_is_refused_mid_turn() {
    let mut app = app_with_swap_command(|| AgentSwap::new(replacement_agent()));
    app.set_agent(make_test_agent_with_models(
        ModelSpec::new("anthropic", "original-model"),
        Arc::new(ScriptedStreamFn::new(vec![text_events("original")])) as Arc<dyn StreamFn>,
        Vec::new(),
    ));

    type_command(&mut app, "/model replacement");
    app.agent_io.status = AgentStatus::Running;
    app.flush_host_task().await;

    assert_eq!(app.mode.model_name, "original-model");
    assert!(
        last_message(&app).content.contains("blocked while"),
        "{:?}",
        last_message(&app)
    );
}

#[tokio::test]
async fn agent_swap_is_refused_on_an_external_transport() {
    let extensions = TuiExtensions::new().with_command("model", |_app: &App, _args: &str| {
        CustomCommandOutcome::deferred(|| async {
            HostAction::ReplaceAgent(AgentSwap::new(replacement_agent()))
        })
    });
    let (input_tx, _input_rx) = tokio::sync::mpsc::channel(1);
    let (_event_tx, event_rx) = tokio::sync::mpsc::channel(1);
    let mut app = App::new(TuiConfig::default())
        .with_extensions(extensions)
        .with_transport(Box::new(
            crate::transport::InProcessTransport::from_channels(input_tx, event_rx),
        ));

    type_command(&mut app, "/model replacement");
    app.flush_host_task().await;

    assert!(app.agent_io.agent.is_none());
    assert!(
        last_message(&app).content.contains("external transport"),
        "{:?}",
        last_message(&app)
    );
}
