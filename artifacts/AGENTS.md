# AGENTS.md — swink-agent-artifacts

## Scope

`artifacts/` — Versioned artifact storage for agent sessions. Implements the `ArtifactStore` trait from core (behind the `artifact-store` feature gate).

## Key Facts

- Two backends: `FileArtifactStore` (persistent, local filesystem) and `InMemoryArtifactStore` (testing).
- Artifacts are versioned — each write produces a new version; prior versions are retained.
- `validate_artifact_name` enforces name constraints shared by both backends.
- `FileArtifactStore` layout: versioned files + JSON metadata sidecar per artifact.
- Streaming reads available via the `streaming` module.

## Build & Test

```bash
cargo build -p swink-agent-artifacts
cargo test -p swink-agent-artifacts
cargo clippy -p swink-agent-artifacts -- -D warnings
```
