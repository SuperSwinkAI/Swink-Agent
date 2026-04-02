# Data Model: Artifact Service

**Feature**: 036-artifact-service | **Date**: 2026-04-02

## Entities

### ArtifactData

Content payload for a single artifact save operation.

| Field | Type | Description |
|-------|------|-------------|
| `content` | `Vec<u8>` | Raw byte content of the artifact |
| `content_type` | `String` | MIME type (e.g., `"text/plain"`, `"image/png"`) |
| `metadata` | `HashMap<String, String>` | Consumer-defined key-value pairs |

**Validation**: None on `ArtifactData` itself — name validation happens at the store level.

### ArtifactVersion

Record describing a specific saved version. Returned by `save` and used for version discovery.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Artifact name (validated) |
| `version` | `u32` | Monotonically increasing per artifact per session, starting at 1 |
| `created_at` | `DateTime<Utc>` | Timestamp when this version was saved |
| `size` | `usize` | Byte size of the content |
| `content_type` | `String` | MIME type of this version's content |

**Derives**: `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`, `Eq`

### ArtifactMeta

Summary metadata for an artifact across all its versions. Used in list results.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Artifact name |
| `latest_version` | `u32` | Highest version number |
| `created_at` | `DateTime<Utc>` | Timestamp of version 1 |
| `updated_at` | `DateTime<Utc>` | Timestamp of the latest version |
| `content_type` | `String` | Content type of the latest version |

**Derives**: `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`, `Eq`

### ArtifactError

Error type for artifact operations.

| Variant | Fields | Description |
|---------|--------|-------------|
| `InvalidName` | `name: String`, `reason: String` | Name validation failure |
| `Storage` | `source: Box<dyn Error + Send + Sync>` | Underlying storage I/O error |
| `NotConfigured` | (none) | Artifact store not set on agent |

**Derives**: `Debug`, `thiserror::Error`

### AgentEvent::ArtifactSaved

New variant on the existing `AgentEvent` enum (behind `artifact-store` feature gate).

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `String` | Session the artifact belongs to |
| `name` | `String` | Artifact name |
| `version` | `u32` | Version number that was saved |

## Relationships

```text
ArtifactStore (trait)
  ├── save(session_id, name, data: ArtifactData) → Result<ArtifactVersion>
  ├── load(session_id, name) → Result<Option<(ArtifactData, ArtifactVersion)>>
  ├── load_version(session_id, name, version: u32) → Result<Option<(ArtifactData, ArtifactVersion)>>
  ├── list(session_id) → Result<Vec<ArtifactMeta>>
  └── delete(session_id, name) → Result<()>

StreamingArtifactStore (extension trait)
  ├── save_stream(session_id, name, content_type, metadata, stream) → Result<ArtifactVersion>
  └── load_stream(session_id, name, version?) → Result<Option<impl Stream<Item = Result<Bytes>>>>

FileArtifactStore ──implements──▶ ArtifactStore + StreamingArtifactStore
InMemoryArtifactStore ──implements──▶ ArtifactStore (only)
```

## Filesystem Layout (FileArtifactStore)

```text
{root}/
└── {session_id}/
    └── {artifact_name}/
        ├── meta.json       # Serialized: Vec<ArtifactVersion> + per-version metadata
        ├── v1.bin          # Version 1 raw content
        ├── v2.bin          # Version 2 raw content
        └── ...
```

### meta.json Schema

```json
{
  "versions": [
    {
      "name": "report.md",
      "version": 1,
      "created_at": "2026-04-02T10:30:00Z",
      "size": 2048,
      "content_type": "text/markdown",
      "metadata": { "tool": "report-generator" }
    }
  ]
}
```

## InMemoryArtifactStore Internal Structure

```text
Arc<tokio::sync::Mutex<
  HashMap<String,                              // session_id
    HashMap<String,                            // artifact_name
      Vec<(ArtifactVersion, ArtifactData)>     // versions (index 0 = v1)
    >
  >
>>
```

## State Transitions

Artifacts have a simple lifecycle — no complex state machine:

1. **Created**: First `save` creates version 1
2. **Updated**: Subsequent `save` with same name creates version N+1
3. **Deleted**: `delete` removes all versions (artifact ceases to exist)

No intermediate states (draft, pending, etc.). Artifacts are immediately available after save completes.
