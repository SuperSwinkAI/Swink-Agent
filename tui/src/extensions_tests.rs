//! Tests for `extensions`.
#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::config::TuiConfig;

fn app() -> App {
    App::new(TuiConfig::default())
}

/// Feedback text of a dispatched outcome, or `None` for a fall-through.
fn feedback(outcome: Option<CustomCommandOutcome>) -> Option<String> {
    match outcome {
        Some(CustomCommandOutcome::Feedback(text)) => Some(text),
        _ => None,
    }
}

#[test]
fn empty_extensions_dispatch_nothing() {
    let extensions = TuiExtensions::new();
    assert!(extensions.is_empty());
    assert!(extensions.dispatch(&app(), "usage", "").is_none());
}

#[test]
fn registered_command_dispatches() {
    let extensions = TuiExtensions::new().with_command("hi", |_app, _args| {
        CustomCommandOutcome::Feedback("hello".to_string())
    });
    assert_eq!(
        feedback(extensions.dispatch(&app(), "hi", "")),
        Some("hello".to_string())
    );
}

#[test]
fn handler_receives_argument_string() {
    let extensions = TuiExtensions::new().with_command("echo", |_app, args| {
        CustomCommandOutcome::Feedback(format!("got:{args}"))
    });
    assert_eq!(
        feedback(extensions.dispatch(&app(), "echo", "a b c")),
        Some("got:a b c".to_string())
    );
}

#[test]
fn handler_can_read_app_state() {
    let extensions = TuiExtensions::new().with_command("cost", |app, _args| {
        CustomCommandOutcome::Feedback(format!("{:.2}", app.usage.total_cost))
    });
    let mut app = app();
    app.usage.total_cost = 1.5;
    assert_eq!(
        feedback(extensions.dispatch(&app, "cost", "")),
        Some("1.50".to_string())
    );
}

#[tokio::test]
async fn deferred_outcome_carries_a_runnable_task() {
    let extensions = TuiExtensions::new().with_command("swap", |_app, args| {
        let args = args.to_string();
        CustomCommandOutcome::deferred_with_notice("working…", move || {
            let args = args.clone();
            async move { HostAction::Feedback(format!("done:{args}")) }
        })
    });
    let Some(CustomCommandOutcome::Deferred { notice, task }) =
        extensions.dispatch(&app(), "swap", "sonnet")
    else {
        panic!("expected a deferred outcome");
    };
    assert_eq!(notice.as_deref(), Some("working…"));
    match task.call().await {
        HostAction::Feedback(text) => assert_eq!(text, "done:sonnet"),
        other => panic!("expected feedback, got {other:?}"),
    }
}

#[test]
fn deferred_without_notice_shows_nothing_up_front() {
    let extensions = TuiExtensions::new().with_command("quiet", |_app, _args| {
        CustomCommandOutcome::deferred(|| async { HostAction::Nothing })
    });
    let Some(CustomCommandOutcome::Deferred { notice, .. }) =
        extensions.dispatch(&app(), "quiet", "")
    else {
        panic!("expected a deferred outcome");
    };
    assert!(notice.is_none());
}

#[test]
fn not_handled_falls_through() {
    let extensions =
        TuiExtensions::new().with_command("maybe", |_app, _args| CustomCommandOutcome::NotHandled);
    assert!(extensions.dispatch(&app(), "maybe", "").is_none());
}

#[test]
fn unregistered_name_dispatches_nothing() {
    let extensions = TuiExtensions::new().with_command("known", |_app, _args| {
        CustomCommandOutcome::Feedback(String::new())
    });
    assert!(extensions.dispatch(&app(), "unknown", "").is_none());
}

#[test]
fn duplicate_registration_keeps_the_first() {
    let extensions = TuiExtensions::new()
        .with_command("dup", |_app, _args| {
            CustomCommandOutcome::Feedback("first".to_string())
        })
        .with_command("dup", |_app, _args| {
            CustomCommandOutcome::Feedback("second".to_string())
        });
    assert_eq!(
        feedback(extensions.dispatch(&app(), "dup", "")),
        Some("first".to_string())
    );
}

#[test]
fn debug_lists_command_names() {
    let extensions = TuiExtensions::new().with_command("alpha", |_app, _args| {
        CustomCommandOutcome::Feedback(String::new())
    });
    assert!(format!("{extensions:?}").contains("alpha"));
}

#[test]
fn completion_without_a_provider_is_empty() {
    let extensions = TuiExtensions::new();
    assert!(!extensions.has_path_completions());
    assert!(extensions.complete_path("src").is_empty());
}

