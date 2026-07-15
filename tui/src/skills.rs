//! Parsing for leading `/skill` invocations.
//!
//! This module knows how to *find* a skill invocation in prompt text. It
//! deliberately knows nothing about which skills exist or what they expand to
//! — discovery, documentation, and expansion all belong to the host (see
//! [`TuiExtensions::with_skill_completions`](crate::TuiExtensions::with_skill_completions),
//! [`TuiExtensions::with_skill_details`](crate::TuiExtensions::with_skill_details)
//! and
//! [`TuiExtensions::with_skill_resolver`](crate::TuiExtensions::with_skill_resolver)).
//!
//! Unlike `@path` mentions, at most **one** invocation exists per prompt and it
//! must be *leading*: a `/` that is the first non-whitespace character of the
//! text, matching the single-leading-sigil model of the command table. A
//! mid-sentence `/` (a path, a fraction, an "either/or") is not an invocation.

/// A `/skill` invocation located at the start of prompt text.
///
/// `start`/`end` are byte offsets into the text the invocation was parsed
/// from, covering the leading `/name` token only — `args` (everything after
/// the name, trimmed) stays in place, so a resolver can splice a replacement
/// for `&text[invocation.start..invocation.end]` without touching the
/// arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillInvocation {
    /// The invoked skill's bare name, without the leading `/`. Interpreted by
    /// the host — the TUI never resolves it against a skill index of its own.
    pub name: String,
    /// Everything after the name token, trimmed. Empty when no arguments were
    /// given.
    pub args: String,
    /// Byte offset of the `/`.
    pub start: usize,
    /// Byte offset one past the end of the name token.
    pub end: usize,
}

/// Find the leading `/skill` invocation in `text`, if there is one.
///
/// # Example
/// ```rust
/// # use swink_agent_tui::parse_skill_invocation;
/// let invocation = parse_skill_invocation("/deploy prod --fast").unwrap();
/// assert_eq!(invocation.name, "deploy");
/// assert_eq!(invocation.args, "prod --fast");
/// assert_eq!(
///     &"/deploy prod --fast"[invocation.start..invocation.end],
///     "/deploy"
/// );
///
/// // A mid-sentence slash is not an invocation.
/// assert!(parse_skill_invocation("either/or").is_none());
/// assert!(parse_skill_invocation("see /etc for details").is_none());
/// ```
#[must_use]
pub fn parse_skill_invocation(text: &str) -> Option<SkillInvocation> {
    let start = text.len() - text.trim_start().len();
    let rest = text[start..].strip_prefix('/')?;

    let name_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..name_len];

    // A bare `/`, or `//foo`, is not an invocation.
    if name.is_empty() || name.starts_with('/') {
        return None;
    }

    Some(SkillInvocation {
        name: name.to_string(),
        args: rest[name_len..].trim().to_string(),
        start,
        end: start + '/'.len_utf8() + name_len,
    })
}

/// Filesystem-backed skill discovery for [`TuiExtensions::with_skill_dirs`]
/// (`skills` feature).
///
/// [`TuiExtensions::with_skill_dirs`]: crate::TuiExtensions::with_skill_dirs
#[cfg(feature = "skills")]
pub(crate) mod discovery {
    use std::path::{Path, PathBuf};

    /// One indexed skill: parsed frontmatter plus the body below it.
    #[derive(Debug, Clone)]
    pub(crate) struct SkillEntry {
        pub(crate) name: String,
        pub(crate) description: Option<String>,
        pub(crate) body: String,
    }

    /// The subset of SKILL.md YAML frontmatter the index cares about. Unknown
    /// keys are ignored rather than treated as malformed.
    #[derive(Debug, Default, serde::Deserialize)]
    struct Frontmatter {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
    }

