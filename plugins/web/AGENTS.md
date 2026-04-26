# AGENTS.md — plugins/web

## Key Invariants

- Playwright bridge scripts must be unique per `start()` call (no shared temp filename).
- `playwright_bridge.js` lazy-loads playwright inside browser startup, not at module load (keeps extraction helpers testable without Playwright installed).
- `Instant::checked_sub()` underflow in rate limiter → no-prune tick, not panic.
- Screenshot and extract tools must drop shared `PlaywrightBridge` after cancel/timeout (stale stdout line corrupts next exchange).
- Web content sanitization happens before tools return `AgentToolResult::text` — post-turn sanitizer is only an audit backstop.
