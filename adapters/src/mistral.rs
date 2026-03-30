//! Native Mistral adapter.
//!
//! Mistral's chat completions API is largely OpenAI-compatible but has several
//! important divergences that require request and response normalization:
//!
//! - **Tool call IDs**: Mistral requires exactly 9-char `[a-zA-Z0-9]` IDs
//!   (rejects OpenAI-style `call_*` IDs with HTTP 422).
//! - **`stream_options`**: Mistral rejects the field entirely; usage arrives
//!   automatically in the final chunk.
//! - **`max_completion_tokens`**: Mistral rejects it; must use `max_tokens`.
//! - **`model_length` finish reason**: Mistral-specific, mapped to `Length`.
//! - **Message ordering**: Mistral rejects `user` immediately after `tool`;
//!   a synthetic assistant message must be inserted.
//!
//! This adapter holds [`AdapterBase`] directly (like Azure) and reuses the
//! shared `openai_compat` types for message serialization and SSE parsing.

use std::collections::HashMap;
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, AgentMessage, ModelSpec};

use crate::base::AdapterBase;
use crate::convert;
use crate::openai_compat::{
    OaiConverter, OaiMessage, build_oai_tools, parse_oai_sse_stream,
};

/// Charset for generating Mistral-compatible 9-char tool call IDs.
const MISTRAL_ID_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

// ─── MistralStreamFn ───────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for the Mistral chat completions API.
///
/// Handles Mistral-specific API divergences from the `OpenAI` protocol:
/// tool call ID format, `max_tokens` naming, no `stream_options`,
/// `model_length` finish reason, and message ordering constraints.
pub struct MistralStreamFn {
    base: AdapterBase,
}

impl MistralStreamFn {
    /// Create a new Mistral adapter.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Mistral API base URL (e.g. `https://api.mistral.ai`).
    /// * `api_key` — Mistral API key for Bearer authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url.into().trim_end_matches('/').to_string(), api_key),
        }
    }
}

impl std::fmt::Debug for MistralStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralStreamFn")
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for MistralStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(mistral_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── MistralIdMap ──────────────────────────────────────────────────────────

/// Bidirectional mapping between harness tool-call IDs (`call_*`) and
/// Mistral-compatible 9-char alphanumeric IDs.
///
/// - **Outbound** (request): harness → Mistral (via [`remap_to_mistral`]).
/// - **Inbound** (response): Mistral → harness (via [`remap_to_harness`]).
///   If a Mistral ID is unknown (new tool call from the model), a fresh
///   harness-style ID is generated on the fly.
struct MistralIdMap {
    harness_to_mistral: HashMap<String, String>,
    mistral_to_harness: HashMap<String, String>,
    counter: u32,
}

impl MistralIdMap {
    fn new() -> Self {
        Self {
            harness_to_mistral: HashMap::new(),
            mistral_to_harness: HashMap::new(),
            counter: 0,
        }
    }

    /// Register a harness ID and return the corresponding Mistral 9-char ID.
    fn remap_to_mistral(&mut self, harness_id: &str) -> String {
        if let Some(mid) = self.harness_to_mistral.get(harness_id) {
            return mid.clone();
        }
        let mid = self.generate_mistral_id();
        self.harness_to_mistral
            .insert(harness_id.to_string(), mid.clone());
        self.mistral_to_harness
            .insert(mid.clone(), harness_id.to_string());
        mid
    }

    /// Look up a Mistral ID and return the harness equivalent.
    /// If unknown (new tool call from the model), generate a new harness ID.
    fn remap_to_harness(&mut self, mistral_id: &str) -> String {
        if let Some(hid) = self.mistral_to_harness.get(mistral_id) {
            return hid.clone();
        }
        // New ID from the model — create a harness-style ID.
        let hid = format!("call_{mistral_id}");
        self.mistral_to_harness
            .insert(mistral_id.to_string(), hid.clone());
        self.harness_to_mistral
            .insert(hid.clone(), mistral_id.to_string());
        hid
    }

    /// Generate a 9-char `[a-zA-Z0-9]` ID using a UUID.
    fn generate_mistral_id(&mut self) -> String {
        let uuid = uuid::Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let mut id = String::with_capacity(9);
        for &b in &bytes[..9] {
            id.push(MISTRAL_ID_CHARSET[b as usize % MISTRAL_ID_CHARSET.len()] as char);
        }
        self.counter += 1;
        id
    }
}

// ─── Mistral-specific request type ─────────────────────────────────────────

/// Mistral chat request body. Like `OaiChatRequest` but:
/// - No `stream_options` field (Mistral rejects it).
/// - Uses `max_tokens` (not `max_completion_tokens`).
#[derive(Debug, Serialize)]
struct MistralChatRequest {
    model: String,
    messages: Vec<OaiMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<crate::openai_compat::OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

// ─── Stream implementation ─────────────────────────────────────────────────

fn mistral_stream<'a>(
    mistral: &'a MistralStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        // Build the ID map from existing tool call IDs in context.
        let mut id_map = MistralIdMap::new();

        let response = match send_request(mistral, model, context, options, &mut id_map).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Mistral HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "Mistral");
            return stream::iter(vec![event]).left_stream();
        }

        // Parse SSE then normalize response events.
        let raw_stream = parse_oai_sse_stream(response, cancellation_token, "Mistral");
        normalize_response_stream(raw_stream, id_map).right_stream()
    })
    .flatten()
}

