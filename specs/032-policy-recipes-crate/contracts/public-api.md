# Public API Contract: swink-agent-policies

## Re-exports from lib.rs

```rust
// Feature: prompt-guard
pub use prompt_guard::PromptInjectionGuard;

// Feature: pii
pub use pii_redactor::{PiiRedactor, PiiMode, PiiPattern};

// Feature: content-filter
pub use content_filter::{ContentFilter, ContentFilterError, FilterRule};

// Feature: audit
pub use audit_logger::{AuditLogger, AuditSink, AuditRecord, AuditUsage, AuditCost, JsonlAuditSink};
```

## PromptInjectionGuard

```rust
pub struct PromptInjectionGuard { /* private */ }

impl PromptInjectionGuard {
    /// Create with default injection patterns (~10 common patterns).
    pub fn new() -> Self;

    /// Add a custom pattern alongside defaults.
    pub fn with_pattern(self, name: impl Into<String>, pattern: &str) -> Result<Self, regex::Error>;

    /// Start with no default patterns (custom-only).
    pub fn without_defaults() -> Self;
}

impl PreTurnPolicy for PromptInjectionGuard {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict;
}

impl PostTurnPolicy for PromptInjectionGuard {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict;
}
```

## PiiRedactor

```rust
pub struct PiiRedactor { /* private */ }

pub enum PiiMode {
    Redact,
    Stop,
}

pub struct PiiPattern {
    pub name: String,
    pub regex: Regex,
}

impl PiiRedactor {
    /// Create with default US PII patterns (email, phone, SSN, credit card, IPv4).
    pub fn new() -> Self;

    /// Set redaction mode.
    pub fn with_mode(self, mode: PiiMode) -> Self;

    /// Set custom placeholder (default: "[REDACTED]").
    pub fn with_placeholder(self, placeholder: impl Into<String>) -> Self;

    /// Add a custom PII pattern.
    pub fn with_pattern(self, name: impl Into<String>, pattern: &str) -> Result<Self, regex::Error>;
}

impl PostTurnPolicy for PiiRedactor {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict;
}
```

## ContentFilter

```rust
pub struct ContentFilter { /* private */ }

pub struct FilterRule {
    pub pattern: Regex,
    pub display_name: String,
    pub category: Option<String>,
}

pub enum ContentFilterError {
    InvalidRegex { pattern: String, source: regex::Error },
}

impl ContentFilter {
    /// Create empty filter (no rules).
    pub fn new() -> Self;

    /// Add a keyword to block (exact or whole-word match depending on config).
    pub fn with_keyword(self, word: impl Into<String>) -> Self;

    /// Add a regex pattern to block.
    pub fn with_regex(self, pattern: &str) -> Result<Self, ContentFilterError>;

    /// Add a keyword in a specific category.
    pub fn with_category_keyword(self, category: impl Into<String>, word: impl Into<String>) -> Self;

    /// Add a regex in a specific category.
    pub fn with_category_regex(self, category: impl Into<String>, pattern: &str) -> Result<Self, ContentFilterError>;

    /// Enable case-insensitive matching (default: true).
    pub fn with_case_insensitive(self, enabled: bool) -> Self;

    /// Enable whole-word-only matching (default: false).
    pub fn with_whole_word(self, enabled: bool) -> Self;

    /// Restrict active rules to these categories only.
    pub fn with_enabled_categories(self, categories: impl IntoIterator<Item = impl Into<String>>) -> Self;
}

impl PostTurnPolicy for ContentFilter {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict;
}
```

## AuditLogger

```rust
pub struct AuditLogger { /* private */ }

pub trait AuditSink: Send + Sync {
    fn write(&self, record: &AuditRecord);
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    pub timestamp: String,
    pub turn_index: usize,
    pub content_summary: String,
    pub tool_calls: Vec<String>,
    pub usage: AuditUsage,
    pub cost: AuditCost,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditUsage {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditCost {
    pub total: f64,
}

pub struct JsonlAuditSink { /* private */ }

impl JsonlAuditSink {
    pub fn new(path: impl Into<PathBuf>) -> Self;
}

impl AuditSink for JsonlAuditSink {
    fn write(&self, record: &AuditRecord);
}

impl AuditLogger {
    pub fn new(sink: impl AuditSink + 'static) -> Self;
}

impl PostTurnPolicy for AuditLogger {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict;
}
```

## Cargo.toml Feature Gates

```toml
[features]
default = ["all"]
all = ["prompt-guard", "pii", "content-filter", "audit"]
prompt-guard = ["regex"]
pii = ["regex"]
content-filter = ["regex"]
audit = ["chrono", "serde", "serde_json"]

[dependencies]
swink-agent = { path = ".." }
regex = { workspace = true, optional = true }
chrono = { workspace = true, optional = true }
serde = { workspace = true, optional = true }
serde_json = { workspace = true, optional = true }
tracing = { workspace = true }
```
