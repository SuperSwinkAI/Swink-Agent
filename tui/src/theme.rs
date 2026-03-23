//! Color palette and style helpers for the TUI.
//!
//! All color access goes through functions that respect the current [`ColorMode`].
//! In monochrome modes every color resolves to a single value (`White` or `Black`),
//! preserving only modifiers (bold, dim, underline) for semantic differentiation.

use std::sync::atomic::{AtomicU8, Ordering};

use ratatui::style::{Color, Modifier, Style};

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

static COLOR_MODE: AtomicU8 = AtomicU8::new(0);

/// Set the active color mode (e.g. from config on startup).
pub fn set_color_mode(mode: ColorMode) {
    COLOR_MODE.store(mode as u8, Ordering::Relaxed);
}

/// Read the current color mode.
pub fn color_mode() -> ColorMode {
    match COLOR_MODE.load(Ordering::Relaxed) {
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
mod tests {
    use super::*;

    /// Reset color mode after each test to avoid cross-test pollution.
    fn reset() {
        set_color_mode(ColorMode::Custom);
    }

    #[test]
    fn role_colors_are_distinct() {
        reset();
        let colors = [
            user_color(),
            assistant_color(),
            tool_color(),
            error_color(),
            thinking_color(),
        ];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "role colors at index {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn status_colors_are_distinct() {
        reset();
        let colors = [
            status_idle(),
            status_running(),
            status_error(),
            status_aborted(),
        ];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "status colors at index {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn context_colors_are_distinct() {
        reset();
        let colors = [context_green(), context_yellow(), context_red()];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "context colors at index {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn dim_style_has_dim_modifier() {
        let style = dim();
        assert!(style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn mono_white_returns_white() {
        set_color_mode(ColorMode::MonoWhite);
        assert_eq!(user_color(), Color::White);
        assert_eq!(assistant_color(), Color::White);
        assert_eq!(tool_color(), Color::White);
        assert_eq!(error_color(), Color::White);
        assert_eq!(status_idle(), Color::White);
        assert_eq!(border_color(), Color::White);
        assert_eq!(heading_color(), Color::White);
        reset();
    }

    #[test]
    fn mono_black_returns_black() {
        set_color_mode(ColorMode::MonoBlack);
        assert_eq!(user_color(), Color::Black);
        assert_eq!(assistant_color(), Color::Black);
        assert_eq!(tool_color(), Color::Black);
        assert_eq!(error_color(), Color::Black);
        assert_eq!(status_idle(), Color::Black);
        assert_eq!(border_color(), Color::Black);
        assert_eq!(heading_color(), Color::Black);
        reset();
    }

    #[test]
    fn bar_colors_have_contrast_in_all_modes() {
        for mode in [
            ColorMode::Custom,
            ColorMode::MonoWhite,
            ColorMode::MonoBlack,
        ] {
            set_color_mode(mode);
            assert_ne!(bar_fg(), bar_bg(), "bar_fg == bar_bg in {mode:?}");

        }
        reset();
    }

    #[test]
    fn cycle_color_mode_cycles() {
        reset();
        assert_eq!(color_mode(), ColorMode::Custom);
        assert_eq!(cycle_color_mode(), ColorMode::MonoWhite);
        assert_eq!(cycle_color_mode(), ColorMode::MonoBlack);
        assert_eq!(cycle_color_mode(), ColorMode::Custom);
        reset();
    }

}
