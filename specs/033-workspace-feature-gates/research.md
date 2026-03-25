# Research: 033 Workspace Feature Gates

**Date**: 2026-03-25

## R1: Adapter Dependency Isolation

**Decision**: Gate each adapter module behind its own Cargo feature flag. Shared infrastructure compiles unconditionally.

**Rationale**: The adapters crate has 9 adapter modules but only 2 provider-specific dependencies (`eventsource-stream` for proxy, `sha2` for bedrock). All other dependencies (`reqwest`, `tokio`, `serde_json`, etc.) are shared across adapters and must remain unconditional. The `openai_compat` module is shared infrastructure used by `openai`, `azure`, `mistral`, and `xai` — it compiles unconditionally.

**Alternatives considered**:
- Gate shared modules like `openai_compat` per-provider: Rejected — `openai_compat` is used by 4 adapters (openai, azure, mistral, xai) making it effectively shared infrastructure. Gating it would require complex feature combinations.
- Group adapters by protocol (SSE vs NDJSON vs non-streaming): Rejected — doesn't match consumer mental model (they think in provider names, not protocols).

**Shared (always-compiled) modules**: `base`, `sse`, `classify`, `convert`, `finalize`, `openai_compat`, `remote_presets`.

**Provider-specific dependency mapping**:
| Feature | Deps to gate |
|---------|-------------|
| `proxy` | `eventsource-stream` (sole consumer) |
| `bedrock` | `sha2` (AWS SigV4 signing) |
| All others | No provider-specific deps (only shared) |

## R2: Mistralrs Backend Feature Flags

**Decision**: Forward mistralrs feature flags (`metal`, `cuda`, `cudnn`, `flash-attn`, `mkl`, `accelerate`) through the local-llm crate. Default to no backend features (CPU-only inference via candle's default behavior).

**Rationale**: Mistralrs 0.7 exposes compile-time backend features: `metal`, `cuda`, `cudnn`, `flash-attn`, `mkl`, `accelerate`, `nccl`, `ring`. Without any backend feature enabled, mistralrs uses CPU-only inference. The `gpu_layers` runtime config already exists in the crate but only matters if a GPU backend was compiled in.

**Alternatives considered**:
- Only expose `metal` and `cuda`: Rejected — `mkl` (Intel MKL math acceleration) and `accelerate` (Apple Accelerate framework) are also valuable. Forward all that are safe.
- Make `metal` default on macOS via `cfg(target_os)`: Rejected — feature defaults must be platform-agnostic in Cargo.toml. Consumers select based on their target.

**Feature mapping**:
| local-llm feature | Forwards to mistralrs |
|---|---|
| `metal` | `mistralrs/metal` |
| `cuda` | `mistralrs/cuda` |
| `cudnn` | `mistralrs/cudnn` |
| `flash-attn` | `mistralrs/flash-attn` (implies cuda) |
| `mkl` | `mistralrs/mkl` |
| `accelerate` | `mistralrs/accelerate` |

No `default` or `all` feature for backends — explicit opt-in only (wrong backend = build failure or wasted compile time).

## R3: Root Crate Feature Forwarding Architecture

**Decision**: The root `swink-agent` crate currently has NO compile-time dependency on adapters, local-llm, or TUI (only dev-dependencies). Add them as optional dependencies with feature forwarding.

**Rationale**: For consumers to write `swink-agent = { features = ["anthropic"] }`, the root crate must have an optional dependency on `swink-agent-adapters` that activates when any adapter feature is enabled. Each adapter feature on root forwards to the corresponding feature on the adapters crate.

**Alternatives considered**:
- Keep adapters/local-llm/TUI as separate direct dependencies only: Rejected — ergonomics are poor. Consumers want a single dependency line.
- Make adapters a mandatory dependency: Rejected — core crate must remain provider-agnostic per constitution Principle V.

**Forwarding chain**:
```
swink-agent feature "anthropic" → dep:swink-agent-adapters + swink-agent-adapters/anthropic
swink-agent feature "adapters-all" → dep:swink-agent-adapters + swink-agent-adapters/all
swink-agent feature "local-llm" → dep:swink-agent-local-llm
swink-agent feature "local-llm-metal" → dep:swink-agent-local-llm + swink-agent-local-llm/metal
swink-agent feature "tui" → dep:swink-agent-tui
```

## R4: Policies Crate Reference Pattern

**Decision**: Replicate the policies crate's feature gating pattern exactly.

**Rationale**: Already proven, consistent, zero overhead when disabled.

**Pattern**:
1. `Cargo.toml`: `default = ["all"]`, `all = [list of all features]`, individual features as markers or with `dep:` for optional deps.
2. `lib.rs`: Paired `#[cfg(feature = "X")]` on both `mod` declaration and `pub use` re-export.
3. When feature disabled: module doesn't compile, type doesn't exist → clear compile error if referenced.

## R5: Existing Test Compatibility

**Decision**: All workspace tests run with default features (`all` enabled). No test changes needed for backward compatibility.

**Rationale**: `default = ["all"]` enables everything, matching current behavior. Tests that reference specific adapter types will only run when those features are active. Integration tests in the root crate's dev-dependencies already pull in all sub-crates.

**CI addition**: Add matrix entries for `--no-default-features` and single-feature builds to verify isolation.
