# AGENTS.md — swink-agent-artifacts

## Scope

`artifacts/` — Versioned artifact storage. `FileArtifactStore` (persistent) and `InMemoryArtifactStore` (testing).

## Key Invariants

- Each write produces a new version; prior versions retained. `validate_artifact_name` shared by both backends.
- `FileArtifactStore`: versioned files + JSON metadata sidecar. Streaming reads via `streaming` module.
- Missing `vN.bin` in `meta.json` or orphan `vN.bin` without membership = corruption (detect before allocating next version).
- `save`/`save_stream` must remove `vN.bin` if `meta.json` write fails.
- `delete` removes exact artifact files then prunes empty parents — no recursive dir delete (breaks nested IDs like `foo/bar`).
- `discover_artifacts` normalizes filesystem separators to `/` for cross-platform validity.
- All mutations for one artifact key share `artifact_lock(session_id, name)`.
