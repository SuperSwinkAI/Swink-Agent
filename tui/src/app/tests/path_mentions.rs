//! `@path` file-mention completion and lazy injection (issue #1093).
//!
//! The load-bearing property here is *when* things run: the completion provider
//! fires on every keystroke inside a mention, while the resolver — the seam that
//! reads files — must not fire until the prompt is submitted.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::*;
use super::helpers::{
    PromptCapturingStreamFn, ResolverSpy, drain_agent_events_until_idle, make_test_agent,
};
use crate::config::TuiConfig;
use crate::extensions::{PathCandidate, TuiExtensions};

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

/// Extensions with a fixed candidate set and a resolver wired to `spy`.
fn extensions_with_spy(spy: &Arc<ResolverSpy>) -> TuiExtensions {
    let resolver_spy = Arc::clone(spy);
    TuiExtensions::new()
        .with_path_completions(|query| {
            ["src/lib.rs", "src/main.rs", "README.md"]
                .into_iter()
                .filter(|path| path.contains(query))
                .map(PathCandidate::new)
                .collect()
        })
        .with_mention_resolver(move |text, mentions| {
            resolver_spy.calls.fetch_add(1, Ordering::SeqCst);
            resolver_spy.seen.lock().unwrap().push(text.to_string());

            let mut out = text.to_string();
            for mention in mentions.iter().rev() {
                out.replace_range(
                    mention.start..mention.end,
                    &format!("<file:{}>CONTENT</file>", mention.path),
                );
            }
            Some(out)
        })
}

// ─── the load-bearing test ────────────────────────────────────────────────

/// The requirement most likely to regress silently: file content must be
/// resolved when the prompt is submitted, and never as the user types.
#[tokio::test]
async fn mentions_resolve_at_submit_and_never_while_typing() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let spy = Arc::new(ResolverSpy::default());

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    // Typing the whole mention out, character by character. Every one of these
    // keystrokes runs a completion refresh.
    type_text(&mut app, "explain @src/lib.rs");
    assert!(
        app.editor.path_completion.is_some(),
        "completion popup should be open mid-mention"
    );
    assert_eq!(
        spy.call_count(),
        0,
        "resolver must not run while typing — that would read files on every keystroke"
    );

    // Deleting and retyping, then moving the cursor across the mention, all
    // refresh completion. Still nothing resolved.
    press(&mut app, KeyCode::Backspace);
    type_text(&mut app, "s");
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Right);
    assert_eq!(
        spy.call_count(),
        0,
        "resolver must not run on cursor movement or edits"
    );

    // Dismiss the popup so Enter submits rather than accepting a candidate.
    press(&mut app, KeyCode::Esc);
    assert_eq!(spy.call_count(), 0, "dismissing the popup resolves nothing");

    // Submit — the one and only moment a mention is allowed to resolve.
    press(&mut app, KeyCode::Enter);
    assert_eq!(spy.call_count(), 1, "resolver runs exactly once, at submit");
    assert_eq!(
        spy.seen.lock().unwrap().as_slice(),
        ["explain @src/lib.rs"],
        "resolver receives the raw submitted text"
    );

    // And the expansion is what actually reached the model.
    drain_agent_events_until_idle(&mut app).await;
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0], "explain <file:src/lib.rs>CONTENT</file>");
}

#[tokio::test]
async fn conversation_shows_the_raw_mention_while_the_agent_sees_the_expansion() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let spy = Arc::new(ResolverSpy::default());

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    app.submit_user_text("explain @src/lib.rs".to_string());
    drain_agent_events_until_idle(&mut app).await;

    let displayed = app
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .expect("user message should be displayed");
    assert_eq!(
        displayed.content, "explain @src/lib.rs",
        "the transcript keeps the mention readable"
    );
    assert_eq!(
        prompts.lock().unwrap()[0],
        "explain <file:src/lib.rs>CONTENT</file>",
        "the model receives the expansion"
    );
}

#[tokio::test]
async fn prompts_without_mentions_never_reach_the_resolver() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let spy = Arc::new(ResolverSpy::default());

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    app.submit_user_text("no mentions in this one".to_string());
    drain_agent_events_until_idle(&mut app).await;

    assert_eq!(spy.call_count(), 0);
    assert_eq!(prompts.lock().unwrap()[0], "no mentions in this one");
}

#[tokio::test]
async fn a_host_that_registers_no_resolver_sends_text_untouched() {
    let prompts = Arc::new(Mutex::new(Vec::new()));

    let mut app = App::new(TuiConfig::default());
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    app.submit_user_text("explain @src/lib.rs".to_string());
    drain_agent_events_until_idle(&mut app).await;

    assert_eq!(prompts.lock().unwrap()[0], "explain @src/lib.rs");
}

#[tokio::test]
async fn a_declining_resolver_sends_text_untouched() {
    let prompts = Arc::new(Mutex::new(Vec::new()));

    let mut app = App::new(TuiConfig::default())
        .with_extensions(TuiExtensions::new().with_mention_resolver(|_text, _mentions| None));
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    app.submit_user_text("explain @src/lib.rs".to_string());
    drain_agent_events_until_idle(&mut app).await;

    assert_eq!(prompts.lock().unwrap()[0], "explain @src/lib.rs");
}

// ─── completion popup behaviour ───────────────────────────────────────────

#[test]
fn typing_an_at_sign_opens_the_popup_with_every_candidate() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "look at @");

    let completion = app.editor.path_completion.as_ref().expect("popup should open");
    assert_eq!(completion.candidates.len(), 3);
    assert_eq!(completion.selected, 0);
}

