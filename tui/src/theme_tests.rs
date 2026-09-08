//! Tests for `theme`.
#![cfg(test)]

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

/// Regression test for #1107.
///
/// Asserts the property that *discriminates* the two backing stores rather
/// than merely passing: a thread-local mode set here must be invisible from
/// another thread, whereas a process-wide atomic would leak into it. Rewire
/// `storage` back to a global and this test fails — which is the point. A
/// test that only asserted `mono_white_returns_white` passes would prove
/// nothing, since that test passed ~2 of 6 runs while broken.
#[test]
fn color_mode_does_not_leak_across_threads_in_test_builds() {
    set_color_mode(ColorMode::MonoWhite);

    let seen_on_other_thread = std::thread::spawn(color_mode)
        .join()
        .expect("color mode probe thread should not panic");

    assert_eq!(
        seen_on_other_thread,
        ColorMode::Custom,
        "another thread must not observe this thread's color mode; a shared \
             global would leak it and reintroduce the #1107 race"
    );
    assert_eq!(
        color_mode(),
        ColorMode::MonoWhite,
        "this thread's own color mode must survive the other thread's read"
    );

    reset();
}
