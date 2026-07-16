#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn unsafe_code_is_scoped_to_posix_sandbox_carveout() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = crate_dir.join("src");
    let posix_path = src_dir.join("evaluators/code/sandbox/posix.rs");

    let lib_rs = fs::read_to_string(src_dir.join("lib.rs")).expect("read lib.rs");
    assert!(lib_rs.contains("#![deny(unsafe_code)]"));
    assert!(lib_rs.contains("forbid` cannot be relaxed by a nested `allow"));

    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml")).expect("read Cargo.toml");
    assert!(manifest.contains("unsafe_code = \"deny\""));

    let posix_rs = fs::read_to_string(&posix_path).expect("read posix sandbox");
    assert!(posix_rs.contains("#![allow(unsafe_code)]"));
    assert_safety_comments_cover_unsafe_blocks(&posix_rs);

    let mut violations = Vec::new();
    for path in rust_files(&src_dir) {
        if path == posix_path {
            continue;
        }
        let contents = fs::read_to_string(&path).expect("read Rust source");
        if contains_unsafe_surface(&contents) {
            violations.push(path.strip_prefix(&crate_dir).unwrap().display().to_string());
        }
    }

    assert!(
        violations.is_empty(),
        "unsafe code outside sandbox carve-out: {violations:?}"
    );
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

fn contains_unsafe_surface(contents: &str) -> bool {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("//")
                && !line.starts_with("/*")
                && !line.starts_with('*')
        })
        .any(|line| {
            line.contains("#![allow(unsafe_code)]")
                || line.contains("#[allow(unsafe_code)]")
                || line.contains("unsafe {")
                || line.contains("unsafe fn")
                || line.contains("unsafe impl")
                || line.contains("extern \"C\"")
        })
}

fn assert_safety_comments_cover_unsafe_blocks(contents: &str) {
    let lines = contents.lines().collect::<Vec<_>>();
    let unsafe_lines = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim().contains("unsafe {"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    assert!(!unsafe_lines.is_empty(), "expected POSIX FFI unsafe blocks");

    for index in unsafe_lines {
        let start = index.saturating_sub(8);
        let has_safety_comment = lines[start..index]
            .iter()
            .any(|line| line.trim_start().starts_with("// SAFETY:"));
        assert!(
            has_safety_comment,
            "unsafe block on line {} lacks nearby SAFETY comment",
            index + 1
        );
    }
}