#[test]
fn typing_narrows_the_candidate_list() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/m");

    let completion = app
        .path_completion
        .as_ref()
        .expect("popup should stay open");
    assert_eq!(completion.candidates.len(), 1);
    assert_eq!(completion.candidates[0].path, "src/main.rs");
}

#[test]
fn a_query_matching_nothing_closes_the_popup() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@zzz");

    assert!(app.editor.path_completion.is_none());
}

#[test]
fn no_popup_without_a_registered_provider() {
    let mut app = App::new(TuiConfig::default());
    type_text(&mut app, "look at @src/");
    assert!(app.editor.path_completion.is_none());
}

#[test]
fn tab_accepts_the_highlighted_candidate() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "look at @src/m");
    press(&mut app, KeyCode::Tab);

    assert_eq!(app.editor.input.lines(), ["look at @src/main.rs "]);
    assert!(app.editor.path_completion.is_none(), "accepting closes the popup");
}

#[test]
fn enter_accepts_the_candidate_instead_of_submitting() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/m");
    press(&mut app, KeyCode::Enter);

    assert_eq!(app.editor.input.lines(), ["@src/main.rs "]);
    assert!(app.view.messages.is_empty(), "Enter must not have submitted");
    assert_eq!(spy.call_count(), 0, "Enter must not have resolved");
}

#[tokio::test]
async fn enter_submits_normally_once_the_popup_is_closed() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let spy = Arc::new(ResolverSpy::default());

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    type_text(&mut app, "@src/m");
    press(&mut app, KeyCode::Enter); // accepts the candidate
    assert_eq!(spy.call_count(), 0, "accepting is not submitting");

    press(&mut app, KeyCode::Enter); // popup closed, so this submits

    assert!(app.editor.input.is_empty(), "second Enter submits the prompt");
    assert_eq!(spy.call_count(), 1);
    drain_agent_events_until_idle(&mut app).await;
    assert_eq!(
        prompts.lock().unwrap()[0],
        "<file:src/main.rs>CONTENT</file>"
    );
}

#[test]
fn down_and_up_move_the_highlight_and_wrap() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@");
    assert_eq!(app.editor.path_completion.as_ref().unwrap().selected, 0);

    press(&mut app, KeyCode::Down);
    assert_eq!(app.editor.path_completion.as_ref().unwrap().selected, 1);

    press(&mut app, KeyCode::Up);
    assert_eq!(app.editor.path_completion.as_ref().unwrap().selected, 0);

    press(&mut app, KeyCode::Up);
    assert_eq!(
        app.editor.path_completion.as_ref().unwrap().selected,
        2,
        "Up from the first candidate wraps to the last"
    );

    press(&mut app, KeyCode::Down);
    assert_eq!(
        app.editor.path_completion.as_ref().unwrap().selected,
        0,
        "Down from the last candidate wraps to the first"
    );
}

#[test]
fn down_navigates_the_popup_rather_than_input_history() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    // Put something in history first.
    type_text(&mut app, "earlier prompt");
    press(&mut app, KeyCode::Enter);

    type_text(&mut app, "@");
    press(&mut app, KeyCode::Up);

    assert_eq!(
        app.editor.input.lines(),
        ["@"],
        "Up moved the popup highlight, not the input history"
    );
    assert_eq!(app.editor.path_completion.as_ref().unwrap().selected, 2);
}

#[test]
fn escape_dismisses_the_popup_and_leaves_the_text() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/m");
    press(&mut app, KeyCode::Esc);

    assert!(app.editor.path_completion.is_none());
    assert_eq!(app.editor.input.lines(), ["@src/m"]);
}

#[test]
fn accepting_a_candidate_lets_the_user_keep_typing() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/m");
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "and go");

    assert_eq!(app.editor.input.lines(), ["@src/main.rs and go"]);
    assert!(
        app.editor.path_completion.is_none(),
        "the trailing space ends the mention, so the popup stays closed"
    );
}

#[test]
fn a_space_after_a_mention_closes_the_popup() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/m");
    assert!(app.editor.path_completion.is_some());

    type_text(&mut app, " ");
    assert!(app.editor.path_completion.is_none());
}

#[test]
fn backspacing_back_into_a_mention_reopens_the_popup() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/m ");
    assert!(app.editor.path_completion.is_none());

    press(&mut app, KeyCode::Backspace);
    assert!(
        app.editor.path_completion.is_some(),
        "removing the space puts the cursor back inside the mention"
    );
}

#[test]
fn the_highlight_survives_narrowing_the_query() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "@src/");
    press(&mut app, KeyCode::Down); // highlight src/main.rs
    assert_eq!(
        app.editor.path_completion
            .as_ref()
            .unwrap()
            .selected_candidate()
            .unwrap()
            .path,
        "src/main.rs"
    );

    type_text(&mut app, "m"); // narrows to just src/main.rs
    assert_eq!(
        app.editor.path_completion
            .as_ref()
            .unwrap()
            .selected_candidate()
            .unwrap()
            .path,
        "src/main.rs",
        "the highlighted candidate stays highlighted"
    );
}

#[test]
fn submitting_does_not_implicitly_accept_an_open_popup() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    // No agent is set, so submit_input stops after draining the editor — enough
    // to prove the popup does not survive a submit.
    type_text(&mut app, "@src/m");
    app.submit_input();

    assert!(app.editor.path_completion.is_none());
}

#[test]
fn an_email_address_does_not_open_the_popup() {
    let spy = Arc::new(ResolverSpy::default());
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions_with_spy(&spy));

    type_text(&mut app, "mail wes@src");

    assert!(
        app.editor.path_completion.is_none(),
        "an @ inside a word is not a mention"
    );
}
