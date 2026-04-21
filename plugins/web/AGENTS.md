# AGENTS.md — plugins/web

## Lessons Learned

- `src/playwright.rs` must write a unique temp bridge script per `PlaywrightBridge::start()` call. Reusing one fixed temp filename is race-prone when multiple bridge instances start concurrently.
- `src/playwright_bridge.js` must lazy-load the `playwright` package inside browser startup instead of at module load time. That keeps the bridge's pure extraction helpers testable from plain Node without requiring Playwright to be installed for unit tests.
- `src/policy/rate_limiter.rs` must treat `Instant::checked_sub()` underflow as a no-prune tick instead of unwrapping. Short-uptime hosts can otherwise panic in the live rate-limit path.
- `src/tools/screenshot.rs` must drop the shared `PlaywrightBridge` after a screenshot request is cancelled or times out. Once the JSON request is written, abandoning the response read can leave a stale line on stdout that corrupts the next bridge exchange.
