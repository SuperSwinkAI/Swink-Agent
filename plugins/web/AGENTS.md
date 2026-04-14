# AGENTS.md — plugins/web

## Lessons Learned

- `src/playwright.rs` must write a unique temp bridge script per `PlaywrightBridge::start()` call. Reusing one fixed temp filename is race-prone when multiple bridge instances start concurrently.
- `src/playwright_bridge.js` must lazy-load the `playwright` package inside browser startup instead of at module load time. That keeps the bridge's pure extraction helpers testable from plain Node without requiring Playwright to be installed for unit tests.
