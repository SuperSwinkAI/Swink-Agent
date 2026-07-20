# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `tui`: F5 hotkey toggles hidden channels (reasoning/thinking content) inline for the whole conversation — including scrollback — instead of the collapsed `[thinking...]` placeholder; reasoning renders dim/italic so it never reads as the assistant's visible reply, and the status bar shows a `HIDDEN` badge while the toggle is on (#1194)
- `TurnEndReason::ReasoningOnly` — structural signal for turns that end with hidden-channel reasoning only (no visible text, no tool call), plus `AssistantMessage::has_visible_content()` / `is_reasoning_only()` helpers; hosts can now tell "answered invisibly" apart from a normal completion (#1195)
- `AgentOptions::with_reasoning_only_nudge` — opt-in one-shot corrective reminder + single retry when a turn ends reasoning-only; a second consecutive reasoning-only turn is accepted as-is (#1195)
- `testing::thinking_only_events` — mock event sequence for a reasoning-only response (#1195)
- Documented guarantee + regression tests that bash-tool subprocesses inherit the host process environment unmodified — orchestrators inject per-agent identity env overlays (`GH_TOKEN`, `GIT_AUTHOR_*`, `GIT_SSH_COMMAND`) and depend on full inheritance; any future env-policy feature must keep inheritance the default (#1197)

## [0.12.1] - 2026-07-19

### Added
- `AgentError::Auth` structured variant (with `AgentError::auth()` constructor); stream-error classification maps `StreamErrorKind::Auth` and auth-shaped provider messages ("unauthorized", "invalid api key", "authentication", "forbidden") to it instead of the generic stream error, and `ContextWindowOverflow` now carries the failing model id on both classification paths (#1176)
- Public context-level token estimators: `estimate_context_tokens(&AgentContext)` and `estimate_tool_schema_tokens(&[Arc<dyn AgentTool>])` — system prompt + tool schemas + messages, matching the loop's internal accounting (#1177)
- `AgentMessage::try_clone` (Custom messages via `clone_box`, falling back to a `SerializedCustomMessage` snapshot) plus `AgentContext::try_clone` (all-or-nothing) and `AgentContext::clone_for_send` (best-effort) (#1178)
- `tui`: built-in `/compact` slash command wired through the new `ControlRequest::Compact` / `ControlResponse::Compacted` transport seam; in-process sessions compact directly, external transports round-trip through the control channel and render the compaction report (#1179)
- `ServingOptionSupport`: `StreamFn` implementations declare which `ServingOptions` fields they honor (`supported_serving_options`, default all); `ServingOptions::unsupported_fields` reports what an adapter will ignore; all nine bundled adapters carry accurate overrides (#1181)
- `tui`: `TuiLauncher` builder — `build()` returns the assembled `App` as an embeddable/testable seam; `launch_with_extensions` / `launch_with_session` now delegate to it (#1182)
- `stream_owned` — adapt any `Arc<dyn StreamFn>` call into a `'static` event stream (spawn + channel) — and `MapOptionsStreamFn`, a supported decorator that rewrites `StreamOptions` per call or short-circuits with synthetic events (`MappedOptions`); delegates `supported_serving_options` to the inner adapter (#1183)
- `rpc`: `context.compact` control method + `AgentClient::compact()`; `tui-remote` maps `ControlRequest::Compact` over JSON-RPC. Additive — protocol stays 1.1, pre-existing servers degrade gracefully via `METHOD_NOT_FOUND` → `TransportError::Unsupported` (#1184)

### Fixed
- `mcp`: flaky `discovery_timeout_skips_hung_server_and_keeps_healthy_tools` test — the healthy server's 500ms discovery budget and 2s elapsed bound were too tight under full-suite load; budgets now only guard genuine hangs (#1160, #1185)

## [0.12.0] - 2026-07-18

### Added
- `rpc`: control-plane methods (protocol 1.1) — `model.list`/`model.set`, `thinking.set`, `approval.get`/`approval.set`, `system_prompt.set`, `agent.reset`, `plan.enter`/`plan.exit` (the server holds the non-serializable saved tool set), `session.snapshot`/`session.restore` — with typed `AgentClient` helpers, `AgentClient::sender()` for out-of-band traffic, and a new `RpcError::BUSY` (-32094) answered to control requests while a turn is in flight (#1161)
- `tui`: `TuiTransport::control` seam — `ControlRequest`/`ControlResponse` plus `TransportError::Unsupported` (default impl, so existing transports keep compiling); with an external transport installed, the App now routes abort, model cycling (lazy `ListModels`), thinking level, approval mode set/query, system prompt, reset, plan mode, and session save/load through the transport instead of silently no-opping; the in-process path is behavior-unchanged (#1162)
- `tui-remote`: `RemoteTransport` implements the control plane over JSON-RPC — prompts and control requests share one ordered bridge queue (deferred `model.set` lands before the next prompt), abort rides an out-of-band `cancel` notification, and session snapshot/restore uses the memory-JSONL wire representation; a pre-1.1 server degrades to turn-I/O-only via `Unsupported` (#1163)
- `patterns`: first integration test suite (17 tests: pipeline sequential/parallel/loop execution, merge strategies, exit conditions, event ordering, failure propagation) (#1137)
- `eval`: `RawSession::otel_spans` constructor for OTel-backed raw sessions (#1144)
- `rpc`: `PeerSender` requests now carry a 10-minute default timeout (`DEFAULT_REQUEST_TIMEOUT`, error code `TIMEOUT = -32095`) so a dead peer can no longer hang a caller (#1141)
- `tui-remote`: new `swink-agent-tui-remote` crate bridges the TUI to a remote agent served by `swink-agentd` — `RemoteTransport` implements `TuiTransport` over `rpc::AgentClient` with live event streaming, and the `swink-tui-remote` binary attaches the stock terminal UI to an agentd Unix socket; turn I/O only, control-plane operations still require the in-process agent (#1157)
- `rpc`: `AgentClient::prompt_text_with(text, on_event)` streams `AgentEvent`s through a callback as they arrive, instead of returning them in a batch at turn end (#1157)
- `macros`: crate is now explicitly scoped to external SDK consumers, with a compile-checked `derived_tool` example exercising `#[tool]` and `#[derive(ToolSchema)]` end-to-end against the real `AgentTool` trait — in-tree builtins intentionally hand-roll the trait (#1149)

### Changed
- `local-llm`: hf-hub 0.5 → 1.0 (`HFClient` + `ProgressHandler`; the removed `api::tokio` API is gone) — drops the workspace's second reqwest stack (single reqwest 0.13 in the lockfile); cached model loads now revalidate via If-None-Match when online and fall back to the local cache when offline (#1159)
- release pipeline: `swink-agent-rpc` and `swink-agent-tui-remote` added to the crates.io publish order and dry-run gate — first publish of both crates (#1164)
- **tui**: `App` state is now grouped into cohesive sub-structs — `view`, `editor`, `agent_io`, `mode`, `session`, `usage` — replacing ~50 flat fields; embedders must update field paths (e.g. `app.total_cost` → `app.usage.total_cost`), names and visibilities are unchanged (#1151)
- **tui**: `App` consumes agent events through the `TuiTransport` seam — `App::with_transport` routes turn I/O through a host-supplied transport, `App::pump_transport_events` drives an `App` without a terminal; the default in-process path is behavior-unchanged (#1151)
- **mcp**: public API no longer exposes `rmcp` types — `McpConnection::discovered_tools` is now `Vec<McpToolInfo>` (new owned type), `call_tool` returns `AgentToolResult`, `McpTool::new` takes `&McpToolInfo`, the `convert` module is private, and `from_service` takes an opaque `McpServiceHandle` (`McpServiceHandle::from_rmcp` is the single documented rmcp seam); rmcp major bumps no longer force a semver-major on `swink-agent-mcp` (#1150)
- **agent loop performance**: `TurnSnapshot.messages` is now `Arc<Vec<Arc<LlmMessage>>>`; per-turn history deep-clones replaced by shared snapshots, and per-turn LLM-message conversion is amortized through a lazily-extended mirror (`ContextMessages`) (#1140)
- `build_loop_config` destructures `AgentOptions` exhaustively — adding an option field without wiring it into the loop config is now a compile error (#1140)
- **memory performance**: long-lived `IndexWriter` with incremental per-append indexing (full reindex only on first search or corruption), checkpoint retention bounded at `DEFAULT_MAX_CHECKPOINTS = 20` with `unbounded()` opt-out, atomic-write lock map now evicts dead entries (#1138)
- **tui performance**: per-message render cache keyed by fingerprint + width + theme; selection capture skipped while selection mode is inactive (#1139)
- `rpc`: `respond_ok`/`respond_err` are now async with real backpressure; `RequestId` is zero-clone on the hot path; agentd session factory returns `Result<AgentOptions, String>` and surfaces construction failures as JSON-RPC errors; SIGTERM handler installs before session spawn (#1141)
- `mcp`: `McpError` reworked into a five-variant taxonomy with typed sources and `ProtocolError { context }`; blanket `From<io::Error>` removed; `McpTransport::Sse` renamed to `StreamableHttp` (serde tag `"streamable_http"`); bearer-token/header conflicts now warn (#1141)
- `adapters`: ollama adapter migrated to the shared `sse_adapter_stream`; bedrock/proxy share `base::prefix_start_if_unstarted`; `OaiParserOptions.error_finish_reason_is_error` surfaces error finish reasons as stream errors instead of silent truncation (#1141)
- `policies`, `auth`: constructor coverage (`MemoryNudge`, `FilterRule`, `PiiPattern`, `AuditRecord`), `with_*` naming consistency, keychain `with_service`, oauth2 doc-contract sweep (#1136)
- `patterns`: crate now inherits `[lints] workspace = true`; remaining exported types swept with `#[non_exhaustive]` + constructors (#1137)
- `eval`: crate lint table re-synced with the workspace set (it had drifted, silently exempting the crate); ~50 exported types incl. 17 unit marker structs protected with `#[non_exhaustive]` + `const fn new()`; `CanonicalJsonValue` documented frozen-by-design (#1144)
- workspace: `serde_yaml` (unmaintained) replaced by `serde_yaml_ng` 0.10; `tracing-subscriber`, `url`, `clap` centralized in `[workspace.dependencies]`; `macros` no longer declares the unused `attributes(tool)`; plugin-web dead feature + re-exports removed (#1145)
- CI: every workflow job now carries a timeout; clippy runs `--all-features --all-targets` (excluding `swink-agent-local-llm`); composite rust-setup action adopted across jobs; deny.toml documents the four tolerated ghost dependency chains (#1145)
- test layout: integration tests consolidated into per-crate suites — root 61→2, eval 64→1, adapters 20→2 test binaries; former per-file `#![cfg(...)]` gates now live on `mod` lines in `tests/suite/main.rs`; the two `no_default_features.rs` sentinels stay standalone (#1147)

### Fixed
- `rpc`: the server now mirrors streamed events into agent state via `handle_stream_event`, so remote sessions keep assistant context across turns and `session.snapshot` reads the live transcript instead of a stale one (#1161)
- `eval`: `JudgeVerdict::new` no longer clamps at construction — clamping and `ScoreClamped` detail recording (FR-021) stay in `dispatch_judge`; the constructor clamp introduced by the sweep hid out-of-range judge scores from that instrumentation (#1146)
- coverage workflow: tarpaulin config key is `out`, not `generate` — first green coverage run on integration (#1134)
- `eval`: lost-wakeup hang — `Notified` future `.enable()`d before the condition re-check, with a 30s timeout backstop (#1144)
- `adapters`: `prefix_start_if_unstarted` feature-gated to its bedrock/proxy callers; single-feature builds (e.g. `--features anthropic`) no longer fail `-D dead-code` (#1142)
- API seams from the non_exhaustive sweep closed: missing constructors/re-exports, `ScriptToolDef`/`OtelInitConfig` (feature-gated escapees), latent all-features test lints (#1135)


### Changed — public API hardened with #[non_exhaustive] (gap 1)

Architecture-review gap 1: `constructible_struct_adds_field` semver breaks had
already burned three PRs this cycle (#1097, #1108, #1121). Every externally-
constructible `pub` struct with all-`pub` fields, and every `pub` enum, across
the published crates (`swink-agent`, `swink-agent-adapters`, `-artifacts`,
`-auth`, `-eval`, `-local-llm`, `-macros`, `-mcp`, `-memory`, `-patterns`,
`-plugin-web`, `-policies`, `-rpc`, `-tui`) now carries `#[non_exhaustive]`
unless deliberately exempted. This is a breaking change in itself, so it had
to land inside this window, before the 0.12.0 tag freezes the surface.

- **184 public types protected**: 82 enums + 96 structs marked
  `#[non_exhaustive]`; 6 types left deliberately exhaustive (3 enums, 3
  structs) as frozen-by-design data carriers:
  - `swink-agent-rpc`: `RequestId`, `MessageKind<'a>`, `IncomingMessage`
    (enums) and `RpcError`, `RawMessage` (structs) — all fixed shapes defined
    by the JSON-RPC 2.0 spec.
  - `swink-agent-plugin-web`: `Viewport` (struct) — a width × height pair,
    frozen by design.
  Each carries `#[allow(clippy::exhaustive_structs)]` / `exhaustive_enums`
  plus a one-line comment stating why it is frozen.
- Every protected struct that users construct directly (not only via
  `Default`) keeps a documented `new()`/builder seam — `#[non_exhaustive]`
  blocks external struct-literal and functional-update (`..Default::default()`)
  syntax, so construction now goes through `new()` plus `with_*()` chains.
  New or extended constructors/builders were added where none existed,
  including `UserMessage::new`, `AssistantMessage::new` (+ `impl Default`),
  `ToolResultMessage::new`, `Usage`/`Cost` per-field `with_*`, `AgentResult::new`,
  `AgentContext::new`, `TurnSnapshot::new`, `PolicyContext::new`,
  `ToolDispatchContext::new`, `TurnPolicyContext::new`, `ArtifactData`/
  `ArtifactVersion`/`ArtifactMeta::new`, `DeviceCodePrompt::new`,
  `AuthConfig::new`, `ToolApprovalRequest::new`, `AgentToolResult::new`,
  `ModelRates` per-field `with_*`, `ProviderCatalog`/`PresetCatalog`/
  `CatalogPreset::new`, and equivalents in `swink-agent-mcp`, `-memory`,
  `-policies`, `-rpc`, and `-tui` for their own load-bearing types
  (`McpServerConfig`, `SessionMeta`, `DisplayMessage`, `TurnUsage`,
  `HunkReview`, `PathCandidate`, `UserInput`, `ToolApprovalRequestDto`, etc).
- All internal fallout fixed across the workspace: every cross-crate
  struct-literal, functional-update, and non-exhaustive `match` without a
  wildcard arm (adapters' provider implementations and live-integration
  tests, the TUI's own event/persistence/display code, policies' own
  policy-context construction, memory's JSONL codec, artifacts' three
  storage backends, rpc's DTO conversions, patterns' pipeline executors, and
  the core crate's own `tests/` integration suite) was migrated to the new
  constructor/builder seams or given an explicit, behavior-preserving
  wildcard arm.
- New workspace-wide lints in `[workspace.lints.clippy]`: `exhaustive_structs
  = "warn"` and `exhaustive_enums = "warn"` — restriction lints that flag any
  future exported struct/enum missing `#[non_exhaustive]` (or an explicit
  frozen-by-design allow), so this class of semver break can't recur
  silently.

Bundles the full 0.11.x development line off `integration`. **Supersedes the
unreleased 0.11.1** — that version was never tagged or published, so its entries
are folded in here rather than kept as a phantom release.

### Fixed — dropping an un-drained prompt stream no longer empties history

- **`swink-agent`**: `start_loop` moved the entire conversation history into the spawned loop task via `mem::take`, leaving `state.messages` empty until `AgentEnd` wrote it back. A host that dropped the stream returned by `prompt_stream`/`continue_stream` before draining it silently lost the whole conversation — `LoopGuardStream::drop` restored `loop_active` but never the history.
- The agent now keeps an equivalent snapshot of the full pre-run context in `state.messages` when the loop starts, and completed turns are written back on `TurnEnd` in both drain paths. History is never removed from observable state, so an early drop loses at most the in-flight turn, and `Drop` needs no restore (no host/loop-task race window). Every completion path still replaces `state.messages` wholesale on `AgentEnd`, so nothing duplicates.

### Fixed — `ServingOptions.extra` honored (or loudly rejected) by every provider (#1130)

- **`swink-agent-adapters`**: `ServingOptions::extra` — documented as "passed through verbatim" — was a silent no-op on Anthropic, Google, Bedrock, Mistral, and Proxy. Now: Anthropic and Mistral merge it into the request-body top level, Google into `generationConfig`, and Bedrock into the Converse API's `additionalModelRequestFields`; the Proxy wire protocol has no pass-through channel, so non-empty `extra` emits one `tracing::warn!` per stream call naming the dropped keys instead of vanishing.
- The "typed fields win on collision" merge rule is now implemented once (`base::merge_extra`) and shared by every adapter, replacing the two divergent copies in the OAI transport (blocklist filter) and Ollama (build-order overwrite). OAI-compatible and Ollama request bytes are unchanged — the pinned byte-identity tests still pass.
- **`swink-agent`**: the `ServingOptions` rustdoc support matrix now covers every built-in provider, including each provider's `extra` merge target.

### Added — manual context compaction (#1102)

- **`swink-agent`**: `Agent::compact_context()` — an on-demand counterpart to the automatic in-loop compaction, for host `/compact` commands (unblocks SuperSwink-Coding's TUI `/compact`). Runs the configured context transformer(s) against the stored history with `overflow = true` (a host asking to compact wants maximal pruning), persists the pruned history, and returns the resulting `CompactionReport` synchronously.
- Mirrors the loop pipeline exactly: async transformer first, sync second, one `AgentEvent::ContextCompacted` per transformer that compacts — dispatched through the normal subscriber/forwarder path, so existing host event rendering picks it up unchanged. Returns `Ok(None)` when no transformer is configured or the history is already under budget (no event emitted), and `Err(AgentError::AlreadyRunning)` while a loop is active, since compacting mid-turn would race the loop's view of the history.
- Spec 006 amended additively (new FR-020 plus a Session 2026-07-15 clarification): manual invocation means `overflow = true` no longer implies a provider overflow error occurred.

### Added — TUI skills discovery (#1092)

- **`swink-agent-tui`**: three new `TuiExtensions` seams for pluggable skill discovery with progressive disclosure — `with_skill_completions` (`SkillCandidate` list as a leading `/name` is typed), `with_skill_details` (SKILL.md body on highlight, cached per popup so arrow-key travel never re-invokes the callback), and `with_skill_resolver` (expansion at submit only, via the new `SkillInvocation`/`parse_skill_invocation`). Plus `App::skill_completion`/`SkillCompletion`, `InputEditor::slash_query`, and a preview block in the completion popup.
- Submit precedence is secrets → host commands → skills → built-ins: a known skill submits as a prompt (raw `/deploy` stays in the transcript; the model receives the expansion) instead of hitting the Unknown-command fallback. `@path` mentions expand on the raw text first, so a skill body is never mention-scanned.
- New off-by-default **`skills` cargo feature**: `TuiExtensions::with_skill_dirs` eagerly indexes `<dir>/<name>/SKILL.md` (YAML frontmatter `name`/`description`, directory-name fallback) under explicitly passed directories only — no implicit default paths — and wires all three seams over that index.

### Added — checkpoint hardening: session-scoped IDs, retention, RollingCheckpointPolicy (#1070)

- Default-on crash-safety checkpointing was considered and **rejected** (FR-019 stands: no policy is enabled by default); the opt-in path was hardened instead. `CheckpointPolicy::with_session_id` scopes IDs to `"{session}-turn-{n}"` — without it, the per-run turn index means a second `prompt()` run silently overwrites the first run's checkpoints, and "restore the highest turn" can resurrect stale history. The unscoped format remains the default for backward compatibility.
- **`swink-agent-memory`**: `FileCheckpointStore::with_max_checkpoints(n)` prunes the oldest checkpoints (by `created_at`) after each save; the default stays unlimited, and pruning never deletes files that don't parse as checkpoints.
- **`swink-agent-policies`**: new `RollingCheckpointPolicy` (same `checkpoint` feature) overwrites one stable checkpoint — optionally `"{session}-rolling"` — via the store's atomic save path. Recommended for long-session crash-safety: `CheckpointPolicy` writes the full history under a new ID every turn (O(N²) bytes for N turns); rolling keeps disk at O(context) and loses at most one turn on crash, giving up time-travel. Documented in a new "Crash Safety" section of the policies README and on `AgentOptions::checkpoint_store`.

### Fixed — `swink-agentd` honors `LLM_SYSTEM_PROMPT` / `LLM_MODEL` (#931)

- The daemon loads `.env` via dotenvy but its clap args used baked-in `default_value`s, so `LLM_SYSTEM_PROMPT` and `LLM_MODEL` from the environment were silently ignored — set in `.env`, the TUI honored them and `swink-agentd` didn't. Both args are now `Option<String>` resolved as CLI flag > env var > shared default, mirroring the TUI's tested `resolve_system_prompt` chain.
- The duplicated default literals are promoted to shared core constants `swink_agent::{DEFAULT_SYSTEM_PROMPT, DEFAULT_MODEL}` (additive), and the TUI's proxy-mode model fallback now uses the documented `claude-sonnet-4-6` alias instead of the dated `claude-sonnet-4-20250514` snapshot.

### Added — TUI cost/usage display with pluggable pricing (#1084, #1108)

- **`swink-agent`**: new `pricing` module (`CostCalculator`, `ModelRates`, `PricingTable`), `AgentOptions::with_cost_calculator`/`with_pricing_table`, `AgentLoopConfig::cost_calculator`, and `price_assistant_message_with` — operators can declare per-tier rates that outrank the compiled catalog.
- **`swink-agent-tui`**: `TuiExtensions` (a consuming builder threaded via `App::with_extensions`/`launch_with_extensions`) for host-supplied code, `CustomCommandOutcome`/`CustomCommandFn` for host slash commands, `App::turn_usage`/`TurnUsage`, `TuiConfig::pricing`/`apply_pricing`, and `handle_agent_event` widened to `pub`. `launch()` delegates to `launch_with_extensions()`, so there is one code path.
- **Completes #1100's fix for event consumers.** #1103 priced at the turn seam, but the TUI reads cost from `AgentEvent::MessageEnd`, emitted earlier in `finalize_stream_message` — so the loop accumulated real cost while every event consumer still displayed `$0.0000`. Pricing now happens before `MessageEnd` is emitted; the turn-level call remains as an idempotent safety net for paths that bypass streaming (overflow recovery, aborts).

### Added — `@path` file mentions in the TUI (#1093, #1112)

- `@path` completion and lazy injection via the `TuiExtensions` seam #1108 established: `PathCandidate`, `PathCompletionFn`, `MentionResolverFn`, `TuiExtensions::with_path_completions`/`with_mention_resolver`, and `mentions::{PathMention, parse_mentions}`.
- Two seams rather than one, split by *when* they run: completion supplies candidates as the user types; the resolver injects file content at send time, so a mention costs nothing until the turn is actually dispatched.
- No new `launch_*` overload and no `TuiConfig` churn. `TuiExtensions` keeps private fields behind consuming `with_*` builders, so hosts opt in explicitly and later extension points stay additive rather than breaking struct-literal construction.

### Added — OAuth2 device-code grant, RFC 8628 (#1071, #1106)

- `DeviceCodeHandler` / `DeviceCodePrompt` in `swink-agent`, and `DeviceAuthorizationConfig` plus `oauth2::{DeviceAuthorizationResponse, request_device_code, poll_device_token}` in `swink-agent-auth`, for CLI/TUI and headless contexts where an authorization-code redirect isn't practical.
- Added as a *parallel* seam rather than an extension of the authorization-code types: `AuthorizationHandler::authorize` must return an authorization code, which the device flow has no equivalent for (the resolver polls instead). The shared machinery — single-flight dedup, authorization timeout, credential storage, error variants — is reused unchanged.
- The authorization-code flow takes precedence when a key is configured for both, so existing behavior is unchanged. `DeviceCodePrompt` deliberately excludes `device_code`, so the polling secret never reaches a handler.
- Fixes a latent bug on the shared path: OAuth2 error classification was driven by HTTP status, and RFC 8628 returns `authorization_pending`/`slow_down` as HTTP 400 — normal polling states would have been treated as terminal failures. Classification now keys off the OAuth2 `error` code.

### Added — optional keychain-backed `CredentialStore` (#1068, #1105)

- `KeychainCredentialStore` (with `KeychainBackend`, `SystemKeychain`, `KeychainError`, `DEFAULT_SERVICE`) in `swink-agent-auth`, behind the new **`keychain` feature, off by default**. A default build sees no change.
- Consumers construct and pass it explicitly; nothing resolves through it implicitly. Spec 035 amended accordingly: FR-003 now reads "only store *enabled by default*", with new FR-003a admitting opt-in stores. Env-var stores (FR-004) and store chaining (FR-006) remain dropped.
- Namespaced under the `swink-agent-auth` service, distinct from the TUI's `swink-agent` LLM-provider keys; neither reads the other's entries.

### Added — per-hunk approve/reject for `write_file` (#1069, #1104)

- Hunk-level review at the `write_file` approval prompt, before the write lands. Rejected hunks generate a follow-up tool result explaining which changes were reverted.
- Placed at the approval prompt rather than the post-write diff view: conversation diffs render from tool results, by which point the write has already happened, so rejection there would mean reverting bytes on disk. The post-write diff remains display-only.
- Fails closed at every branch — undecided hunks count as rejected, and arguments that can't be safely rewritten resolve to `Rejected` rather than silently applying the original content.

### Changed — BREAKING

- **`AgentLoopConfig` gains a `cost_calculator` field.** It is an externally-constructible struct with public fields, so exhaustive struct-literal construction breaks — add `cost_calculator: None` or use `..Default::default()`. This is the 0.12.0 version driver (`cargo semver-checks`: `constructible_struct_adds_field`), the same class of break that drove 0.11.0.
- **`AgentEvent::MessageEnd` now carries a non-zero `cost`** for catalog-known models. Code asserting `cost == 0` there will now observe real values. This is the intended fix, but it is a behavior change.

### Changed — TLS backend: ring replaces aws-lc-rs (#1110)

- **Consumers no longer need a C/C++ toolchain to build.** rustls's crypto provider is now the pure-Rust `ring` instead of the default `aws-lc-rs`, whose `aws-lc-sys` build required `cc` + CMake (+ NASM on Windows) in every downstream project. `cargo tree -i aws-lc-sys` is empty for a default-features consumer; `aws-lc-rs`, `aws-lc-sys`, `cmake`, and `quinn` (the subject of RUSTSEC-2026-0185, above) all leave the dependency tree.
- Workspace `reqwest` drops default features for an explicit list: `charset`, `form`, `http2`, `json`, `rustls-no-provider`, `stream`, `system-proxy` — functionally identical to before minus the aws-lc provider. Root trust is unchanged: `rustls-platform-verifier` (OS-native root store), which reqwest 0.13 uses for all rustls configurations.
- Every reqwest-consuming crate takes a direct `rustls` dependency with the `ring` feature and installs ring as the process-default `CryptoProvider` immediately before constructing a client — under `rustls-no-provider`, reqwest panics at `Client` construction until a default is installed. The install is idempotent and loses ties deliberately: a provider already installed by the host application (e.g. aws-lc-rs for FIPS) wins. New public API: `swink_agent_adapters::ensure_default_crypto_provider()`, for hosts that build their own `reqwest::Client` against the workspace's feature unification.
- `jsonschema` also drops default features: its defaults pulled `resolve-http` (a second reqwest + rustls + aws-lc edge) and `resolve-file` for external `$ref` resolution, which nothing in this workspace uses — schemas are compiled inline only. External `$ref`s now fail at validator build instead of silently fetching.
- Behavior note: ring offers no post-quantum key exchange (rustls's `prefer-post-quantum` requires aws-lc), so TLS handshakes negotiate classical X25519 instead of X25519MLKEM768 where servers supported it.

### Fixed — `BudgetPolicy.max_cost` was inert (#1100, #1103)

- **`BudgetPolicy.max_cost` could never fire against any real provider**, including the $10 default bundled into `RecommendedPolicies`. Every built-in remote adapter reports `Usage` but emits `cost: Cost::default()`; only the proxy adapter passed real billed cost through. The loop accumulated that zero verbatim, so `PolicyCtx.accumulated_cost` stayed at `0` and the limit never tripped.
- The loop now prices assistant messages from the compiled model catalog at a single seam in `run_single_turn`, rather than in each adapter — so third-party `StreamFn` implementations are covered too, and the priced cost reaches accumulation, policies, turn metrics, the context history, and the `TurnEnd` event alike. Adapters that price their own response keep precedence.
- Event consumers reading `cost` from `AgentEvent::MessageEnd` were still observing zero after #1103, because `MessageEnd` is emitted before this seam. That is fixed by #1108 in this same release (see above); the two land together, so no published version ever carries the half-fix.

### Fixed — feature-conditional dead code (#919, #1116)

- Gated dead code so `-D warnings` passes under every feature combination — `cargo hack --each-feature` is now clean (127/127). Previously invisible because `release.yml` only ever builds `--all-features`, while `ci.yml` is what runs `--no-default-features` and `--each-feature`.
- Found along the way: the `openai-compat` gate named an internal umbrella feature implying no adapter, so `--features openai-compat` compiled the whole OpenAI plumbing with zero consumers, and `mod openai` was fully dead under `xai`. Also fixed three tests in `swink-agent-adapters` whose items were `ollama`-gated but whose tests were not — that crate did not compile standalone.

### Fixed — TUI tests reached the real OS keychain (#1111, #1113)

- `swink-agent-tui`'s tests called the **real** OS keychain, raising macOS password prompts and **hanging `cargo test --workspace` indefinitely** on `SecKeychainFindGenericPassword`.
- `tui/src/credentials.rs` now routes all access through a `KeychainBackend`, and the real `SystemKeychain` is `#[cfg(not(test))]` — a unit test cannot reach a real keyring even by wiring the backend back in, which is how the bug arose. Caller signatures unchanged; production behavior identical.

### Fixed — flaky theme test (#1107, #1118)

- `swink-agent-tui`'s theme tests raced on a process-global `COLOR_MODE` atomic: a concurrent test reset it mid-assertion, so `mono_white_returns_white` observed `Cyan` (the `Custom`-mode assistant color) instead of `White`. Reproduced at **5 failures in 20 runs** before the fix. The global is gone; each test owns its own theme state.

### Internal — CI hardening (#919, #1109)

- `ci.yml` gains a `cargo audit` job, `RUSTFLAGS: -D warnings` at workflow level (parity with `release.yml`, which was the only workflow enforcing it), pinned toolchain action SHAs, `--locked` on every cargo invocation that accepts it, and a Windows `cargo check` smoke gate on PRs.
- The audit job justified itself on its first run: it caught **RUSTSEC-2026-0185** — `quinn-proto` ≤ 0.11.14, remote memory exhaustion, CVSS 7.5 high — reaching the tree through `reqwest` → `quinn`. Bumped to 0.11.16 per the advisory rather than added to the ignore list.
- `cargo hack --each-feature --no-dev-deps` deliberately omits `--locked`: `--no-dev-deps` rewrites `Cargo.toml` as it runs and `--locked` then refuses to update `Cargo.lock`. The two flags are mutually exclusive; every other step keeps `--locked`.

## [0.11.0] - 2026-07-13

### Added — per-request serving-options seam

- **`ServingOptions`** on `StreamOptions.serving` — a provider-agnostic surface for local/self-hosted serving knobs: `context_length`, `top_p`, `keep_alive`, and an `extra` passthrough map (`BTreeMap<String, serde_json::Value>`). Typed fields win over colliding `extra` keys; the default leaves request bodies unchanged.
- **Ollama adapter** serializes `context_length` as `options.num_ctx`, `top_p` into `options`, `keep_alive` at the top level, and merges `extra` into `options`. Requests with no generation options now omit the `options` key entirely (previously an empty `"options":{}` was sent).
- **OpenAI-protocol adapters** (OpenAI, Azure, xAI via the shared transport) serialize `top_p` and merge `extra` into the request body; `context_length`/`keep_alive` have no OAI equivalent and are ignored.
- `StreamOptionsConfig` round-trips the new field (omitted from snapshots when default).

### Notes

- Additive but breaking for downstream code that constructs `StreamOptions` or `StreamOptionsConfig` with exhaustive struct literals — add `serving: ServingOptions::default()` or use `..Default::default()`. This is the 0.11.0 version driver (`cargo semver-checks`: `constructible_struct_adds_field`).

## [0.10.0] - 2026-07-12

### Added — model deprecation & pricing staleness (#1064)

- **`PresetStatus::Deprecated { replacement_model_id }`** — the compiled model catalog can now mark a still-listed preset as deprecated and point consumers at its replacement (TOML table syntax; existing `status = "ga"`/`"preview"` entries unchanged). Adds `PresetStatus::is_deprecated()`, `CatalogPreset::is_deprecated()`/`replacement_model_id()`.
- **`StreamErrorKind::ModelRetired`** and **`AssistantMessageEvent::error_model_retired()`** — a structured signal for provider responses indicating a retired/decommissioned model (HTTP 400/404/410 with provider wording), classified via `adapters::classify::is_model_retired_response`. The agent loop maps it to the non-retryable `AgentError::ModelRetired`.
- **Pricing staleness warning** — the catalog carries a `pricing_as_of` date; `Agent::new` emits a once-per-process warning when the compiled pricing is older than `SWINK_PRICING_STALENESS_DAYS` (default 180). Adds `pricing_staleness()` / `ModelCatalog::pricing_staleness_at()`.

### Added — production guardrail preset (#1065)

- **`RecommendedPolicies`** builder in `swink-agent-policies` (feature `recommended`) — bundles `BudgetPolicy`, `MaxTurnsPolicy`, `SandboxPolicy`, and `ToolDenyListPolicy` with sensible production defaults and applies all four to `AgentOptions` in one call. The library default remains anything-goes.
- **`verify_production_guardrails` / `assert_production_guardrails`** — integration-contract helpers that behaviorally probe each guardrail so trivial wiring fails the check.

### Added — spec 045 (JSON-RPC agent service)

- **`swink-agent-rpc` crate** — new workspace crate exposing a `swink_agent::Agent` over a Unix-domain socket using JSON-RPC 2.0 / NDJSON.
- **`AgentServer`** — Unix socket server with `0600` permissions, peer-credential check (Linux `SO_PEERCRED` / macOS `getpeereid`), single-session enforcement, and graceful Ctrl-C shutdown. Tool-approval round-trips via `tool.approve` JSON-RPC requests.
- **`AgentClient`** — async client with `connect`, `prompt_text`, `with_approval_handler`, `cancel`, and `shutdown`. Streams `AgentEvent` values while driving a `prompt` request.
- **`JsonRpcPeer` / `PeerSender`** — transport-agnostic JSON-RPC 2.0 peer over any async I/O pair. Reader and writer tasks run independently; `PeerSender` is `Clone` for use in callbacks.
- **`swink-agentd` binary** — daemon binary (`cli` feature, unix-only) with `--listen`, `--force`, `--model`, and `--system-prompt` flags.
- **Feature flags:** `client`, `server`, `cli` (default: `client + server`).

### Removed

- **`SocketTransport` stub** in `swink-agent-tui` — the `#[cfg(feature = "remote")]` stub that returned `TransportError::Unavailable` for all operations has been removed. Remote agent sessions are now provided by `swink-agent-rpc` + `swink-agentd`. The `remote` feature flag in `swink-agent-tui` has been removed. Closes #898.

### Changed

- **Structured stream-error classification is now primary (#1063).** All built-in adapters (Anthropic, OpenAI, Azure, xAI, Mistral, Bedrock, Google) construct a structured `StreamErrorKind` — notably `ContextWindowExceeded` and `ContentFiltered` — from provider-specific error codes/wording, rather than relying on the core loop's substring fallback (which now exists only for third-party `StreamFn` implementations).

### Fixed

- **JSONL session load tolerates a truncated/corrupt tail line (#1067).** A crash mid-append can leave a truncated multi-byte UTF-8 sequence in the final line; load now skips such a line with a warning and recovers the remaining messages instead of failing the whole load. The metadata line remains strict.
- **`SessionStore` no-op state defaults now warn (#1066).** The default `save_state`/`load_state` implementations emit a once-per-process `tracing::warn!` so a store that hasn't implemented state persistence no longer silently drops session state. (These methods are planned to become required in a future major version.)
- **Deterministic `swink-agent-plugin-web` log tests (#1082).** The `execute_logs_*` tests pinned to a current-thread runtime so the thread-local capture subscriber reliably observes the tool's completion log.

### Notes

- This is a **minor** release: `PresetStatus` gains a variant, `ModelCatalog` gains the `pricing_as_of` field, and `AgentError`/`StreamErrorKind` gain variants — all additive but breaking under Rust's exhaustive-match rules for downstream consumers.
- **Dependencies:** `rmcp` 2.0 → 2.1; `crossbeam-epoch` bumped to 0.9.20 (RUSTSEC-2026-0204); patch-updates group; `crate-ci/typos` action.

## [0.9.0] - 2026-04-27

### Added — spec 044 (eval-driven self-improvement loop)

- **`swink-agent-evolve` crate** — new workspace crate implementing a closed-loop prompt and tool-schema optimization cycle: baseline evaluation → weak-point diagnosis → mutation → candidate evaluation → acceptance gating → versioned persistence.
- **`EvolutionRunner`** — orchestrates full cycles via `baseline()` + `run_cycle()` / `run_cycles()`; panic-isolated so a crashing candidate never tears down the loop.
- **`OptimizationTarget` / `OptimizationConfig` / `CycleBudget`** — declarative configuration of what to mutate (system prompt sections, tool descriptions) and how many cycles/candidates to allow.
- **`Diagnoser`** — maps failing `EvalCase` results to `TargetComponent` weak points by evaluator name; `WeakPoint` carries a confidence score and sample of failing cases.
- **`MutationStrategy` trait** — pluggable mutation interface; three built-in strategies: `Ablation` (remove/simplify prompt sections), `LlmGuided` (LLM-generated rewrites via optional judge), `TemplateBased` (pattern-based variations). `deduplicate()` collapses near-identical candidates before evaluation.
- **`AcceptanceGate` / `AcceptanceVerdict`** — ranks candidates by composite score with P1-regression protection; `AcceptanceResult` carries the winning `Candidate` or a `NoCandidateImproved` verdict.
- **`CyclePersister` / `ManifestEntry`** — saves winning candidates to versioned cycle directories with a JSONL manifest for audit and rollback.
- **`otel` feature** — optional OpenTelemetry tracing for optimization cycles.

### Added — spec 031 (memory nudge policy)

- **`MemoryNudgePolicy`** in `swink-agent-policies` — `PostTurnPolicy` that scans assistant turns for four heuristic patterns (Correction, ExplicitSave, Decision, Preference) and emits `MemoryNudge` payloads when confidence exceeds the configured threshold.
- **`NudgeSensitivity`** enum (Low / Medium / High) — controls per-category confidence cutoffs.
- **`MemoryNudgeCategory` / `MemoryNudge`** — structured payload carrying category, summary, confidence, and turn number, ready for downstream memory-store writes.
- Feature-gated: `memory-nudge` in `swink-agent-policies`.

### Added — spec 025 (TUI transport abstraction)

- **`TuiTransport` trait** in `swink-agent-tui` — abstracts message exchange between the TUI and the agent loop, enabling future remote/socket-backed sessions without changing the event loop.
- **`InProcessTransport`** — zero-behavior-change extraction of the existing direct-channel path; all current users automatically use this impl.
- **`SocketTransport` stub** — skeleton behind the `remote` feature gate for upcoming remote-session support.

### Added — spec 021 (cross-session full-text search)

- **`TantivyIndex`** in `swink-agent-memory` — tantivy-backed full-text index stored alongside session files at `<sessions_dir>/.search_index/`; lazy-built on first search, incrementally updated on `save_entries`, removed on `delete`.
- **`JsonlSessionStore::search()`** — delegates to `TantivyIndex` when the `search` feature is active; falls back to the existing linear scan otherwise (zero regressions on the default build).
- **`open_search_index()` / `rebuild_search_index()`** on `JsonlSessionStore` — explicit index lifecycle management.
- Feature-gated: `search` in `swink-agent-memory`.

### Added — spec 023 (RL-compatible training format export)

- **`TrainingExporter` trait** in `swink-agent-eval` — export `Invocation` traces as fine-tuning data in three formats.
- **ChatML/SFT exporter** — conversation-style JSONL with tool calls, suitable for standard SFT pipelines.
- **DPO pair generator** — chosen/rejected pairs derived from high/low-scoring traces per `EvalCase`; score spread is configurable.
- **ShareGPT exporter** — community-format conversation export.
- **`TrainingReporter`** — integrates with the existing `Reporter` trait so training export slots into any `EvalRunner` pipeline.
- **`ExportOptions`** — `quality_threshold` filter, format selector, output path.
- Feature-gated: `training-export` in `swink-agent-eval`.

### Added — spec 043 (evals: advanced features)

- **`swink-agent-eval-judges` crate** — nine per-provider `JudgeClient` implementations (Anthropic, OpenAI, Bedrock, Gemini, Mistral, Azure, xAI, Ollama, Proxy) plus `Blocking<Provider>JudgeClient` sync wrappers, behind the `all-judges` umbrella feature.
- **Prompt-template registry** (`judge-core` feature) — `JudgePromptTemplate`, `MinijinjaTemplate`, `PromptTemplateRegistry`, built-in `*_v0` templates; duplicate-version registration rejected.
- **24 evaluators across seven families** — Quality (10), Safety (7), RAG (3 + `Embedder`), Agent (9), Simple (2), Structured (2), Code (4) plus Multimodal (`ImageSafetyEvaluator`). Shared `JudgeEvaluatorConfig` + `JudgeEvaluatorBuilder` trait expose `.with_prompt()`, `.with_few_shot()`, `.with_system_prompt()`, `.with_output_schema()`, `.with_use_reasoning()`, `.with_feedback_key()`, `.with_aggregator()` on every judge-backed evaluator.
- **`EvalRunner` upgrades** — parallelism, `num_runs`, disk-backed judge cache, cancellation, initial-session hydration.
- **Multi-turn simulation + experiment generation** — `ActorSimulator`, `ToolSimulator`, `ExperimentGenerator`, `TopicPlanner` behind `simulation` / `generation` features.
- **Trace ingestion** (`trace-ingest`) — `OtelInMemoryTraceProvider`, `OpenInferenceSessionMapper`, `LangChainSessionMapper`, `OtelGenAiSessionMapper`, `SwarmExtractor`, `GraphExtractor`, `ToolLevelExtractor`. Optional backends: `OtlpHttpTraceProvider` (`trace-otlp`), `LangfuseTraceProvider` (`trace-langfuse`), `OpenSearchTraceProvider` (`trace-opensearch`), `CloudWatchTraceProvider` (`trace-cloudwatch`, takes a caller-supplied `CloudWatchLogsFetcher`).
- **`EvalsTelemetry`** — OTel span emission inside the runner (`telemetry` feature).
- **Reporters** — `ConsoleReporter`, `JsonReporter` (schema-stable via `SCHEMA_VERSION`), `MarkdownReporter`, `HtmlReporter` (`html-report` feature, self-contained artifact), `LangSmithExporter` (`langsmith` feature, pushes runs + feedback with partial-failure reporting).
- **SC-008 deterministic replay** — `OpenInferenceSessionMapper` round-trips scores bit-identically between in-process and reloaded OTel sessions.

### Changed — spec 043

- **`EvalCase`** extended with `expected_assertion`, `expected_interactions`, `few_shot_examples`, `attachments`, `session_id`, `metadata` (serde backwards-compatible — new fields default on deserialize).
- **`EvaluatorRegistry::add`** now rejects duplicate evaluator names with `EvalError::DuplicateEvaluator`; `register` panics on collision for ergonomic setup.
- **Judge scores** are clamped to `[0.0, 1.0]` with a structured `Detail::ScoreClamped { original, clamped }` recorded in `EvalMetricResult::details` when the raw verdict is out of range (FR-021).
- **`GateConfig`** now derives `Serialize`/`Deserialize` so downstream tooling can persist gate thresholds in JSON/YAML without adapter-specific wrapper types.

### Breaking changes — spec 043

- Judge-backed eval setup no longer relies on a default model id — `JudgeRegistry::builder(client, model_id)` requires the explicit `model_id` positional arg so score histories stay pinned to the caller-selected model (FR-007 clarification Q9).
- FR-044 legacy-result converter was deliberately **not** shipped. The converter was a no-op shim for a shape that never reached a public release; downstream users already consume `EvalCaseResult` / `EvalSetResult` directly.

### Fixed

- **Tools**: drain stdout/stderr pipe to EOF after hitting `MAX_OUTPUT_BYTES` so the child process receives no SIGPIPE (exit 141) on large output.
- **Local LLM**: enforce stream context contracts (context window, cancellation, readiness race); retry model loads after failed state.
- **Adapters**: bound error-body reads; treat OpenAI EOF-before-DONE as a network error; finalize Bedrock blocks before terminal frames; classify unexpected Ollama EOF as network error.
- **Artifacts**: fix lock contention across store instances; populate pre-dispatch execution roots.
- **Memory**: lock JSONL reads during append commits; fail closed on missing session migrations.
- **TUI**: block session mutations while streaming; add wide side-by-side diff view.
- **Loop**: run post-turn policies on overflow errors; preserve multi-tool results during context compaction; preserve prompt batch for pre-turn policies; reject stream starts after terminal events.
- **Web**: harden private-address filtering; harden Playwright subresource filtering; harden redirect filtering.
- **Auth**: redact credential debug output.
- **xtask**: fail `verify-catalog` on unknown provider filters.
- **CI**: add no-default local validation sentinels.

## [0.8.1] - 2026-04-22

### Added
- `swink-agent-adapters::build_remote_connection_with_credential` and public `build_connection_from_preset` — explicit-credential remote-connection builders for embedders that manage secrets in keychains/Vault and cannot mutate `std::env` (#791, #792).
- `swink-agent-eval` semantic trajectory matching (spec 023): `SemanticToolSelectionEvaluator`, `SemanticToolParameterEvaluator`, `EnvironmentStateEvaluator`, and a `JudgeClient` trait with pluggable providers. Each evaluator wraps judge calls in a configurable `tokio::time::timeout` (5 min default, `with_timeout` override) so evals own their own non-hang guarantee. Includes `MockJudge` in `swink-agent-eval::testing`.
- `swink-agent-eval` foundational score aggregators (#747) and deterministic case-session IDs — enables downstream experiment tooling.
- `swink-agent-eval-judges` crate scaffold (spec 043 Phase 1) — advanced evals framework foundation.
- `swink-agent-eval` default URL filter — a built-in `url_filter` module for trajectory/content scoring.
- Panic isolation across eval scorers (#731, #767) — a panicking scorer no longer tears down the evaluator run.
- `FnTool::with_execute_async` alias for untyped async builder discoverability (#663).
- Built-in `TiktokenCounter` for token counting without external dependencies (#662).
- TUI click-drag text selection and copy in chat view (#605, #606).
- Resolver-backed SSE MCP auth bootstrap (#679).
- `ApprovalMode` and `ToolMiddleware` exports from the prelude (#659, #660).

### Changed
- `BudgetGuard` ported to the `BudgetPolicy` loop-policy interface (spec 023 Phase 13) — budget constraints now compose through the same slot vectors as other policies.
- MCP tool registration names are now sanitized for provider compatibility (#702).
- Composed plugin tool names use hash-tail truncation to prevent long-name collisions; `Agent::new()` and `Agent::set_tools()` fail fast on duplicate final names (#674).
- Agent loop `PreTurn` now exposes the initial prompt batch as the first-turn `new_messages` slice; post-turn policy-injected messages processed before follow-up polling or `AgentEnd` (#676).

### Fixed
- **Streams and cancellation**: honor cooperative cancellation in web tools (#734); short-circuit pre-cancelled local-LLM streams; emit aborted stop reason on local-LLM cancel; honor pre-send stream cancellation in adapters; preserve single `MessageStart` across overflow recovery (#721); bound per-tool update channel (#770, #777); bound tool-update buffering.
- **MCP**: clear stdio child environments; fix reconnect and shutdown lifecycle (#701); emit connect/discovery/call lifecycle events (#625); roll back MCP collisions (#723); refresh SSE resolver auth on recovery (#680).
- **Adapters**: reject nameless terminal tool calls; hard-fail malformed Anthropic SSE events (#720); stop retrying parse and protocol faults (#629); gate Azure auth dependency (#631); sanitize incomplete `tool_use` arguments before dispatch (#621); normalize parse error classification for OAI/Gemini (#703).
- **Auth and secrets**: sanitize OAuth2 refresh diagnostics; include sanitized endpoint in OAuth2 refresh-failure debug log; redact OAuth2 refresh error bodies (#626); sanitize credential store tool errors (#706); redact `#key` secrets from TUI input history (#628).
- **Artifacts**: enforce streaming metadata integrity; treat missing content as corruption; serialize delete mutations (#682); make delete exact-name-safe for nested IDs (#705); validate `session_id` and enforce canonical artifact root (#622).
- **Memory**: require explicit atomic `save_full` (#683); serialize JsonlSessionStore delete locking (#724); take `Checkpoint` by value in `save_checkpoint` (#661).
- **Loop**: enforce two-pass `PreDispatch` before approval (#627); stop loop on pre-dispatch Stop (#699); emit single terminal event on overflow failure (#644); drain steering after text-only turns; block post-turn tool-call injection; preserve one retry message lifecycle (#677); preserve dynamic prompt during overflow retry (#700).
- **Patterns**: isolate parallel branch failures.
- **Eval**: reject duplicate evaluator registrations; restore cache-prefix tracking.
- **TUI**: harden setup wizard and editor temp files; fail closed on approval channel errors.
- **CI**: unbreak integration clippy + deny on rust 1.95; repair malformed YAML in bench/approve-contributor workflows; replace unsupported expression functions in approve-contributor.
- **Telemetry**: redact custom message warning logs.

### Internal
- Centralized workspace clippy config (`[workspace.lints]`).
- Pinned toolchain to Rust 1.95 stable (#737).
- Dependabot cadence changed from weekly to daily for cargo updates.
- Dependency bumps: `rmcp` 1.3.0 → 1.5.0 (#787), `scraper` 0.25.0 → 0.26.0 (#786), `notify` 7.0.0 → 8.2.0 (#790).

## [0.8.0] - 2026-04-19

### Added
- `FileCheckpointStore` in `swink-agent-memory` — durable file-backed checkpoint persistence across process restarts (#666).
- `FnTool::with_execute_async` alias — explicit untyped async builder for discoverability (#663).
- Custom SSE MCP headers (`McpTransport::Sse { headers }`) — supports `x-api-key` and other non-standard MCP server auth; also fixes bearer-token prefix duplication (#665).
- Built-in `TiktokenCounter` for token counting without external dependencies (#662).
- TUI click-drag text selection and copy in chat view (#605).

### Changed
- Composed plugin tool names now use hash-tail truncation to prevent long-name collisions; `Agent::new()` and `Agent::set_tools()` fail fast on duplicate final names (#674).
- Agent loop `PreTurn` now exposes the initial prompt batch as the first-turn `new_messages` slice; post-turn policy-injected messages are processed before follow-up polling or `AgentEnd` (#676).

### Fixed
- OAuth2 refresh failures no longer leak `error_description` or raw token-endpoint bodies into tool-facing errors or debug logs (#675).
- Fixed pre-dispatch state snapshot reuse, two-pass `PreDispatch` enforcement before approval, and cache-miss retry strategy (#627, #639, #643).
- Fixed overflow terminal event emission, stream/import cycle, and sync runtime init errors (#642, #644, #649).
- Fixed TUI corrupted session state load, Bedrock terminal frame requirement, and approval debug context redaction (#646, #647, #650).
- Fixed Azure auth dependency gate, adapter retry on parse/protocol faults, plugin tool name sanitization for provider compatibility, and incomplete `tool_use` argument sanitization (#620, #621, #629, #631).
- Fixed MCP lifecycle event emission and artifact session/root validation (#622, #625).
- `ApprovalMode` and `ToolMiddleware` exported from prelude (#659, #660).
- `Checkpoint` now taken by value in `save_checkpoint` (#661).

## [0.7.9] - 2026-04-16

### Changed
- **Breaking**: `swink-agent-local-llm` backend replaced from `mistralrs 0.8`
  to `llama-cpp-2` (Rust bindings for llama.cpp). All models now use GGUF
  format uniformly. Feature flags changed: removed `cudnn`, `flash-attn`,
  `mkl`, `accelerate`; added `vulkan`.
- Gemma 4 presets updated to GGUF repos (`bartowski/`). EmbeddingGemma
  updated to GGUF (`unsloth/embeddinggemma-300m-GGUF`).
- TUI no longer auto-wires the local model as a default/fallback. The
  `local` feature is kept for explicit opt-in.
- Spec 041 (Gemma 4 local adapter) folded into spec 022 (local-llm crate).

### Fixed
- SmolLM3 GGUF models now produce text output instead of empty responses
  (#594, #586). The `mistralrs` backend rejected the SmolLM3 architecture;
  `llama-cpp-2` supports it natively.
- Gemma 4 E2B/E4B models now produce text output. The GGUF-embedded Jinja
  template was too complex for llama.cpp's template engine; prompt is now
  formatted manually for Gemma 4 models.
- Tool pre-dispatch is now cancellation-aware (#592).
- Turn index increments correctly after no-tool turns (#595).

## [0.7.8] - 2026-04-16

### Changed
- Model catalog: add GPT-5 series (`gpt-5`, `gpt-5-mini`, `gpt-5-nano` +
  dated variants) and GPT-5.4 series (`gpt-5.4`, `gpt-5.4-mini`,
  `gpt-5.4-nano`). Remove deprecated OpenAI models below version 5
  (`gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini`, `o3-mini`, `o1`).
- CI consolidated from 9 jobs to 3 on PRs (~5 min instead of ~20 min).
  Full platform matrix, semver, and MSRV checks now run only on main
  pushes or weekly schedule. `integration` branch removed from push trigger
  to avoid redundant double-runs.

### Fixed
- Fail fast with a clear error when the unsupported SmolLM3 local preset
  is selected instead of silently producing garbage output (#587).
- Force-reinstall `cargo-binstall` tools (`cargo-hack`, `cargo-nextest`,
  `cargo-semver-checks`) on every CI run to prevent stale binary cache
  failures (#589).

## [0.7.7] - 2026-04-16

### Fixed
- Remove `version` field from internal workspace path **dev-dependencies**
  (8 entries across the workspace). Dev-deps are stripped on publish, so
  adding `version` does nothing useful, and worse — cargo tries to resolve
  them via the registry during packaging, failing for crates that aren't on
  crates.io yet. The v0.7.6 publish job failed at dry-run on swink-agent
  itself with "no matching package named `swink-agent-adapters` found"
  because the root crate's dev-dep on adapters was given a version field.
- Fix topological publish order in `release.yml`: `swink-agent-adapters`
  was published before `swink-agent-auth`, but adapters has a regular dep
  on auth. Reordered to: tier 1 (auth, memory, policies, artifacts, eval,
  local-llm, mcp, patterns, plugin-web) → tier 2 (adapters) → tier 3 (tui).
  This bug would have surfaced if v0.7.6 had reached the publish step.

## [0.7.6] - 2026-04-16

### Fixed
- Add explicit `version` field to every internal workspace path dependency
  (e.g. `swink-agent = { path = "..", version = "0.7.6", ... }`). `cargo
  publish` requires a version on every dep so it can strip the path and
  resolve via crates.io. The v0.7.5 publish run shipped `swink-agent` and
  `swink-agent-macros` (no internal deps) but failed on
  `swink-agent-adapters` with "all dependencies must have a version
  requirement specified when publishing." The CI `cargo package --list`
  fallback used for downstream crates does not validate this; only real
  `cargo publish` does. v0.7.6 republishes everything; `swink-agent` and
  `swink-agent-macros` v0.7.5 remain on crates.io but have no dependents.

## [0.7.5] - 2026-04-16

### Fixed
- Replace invalid crates.io category slugs (`development-tools::proc-macros` →
  `development-tools::procedural-macro-helpers` in `swink-agent-macros`;
  `machine-learning` → `science` in `swink-agent-local-llm`). The v0.7.4
  publish run uploaded `swink-agent` to crates.io but failed at the macros step
  because crates.io validates categories server-side only during real upload —
  `cargo publish --dry-run` cannot detect this. v0.7.5 republishes everything
  with corrected metadata; `swink-agent` v0.7.4 remains on crates.io but has no
  dependents.

## [0.7.4] - 2026-04-16

### Changed
- Repo made public. Open-source readiness: MIT-only license, full Cargo.toml
  metadata across all crates, crates.io + docs.rs badges, CONTRIBUTING.md,
  SECURITY.md, THANKYOU.md, branch model (`main` + `integration`), PR gate,
  approve-contributor workflow, issue templates, and AGENTS.md for all crates.

## [0.7.3] - 2026-04-15

### Added
- `EditFileTool` — surgical find-and-replace file editing tool, re-exported from crate root.
- Mid-stream steering interrupt: queued messages now land at the turn boundary without aborting in-flight tool batches.

### Fixed
- Adapter pre-stream `Start`/`Error` event ordering (#571).
- Preserve Ollama NDJSON UTF-8 chunk boundaries (#570).
- Pre-dispatch stop result parity (#568).
- TUI streaming jitter and per-token redraw churn eliminated.

## [0.7.2] - 2026-04-10

### Fixed
- TUI approval mode: `Agent` is now the single source of truth (#567).
- Inline aborted tool turns instead of surfacing them as errors (#566).
- Isolate `adapters` no-default-features sentinel (#564).
- Include loop context in pause snapshot to prevent message loss (#563).
- Abort spawned tool handles on `ChannelClosed` (#562).

### Changed
- Examples migrated to [SuperSwinkAI/Swink-Agent-Examples](https://github.com/SuperSwinkAI/Swink-Agent-Examples).

## [0.7.1] - 2026-04-15

### Fixed
- Enforce proxy terminal event before `[DONE]` to prevent stray trailing tokens (#552).
- Web plugin rate-limiter cutoff underflow when body is shorter than the byte window (#551).
- Preserve custom message envelopes during JSONL entry saves (#550).
- `atomic_fs` replace semantics on Windows — use `MOVEFILE_REPLACE_EXISTING` flag (#549).
- Guard checkpoint restore against concurrent agent runs to prevent state corruption (#548).
- Thread raw SSE payload callbacks through all runtime adapters (#547).
- SSE parser now handles field lines without a trailing space after the colon (#546).
- Custom tool execution partition validation to reject mismatched call/result pairings (#545).
- Abort in-flight tool batches when parent `CancellationToken` fires (#544).
- Delay OpenAI tool-call `Start` event until the tool name is fully known (#532).
- Validate eval store filesystem IDs to reject path-traversal inputs (#531).
- Make Gemini final tool-call deltas deterministic (#530).
- Prevent steering message drop in concurrent tool-dispatch workers (#529).
- Emit terminal error on local-LLM EOF without a `Response::Done` frame (#528).
- Apply session migrators in `JsonlSessionStore::load` (#527).
- Preserve steering interrupt messages across checkpoint cycles (#526).
- Make artifact streaming saves incremental rather than full-file rewrites (#515).
- Centralize local LLM preset defaults to avoid divergence across callers (#514).
- Reject duplicate orchestrator registrations (#513).
- Emit pipeline failure events on execution errors (#512).

## [0.7.0] - 2026-04-09

### Breaking
- **Stabilize public API surface (#263).** 15 internal modules changed from `pub mod` to `pub(crate) mod`. All public items remain accessible via root re-exports (`use swink_agent::StreamFn`). Downstream consumers must update module-path imports.

### Added
- Feature-matrix smoke tests for all optional root features (#292).
- `pub const VERSION` re-exported from the lib root, sourced from `CARGO_PKG_VERSION`.
- Release workflow triggered on `v*` tags: dry-run publish of all workspace crates, GitHub release with generated notes and `Cargo.lock` attached.
- Windows CI coverage for default builtin tools (#294).

### Fixed
- Remove duplicate `#![forbid(unsafe_code)]` attributes in policies and mcp crates (#262).
- Replace panicking unwraps in xtask report with proper error handling (#288).
- `SessionState::set` now returns `Result` instead of panicking (#291).
- Gate builtin-tools references behind feature flag in tests and examples (#261).

### Changed
- Centralize shared workspace dependencies: `regex`, `dirs`, `toml`, `bytes` (#264).
- License simplified to MIT-only.

## [0.6.x] - 2026-03-10 to 2026-04-05

Major additions: Gemma 4 local inference, `BlockAccumulator` for streaming event assembly, `schemars`-based proc-macro engine, multi-agent patterns and artifact service, MCP integration, plugin system, policy slots, credential management, TUI session management, and web browse plugin. 42 specs implemented across the 0.6 lifecycle.

[Unreleased]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.12.1...HEAD
[0.12.1]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.11.0...v0.12.0
[0.7.8]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.6.2...v0.7.0
[0.6.x]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.5.0...v0.6.2