/// Construct and send the HTTP POST request with Mistral-specific normalization.
async fn send_request(
    mistral: &MistralStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
    id_map: &mut MistralIdMap,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/v1/chat/completions", mistral.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Mistral request"
    );

    // Convert messages with Mistral-specific normalization.
    let messages = convert_messages_for_mistral(&context.messages, &context.system_prompt, id_map);

    let (tools, tool_choice) = build_oai_tools(&context.tools);

    let body = MistralChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tools,
        tool_choice,
    };

    let api_key = options.api_key.as_deref().unwrap_or(&mistral.base.api_key);

    mistral
        .base
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            AssistantMessageEvent::error_network(format!("Mistral connection error: {e}"))
        })
}

// ─── Message conversion ────────────────────────────────────────────────────

/// Convert agent messages to OAI format with Mistral-specific normalization:
/// 1. Remap tool call IDs from harness format to 9-char Mistral format.
/// 2. Insert synthetic assistant message between consecutive tool→user sequences.
fn convert_messages_for_mistral(
    messages: &[AgentMessage],
    system_prompt: &str,
    id_map: &mut MistralIdMap,
) -> Vec<OaiMessage> {
    // Start with standard OAI conversion.
    let raw_messages = convert::convert_messages::<OaiConverter>(messages, system_prompt);

    let mut result: Vec<OaiMessage> = Vec::with_capacity(raw_messages.len() + 4);
    let mut prev_was_tool = false;

    for mut msg in raw_messages {
        // Insert synthetic assistant between tool result and user message.
        if prev_was_tool && msg.role == "user" {
            result.push(OaiMessage {
                role: "assistant".to_string(),
                content: Some(String::new()),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Remap tool call IDs in assistant replay messages.
        if msg.role == "assistant"
            && let Some(ref mut tool_calls) = msg.tool_calls
        {
            for tc in tool_calls.iter_mut() {
                tc.id = id_map.remap_to_mistral(&tc.id);
            }
        }

        // Remap tool_call_id in tool result messages.
        if msg.role == "tool"
            && let Some(ref id) = msg.tool_call_id
        {
            msg.tool_call_id = Some(id_map.remap_to_mistral(id));
        }

        prev_was_tool = msg.role == "tool";
        result.push(msg);
    }

    result
}

// ─── Response normalization ────────────────────────────────────────────────

/// Wrap the parsed SSE stream to normalize Mistral-specific response quirks:
/// - Remap tool call IDs from Mistral 9-char format back to harness format.
///
/// Note: `model_length` → `Length` mapping is handled in the shared
/// `process_oai_chunk` parser. `finish_reason: "error"` maps to
/// `StopReason::Stop` (catch-all) which is acceptable — errors from the
/// Mistral side are rare and the stop reason still allows callers to inspect.
fn normalize_response_stream(
    raw: Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>,
    mut id_map: MistralIdMap,
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    raw.map(move |event| match event {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            name,
        } => {
            let harness_id = id_map.remap_to_harness(&id);
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id: harness_id,
                name,
            }
        }
        other => other,
    })
}

// ─── Compile-time assertions ───────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MistralStreamFn>();
};
