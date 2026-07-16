//! Host-supplied extension points for an embedded TUI.
//!
//! [`TuiConfig`](crate::TuiConfig) is deserialized from `tui.toml`, so it can
//! only hold data. Anything a host supplies *in code* — closures, trait
//! objects, registries — lives here instead, and reaches the event loop via
//! [`App::with_extensions`](crate::App::with_extensions) or
//! [`launch_with_extensions`](crate::launch_with_extensions).
//!
//! `TuiExtensions` is a consuming builder with a `Default` impl, so new seams
//! can be added as further `with_*` methods without breaking existing callers.
//! Today it carries host-defined slash/hash commands, the `@path` file-mention
//! seam, and the `/skill` discovery seam.
//!
//! # Example
//!
//! ```rust
//! use swink_agent_tui::{CustomCommandOutcome, TuiExtensions};
//!
//! let extensions = TuiExtensions::new().with_command("budget", |app, _args| {
//!     CustomCommandOutcome::Feedback(format!("Spent ${:.4}", app.total_cost))
//! });
//!
//! assert_eq!(extensions.command_names().collect::<Vec<_>>(), ["budget"]);
//! ```

use std::sync::Arc;

use crate::app::App;
use crate::mentions::{PathMention, parse_mentions};
use crate::skills::{SkillInvocation, parse_skill_invocation};

/// What a host-defined command wants the TUI to do.
///
/// Deliberately narrow: hosts render information, they do not drive the event
/// loop. Further variants can be added as the need is demonstrated.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum CustomCommandOutcome {
    /// Show this text in the conversation as a system message.
    Feedback(String),
    /// Decline the command; fall through to the built-in command table.
    NotHandled,
}

/// A host-defined command handler.
///
/// Receives the live [`App`] and the command's argument string (everything
/// after the command name, trimmed; empty when no arguments were given).
pub type CustomCommandFn = Arc<dyn Fn(&App, &str) -> CustomCommandOutcome + Send + Sync>;

/// A project-relative path offered while the user is typing an `@path` mention.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCandidate {
    /// Path text inserted (after an `@`) when the candidate is accepted.
    ///
    /// Conventionally project-relative, but the TUI treats it as opaque: it is
    /// inserted verbatim and handed back to the resolver at submit time.
    pub path: String,
    /// Optional dimmed text shown beside the path (size, kind, match reason).
    pub detail: Option<String>,
}

impl PathCandidate {
    /// Create a candidate with no detail text.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            detail: None,
        }
    }

    /// Attach dimmed secondary text, shown right of the path in the popup.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Supplies `@path` completion candidates as the user types.
///
/// Called with the text between the `@` and the cursor, which is empty
/// immediately after the `@` is typed. Return candidates in the order they
/// should be offered; an empty vec closes the popup.
///
/// This runs on the UI thread on every keystroke inside a mention, so it should
/// be cheap — cache or index rather than walking the tree per call.
pub type PathCompletionFn = Arc<dyn Fn(&str) -> Vec<PathCandidate> + Send + Sync>;

/// A skill offered while the user is typing a leading `/name` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillCandidate {
    /// Bare skill name, inserted after the `/` when the candidate is accepted.
    pub name: String,
    /// Optional one-line summary, shown dimmed beside the name.
    pub description: Option<String>,
}

impl SkillCandidate {
    /// Create a candidate with no description.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
        }
    }

    /// Attach a one-line summary, shown dimmed right of the name in the popup.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Supplies skill completion candidates as the user types a leading `/name`
/// (tier 1 of progressive disclosure).
///
/// Called with the text between the `/` and the cursor, which is empty
/// immediately after the `/` is typed. Return candidates in the order they
/// should be offered; an empty vec closes the popup.
///
/// This runs on the UI thread on every keystroke inside an invocation, so it
/// should be cheap — index at registration rather than walking directories per
/// call.
pub type SkillCompletionFn = Arc<dyn Fn(&str) -> Vec<SkillCandidate> + Send + Sync>;

