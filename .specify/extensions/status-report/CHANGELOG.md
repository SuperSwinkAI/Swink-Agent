# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.1.4] - 2026-03-20

### Changed

- Renamed extension ID from `status` to `status-report` to avoid collision with existing community extension
- Renamed extension from "Project Status" to "Status Report"
- Updated script paths in command spec to match new extension ID

## [1.1.3] - 2026-03-16

### Fixed

- Fix specs directory lookup order to prefer `specs/` over `.specify/specs/`

## [1.1.2] - 2026-03-15

### Fixed

- Removed incorrect "Read-Only Operation" claim from command spec — the command writes/updates `spec-status.md` as designed

## [1.1.1] - 2026-03-15

### Fixed

- Bash script compatibility with macOS bash 3.2 — replaced `declare -A` associative arrays with indexed arrays
- Replaced `grep -oP` (Perl regex) with portable `sed` for cache field extraction
- Fixed `grep -c` exit code 1 on zero matches causing doubled output in task counting
- Script path resolution after extension installation — frontmatter now uses full `.specify/extensions/status/scripts/...` paths
- Replaced `{SCRIPT}` placeholder in command body with reference to frontmatter scripts

## [1.1.0] - 2026-03-03

### Added

- Cache file (`{SPECS_DIR}/spec-status.md`) — human-readable markdown summary written by the scripts after each run and committed to git
- Git-based staleness detection — only feature folders changed since the cache was last committed are rescanned; unchanged features are served from cache
- Task counting in scripts — `tasks_total` and `tasks_completed` are now computed by the scripts and included in JSON output, eliminating the need for the AI to read individual `tasks.md` files
- `from_cache` field in JSON output per feature — indicates whether data came from cache or a fresh scan
- `cache_file` field in JSON output — path to the written cache file

### Changed

- `commands/status.md` — updated to use pre-computed task counts from script JSON output instead of counting lines from `tasks.md`

## [1.0.0] - 2026-02-27

### Added

- `/speckit.status.report` command — display project status, feature progress, and recommended next actions
- Support for `--all`, `--verbose`, `--json`, and `--feature` flags
- Bash discovery script (`scripts/bash/get-project-status.sh`)
- PowerShell discovery script (`scripts/powershell/Get-ProjectStatus.ps1`)
- Pipeline view showing all features with workflow stages (Specify → Plan → Tasks → Implement)
- Artifact status for the current/selected feature
- Task completion tracking for features in implementation
- Next action recommendations based on current state
- JSON output format for machine-readable integration

[1.1.4]: https://github.com/Open-Agent-Tools/spec-kit-status/releases/tag/v1.1.4
[1.1.3]: https://github.com/Open-Agent-Tools/spec-kit-status/releases/tag/v1.1.3
[1.1.2]: https://github.com/Open-Agent-Tools/spec-kit-status/releases/tag/v1.1.2
[1.1.1]: https://github.com/Open-Agent-Tools/spec-kit-status/releases/tag/v1.1.1
[1.1.0]: https://github.com/Open-Agent-Tools/spec-kit-status/releases/tag/v1.1.0
[1.0.0]: https://github.com/Open-Agent-Tools/spec-kit-status/releases/tag/v1.0.0
