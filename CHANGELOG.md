# Changelog

All notable changes to OpenAudit will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Phase 0: ORIENTATION.md describing the inherited Claude Code (Rust `claw` CLI) architecture.
- docs/OPENAUDIT.md feature-and-vision document.
- docs/superpowers/plans/2026-05-04-openaudit-mvp.md phased execution plan.
- Direct Moonshot routing for `kimi-2.6` (`MOONSHOT_API_KEY`, `MOONSHOT_BASE_URL`, `https://api.moonshot.ai/v1`).
- `kimi-dashscope` model alias for the legacy DashScope-hosted Kimi proxy.
- Phase 1 sub-plan at `docs/superpowers/plans/2026-05-04-phase-1-rebrand-and-providers.md`.

### Changed
- Binary renamed `claw` → `openaudit`. The `claw` name is retained as a deprecation shim that delegates to `openaudit` for one release.
- Per-workspace session directory moved from `<cwd>/.claw/sessions/` to `<cwd>/.openaudit/sessions/`. A one-shot copy migration runs on first use; the legacy directory is left in place. Unpartitioned legacy session files at `<cwd>/.claw/sessions/<id>.jsonl` continue to resolve via a fallback path.