/// Supplies a skill's full documentation — conventionally its SKILL.md body —
/// when the candidate is *highlighted* in the popup (tier 2).
///
/// Called with the candidate's bare name. The TUI caches the result per popup,
/// so moving the highlight back and forth does not re-invoke the callback.
/// Return `None` when there is nothing beyond the one-line description.
pub type SkillDetailsFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Expands a leading `/name args` invocation into the text actually sent to
/// the agent (tier 3).
///
/// Called **once per submitted prompt that starts with an invocation**, and
/// never while the user is typing. Receives the submitted text and the parsed
/// [`SkillInvocation`] (with byte spans, so a replacement can be spliced), and
/// returns the text to send in place of it — or `None` to send the prompt
/// unchanged.
///
/// The host does the file reading here: the TUI never touches the filesystem,
/// so skill directories and their contents stay entirely on the host's side.
pub type SkillResolverFn = Arc<dyn Fn(&str, &SkillInvocation) -> Option<String> + Send + Sync>;

/// Expands `@path` mentions into the text actually sent to the agent.
///
/// Called **once per submitted prompt that contains at least one mention**, and
/// never while the user is typing. Receives the raw submitted text and every
/// mention parsed from it (with byte spans, so replacements can be spliced),
/// and returns the text to send in place of it — or `None` to send the prompt
/// unchanged.
///
/// The host does the file reading here: the TUI never touches the filesystem,
/// so working-directory and ignore rules stay entirely on the host's side.
pub type MentionResolverFn = Arc<dyn Fn(&str, &[PathMention]) -> Option<String> + Send + Sync>;

/// Host-supplied extension points, handed to the TUI at construction.
#[derive(Clone, Default)]
pub struct TuiExtensions {
    commands: Vec<(String, CustomCommandFn)>,
    path_completions: Option<PathCompletionFn>,
    mention_resolver: Option<MentionResolverFn>,
    skill_completions: Option<SkillCompletionFn>,
    skill_details: Option<SkillDetailsFn>,
    skill_resolver: Option<SkillResolverFn>,
}

impl TuiExtensions {
    /// Create an empty extension set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a host-defined command.
    ///
    /// `name` is the bare command name without its `/` or `#` sigil — register
    /// `"usage"` to handle both `/usage` and `#usage`.
    ///
    /// Host commands are matched **before** the built-in command table, so
    /// registering a built-in's name shadows it. Registering the same name
    /// twice keeps the first registration.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::{CustomCommandOutcome, TuiExtensions};
    /// let extensions = TuiExtensions::new()
    ///     .with_command("whoami", |_app, _args| {
    ///         CustomCommandOutcome::Feedback("you".to_string())
    ///     })
    ///     .with_command("echo", |_app, args| {
    ///         CustomCommandOutcome::Feedback(args.to_string())
    ///     });
    /// assert_eq!(extensions.command_names().count(), 2);
    /// ```
    #[must_use]
    pub fn with_command(
        mut self,
        name: impl Into<String>,
        handler: impl Fn(&App, &str) -> CustomCommandOutcome + Send + Sync + 'static,
    ) -> Self {
        self.commands.push((name.into(), Arc::new(handler)));
        self
    }

    /// Supply `@path` completion candidates as the user types a mention.
    ///
    /// The provider is called with the partial path between the `@` and the
    /// cursor and owns discovery entirely — the TUI does no globbing, no
    /// directory walking, and applies no ignore rules of its own. Returning an
    /// empty vec closes the popup.
    ///
    /// Registering a second provider replaces the first.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::{PathCandidate, TuiExtensions};
    /// let files = ["src/lib.rs", "src/main.rs", "README.md"];
    /// let extensions = TuiExtensions::new().with_path_completions(move |query| {
    ///     files
    ///         .iter()
    ///         .copied()
    ///         .filter(|path| path.contains(query))
    ///         .map(PathCandidate::new)
    ///         .collect()
    /// });
    /// ```
    #[must_use]
    pub fn with_path_completions(
        mut self,
        provider: impl Fn(&str) -> Vec<PathCandidate> + Send + Sync + 'static,
    ) -> Self {
        self.path_completions = Some(Arc::new(provider));
        self
    }

