# AGENTS.md — swink-agent-artifacts

## Scope

`artifacts/` — Versioned artifact storage for agent sessions. Implements the `ArtifactStore` trait from core (behind the `artifact-store` feature gate).

## Key Facts

- Two backends: `FileArtifactStore` (persistent, local filesystem) and `InMemoryArtifactStore` (testing).
- Artifacts are versioned — each write produces a new version; prior versions are retained.
- `validate_artifact_name` enforces name constraints shared by both backends.
- `FileArtifactStore` layout: versioned files + JSON metadata sidecar per artifact.
- Streaming reads available via the `streaming` module.
- `FileArtifactStore::delete` must remove only the exact artifact's direct files, then prune empty parent directories. Recursive directory deletion breaks exact-name semantics for slash-containing artifact IDs like `foo` and `foo/bar`.
- `FileArtifactStore::discover_artifacts` must normalize filesystem separators back to `/` before returning artifact names; Windows paths otherwise fail `validate_artifact_name` for nested IDs.
- All filesystem mutations for one artifact key must share `artifact_lock(session_id, name)`. `delete` needs the same lock path as `save` and `save_stream` or concurrent writers can race directory removal.

## Build & Test

```bash
cargo build -p swink-agent-artifacts
cargo test -p swink-agent-artifacts
cargo clippy -p swink-agent-artifacts -- -D warnings
```
