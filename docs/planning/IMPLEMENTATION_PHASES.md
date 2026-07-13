> **Archived — historical record, no longer maintained.** This document
> described the original 6-phase (P1–P6) implementation plan for the core
> library, which completed on 2026-03-14. Its phase numbering predates and
> conflicts with the Phase 0–11 numbering used by the live index; all current
> status lives in [SPECIFICATION_TRACKER.md](SPECIFICATION_TRACKER.md).

---

# Swink Agent — Implementation Phases (Historical)

**Related Documents:**
- [PRD](./PRD.md)
- [HLD](../architecture/HLD.md)
- Live spec index: [SPECIFICATION_TRACKER.md](SPECIFICATION_TRACKER.md)

## Summary

The original core-library build ran as six sequential phases, all complete:

| Phase | Scope | Superseded by |
|---|---|---|
| P1 — Foundation Types | `types.rs`, `error.rs` | Spec 002 |
| P2 — Core Traits | `tool.rs`, `stream.rs`, `retry.rs` | Spec 003 |
| P3 — Agent Loop | `loop_.rs` | Spec 004 |
| P4 — Agent Struct | `agent.rs`, `lib.rs` | Spec 005 |
| P5 — Proxy StreamFn | `adapters/src/proxy.rs` | Spec 020 |
| P6 — Integration | `tests/integration/` | Spec 030 |

The per-phase deliverable lists, test criteria, and AC-to-test mappings that
used to live here are realized in the corresponding `specs/` directories
(the AC mapping in `specs/030-integration-tests/`). Everything built after
these six phases (companion crates, remaining adapters, TUI, policies,
extensibility) is tracked exclusively in
[SPECIFICATION_TRACKER.md](SPECIFICATION_TRACKER.md).