    /// Expand `@path` mentions into file content when a prompt is submitted.
    ///
    /// The resolver runs at submit time only — never on a keystroke — and only
    /// when the submitted text contains at least one mention. It receives the
    /// raw text plus the parsed [`PathMention`]s and returns the text to send
    /// to the agent, or `None` to send the prompt unchanged.
    ///
    /// The host reads the files. The conversation view keeps showing the raw
    /// `@path` text the user typed; only the agent sees the expansion.
    ///
    /// Registering a second resolver replaces the first.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::TuiExtensions;
    /// let extensions = TuiExtensions::new().with_mention_resolver(|text, mentions| {
    ///     let mut out = text.to_string();
    ///     // Splice back-to-front so earlier spans stay valid.
    ///     for mention in mentions.iter().rev() {
    ///         let content = std::fs::read_to_string(&mention.path).ok()?;
    ///         out.replace_range(
    ///             mention.start..mention.end,
    ///             &format!("<file path=\"{}\">\n{content}\n</file>", mention.path),
    ///         );
    ///     }
    ///     Some(out)
    /// });
    /// ```
    #[must_use]
    pub fn with_mention_resolver(
        mut self,
        resolver: impl Fn(&str, &[PathMention]) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.mention_resolver = Some(Arc::new(resolver));
        self
    }

    /// Supply skill completion candidates as the user types a leading `/name`.
    ///
    /// The provider is called with the partial name between the `/` and the
    /// cursor and owns discovery entirely — the TUI does no directory walking
    /// and applies no naming rules of its own. Returning an empty vec closes
    /// the popup.
    ///
    /// Registering a second provider replaces the first.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::{SkillCandidate, TuiExtensions};
    /// let skills = [("deploy", "Ship a release"), ("review", "Review a diff")];
    /// let extensions = TuiExtensions::new().with_skill_completions(move |query| {
    ///     skills
    ///         .iter()
    ///         .filter(|(name, _)| name.starts_with(query))
    ///         .map(|(name, summary)| SkillCandidate::new(*name).with_description(*summary))
    ///         .collect()
    /// });
    /// ```
    #[must_use]
    pub fn with_skill_completions(
        mut self,
        provider: impl Fn(&str) -> Vec<SkillCandidate> + Send + Sync + 'static,
    ) -> Self {
        self.skill_completions = Some(Arc::new(provider));
        self
    }

    /// Supply a skill's full documentation when it is highlighted in the popup.
    ///
    /// This is tier 2 of progressive disclosure: the one-line
    /// [`SkillCandidate::description`] rides along with every candidate, while
    /// the full SKILL.md body is fetched lazily — only for the highlighted
    /// candidate, and cached per popup so arrow-key travel does not re-invoke
    /// the callback.
    ///
    /// Registering a second provider replaces the first.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::TuiExtensions;
    /// let extensions = TuiExtensions::new().with_skill_details(|name| {
    ///     (name == "deploy").then(|| "1. build\n2. test\n3. ship".to_string())
    /// });
    /// ```
    #[must_use]
    pub fn with_skill_details(
        mut self,
        provider: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.skill_details = Some(Arc::new(provider));
        self
    }

    /// Expand a leading `/name args` invocation when a prompt is submitted.
    ///
    /// The resolver runs at submit time only — never on a keystroke — and only
    /// when the submitted text starts with an invocation. It receives the text
    /// plus the parsed [`SkillInvocation`] and returns the text to send to the
    /// agent, or `None` to send the prompt unchanged.
    ///
    /// The host reads the skill files. The conversation view keeps showing the
    /// raw `/name args` the user typed; only the agent sees the expansion.
    ///
    /// Registering a second resolver replaces the first.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::TuiExtensions;
    /// let extensions = TuiExtensions::new().with_skill_resolver(|text, invocation| {
    ///     let body = match invocation.name.as_str() {
    ///         "deploy" => "Follow the deploy runbook.",
    ///         _ => return None,
    ///     };
    ///     let mut out = text.to_string();
    ///     out.replace_range(invocation.start..invocation.end, body);
    ///     Some(out)
    /// });
    /// ```
    #[must_use]
    pub fn with_skill_resolver(
        mut self,
        resolver: impl Fn(&str, &SkillInvocation) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.skill_resolver = Some(Arc::new(resolver));
        self
    }

