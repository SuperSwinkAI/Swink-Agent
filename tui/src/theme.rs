//! Color palette and style helpers for the TUI.
//!
//! All color access goes through functions that respect the current [`ColorMode`].
//! In monochrome modes every color resolves to a single value (`White` or `Black`),
//! preserving only modifiers (bold, dim, underline) for semantic differentiation.
//!
//! # Test isolation
//!
//! The active mode lives in the [`storage`] module, which is `cfg`-selected: a
//! process-wide `AtomicU8` in production, a thread-local `Cell` under
//! `cfg(test)`. The atomic does not exist in test builds, so parallel tests
//! *cannot* observe each other's color mode even by calling [`set_color_mode`]
//! directly — the isolation is enforced by the compiler rather than by
//! convention, mirroring the `KeychainBackend` seam in [`crate::credentials`].
//!
//! This closes issue #1107, where `mono_white_returns_white` failed ~4 of 6
//! parallel runs. The racers were never only the theme tests: `App::new`
//! applies `config.color_mode` on every construction, so each of the ~100
//! `App::new` call sites across the crate's test tree stomped the global
//! mid-assertion. Serializing the theme tests would not have fixed that; a
//! `COLOR_TEST_LOCK` in `app/tests/input_ui.rs` tried and failed, because a
//! mutex only one participant takes guards nothing.
//!
//! ## Scope of the guarantee
//!
//! `cfg(test)` is set only while compiling *this crate's own* unit tests, which
//! is where every test that touches the color mode lives. It is **not** set for
//! the `tui/tests/*` integration tests or `src/main.rs`'s test module: those
//! link the library compiled normally and share the one atomic. None touch the
//! color mode today. Anything added there that sets it must not assume
//! isolation; prefer keeping such tests in this crate's unit-test tree, where
//! the seam applies automatically.
//!
//! Production is unchanged: reads and writes still go through one process-wide
//! atomic. The thread-local substitution is sound for tests because the mode is
//! written and read on the same thread (`App::new` and rendering both run on
//! the TUI thread); a test that set the mode and then rendered on a *spawned*
//! thread would read the default instead. None do.

use ratatui::style::{Color, Modifier, Style};

/// Backing store for the active [`ColorMode`], as a raw `u8` discriminant.
///
/// Production uses one process-wide atomic. Test builds get a thread-local so
/// that parallel tests are isolated by construction. See the module docs.
#[cfg(not(test))]
mod storage {
    use std::sync::atomic::{AtomicU8, Ordering};

    static COLOR_MODE: AtomicU8 = AtomicU8::new(0);

    pub(super) fn store(mode: u8) {
        COLOR_MODE.store(mode, Ordering::Relaxed);
    }

    pub(super) fn load() -> u8 {
        COLOR_MODE.load(Ordering::Relaxed)
    }
}

/// Thread-local backing store used under `cfg(test)`.
///
/// Deliberately *not* an atomic: `cargo test` runs each test on its own thread,
/// so a thread-local gives every test a private color mode and makes the #1107
/// race structurally impossible.
#[cfg(test)]
mod storage {
    use std::cell::Cell;

    thread_local! {
        static COLOR_MODE: Cell<u8> = const { Cell::new(0) };
    }

    pub(super) fn store(mode: u8) {
        COLOR_MODE.with(|m| m.set(mode));
    }

    pub(super) fn load() -> u8 {
        COLOR_MODE.with(Cell::get)
    }
}

// ---------------------------------------------------------------------------
// Color mode
// ---------------------------------------------------------------------------

/// Three-way color mode cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColorMode {
    /// Original theme colors.
    Custom = 0,
    /// All colors → `Color::White`.
    MonoWhite = 1,
    /// All colors → `Color::Black`.
    MonoBlack = 2,
}

/// Set the active color mode (e.g. from config on startup).
///
/// In test builds this is scoped to the calling thread; see the module docs.
pub fn set_color_mode(mode: ColorMode) {
    storage::store(mode as u8);
}

/// Read the current color mode.
pub fn color_mode() -> ColorMode {
    match storage::load() {
        1 => ColorMode::MonoWhite,
        2 => ColorMode::MonoBlack,
        _ => ColorMode::Custom,
    }
}

