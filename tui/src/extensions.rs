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
//! Today it carries host-defined slash/hash commands.
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

/// What a host-defined command wants the TUI to do.
///
/// Deliberately narrow: hosts render information, they do not drive the event
/// loop. Further variants can be added as the need is demonstrated.
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

/// Host-supplied extension points, handed to the TUI at construction.
#[derive(Clone, Default)]
pub struct TuiExtensions {
    commands: Vec<(String, CustomCommandFn)>,
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

    /// Names of every registered command, in registration order.
    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|(name, _)| name.as_str())
    }

    /// Whether any extension points are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
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
}

impl std::fmt::Debug for TuiExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiExtensions")
            .field("commands", &self.command_names().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
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
}