    /// Register every `<dir>/<name>/SKILL.md` under the given directories as a
    /// discoverable skill, wiring all three skill seams over one eager index.
    ///
    /// Convenience over [`with_skill_completions`](Self::with_skill_completions),
    /// [`with_skill_details`](Self::with_skill_details) and
    /// [`with_skill_resolver`](Self::with_skill_resolver) — hosts with their own
    /// index use those directly. Rules:
    ///
    /// - **Only the directories passed here are read.** There are no implicit
    ///   default paths, and the walk happens once, at registration.
    /// - SKILL.md may start with `---`-fenced YAML frontmatter; `name` and
    ///   `description` are honored, with the directory name as the fallback
    ///   name. Unreadable directories and malformed files are skipped.
    /// - Candidates match by case-insensitive name prefix. At submit, the
    ///   invocation's `/name` token is replaced by the skill body wrapped in a
    ///   `<skill name="...">` block; arguments stay in place.
    ///
    /// # Example
    /// ```rust
    /// # use swink_agent_tui::TuiExtensions;
    /// let dir = tempfile::tempdir().unwrap();
    /// std::fs::create_dir(dir.path().join("deploy")).unwrap();
    /// std::fs::write(
    ///     dir.path().join("deploy/SKILL.md"),
    ///     "---\ndescription: Ship a release\n---\nFollow the runbook.",
    /// )
    /// .unwrap();
    ///
    /// let extensions = TuiExtensions::new().with_skill_dirs([dir.path()]);
    /// assert!(extensions.has_skill_completions());
    /// assert!(extensions.has_skill_details());
    /// assert!(extensions.has_skill_resolver());
    /// ```
    #[cfg(feature = "skills")]
    #[must_use]
    pub fn with_skill_dirs(
        self,
        dirs: impl IntoIterator<Item = impl Into<std::path::PathBuf>>,
    ) -> Self {
        let index = Arc::new(crate::skills::discovery::load_index(
            dirs.into_iter().map(Into::into),
        ));
        let for_completions = Arc::clone(&index);
        let for_details = Arc::clone(&index);
        let for_resolver = index;

        self.with_skill_completions(move |query| {
            let query = query.to_lowercase();
            for_completions
                .iter()
                .filter(|entry| entry.name.to_lowercase().starts_with(&query))
                .map(|entry| {
                    let candidate = SkillCandidate::new(&entry.name);
                    match &entry.description {
                        Some(description) => candidate
                            .with_description(description.lines().next().unwrap_or_default()),
                        None => candidate,
                    }
                })
                .collect()
        })
        .with_skill_details(move |name| {
            for_details
                .iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.body.clone())
        })
        .with_skill_resolver(move |text, invocation| {
            let entry = for_resolver
                .iter()
                .find(|entry| entry.name == invocation.name)?;
            let mut out = text.to_string();
            out.replace_range(
                invocation.start..invocation.end,
                &format!("<skill name=\"{}\">\n{}\n</skill>", entry.name, entry.body),
            );
            Some(out)
        })
    }

    /// Names of every registered command, in registration order.
    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|(name, _)| name.as_str())
    }

    /// Whether a `@path` completion provider is registered.
    #[must_use]
    pub const fn has_path_completions(&self) -> bool {
        self.path_completions.is_some()
    }

    /// Whether a `@path` mention resolver is registered.
    #[must_use]
    pub const fn has_mention_resolver(&self) -> bool {
        self.mention_resolver.is_some()
    }

    /// Whether a skill completion provider is registered.
    #[must_use]
    pub const fn has_skill_completions(&self) -> bool {
        self.skill_completions.is_some()
    }

    /// Whether a skill details provider is registered.
    #[must_use]
    pub const fn has_skill_details(&self) -> bool {
        self.skill_details.is_some()
    }

    /// Whether a skill invocation resolver is registered.
    #[must_use]
    pub const fn has_skill_resolver(&self) -> bool {
        self.skill_resolver.is_some()
    }

    /// Whether any extension points are registered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.path_completions.is_none()
            && self.mention_resolver.is_none()
            && self.skill_completions.is_none()
            && self.skill_details.is_none()
            && self.skill_resolver.is_none()
    }

    /// Look up and run the first handler registered under `name`.
    ///
    /// Returns `None` when no handler is registered, or when the registered
    /// handler declined with [`CustomCommandOutcome::NotHandled`] — both mean
    /// "fall through to the built-ins".
    pub(crate) fn dispatch(&self, app: &App, name: &str, args: &str) -> Option<String> {
        let (_, handler) = self.commands.iter().find(|(key, _)| key == name)?;
        match handler(app, args) {
            CustomCommandOutcome::Feedback(text) => Some(text),
            CustomCommandOutcome::NotHandled => None,
        }
    }

    /// Ask the host for candidates matching a partial mention.
    ///
    /// Returns empty when no provider is registered, so the popup stays closed
    /// for hosts that never opted in.
    pub(crate) fn complete_path(&self, query: &str) -> Vec<PathCandidate> {
        self.path_completions
            .as_ref()
            .map(|provider| provider(query))
            .unwrap_or_default()
    }

    /// Expand mentions in a submitted prompt.
    ///
    /// Returns `None` — meaning "send `text` unchanged" — when no resolver is
    /// registered, when the text holds no mentions, or when the resolver itself
    /// declines. The resolver is not called in the first two cases, so a prompt
    /// without mentions costs nothing.
    pub(crate) fn resolve_mentions(&self, text: &str) -> Option<String> {
        let resolver = self.mention_resolver.as_ref()?;
        let mentions = parse_mentions(text);
        if mentions.is_empty() {
            return None;
        }
        resolver(text, &mentions)
    }

    /// Ask the host for skill candidates matching a partial `/name`.
    ///
    /// Returns empty when no provider is registered, so the popup stays closed
    /// for hosts that never opted in.
    pub(crate) fn complete_skills(&self, query: &str) -> Vec<SkillCandidate> {
        self.skill_completions
            .as_ref()
            .map(|provider| provider(query))
            .unwrap_or_default()
    }

    /// Ask the host for a skill's full documentation (tier 2).
    ///
    /// Returns `None` when no provider is registered or the provider has
    /// nothing beyond the candidate's one-line description. Callers cache the
    /// result — see `SkillCompletion` — so this is not hot per keystroke.
    pub(crate) fn skill_details(&self, name: &str) -> Option<String> {
        self.skill_details
            .as_ref()
            .and_then(|provider| provider(name))
    }

    /// Whether `name` is a skill the completion provider recognizes exactly.
    ///
    /// Implemented as a completion query plus an exact-name match, so hosts do
    /// not need to register a fourth callback just to answer membership.
    pub(crate) fn is_known_skill(&self, name: &str) -> bool {
        self.complete_skills(name)
            .iter()
            .any(|candidate| candidate.name == name)
    }

    /// Expand a leading skill invocation in a submitted prompt.
    ///
    /// Returns `None` — meaning "send `text` unchanged" — when no resolver is
    /// registered, when the text does not start with an invocation, or when
    /// the resolver itself declines. The resolver is not called in the first
    /// two cases, so a prompt without an invocation costs nothing.
    pub(crate) fn resolve_skill(&self, text: &str) -> Option<String> {
        let resolver = self.skill_resolver.as_ref()?;
        let invocation = parse_skill_invocation(text)?;
        resolver(text, &invocation)
    }
}