/// Cycle `Custom` → `MonoWhite` → `MonoBlack` → `Custom`. Returns the *new* mode.
pub fn cycle_color_mode() -> ColorMode {
    let next = match color_mode() {
        ColorMode::Custom => ColorMode::MonoWhite,
        ColorMode::MonoWhite => ColorMode::MonoBlack,
        ColorMode::MonoBlack => ColorMode::Custom,
    };
    set_color_mode(next);
    next
}

/// Resolve a color through the current mode.
fn resolve(color: Color) -> Color {
    match color_mode() {
        ColorMode::Custom => color,
        ColorMode::MonoWhite => Color::White,
        ColorMode::MonoBlack => Color::Black,
    }
}

// ---------------------------------------------------------------------------
// Role colors
// ---------------------------------------------------------------------------

/// User message color.
pub fn user_color() -> Color {
    resolve(Color::Green)
}

/// Assistant message color.
pub fn assistant_color() -> Color {
    resolve(Color::Cyan)
}

/// Tool result message color.
pub fn tool_color() -> Color {
    resolve(Color::Yellow)
}

/// Error message color.
pub fn error_color() -> Color {
    resolve(Color::Red)
}

/// Thinking/dimmed content color.
pub fn thinking_color() -> Color {
    resolve(Color::DarkGray)
}

// ---------------------------------------------------------------------------
// Status indicator colors
// ---------------------------------------------------------------------------

/// Idle status.
pub fn status_idle() -> Color {
    resolve(Color::Green)
}

/// Running status.
pub fn status_running() -> Color {
    resolve(Color::Yellow)
}

/// Error status.
pub fn status_error() -> Color {
    resolve(Color::Red)
}

/// Aborted status.
pub fn status_aborted() -> Color {
    resolve(Color::Magenta)
}

// ---------------------------------------------------------------------------
// Context window gauge colors
// ---------------------------------------------------------------------------

/// Context gauge — low usage.
pub fn context_green() -> Color {
    resolve(Color::Green)
}

/// Context gauge — medium usage.
pub fn context_yellow() -> Color {
    resolve(Color::Yellow)
}

/// Context gauge — high usage.
pub fn context_red() -> Color {
    resolve(Color::Red)
}

// ---------------------------------------------------------------------------
// Semantic colors (used across UI files)
// ---------------------------------------------------------------------------

/// System messages (Magenta in custom mode).
pub fn system_color() -> Color {
    resolve(Color::Magenta)
}

/// Plan mode accent (Blue in custom mode).
pub fn plan_color() -> Color {
    resolve(Color::Blue)
}

/// Unfocused borders, secondary text (`DarkGray` in custom mode).
pub fn border_color() -> Color {
    resolve(Color::DarkGray)
}

/// Focused borders (White in custom mode).
pub fn border_focused_color() -> Color {
    resolve(Color::White)
}

/// Diff addition / success checkmark (Green in custom mode).
pub fn diff_add_color() -> Color {
    resolve(Color::Green)
}

/// Diff removal / failure cross (Red in custom mode).
pub fn diff_remove_color() -> Color {
    resolve(Color::Red)
}

/// Success indicator (Green in custom mode).
pub fn success_color() -> Color {
    resolve(Color::Green)
}

/// Failure indicator (Red in custom mode).
pub fn failure_color() -> Color {
    resolve(Color::Red)
}

/// Inline code (Yellow in custom mode).
pub fn inline_code_color() -> Color {
    resolve(Color::Yellow)
}

/// Headings (Cyan in custom mode).
pub fn heading_color() -> Color {
    resolve(Color::Cyan)
}

// ---------------------------------------------------------------------------
// Contrast helpers (bypass `resolve()` to guarantee fg ≠ bg)
// ---------------------------------------------------------------------------

/// Status bar background.
pub fn bar_bg() -> Color {
    match color_mode() {
        ColorMode::Custom => Color::DarkGray,
        ColorMode::MonoWhite => Color::Black,
        ColorMode::MonoBlack => Color::White,
    }
}

/// Status bar foreground.
pub fn bar_fg() -> Color {
    match color_mode() {
        ColorMode::Custom | ColorMode::MonoWhite => Color::White,
        ColorMode::MonoBlack => Color::Black,
    }
}

// ---------------------------------------------------------------------------
// Style helpers
// ---------------------------------------------------------------------------

/// Dimmed style (modifier only — unaffected by color mode).
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
