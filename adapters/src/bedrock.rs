//! AWS Bedrock adapter.
//!
//! Uses the Bedrock `Converse` API and maps the response into the harness
//! event protocol. v1 emits full-message text/tool events after a signed
//! request completes.

use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::stream::{self, Stream, StreamExt as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{
    AgentContext, AgentMessage, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason, Usage,
};

use crate::convert::extract_tool_schemas;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockRequest {
    messages: Vec<BedrockMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_config: Option<BedrockInferenceConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<BedrockToolConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockInferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolConfig {
    tools: Vec<BedrockTool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockTool {
    tool_spec: BedrockToolSpec,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolSpec {
    name: String,
    description: String,
    input_schema: BedrockInputSchema,
}

#[derive(Debug, Serialize)]
struct BedrockInputSchema {
    json: Value,
}

#[derive(Debug, Serialize)]
struct BedrockMessage {
    role: String,
    content: Vec<BedrockContentBlock>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockContentBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use: Option<BedrockToolUse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_result: Option<BedrockToolResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolUse {
    tool_use_id: String,
    name: String,
    input: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolResult {
    tool_use_id: String,
    content: Vec<BedrockToolResultContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

#[derive(Debug, Serialize)]
struct BedrockToolResultContent {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BedrockResponse {
    output: Option<BedrockOutput>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<BedrockUsage>,
}

#[derive(Debug, Deserialize)]
struct BedrockOutput {
    message: Option<BedrockOutputMessage>,
}

#[derive(Debug, Deserialize)]
struct BedrockOutputMessage {
    #[serde(default)]
    content: Vec<BedrockOutputContentBlock>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BedrockOutputContentBlock {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_use: Option<BedrockToolUse>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct BedrockUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

pub struct BedrockStreamFn {
    base_url: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    client: Client,
}

impl BedrockStreamFn {
    #[must_use]
    pub fn new(
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self {
        let region = region.into();
        Self {
            base_url: format!("https://bedrock-runtime.{region}.amazonaws.com"),
            region,
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token,
            client: Client::new(),
        }
    }

    #[must_use]
    pub fn new_with_base_url(
        base_url: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            region: region.into(),
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token,
            client: Client::new(),
        }
    }
}

impl std::fmt::Debug for BedrockStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockStreamFn")
            .field("base_url", &self.base_url)
            .field("region", &self.region)
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

impl StreamFn for BedrockStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(
            stream::once(async move {
                if cancellation_token.is_cancelled() {
                    return vec![AssistantMessageEvent::error_network(
                        "Bedrock request cancelled",
                    )];
                }

                self.converse(model, context, options)
                    .await
                    .unwrap_or_else(|event| vec![event])
            })
            .flat_map(stream::iter),
        )
    }
}

impl BedrockStreamFn {
    async fn converse(
        &self,
        model: &ModelSpec,
        context: &AgentContext,
        options: &StreamOptions,
    ) -> Result<Vec<AssistantMessageEvent>, AssistantMessageEvent> {
        let body = build_request(context, options);
        let body_json = serde_json::to_vec(&body)
            .map_err(|e| AssistantMessageEvent::error(format!("Bedrock JSON error: {e}")))?;
        let path = format!("/model/{}/converse", model.model_id);
        let url = format!("{}{}", self.base_url, path);
        debug!(%url, model = %model.model_id, "sending Bedrock converse request");

        let (amz_date, date_stamp) = amz_dates();
        let host = reqwest::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(ToString::to_string))
            .unwrap_or_else(|| "bedrock-runtime.amazonaws.com".to_string());
        let payload_hash = sha256_hex(&body_json);
        let canonical_headers = canonical_headers(
            &host,
            &amz_date,
            Some("application/json"),
            Some(&payload_hash),
            self.session_token.as_deref(),
        );
        let signed_headers = signed_headers(self.session_token.is_some());
        let canonical_request =
            format!("POST\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
        let credential_scope = format!("{date_stamp}/{}/bedrock/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        let signing_key = signing_key(
            &self.secret_access_key,
            &date_stamp,
            &self.region,
            "bedrock",
        );
        let signature = hex_encode(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key_id, credential_scope, signed_headers, signature
        );

        let mut request = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("host", host)
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", &payload_hash)
            .header("authorization", authorization)
            .body(body_json);
        if let Some(token) = &self.session_token {
            request = request.header("x-amz-security-token", token);
        }

        let response = request
            .send()
            .await
            .map_err(|e| AssistantMessageEvent::error_network(format!("Bedrock connection error: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Bedrock HTTP error");
            return Err(match code {
                401 | 403 => AssistantMessageEvent::error_auth(format!("Bedrock auth error (HTTP {code}): {body}")),
                429 => AssistantMessageEvent::error_throttled(format!("Bedrock rate limit (HTTP 429): {body}")),
                500..=599 => {
                    AssistantMessageEvent::error_network(format!("Bedrock server error (HTTP {code}): {body}"))
                }
                _ => AssistantMessageEvent::error(format!("Bedrock HTTP {code}: {body}")),
            });
        }

        let body = response
            .text()
            .await
            .map_err(|e| AssistantMessageEvent::error_network(format!("Bedrock response read error: {e}")))?;
        let parsed: BedrockResponse = serde_json::from_str(&body)
            .map_err(|e| AssistantMessageEvent::error(format!("Bedrock JSON parse error: {e}")))?;

        Ok(response_to_events(parsed))
    }
}

fn build_request(context: &AgentContext, options: &StreamOptions) -> BedrockRequest {
    let messages = convert_messages(&context.messages);
    let inference_config = Some(BedrockInferenceConfig {
        temperature: options.temperature,
        max_tokens: options.max_tokens,
    });
    let tools = extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|tool| BedrockTool {
            tool_spec: BedrockToolSpec {
                name: tool.name,
                description: tool.description,
                input_schema: BedrockInputSchema {
                    json: tool.parameters,
                },
            },
        })
        .collect::<Vec<_>>();
    let tool_config = (!tools.is_empty()).then_some(BedrockToolConfig { tools });

    let mut messages = if context.system_prompt.is_empty() {
        messages
    } else {
        let mut messages = Vec::with_capacity(messages.len() + 1);
        messages.push(BedrockMessage {
            role: "user".to_string(),
            content: vec![BedrockContentBlock {
                text: Some(context.system_prompt.clone()),
                ..BedrockContentBlock::default()
            }],
        });
        messages.extend(convert_messages(&context.messages));
        messages
    };
    if messages.is_empty() {
        messages.push(BedrockMessage {
            role: "user".to_string(),
            content: vec![BedrockContentBlock {
                text: Some(String::new()),
                ..BedrockContentBlock::default()
            }],
        });
    }

    BedrockRequest {
        messages,
        inference_config,
        tool_config,
    }
}

fn convert_messages(messages: &[AgentMessage]) -> Vec<BedrockMessage> {
    let mut result = Vec::new();
    for message in messages {
        let AgentMessage::Llm(llm) = message else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => {
                let text = ContentBlock::extract_text(&user.content);
                if !text.is_empty() {
                    result.push(BedrockMessage {
                        role: "user".to_string(),
                        content: vec![BedrockContentBlock {
                            text: Some(text),
                            ..BedrockContentBlock::default()
                        }],
                    });
                }
            }
            LlmMessage::Assistant(assistant) => {
                let mut content = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            content.push(BedrockContentBlock {
                                text: Some(text.clone()),
                                ..BedrockContentBlock::default()
                            });
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => content.push(BedrockContentBlock {
                            tool_use: Some(BedrockToolUse {
                                tool_use_id: id.clone(),
                                name: name.clone(),
                                input: arguments.clone(),
                            }),
                            ..BedrockContentBlock::default()
                        }),
                        _ => {}
                    }
                }
                if !content.is_empty() {
                    result.push(BedrockMessage {
                        role: "assistant".to_string(),
                        content,
                    });
                }
            }
            LlmMessage::ToolResult(tool_result) => {
                result.push(BedrockMessage {
                    role: "user".to_string(),
                    content: vec![BedrockContentBlock {
                        tool_result: Some(BedrockToolResult {
                            tool_use_id: tool_result.tool_call_id.clone(),
                            content: vec![BedrockToolResultContent {
                                text: ContentBlock::extract_text(&tool_result.content),
                            }],
                            status: tool_result.is_error.then_some("error".to_string()),
                        }),
                        ..BedrockContentBlock::default()
                    }],
                });
            }
        }
    }
    result
}

fn response_to_events(response: BedrockResponse) -> Vec<AssistantMessageEvent> {
    let mut events = vec![AssistantMessageEvent::Start];
    let mut content_index = 0usize;

    if let Some(output) = response.output
        && let Some(message) = output.message
    {
        for block in message.content {
            if let Some(text) = block.text {
                events.push(AssistantMessageEvent::TextStart { content_index });
                if !text.is_empty() {
                    events.push(AssistantMessageEvent::TextDelta {
                        content_index,
                        delta: text,
                    });
                }
                events.push(AssistantMessageEvent::TextEnd { content_index });
                content_index += 1;
            } else if let Some(tool_use) = block.tool_use {
                events.push(AssistantMessageEvent::ToolCallStart {
                    content_index,
                    id: tool_use.tool_use_id,
                    name: tool_use.name,
                });
                let arguments = tool_use.input.to_string();
                if !arguments.is_empty() {
                    events.push(AssistantMessageEvent::ToolCallDelta {
                        content_index,
                        delta: arguments,
                    });
                }
                events.push(AssistantMessageEvent::ToolCallEnd { content_index });
                content_index += 1;
            }
        }
    }

    let usage = response.usage.unwrap_or_default();
    events.push(AssistantMessageEvent::Done {
        stop_reason: match response.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::Length,
            _ => StopReason::Stop,
        },
        usage: Usage {
            input: usage.input_tokens,
            output: usage.output_tokens,
            total: if usage.total_tokens == 0 {
                usage.input_tokens + usage.output_tokens
            } else {
                usage.total_tokens
            },
            ..Usage::default()
        },
        cost: Cost::default(),
    });
    events
}

fn amz_dates() -> (String, String) {
    let now = chrono_like_now();
    (now.0, now.1)
}

#[allow(clippy::cast_possible_wrap)]
fn chrono_like_now() -> (String, String) {
    // UTC formatting without extra dependencies.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (year, month, day, hour, minute, second) = unix_to_utc(now);
    (
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
        format!("{year:04}{month:02}{day:02}"),
    )
}

#[allow(
    clippy::missing_const_for_fn,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::bool_to_int_with_if
)]
fn unix_to_utc(timestamp: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = timestamp.div_euclid(86_400);
    let secs_of_day = timestamp.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };

    (year as i32, month as u32, day as u32, hour, minute, second)
}

fn canonical_headers(
    host: &str,
    amz_date: &str,
    content_type: Option<&str>,
    payload_hash: Option<&str>,
    session_token: Option<&str>,
) -> String {
    let mut headers = vec![format!("host:{host}"), format!("x-amz-date:{amz_date}")];
    if let Some(content_type) = content_type {
        headers.push(format!("content-type:{content_type}"));
    }
    if let Some(payload_hash) = payload_hash {
        headers.push(format!("x-amz-content-sha256:{payload_hash}"));
    }
    if let Some(session_token) = session_token {
        headers.push(format!("x-amz-security-token:{session_token}"));
    }
    headers.sort_unstable();
    format!("{}\n", headers.join("\n"))
}

fn signed_headers(with_session_token: bool) -> String {
    let mut headers = vec!["content-type", "host", "x-amz-content-sha256", "x-amz-date"];
    if with_session_token {
        headers.push("x-amz-security-token");
    }
    headers.sort_unstable();
    headers.join(";")
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let block_size = 64usize;
    let mut key_block = [0u8; 64];
    if key.len() > block_size {
        let digest = Sha256::digest(key);
        key_block[..32].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut o_key_pad = [0u8; 64];
    let mut i_key_pad = [0u8; 64];
    for (index, byte) in key_block.iter().enumerate() {
        o_key_pad[index] = byte ^ 0x5c;
        i_key_pad[index] = byte ^ 0x36;
    }

    let mut inner = Sha256::new();
    inner.update(i_key_pad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(o_key_pad);
    outer.update(inner_hash);
    outer.finalize().into()
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BedrockStreamFn>();
};
