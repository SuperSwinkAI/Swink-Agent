# AGENTS.md — swink-agent-memory

## Lessons Learned

- `JsonlSessionStore::save_entries()` rewrites must preserve both `_state` records and existing `_custom` message envelopes. Mixed `save()` / `save_entries()` flows rely on those pass-through wrappers surviving entry-oriented rewrites.
- Auto-generated session IDs map directly to JSONL filenames, so second-resolution timestamps are not unique enough; keep the readable UTC timestamp prefix, but append random entropy (currently UUID v4 hex) to avoid same-second collisions without changing file semantics.
