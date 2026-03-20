# Security Policy

## Supported Versions

Only the latest release on `main` is actively supported with security fixes.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

To report a vulnerability, open a [GitHub Security Advisory](https://github.com/SuperSwinkAI/Swink-Agent/security/advisories/new). This keeps the report private while it is investigated.

Include as much of the following as possible:

- Type of issue (e.g., credential leak, unsafe deserialization, dependency vulnerability)
- File paths and line numbers relevant to the issue
- Steps to reproduce or proof-of-concept
- Potential impact and attack scenario

You can expect an acknowledgment within **48 hours** and a resolution timeline within **7 days** for critical issues.

## Scope

Security concerns most relevant to this project:

- **API key handling** — keys are read from environment variables and never logged or serialized
- **Tool execution** — `BashTool` and file tools execute arbitrary commands; approval policies should be configured appropriately in production
- **Dependency vulnerabilities** — tracked via `cargo deny` (see `deny.toml`) and audited in CI

## Dependency Audits

This project uses [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) with a strict advisory policy. All known vulnerabilities in the dependency tree are blocked in CI. If you discover a vulnerable dependency not yet caught by the audit, please report it as described above.
