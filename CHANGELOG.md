# Changelog

This project follows semantic versioning.

## Unreleased

- Initial scheduler workspace with CLI, core job spec, provider, store, daemon,
  Git, logs, TUI foundation, and testkit crates.
- SQLite-backed providers, jobs, runs, artifacts, audit events, locks, queues,
  notification deliveries, and process metadata.
- Provider detection for Codex, Claude Code, OpenCode, and custom command
  providers.
- Provider-backed plain-English job spec generation with repair attempts.
- Manual execution with run context, logs, summaries, artifact indexing,
  timeouts, cancellation, restart recovery, notifications, and worktree cleanup.
- CLI flows for setup, providers, create/edit/list/show/export/import,
  enable/disable/delete, run/cancel/retry/runs/logs/artifacts, daemon status and
  tick, config/data/db/backup/cleanup, and shell completions.

Compatibility notes:

- Job spec schema: `scheduler.job.v1`.
- Config export schema: `scheduler.config.v1`.
- SQLite migrations are currently single-version and create missing tables on
  startup.
- macOS release notarization is not implemented yet.