#[test]
fn registered_provider_receives_the_partial_query() {
    let extensions = TuiExtensions::new()
        .with_path_completions(|query| vec![PathCandidate::new(format!("saw:{query}"))]);
    assert_eq!(
        extensions.complete_path("src/li"),
        [PathCandidate::new("saw:src/li")]
    );
}

#[test]
fn second_provider_replaces_the_first() {
    let extensions = TuiExtensions::new()
        .with_path_completions(|_| vec![PathCandidate::new("first")])
        .with_path_completions(|_| vec![PathCandidate::new("second")]);
    assert_eq!(extensions.complete_path(""), [PathCandidate::new("second")]);
}

#[test]
fn resolver_without_registration_leaves_text_unchanged() {
    let extensions = TuiExtensions::new();
    assert!(!extensions.has_mention_resolver());
    assert!(extensions.resolve_mentions("read @src/lib.rs").is_none());
}

#[test]
fn resolver_is_not_called_for_text_without_mentions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let extensions = TuiExtensions::new().with_mention_resolver(move |_text, _mentions| {
        seen.fetch_add(1, Ordering::SeqCst);
        Some("resolved".to_string())
    });

    assert!(extensions.resolve_mentions("no mentions here").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn resolver_receives_raw_text_and_parsed_mentions() {
    let extensions = TuiExtensions::new()
        .with_mention_resolver(|text, mentions| Some(format!("{text}|{}", mentions.len())));
    assert_eq!(
        extensions.resolve_mentions("@a.rs and @b.rs"),
        Some("@a.rs and @b.rs|2".to_string())
    );
}

#[test]
fn resolver_returning_none_leaves_text_unchanged() {
    let extensions = TuiExtensions::new().with_mention_resolver(|_text, _mentions| None);
    assert!(extensions.resolve_mentions("read @src/lib.rs").is_none());
}

#[test]
fn second_resolver_replaces_the_first() {
    let extensions = TuiExtensions::new()
        .with_mention_resolver(|_, _| Some("first".to_string()))
        .with_mention_resolver(|_, _| Some("second".to_string()));
    assert_eq!(
        extensions.resolve_mentions("@a.rs"),
        Some("second".to_string())
    );
}

#[test]
fn mention_seams_count_against_is_empty() {
    assert!(TuiExtensions::new().is_empty());
    assert!(
        !TuiExtensions::new()
            .with_path_completions(|_| Vec::new())
            .is_empty()
    );
    assert!(
        !TuiExtensions::new()
            .with_mention_resolver(|_, _| None)
            .is_empty()
    );
}

#[test]
fn debug_reports_which_mention_seams_are_registered() {
    let extensions = TuiExtensions::new().with_path_completions(|_| Vec::new());
    let rendered = format!("{extensions:?}");
    assert!(rendered.contains("path_completions: true"));
    assert!(rendered.contains("mention_resolver: false"));
}

// ─── skill seams (issue #1092) ────────────────────────────────────────

#[test]
fn skill_completion_without_a_provider_is_empty() {
    let extensions = TuiExtensions::new();
    assert!(!extensions.has_skill_completions());
    assert!(extensions.complete_skills("dep").is_empty());
}

#[test]
fn registered_skill_provider_receives_the_partial_query() {
    let extensions = TuiExtensions::new()
        .with_skill_completions(|query| vec![SkillCandidate::new(format!("saw:{query}"))]);
    assert_eq!(
        extensions.complete_skills("dep"),
        [SkillCandidate::new("saw:dep")]
    );
}

#[test]
fn second_skill_provider_replaces_the_first() {
    let extensions = TuiExtensions::new()
        .with_skill_completions(|_| vec![SkillCandidate::new("first")])
        .with_skill_completions(|_| vec![SkillCandidate::new("second")]);
    assert_eq!(
        extensions.complete_skills(""),
        [SkillCandidate::new("second")]
    );
}

#[test]
fn skill_details_without_a_provider_are_none() {
    let extensions = TuiExtensions::new();
    assert!(!extensions.has_skill_details());
    assert!(extensions.skill_details("deploy").is_none());
}

#[test]
fn registered_details_provider_receives_the_name() {
    let extensions =
        TuiExtensions::new().with_skill_details(|name| Some(format!("docs for {name}")));
    assert_eq!(
        extensions.skill_details("deploy").as_deref(),
        Some("docs for deploy")
    );
}

#[test]
fn second_details_provider_replaces_the_first() {
    let extensions = TuiExtensions::new()
        .with_skill_details(|_| Some("first".to_string()))
        .with_skill_details(|_| Some("second".to_string()));
    assert_eq!(extensions.skill_details("x").as_deref(), Some("second"));
}

#[test]
fn is_known_skill_requires_an_exact_name_match() {
    let extensions = TuiExtensions::new().with_skill_completions(|query| {
        ["deploy", "deploy-canary"]
            .into_iter()
            .filter(|name| name.starts_with(query))
            .map(SkillCandidate::new)
            .collect()
    });
    assert!(extensions.is_known_skill("deploy"));
    assert!(extensions.is_known_skill("deploy-canary"));
    assert!(!extensions.is_known_skill("dep"), "a prefix is not a skill");
    assert!(!extensions.is_known_skill("release"));
}

#[test]
fn no_skill_is_known_without_a_completion_provider() {
    assert!(!TuiExtensions::new().is_known_skill("deploy"));
}

#[test]
fn skill_resolver_without_registration_leaves_text_unchanged() {
    let extensions = TuiExtensions::new();
    assert!(!extensions.has_skill_resolver());
    assert!(extensions.resolve_skill("/deploy prod").is_none());
}

#[test]
fn skill_resolver_is_not_called_without_an_invocation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let extensions = TuiExtensions::new().with_skill_resolver(move |_text, _invocation| {
        seen.fetch_add(1, Ordering::SeqCst);
        Some("resolved".to_string())
    });

    assert!(extensions.resolve_skill("no invocation here").is_none());
    assert!(extensions.resolve_skill("mid /slash text").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn skill_resolver_receives_the_text_and_parsed_invocation() {
    let extensions = TuiExtensions::new().with_skill_resolver(|text, invocation| {
        Some(format!("{text}|{}|{}", invocation.name, invocation.args))
    });
    assert_eq!(
        extensions.resolve_skill("/deploy prod"),
        Some("/deploy prod|deploy|prod".to_string())
    );
}

#[test]
fn skill_resolver_returning_none_leaves_text_unchanged() {
    let extensions = TuiExtensions::new().with_skill_resolver(|_, _| None);
    assert!(extensions.resolve_skill("/deploy").is_none());
}

#[test]
fn second_skill_resolver_replaces_the_first() {
    let extensions = TuiExtensions::new()
        .with_skill_resolver(|_, _| Some("first".to_string()))
        .with_skill_resolver(|_, _| Some("second".to_string()));
    assert_eq!(
        extensions.resolve_skill("/deploy"),
        Some("second".to_string())
    );
}

#[test]
fn skill_seams_count_against_is_empty() {
    assert!(TuiExtensions::new().is_empty());
    assert!(
        !TuiExtensions::new()
            .with_skill_completions(|_| Vec::new())
            .is_empty()
    );
    assert!(!TuiExtensions::new().with_skill_details(|_| None).is_empty());
    assert!(
        !TuiExtensions::new()
            .with_skill_resolver(|_, _| None)
            .is_empty()
    );
}

#[test]
fn debug_reports_which_skill_seams_are_registered() {
    let extensions = TuiExtensions::new().with_skill_details(|_| None);
    let rendered = format!("{extensions:?}");
    assert!(rendered.contains("skill_completions: false"));
    assert!(rendered.contains("skill_details: true"));
    assert!(rendered.contains("skill_resolver: false"));
}

#[cfg(feature = "skills")]
#[test]
fn with_skill_dirs_wires_all_three_seams_over_one_index() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("deploy");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: Ship a release\n---\nFollow the runbook.",
    )
    .unwrap();

    let extensions = TuiExtensions::new().with_skill_dirs([dir.path()]);

    let candidates = extensions.complete_skills("dep");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "deploy");
    assert_eq!(candidates[0].description.as_deref(), Some("Ship a release"));

    assert!(extensions.is_known_skill("deploy"));
    assert_eq!(
        extensions.skill_details("deploy").as_deref(),
        Some("Follow the runbook.")
    );
    assert_eq!(
        extensions.resolve_skill("/deploy prod").as_deref(),
        Some("<skill name=\"deploy\">\nFollow the runbook.\n</skill> prod")
    );
}

#[cfg(feature = "skills")]
#[test]
fn with_skill_dirs_resolver_declines_unknown_names() {
    let dir = tempfile::tempdir().unwrap();
    let extensions = TuiExtensions::new().with_skill_dirs([dir.path()]);
    assert!(extensions.resolve_skill("/deploy").is_none());
}
