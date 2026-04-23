//! Tool-call simulator and shared state registry (US4, FR-025).

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::judge::{JudgeClient, JudgeError};

/// Default history retention per state bucket.
pub const DEFAULT_HISTORY_CAP: usize = 32;

/// Schema record for the simulator's tool catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    pub name: String,
    /// JSON Schema describing the *response* shape produced for this tool.
    pub response_schema: serde_json::Value,
}

impl ToolSchema {
    #[must_use]
    pub fn new(name: impl Into<String>, response_schema: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            response_schema,
        }
    }
}

/// One recorded tool invocation within a bucket.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool: String,
    pub args: serde_json::Value,
    pub result: serde_json::Value,
    pub timestamp: SystemTime,
}

/// Mutable state shared across tool calls with the same `state_key`.
#[derive(Debug, Clone)]
pub struct StateBucket {
    pub shared_state: serde_json::Value,
    pub history: VecDeque<ToolCallRecord>,
    history_cap: usize,
}

impl StateBucket {
    /// A cap of `0` is promoted to `1`.
    #[must_use]
    pub fn with_capacity(history_cap: usize) -> Self {
        Self {
            shared_state: serde_json::Value::Null,
            history: VecDeque::new(),
            history_cap: history_cap.max(1),
        }
    }

    /// Record a call, evicting the oldest if we exceed the cap.
    pub fn record(&mut self, record: ToolCallRecord) {
        self.history.push_back(record);
        while self.history.len() > self.history_cap {
            self.history.pop_front();
        }
    }

    #[must_use]
    pub const fn history_cap(&self) -> usize {
        self.history_cap
    }
}

/// Registry of [`StateBucket`]s keyed by arbitrary string.
#[derive(Debug)]
pub struct StateRegistry {
    buckets: Mutex<HashMap<String, StateBucket>>,
    history_cap: usize,
}

impl StateRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_history_cap(DEFAULT_HISTORY_CAP)
    }

    #[must_use]
    pub fn with_history_cap(history_cap: usize) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            history_cap: history_cap.max(1),
        }
    }

    /// Run `f` with mutable access to the bucket for `key`, creating if absent.
    pub fn with_bucket<R>(&self, key: &str, f: impl FnOnce(&mut StateBucket) -> R) -> R {
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = buckets
            .entry(key.to_string())
            .or_insert_with(|| StateBucket::with_capacity(self.history_cap));
        f(bucket)
    }

    #[must_use]
    pub fn history_snapshot(&self, key: &str) -> Vec<ToolCallRecord> {
        let buckets = self
            .buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        buckets
            .get(key)
            .map(|bucket| bucket.history.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    #[must_use]
    pub const fn history_cap(&self) -> usize {
        self.history_cap
    }
}

impl Default for StateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Simulates tool responses by consulting a [`JudgeClient`] and validating
/// the result against the registered [`ToolSchema::response_schema`].
pub struct ToolSimulator {
    tools: HashMap<String, ToolSchema>,
    judge: Arc<dyn JudgeClient>,
    model_id: String,
    registry: Arc<StateRegistry>,
}

impl ToolSimulator {
    #[must_use]
    pub fn new(
        tools: Vec<ToolSchema>,
        judge: Arc<dyn JudgeClient>,
        model_id: impl Into<String>,
    ) -> Self {
        Self::with_registry(tools, judge, model_id, Arc::new(StateRegistry::new()))
    }

    #[must_use]
    pub fn with_registry(
        tools: Vec<ToolSchema>,
        judge: Arc<dyn JudgeClient>,
        model_id: impl Into<String>,
        registry: Arc<StateRegistry>,
    ) -> Self {
        let tools = tools
            .into_iter()
            .map(|schema| (schema.name.clone(), schema))
            .collect();
        Self {
            tools,
            judge,
            model_id: model_id.into(),
            registry,
        }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<StateRegistry> {
        &self.registry
    }

    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Simulate one tool invocation, record it in the `state_key` bucket,
    /// and return the schema-validated result body.
    pub async fn invoke(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        state_key: &str,
    ) -> Result<serde_json::Value, ToolSimulationError> {
        let schema = self
            .tools
            .get(tool_name)
            .ok_or_else(|| ToolSimulationError::UnknownTool(tool_name.to_string()))?;

        let history = self.registry.history_snapshot(state_key);
        let prompt = render_tool_prompt(tool_name, args, &history);
        let verdict = self
            .judge
            .judge(&prompt)
            .await
            .map_err(ToolSimulationError::Judge)?;
        let body = verdict
            .reason
            .ok_or_else(|| ToolSimulationError::MissingBody(tool_name.to_string()))?;
        let value: serde_json::Value = serde_json::from_str(body.trim())
            .map_err(|err| ToolSimulationError::Parse(err.to_string()))?;

        validate_against_schema(&value, &schema.response_schema)?;

        self.registry.with_bucket(state_key, |bucket| {
            bucket.record(ToolCallRecord {
                tool: tool_name.to_string(),
                args: args.clone(),
                result: value.clone(),
                timestamp: SystemTime::now(),
            });
        });

        Ok(value)
    }
}

fn render_tool_prompt(tool: &str, args: &serde_json::Value, history: &[ToolCallRecord]) -> String {
    let mut prompt = format!("Simulate a response for tool `{tool}`.\nArguments: {args}\n");
    if !history.is_empty() {
        prompt.push_str("Prior calls in bucket:\n");
        for (idx, record) in history.iter().enumerate() {
            prompt.push_str(&format!(
                "- [{idx}] {} args={} -> {}\n",
                record.tool, record.args, record.result
            ));
        }
    }
    prompt.push_str("Respond with a single JSON object matching the tool's response schema.");
    prompt
}

fn validate_against_schema(
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), ToolSimulationError> {
    let compiled = jsonschema::validator_for(schema)
        .map_err(|err| ToolSimulationError::SchemaValidation(err.to_string()))?;
    if let Err(err) = compiled.validate(value) {
        return Err(ToolSimulationError::SchemaValidation(err.to_string()));
    }
    Ok(())
}

/// Errors surfaced by [`ToolSimulator::invoke`].
#[derive(Debug, thiserror::Error)]
pub enum ToolSimulationError {
    #[error("tool `{0}` not registered with simulator")]
    UnknownTool(String),
    #[error("judge produced no body for tool `{0}`")]
    MissingBody(String),
    #[error("schema validation failed: {0}")]
    SchemaValidation(String),
    #[error("tool response parse error: {0}")]
    Parse(String),
    #[error("judge error: {0}")]
    Judge(#[source] JudgeError),
}
