//! `/skill` discovery and progressive disclosure (issue #1092).
//!
//! Two load-bearing properties:
//! - *when* things run — tier 1 (candidates) fires per keystroke, tier 2
//!   (details) once per highlighted name, tier 3 (the resolver, the seam that
//!   reads skill files) never before submit;
//! - *precedence* — secrets → host commands → skills → built-ins.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::*;
use super::helpers::{
    PromptCapturingStreamFn, ResolverSpy, drain_agent_events_until_idle, make_test_agent,
};
use crate::config::TuiConfig;
use crate::extensions::{CustomCommandOutcome, PathCandidate, SkillCandidate, TuiExtensions};

/// Records every tier-2 details fetch, by skill name.
#[derive(Default)]
struct DetailsSpy {
    calls: Mutex<Vec<String>>,
}

impl DetailsSpy {
    fn calls_for(&self, name: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| *seen == name)
            .count()
    }
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

/// Extensions with two fixed skills, a details spy, and a resolver spy.
fn extensions_with_spies(details: &Arc<DetailsSpy>, resolver: &Arc<ResolverSpy>) -> TuiExtensions {
    let details_spy = Arc::clone(details);
    let resolver_spy = Arc::clone(resolver);
    TuiExtensions::new()
        .with_skill_completions(|query| {
            [("deploy", "Ship a release"), ("review", "Review a diff")]
                .into_iter()
                .filter(|(name, _)| name.starts_with(query))
                .map(|(name, summary)| SkillCandidate::new(name).with_description(summary))
                .collect()
        })
        .with_skill_details(move |name| {
            details_spy.calls.lock().unwrap().push(name.to_string());
            Some(format!("{name} instructions"))
        })
        .with_skill_resolver(move |text, invocation| {
            resolver_spy.calls.fetch_add(1, Ordering::SeqCst);
            resolver_spy.seen.lock().unwrap().push(text.to_string());

            let mut out = text.to_string();
            out.replace_range(
                invocation.start..invocation.end,
                &format!("<skill:{}>BODY</skill>", invocation.name),
            );
            Some(out)
        })
}

fn spied_app() -> (App, Arc<DetailsSpy>, Arc<ResolverSpy>) {
    let details = Arc::new(DetailsSpy::default());
    let resolver = Arc::new(ResolverSpy::default());
    let app =
        App::new(TuiConfig::default()).with_extensions(extensions_with_spies(&details, &resolver));
    (app, details, resolver)
}

// ─── the load-bearing tests ───────────────────────────────────────────────

/// The requirement most likely to regress silently: the skill body must be
/// resolved when the prompt is submitted, and never as the user types.
#[tokio::test]
async fn skill_body_is_read_at_submit_and_never_while_typing() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (mut app, _details, resolver) = spied_app();
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    // Typing the invocation out, character by character. Every keystroke runs
    // a completion refresh (and may fetch tier-2 details — but never tier 3).
    type_text(&mut app, "/deploy");
    assert!(
        app.skill_completion.is_some(),
        "completion popup should be open mid-invocation"
    );
    assert_eq!(
        resolver.call_count(),
        0,
        "resolver must not run while typing — that would read skill files on every keystroke"
    );

    // Arguments close the popup (whitespace ends the token); still no resolve.
    type_text(&mut app, " prod");
    assert!(app.skill_completion.is_none());
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Right);
    assert_eq!(
        resolver.call_count(),
        0,
        "resolver must not run on cursor movement or edits"
    );

    // Submit — the one and only moment the invocation is allowed to resolve.
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        resolver.call_count(),
        1,
        "resolver runs exactly once, at submit"
    );
    assert_eq!(
        resolver.seen.lock().unwrap().as_slice(),
        ["/deploy prod"],
        "resolver receives the raw submitted text"
    );

    // And the expansion is what actually reached the model, while the
    // transcript keeps showing what the user typed.
    drain_agent_events_until_idle(&mut app).await;
    assert_eq!(
        prompts.lock().unwrap().as_slice(),
        ["<skill:deploy>BODY</skill> prod"]
    );
    let displayed = app
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .expect("user message should be displayed");
    assert_eq!(displayed.content, "/deploy prod");
}

