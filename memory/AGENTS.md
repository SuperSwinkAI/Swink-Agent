# AGENTS.md — swink-agent-memory

## Lessons Learned

- `JsonlSessionStore::save_entries()` rewrites must preserve both `_state` records and existing `_custom` message envelopes. Mixed `save()` / `save_entries()` flows rely on those pass-through wrappers surviving entry-oriented rewrites.
