//! Tests for `skills`.
#![cfg(test)]

use super::*;

#[test]
fn plain_text_has_no_invocation() {
    assert!(parse_skill_invocation("just a normal prompt").is_none());
}

#[test]
fn leading_slash_with_name_parses() {
    let invocation = parse_skill_invocation("/deploy").unwrap();
    assert_eq!(invocation.name, "deploy");
    assert_eq!(invocation.args, "");
    assert_eq!(invocation.start, 0);
    assert_eq!(invocation.end, 7);
}

#[test]
fn arguments_are_captured_and_trimmed() {
    let invocation = parse_skill_invocation("/deploy   prod region=us  ").unwrap();
    assert_eq!(invocation.args, "prod region=us");
}

#[test]
fn leading_whitespace_is_allowed_before_the_slash() {
    let invocation = parse_skill_invocation("  /deploy prod").unwrap();
    assert_eq!(invocation.name, "deploy");
    assert_eq!(invocation.start, 2);
    assert_eq!(
        &"  /deploy prod"[invocation.start..invocation.end],
        "/deploy"
    );
}

#[test]
fn a_mid_sentence_slash_is_not_an_invocation() {
    assert!(parse_skill_invocation("look in /usr/bin please").is_none());
    assert!(parse_skill_invocation("either/or").is_none());
}

#[test]
fn a_bare_slash_is_not_an_invocation() {
    assert!(parse_skill_invocation("/").is_none());
    assert!(parse_skill_invocation("/ deploy").is_none());
}

#[test]
fn a_double_slash_is_not_an_invocation() {
    assert!(parse_skill_invocation("//comment").is_none());
}

#[test]
fn a_path_like_name_parses_whole() {
    // The popup will simply have no matching candidate for this, but the
    // parser itself is name-agnostic.
    let invocation = parse_skill_invocation("/usr/bin").unwrap();
    assert_eq!(invocation.name, "usr/bin");
}

#[test]
fn multiline_args_stop_the_name_at_the_first_whitespace() {
    let invocation = parse_skill_invocation("/deploy\nprod").unwrap();
    assert_eq!(invocation.name, "deploy");
    assert_eq!(invocation.args, "prod");
}

#[test]
fn span_survives_multibyte_names() {
    let text = "/café now";
    let invocation = parse_skill_invocation(text).unwrap();
    assert_eq!(&text[invocation.start..invocation.end], "/café");
    assert_eq!(invocation.args, "now");
}