/// The tier-2 cache IS the progressive-disclosure claim: details are fetched
/// once per highlighted name, not once per keystroke or arrow key.
#[test]
fn skill_details_are_fetched_once_per_highlight() {
    let (mut app, details, _resolver) = spied_app();

    type_text(&mut app, "/");
    assert_eq!(app.skill_completion.as_ref().unwrap().candidates.len(), 2);
    assert_eq!(details.calls_for("deploy"), 1, "highlight fetches deploy");
    assert_eq!(
        details.calls_for("review"),
        0,
        "unhighlighted stays unfetched"
    );

    press(&mut app, KeyCode::Down);
    assert_eq!(
        details.calls_for("review"),
        1,
        "new highlight fetches review"
    );

    // Arrow-key mashing across already-fetched names re-invokes nothing.
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Up);
    assert_eq!(details.calls_for("deploy"), 1);
    assert_eq!(details.calls_for("review"), 1);

    // Narrowing the query rebuilds the popup but carries the cache.
    type_text(&mut app, "d");
    assert_eq!(app.skill_completion.as_ref().unwrap().candidates.len(), 1);
    assert_eq!(
        details.calls_for("deploy"),
        1,
        "refresh must reuse the cache"
    );

    // The cached details are what the renderer reads.
    assert_eq!(
        app.skill_completion.as_ref().unwrap().selected_details(),
        Some("deploy instructions")
    );
}

// ─── submit dispatch precedence ───────────────────────────────────────────

#[test]
fn an_unknown_slash_command_still_hits_the_unknown_command_fallback() {
    let (mut app, _details, resolver) = spied_app();

    type_text(&mut app, "/nope");
    assert!(
        app.skill_completion.is_none(),
        "no candidates for an unknown name"
    );
    press(&mut app, KeyCode::Enter);

    let feedback = app
        .messages
        .iter()
        .find(|message| message.role == MessageRole::System)
        .expect("unknown command should produce feedback");
    assert!(feedback.content.contains("Unknown command: /nope"));
    assert_eq!(resolver.call_count(), 0);
}

#[test]
fn a_known_skill_bypasses_the_unknown_command_fallback() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/deploy");
    press(&mut app, KeyCode::Esc); // dismiss the popup so Enter submits
    press(&mut app, KeyCode::Enter);

    assert!(
        app.messages
            .iter()
            .any(|message| message.role == MessageRole::User && message.content == "/deploy"),
        "a known skill submits as a prompt"
    );
    assert!(
        !app.messages
            .iter()
            .any(|message| message.content.contains("Unknown command")),
        "a known skill must not fall through to the built-in table"
    );
}

#[test]
fn a_host_command_shadows_a_same_named_skill() {
    let details = Arc::new(DetailsSpy::default());
    let resolver = Arc::new(ResolverSpy::default());
    let extensions = extensions_with_spies(&details, &resolver)
        .with_command("deploy", |_app, _args| {
            CustomCommandOutcome::Feedback("host handled".to_string())
        });
    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);

    type_text(&mut app, "/deploy");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);

    assert!(
        app.messages
            .iter()
            .any(|message| message.role == MessageRole::System
                && message.content == "host handled"),
        "host commands match before skills"
    );
    assert!(
        !app.messages
            .iter()
            .any(|message| message.role == MessageRole::User),
        "the shadowed skill must not also submit as a prompt"
    );
    assert_eq!(resolver.call_count(), 0);
}

#[test]
fn a_hash_sigil_never_routes_to_a_skill() {
    let (mut app, _details, resolver) = spied_app();

    type_text(&mut app, "#deploy");
    press(&mut app, KeyCode::Enter);

    assert!(
        app.messages
            .iter()
            .any(|message| message.content.contains("Unknown command: #deploy")),
        "only the leading-/ form is a skill invocation"
    );
    assert_eq!(resolver.call_count(), 0);
}

// ─── expansion composition ────────────────────────────────────────────────

/// Mentions expand on the raw text FIRST; the skill body is spliced in after,
/// so a body containing `@/etc/passwd` is never mention-scanned.
#[tokio::test]
async fn a_skill_body_is_never_mention_scanned() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let mention_spy = Arc::new(ResolverSpy::default());
    let mention_counter = Arc::clone(&mention_spy);

    let extensions = TuiExtensions::new()
        .with_skill_completions(|_| vec![SkillCandidate::new("deploy")])
        .with_skill_resolver(|text, invocation| {
            let mut out = text.to_string();
            // A hostile SKILL.md body that mentions a sensitive path.
            out.replace_range(invocation.start..invocation.end, "read @/etc/passwd first");
            Some(out)
        })
        .with_mention_resolver(move |text, mentions| {
            mention_counter.calls.fetch_add(1, Ordering::SeqCst);
            mention_counter.seen.lock().unwrap().push(text.to_string());
            let mut out = text.to_string();
            for mention in mentions.iter().rev() {
                out.replace_range(
                    mention.start..mention.end,
                    &format!("<file:{}>CONTENT</file>", mention.path),
                );
            }
            Some(out)
        });

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);
    app.set_agent(make_test_agent(Arc::new(PromptCapturingStreamFn {
        prompts: Arc::clone(&prompts),
    })));

    app.submit_user_text("/deploy @notes.md".to_string());
    drain_agent_events_until_idle(&mut app).await;

    assert_eq!(
        mention_spy.call_count(),
        1,
        "the mention resolver runs once, on the raw text"
    );
    assert_eq!(
        mention_spy.seen.lock().unwrap().as_slice(),
        ["/deploy @notes.md"],
        "the mention resolver must never see the skill body"
    );
    assert_eq!(
        prompts.lock().unwrap().as_slice(),
        ["read @/etc/passwd first <file:notes.md>CONTENT</file>"],
        "the skill body's @path must reach the model verbatim, unexpanded"
    );
}

