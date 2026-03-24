# Quickstart: swink-agent-policies

## Add the dependency

```toml
[dependencies]
swink-agent-policies = { path = "../policies" }
# Or cherry-pick features:
# swink-agent-policies = { path = "../policies", default-features = false, features = ["prompt-guard", "audit"] }
```

## Prompt injection protection

```rust
use swink_agent_policies::PromptInjectionGuard;

// Default patterns block common injection phrases
let guard = PromptInjectionGuard::new();

// Add custom pattern
let guard = PromptInjectionGuard::new()
    .with_pattern("custom_override", r"(?i)override\s+safety").unwrap();

// Register in pre_turn (block before LLM) and/or post_turn (block indirect injection via tools)
let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_turn_policy(guard.clone())
        .with_post_turn_policy(guard)
);
```

## PII redaction

```rust
use swink_agent_policies::{PiiRedactor, PiiMode};

// Default: inject-and-redact mode, replaces PII with [REDACTED]
let redactor = PiiRedactor::new();

// Stop mode: block the response entirely when PII detected
let redactor = PiiRedactor::new()
    .with_mode(PiiMode::Stop);

// Custom placeholder
let redactor = PiiRedactor::new()
    .with_placeholder("[REMOVED]");

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_post_turn_policy(redactor)
);
```

## Content filtering

```rust
use swink_agent_policies::ContentFilter;

// Block specific keywords and regex patterns
let filter = ContentFilter::new()
    .with_keyword("competitor-name")
    .with_regex(r"(?i)internal\s+use\s+only").unwrap()
    .with_case_insensitive(true)
    .with_whole_word(true);  // "competitor-name" won't match "competitor-names-list"

// Categorized filtering
let filter = ContentFilter::new()
    .with_category_keyword("compliance", "confidential")
    .with_category_keyword("compliance", "restricted")
    .with_category_keyword("profanity", "badword")
    .with_enabled_categories(["compliance"]);  // Only compliance rules active

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_post_turn_policy(filter)
);
```

## Audit logging

```rust
use swink_agent_policies::{AuditLogger, JsonlAuditSink};

// Log every turn to a JSONL file
let logger = AuditLogger::new(JsonlAuditSink::new("/tmp/agent-audit.jsonl"));

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_post_turn_policy(logger)
);
// After running, /tmp/agent-audit.jsonl contains one JSON record per turn
```

## Composing multiple policies

```rust
use swink_agent_policies::{
    PromptInjectionGuard, PiiRedactor, ContentFilter, AuditLogger, JsonlAuditSink,
};

let guard = PromptInjectionGuard::new();
let redactor = PiiRedactor::new();
let filter = ContentFilter::new()
    .with_keyword("secret-project")
    .with_whole_word(true);
let logger = AuditLogger::new(JsonlAuditSink::new("audit.jsonl"));

// Policies evaluate in registration order:
// 1. PiiRedactor runs first (injects redacted message)
// 2. ContentFilter checks the redacted message
// 3. AuditLogger records whatever made it through
let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_turn_policy(guard.clone())
        .with_post_turn_policy(guard)     // indirect injection guard
        .with_post_turn_policy(redactor)
        .with_post_turn_policy(filter)
        .with_post_turn_policy(logger)
);
```

## Custom audit sink

```rust
use swink_agent_policies::{AuditSink, AuditRecord, AuditLogger};

struct WebhookSink {
    url: String,
    client: reqwest::Client,
    rt: tokio::runtime::Handle,
}

impl AuditSink for WebhookSink {
    fn write(&self, record: &AuditRecord) {
        // Fire-and-forget — AuditSink is sync, so spawn async work
        let url = self.url.clone();
        let body = serde_json::to_string(record).unwrap_or_default();
        let client = self.client.clone();
        self.rt.spawn(async move {
            let _ = client.post(&url).body(body).send().await;
        });
    }
}

let sink = WebhookSink {
    url: "https://example.com/audit".into(),
    client: reqwest::Client::new(),
    rt: tokio::runtime::Handle::current(),
};
let logger = AuditLogger::new(sink);
```
