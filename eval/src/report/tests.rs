//! Tests for `mod`.
#![cfg(test)]

use super::*;

#[test]
fn reporter_error_from_io() {
    let io_err = std::io::Error::other("boom");
    let err: ReporterError = io_err.into();
    assert!(err.to_string().contains("boom"));
    assert!(matches!(err, ReporterError::Io(_)));
}

#[test]
fn reporter_output_variants_are_constructible() {
    let _stdout = ReporterOutput::Stdout("hello".into());
    let _artifact = ReporterOutput::Artifact {
        path: PathBuf::from("/tmp/out.json"),
        bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
    };
    let _remote = ReporterOutput::Remote {
        backend: "langsmith".into(),
        identifier: "run-1234".into(),
    };
}

#[test]
fn schema_path_constant_points_at_repo_contract() {
    // Sanity: the published schema path stays stable; reporters and
    // their regression tests must reference the same string.
    assert!(JSON_SCHEMA_PATH.ends_with("eval-result.schema.json"));
}