// ─── completion popup behaviour ───────────────────────────────────────────

#[test]
fn typing_a_leading_slash_opens_the_popup_with_every_candidate() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/");

    let completion = app.skill_completion.as_ref().expect("popup should open");
    assert_eq!(completion.candidates.len(), 2);
    assert_eq!(completion.selected, 0);
    assert_eq!(
        completion.candidates[0].description.as_deref(),
        Some("Ship a release")
    );
}

#[test]
fn typing_narrows_the_candidate_list() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/re");

    let completion = app
        .skill_completion
        .as_ref()
        .expect("popup should stay open");
    assert_eq!(completion.candidates.len(), 1);
    assert_eq!(completion.candidates[0].name, "review");
}

#[test]
fn a_mid_sentence_slash_does_not_open_the_popup() {
    let (mut app, _details, _resolver) = spied_app();
    type_text(&mut app, "see /dep");
    assert!(app.skill_completion.is_none());
}

#[test]
fn a_slash_on_a_second_line_does_not_open_the_popup() {
    let (mut app, _details, _resolver) = spied_app();
    type_text(&mut app, "context");
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut app, "/dep");
    assert!(
        app.skill_completion.is_none(),
        "invocations are leading-first-line only"
    );
}

#[test]
fn no_popup_without_a_registered_provider() {
    let mut app = App::new(TuiConfig::default());
    type_text(&mut app, "/dep");
    assert!(app.skill_completion.is_none());
}

#[test]
fn tab_accepts_the_highlighted_skill() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/re");
    press(&mut app, KeyCode::Tab);

    assert_eq!(app.input.lines(), ["/review "]);
    assert!(app.skill_completion.is_none(), "accepting closes the popup");
}

#[test]
fn enter_accepts_the_skill_instead_of_submitting() {
    let (mut app, _details, resolver) = spied_app();

    type_text(&mut app, "/de");
    press(&mut app, KeyCode::Enter);

    assert_eq!(app.input.lines(), ["/deploy "]);
    assert!(app.messages.is_empty(), "Enter must not have submitted");
    assert_eq!(resolver.call_count(), 0, "Enter must not have resolved");
}

#[test]
fn down_and_up_move_the_highlight_and_wrap() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/");
    assert_eq!(app.skill_completion.as_ref().unwrap().selected, 0);

    press(&mut app, KeyCode::Down);
    assert_eq!(app.skill_completion.as_ref().unwrap().selected, 1);

    press(&mut app, KeyCode::Down);
    assert_eq!(
        app.skill_completion.as_ref().unwrap().selected,
        0,
        "Down from the last candidate wraps to the first"
    );

    press(&mut app, KeyCode::Up);
    assert_eq!(
        app.skill_completion.as_ref().unwrap().selected,
        1,
        "Up from the first candidate wraps to the last"
    );
}

#[test]
fn escape_dismisses_the_popup_and_leaves_the_text() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/dep");
    press(&mut app, KeyCode::Esc);

    assert!(app.skill_completion.is_none());
    assert_eq!(app.input.lines(), ["/dep"]);
}

#[test]
fn submitting_does_not_implicitly_accept_an_open_popup() {
    let (mut app, _details, _resolver) = spied_app();

    type_text(&mut app, "/dep");
    app.submit_input();

    assert!(app.skill_completion.is_none());
}

#[test]
fn at_most_one_popup_is_ever_open() {
    let details = Arc::new(DetailsSpy::default());
    let resolver = Arc::new(ResolverSpy::default());
    let extensions = extensions_with_spies(&details, &resolver)
        .with_path_completions(|_| vec![PathCandidate::new("src/lib.rs")]);

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions.clone());
    type_text(&mut app, "/dep");
    assert!(app.skill_completion.is_some());
    assert!(
        app.path_completion.is_none(),
        "a leading slash token is not a mention"
    );

    let mut app = App::new(TuiConfig::default()).with_extensions(extensions);
    type_text(&mut app, "look at @src");
    assert!(app.path_completion.is_some());
    assert!(
        app.skill_completion.is_none(),
        "a mention is not a skill invocation"
    );
}
