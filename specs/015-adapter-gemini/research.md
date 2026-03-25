# Research: Adapter: Google Gemini

## 1. Gemini Streaming API Protocol

**Decision**: Use the `streamGenerateContent` endpoint with `?alt=sse` query parameter for Server-Sent Events.

**Rationale**: This is Google's documented streaming endpoint for the Generative Language API. The `alt=sse` parameter switches the response format from chunked JSON to SSE, which aligns with the shared `sse_data_lines` parser already in the adapters crate.

**Alternatives considered**:
- Raw chunked JSON (without `alt=sse`): Would require a custom NDJSON parser instead of reusing `sse_data_lines`. More code, less consistency with Anthropic/OpenAI adapters.

## 2. Message Format Conversion

**Decision**: Custom `convert_messages` function (not the shared `MessageConverter` trait) due to Gemini's distinct content model.

**Rationale**: Gemini uses a parts-based content model that differs significantly from OpenAI-style messages:
- System prompt → `systemInstruction` top-level field (not a message)
- User/Assistant messages → `contents` array with `role` + `parts`
- Tool definitions → `functionDeclarations` (not `tools[].function`)
- Tool results → `functionResponse` parts in a `user`-role content (not a separate message type)
- Images → `inlineData` parts with `mimeType` + base64 `data`
- Thinking → `thought: true` flag on text parts with `thoughtSignature`

This mirrors the Anthropic adapter's approach (also custom conversion due to top-level system prompt and thinking blocks).

**Alternatives considered**:
- Shared `MessageConverter` trait: Doesn't accommodate Gemini's function declaration format or the parts-based structure. Would require significant trait changes that affect all adapters.

## 3. Safety Filter Handling

**Decision**: When `finish_reason` is `"SAFETY"`, emit `AssistantMessageEvent::error()` with a descriptive message.

**Rationale**: FR-006 requires safety filter blocks to be surfaced as errors rather than silently dropped. The current implementation maps `"SAFETY"` to the `_ => StopReason::Stop` fallback, which violates the spec. The fix is to check for `"SAFETY"` in `map_finish_reason` (or in `process_chunk`) and emit an error event instead.

**Alternatives considered**:
- New `StopReason::ContentFiltered` variant: Would require changes to the core crate's `StopReason` enum, affecting all adapters and consumers. Disproportionate for a provider-specific behavior.
- Log warning but continue: Violates FR-006 which explicitly requires error surfacing.

## 4. Multi-Candidate Handling

**Decision**: Use only the first candidate via `chunk.candidates.into_iter().next()`.

**Rationale**: Already implemented. Gemini's streaming mode typically returns a single candidate per chunk. The agent loop expects one assistant message per turn — emitting multiple candidates would break the loop contract.

**Alternatives considered**:
- Process all candidates: Would require fundamental changes to the agent loop's single-response-per-turn model. No use case identified.

## 5. Tool Schema Pass-Through

**Decision**: Pass `schema.parameters` (a `serde_json::Value`) directly to Gemini's `functionDeclarations` without validation or stripping.

**Rationale**: Already implemented. Gemini's API validates schemas server-side and returns clear error messages for unsupported features. Client-side stripping would require maintaining a list of supported/unsupported JSON Schema keywords that could drift from the API's actual support.

**Alternatives considered**:
- Client-side schema validation/stripping: Maintenance burden, could silently remove valid keywords as Gemini's support evolves.

## 6. Authentication

**Decision**: API key via `x-goog-api-key` header. Key provided at construction or overridden per-request via `StreamOptions.api_key`.

**Rationale**: Already implemented. Matches Google's documented authentication method for the Generative Language API. The per-request override enables key rotation and multi-tenant usage.

**Alternatives considered**:
- OAuth2 / service account: More complex, not needed for the Generative Language API's public endpoint. Could be added later as a separate auth strategy if needed.

## 7. API Version Support

**Decision**: Configurable `ApiVersion` enum (`V1`, `V1beta`) passed at construction.

**Rationale**: Already implemented. Some Gemini features (e.g., thinking) may only be available on `v1beta`. The version is embedded in the URL path: `/{version}/models/{model}:streamGenerateContent`.

**Alternatives considered**:
- Hardcode `v1beta`: Would prevent users from targeting the stable `v1` endpoint for production use.
