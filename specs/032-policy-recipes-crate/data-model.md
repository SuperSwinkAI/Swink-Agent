# Data Model: Policy Recipes Crate

## Types Defined in This Crate

### PromptInjectionGuard (struct)

Configurable prompt injection detector. Implements `PreTurnPolicy` + `PostTurnPolicy`.

| Field | Type | Description |
|-------|------|-------------|
| `patterns` | `Vec<Regex>` | Compiled regex patterns to match against message content |
| `pattern_names` | `Vec<String>` | Human-readable name for each pattern (for Stop messages) |

Constructors: `new()` (default patterns), `with_pattern(name, regex)`, `without_defaults()`.
Implements: `Send`, `Sync`, `PreTurnPolicy`, `PostTurnPolicy`.

### PiiRedactor (struct)

PII detection and redaction policy. Implements `PostTurnPolicy`.

| Field | Type | Description |
|-------|------|-------------|
| `patterns` | `Vec<PiiPattern>` | Compiled regex patterns with category metadata |
| `mode` | `PiiMode` | Whether to inject redacted message or stop |
| `placeholder` | `String` | Replacement text for PII matches (default: `[REDACTED]`) |

Constructors: `new()` (default patterns, inject mode), `with_mode(PiiMode)`, `with_placeholder(impl Into<String>)`, `with_pattern(name, regex)`.
Implements: `Send`, `Sync`, `PostTurnPolicy`.

### PiiMode (enum)

| Variant | Description |
|---------|-------------|
| `Redact` | Default. Returns Inject with redacted assistant message. |
| `Stop` | Returns Stop with PII type identified. |

### PiiPattern (struct)

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Category name (e.g., "email", "phone", "ssn") |
| `regex` | `Regex` | Compiled regex pattern |

### ContentFilter (struct)

Keyword and regex blocklist policy. Implements `PostTurnPolicy`.

| Field | Type | Description |
|-------|------|-------------|
| `rules` | `Vec<FilterRule>` | Compiled filter rules with metadata |
| `enabled_categories` | `Option<HashSet<String>>` | If Some, only rules in these categories are active. If None, all rules active. |

Constructors: `new()` (empty), `with_keyword(word)`, `with_regex(pattern)`, `with_category_keyword(category, word)`, `with_category_regex(category, pattern)`, `with_case_insensitive(bool)`, `with_whole_word(bool)`, `with_enabled_categories(impl IntoIterator<Item = impl Into<String>>)`.
Returns `Result<Self, ContentFilterError>` from build methods that accept regex patterns.
Implements: `Send`, `Sync`, `PostTurnPolicy`.

### FilterRule (struct)

| Field | Type | Description |
|-------|------|-------------|
| `pattern` | `Regex` | Compiled regex (keywords are converted to regex at construction) |
| `display_name` | `String` | Original keyword or pattern for Stop messages |
| `category` | `Option<String>` | Optional category for toggling |

### ContentFilterError (enum)

| Variant | Description |
|---------|-------------|
| `InvalidRegex { pattern: String, source: regex::Error }` | Regex compilation failed at construction time |

### AuditLogger (struct)

Passive audit logging policy. Implements `PostTurnPolicy`.

| Field | Type | Description |
|-------|------|-------------|
| `sink` | `Arc<dyn AuditSink>` | Pluggable sink for writing audit records |

Constructors: `new(impl AuditSink + 'static)`.
Implements: `Send`, `Sync`, `PostTurnPolicy`.

### AuditSink (trait)

| Method | Signature | Description |
|--------|-----------|-------------|
| `write` | `fn write(&self, record: &AuditRecord)` | Persist or forward an audit record. Errors should be handled internally. |

Bounds: `Send + Sync`.

### AuditRecord (struct, serializable)

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | `String` | ISO 8601 timestamp of the turn completion |
| `turn_index` | `usize` | Zero-based turn index |
| `content_summary` | `String` | Truncated text content of the assistant message |
| `tool_calls` | `Vec<String>` | Names of tools called during this turn |
| `usage` | `AuditUsage` | Token usage for this turn |
| `cost` | `AuditCost` | Cost for this turn |

Derives: `Debug`, `Clone`, `Serialize`.

### AuditUsage (struct, serializable)

| Field | Type | Description |
|-------|------|-------------|
| `input` | `u64` | Input tokens |
| `output` | `u64` | Output tokens |
| `total` | `u64` | Total tokens |

### AuditCost (struct, serializable)

| Field | Type | Description |
|-------|------|-------------|
| `total` | `f64` | Total cost |

### JsonlAuditSink (struct)

Built-in file-based audit sink.

| Field | Type | Description |
|-------|------|-------------|
| `path` | `PathBuf` | File path for JSONL output |

Constructors: `new(impl Into<PathBuf>)`.
Implements: `Send`, `Sync`, `AuditSink`.

## Types Used from swink-agent (not defined here)

- `PreTurnPolicy`, `PostTurnPolicy` — slot traits
- `PolicyContext`, `TurnPolicyContext` — evaluation context
- `PolicyVerdict` — Continue/Stop/Inject
- `AgentMessage`, `LlmMessage`, `UserMessage`, `AssistantMessage`, `ToolResultMessage` — message types
- `ContentBlock` — text extraction via `extract_text()`
- `Usage`, `Cost` — accumulated metrics
- `StopReason` — turn termination reason

## Relationships

```
PromptInjectionGuard ──uses──> Regex (from regex crate)
                     ──reads──> PolicyContext.new_messages (PreTurn)
                     ──reads──> TurnPolicyContext.tool_results (PostTurn)

PiiRedactor ──owns──> Vec<PiiPattern>
            ──reads──> TurnPolicyContext.assistant_message
            ──constructs──> AgentMessage (Inject verdict)

ContentFilter ──owns──> Vec<FilterRule>
              ──reads──> TurnPolicyContext.assistant_message

AuditLogger ──owns──> Arc<dyn AuditSink>
            ──constructs──> AuditRecord
            ──reads──> TurnPolicyContext + PolicyContext

JsonlAuditSink ──implements──> AuditSink
               ──writes──> filesystem (JSONL)
```