    /// Eagerly index `<dir>/<name>/SKILL.md` under every given directory.
    ///
    /// Only the explicitly passed directories are read — there are no implicit
    /// default paths. Unreadable directories and malformed SKILL.md files are
    /// skipped, never fatal. The first entry wins on a name collision, in
    /// directory order then lexicographic subdirectory order.
    pub(crate) fn load_index(dirs: impl IntoIterator<Item = PathBuf>) -> Vec<SkillEntry> {
        let mut entries: Vec<SkillEntry> = Vec::new();
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut subdirs: Vec<PathBuf> = read
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            subdirs.sort();
            for subdir in subdirs {
                let Some(entry) = load_entry(&subdir) else {
                    continue;
                };
                if entries.iter().all(|existing| existing.name != entry.name) {
                    entries.push(entry);
                }
            }
        }
        entries
    }

    /// Parse one `<dir>/SKILL.md`. `None` means "skip this entry": no
    /// SKILL.md, an unreadable one, or malformed frontmatter.
    fn load_entry(dir: &Path) -> Option<SkillEntry> {
        let content = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
        let dir_name = dir.file_name()?.to_str()?.to_string();

        let (yaml, body) = split_frontmatter(&content)?;
        let frontmatter: Frontmatter = match yaml {
            Some(yaml) => serde_yaml::from_str(yaml).ok()?,
            None => Frontmatter::default(),
        };

        Some(SkillEntry {
            name: frontmatter.name.unwrap_or(dir_name),
            description: frontmatter.description,
            body: body.trim().to_string(),
        })
    }

    /// Split `---`-fenced YAML frontmatter off the top of `content`.
    ///
    /// Returns `(Some(yaml), body)` when fenced, `(None, content)` when the
    /// file has no opening fence, and `None` (malformed) when an opening fence
    /// is never closed.
    fn split_frontmatter(content: &str) -> Option<(Option<&str>, &str)> {
        let first_line_end = content.find('\n').unwrap_or(content.len());
        if content[..first_line_end].trim_end() != "---" {
            return Some((None, content));
        }

        let after_open = content.get(first_line_end + 1..).unwrap_or("");
        let mut offset = 0;
        for line in after_open.split_inclusive('\n') {
            if line.trim_end() == "---" {
                let yaml = &after_open[..offset];
                let body = &after_open[offset + line.len()..];
                return Some((Some(yaml), body));
            }
            offset += line.len();
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Write `<root>/<name>/SKILL.md` with the given content.
        fn write_skill(root: &Path, name: &str, content: &str) {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), content).unwrap();
        }

        #[test]
        fn indexes_name_description_and_body_from_frontmatter() {
            let temp = tempfile::tempdir().unwrap();
            write_skill(
                temp.path(),
                "deploy-dir",
                "---\nname: deploy\ndescription: Ship it\n---\nRun the deploy runbook.",
            );

            let index = load_index([temp.path().to_path_buf()]);
            assert_eq!(index.len(), 1);
            assert_eq!(index[0].name, "deploy");
            assert_eq!(index[0].description.as_deref(), Some("Ship it"));
            assert_eq!(index[0].body, "Run the deploy runbook.");
        }

        #[test]
        fn missing_name_falls_back_to_the_directory_name() {
            let temp = tempfile::tempdir().unwrap();
            write_skill(
                temp.path(),
                "review",
                "---\ndescription: Review the diff\n---\nbody",
            );

            let index = load_index([temp.path().to_path_buf()]);
            assert_eq!(index.len(), 1);
            assert_eq!(index[0].name, "review");
        }

        #[test]
        fn a_file_without_frontmatter_is_all_body() {
            let temp = tempfile::tempdir().unwrap();
            write_skill(temp.path(), "plain", "Just instructions, no fences.");

            let index = load_index([temp.path().to_path_buf()]);
            assert_eq!(index.len(), 1);
            assert_eq!(index[0].name, "plain");
            assert!(index[0].description.is_none());
            assert_eq!(index[0].body, "Just instructions, no fences.");
        }

        #[test]
        fn malformed_frontmatter_skips_the_entry_not_the_walk() {
            let temp = tempfile::tempdir().unwrap();
            write_skill(temp.path(), "broken", "---\n: not [valid yaml\n---\nbody");
            write_skill(temp.path(), "unclosed", "---\nname: nope\nno closing fence");
            write_skill(temp.path(), "good", "---\nname: good\n---\nbody");

            let index = load_index([temp.path().to_path_buf()]);
            let names: Vec<&str> = index.iter().map(|entry| entry.name.as_str()).collect();
            assert_eq!(names, ["good"]);
        }

        #[test]
        fn a_subdirectory_without_a_skill_md_is_skipped() {
            let temp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(temp.path().join("empty")).unwrap();
            write_skill(temp.path(), "real", "body");

            let index = load_index([temp.path().to_path_buf()]);
            assert_eq!(index.len(), 1);
            assert_eq!(index[0].name, "real");
        }

        #[test]
        fn an_unreadable_dir_is_skipped_without_panicking() {
            let temp = tempfile::tempdir().unwrap();
            write_skill(temp.path(), "real", "body");

            let missing = temp.path().join("does-not-exist");
            let index = load_index([missing, temp.path().to_path_buf()]);
            assert_eq!(index.len(), 1);
        }

        #[test]
        fn first_registration_wins_across_dirs_on_a_name_collision() {
            let first = tempfile::tempdir().unwrap();
            let second = tempfile::tempdir().unwrap();
            write_skill(first.path(), "deploy", "first body");
            write_skill(second.path(), "deploy", "second body");

            let index = load_index([first.path().to_path_buf(), second.path().to_path_buf()]);
            assert_eq!(index.len(), 1);
            assert_eq!(index[0].body, "first body");
        }
    }
}

#[cfg(test)]
mod tests {
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
}