impl std::fmt::Debug for TuiExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiExtensions")
            .field("commands", &self.command_names().collect::<Vec<_>>())
            .field("path_completions", &self.has_path_completions())
            .field("mention_resolver", &self.has_mention_resolver())
            .field("skill_completions", &self.has_skill_completions())
            .field("skill_details", &self.has_skill_details())
            .field("skill_resolver", &self.has_skill_resolver())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::config::TuiConfig;

    fn app() -> App {
        App::new(TuiConfig::default())
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
            extensions.dispatch(&app(), "hi", ""),
            Some("hello".to_string())
        );
    }

    #[test]
    fn handler_receives_argument_string() {
        let extensions = TuiExtensions::new().with_command("echo", |_app, args| {
            CustomCommandOutcome::Feedback(format!("got:{args}"))
        });
        assert_eq!(
            extensions.dispatch(&app(), "echo", "a b c"),
            Some("got:a b c".to_string())
        );
    }

    #[test]
    fn handler_can_read_app_state() {
        let extensions = TuiExtensions::new().with_command("cost", |app, _args| {
            CustomCommandOutcome::Feedback(format!("{:.2}", app.total_cost))
        });
        let mut app = app();
        app.total_cost = 1.5;
        assert_eq!(
            extensions.dispatch(&app, "cost", ""),
            Some("1.50".to_string())
        );
    }

    #[test]
    fn not_handled_falls_through() {
        let extensions = TuiExtensions::new()
            .with_command("maybe", |_app, _args| CustomCommandOutcome::NotHandled);
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
            extensions.dispatch(&app(), "dup", ""),
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
}
