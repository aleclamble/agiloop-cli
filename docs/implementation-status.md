# Implementation Status

This file maps `PRD.md` to the current implementation state. It is not a replacement for the PRD.

Last audited: 2026-05-12

## Verified Commands

These commands passed locally:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

## Implemented Or Substantially Implemented

- A001 Cargo workspace and crate layout.
- A002 formatting/lint/test configuration through Cargo and CI.
- A003 root README with summary and development commands.
- A004 CI workflow for format, clippy, test, and build.
- B001 job spec v1 Rust types.
- B002 schedule types for cron, interval, once, and manual.
- B003 concurrency enum and decision function.
- B004 repo lock enum model.
- B005 semantic validation for job specs.
- B006 JSON schema generation for job spec v1.
- C001 SQLite migration runner with recorded additive migration versions.
- C003 job repository with create/read/list/enable/disable/soft delete/version history.
- C004 run repository with transactional state transitions.
- C005 queue and lock persistence with queued-run lookup/start primitives and daemon lock acquisition.
- C006 audit event persistence for core job/provider/run mutations.
- C002 required state tables for settings, provider probes, schedule projections, run logs,
  notification delivery rows, and core queue/lock metadata.
- D001 provider adapter trait and capability model.
- D002 built-in provider detection framework.
- D003 Codex provider adapter builds provider-specific spec-builder and execution invocations.
- D004 Claude Code provider adapter builds provider-specific spec-builder and execution invocations.
- D005 OpenCode provider adapter builds provider-specific spec-builder and execution invocations.
- D006 custom command provider foundation.
- D007 provider test command for built-in detections.
- D008 provider config persistence.
- E001 spec-builder prompt template.
- E002 spec-builder invocation through configured provider command.
- E003 spec-builder output envelope parser.
- E004 validation after provider response.
- E005 repair prompt loop for invalid spec-builder JSON.
- E006 create flow prints a confirmation summary with provider envelope details and normalized job settings before saving.
- F001 Git repository discovery and canonicalization.
- F002 base ref resolution to commit SHA with optional fetch when remotes exist.
- F003 branch template rendering.
- F004 worktree creation helper.
- F005 worktree cleanup according to retention policy.
- F006 worktree failure handling before provider launch, including missing base ref coverage.
- G001 run context directory creation.
- G002 provider process launch and stdout/stderr capture.
- G003 scheduler environment variable injection into provider process.
- G004 execution prompt generation.
- G005 timeout handling.
- G006 active provider process cancellation with persisted process-group metadata.
- G007 run summary ingestion.
- G008 artifact discovery and store-backed artifact indexing.
- H001 daemon lock primitive over SQLite locks.
- H002 schedule evaluation with deterministic scheduler tick and misfire handling.
- H003 due-run creation primitive.
- H004 same-job concurrency policy application for `skip`, `queue`, `parallel`, and `replace`.
- H005 repo lock enforcement for `repo_lock = exclusive`.
- H006 restart recovery marks interrupted active runs as `lost` and can emit failure notifications.
- H007 daemon status reports PID/running state, heartbeat, last tick, active runs, next due run, and last error.
- H008 retention cleanup primitive for run worktrees.
- I001 command parser and global flags.
- I002 setup and provider commands.
- I003 file-backed and provider-backed job create flow.
- I004 job list/show/edit/export/import with version-preserving updates.
- I005 enable/disable/delete with confirmation for delete.
- I006 run/runs/logs/artifacts/cancel/retry command surface, with logs printing or following run logs and cancel terminating active provider processes.
- I007 daemon status/tick/start/stop/restart/install/uninstall commands, with start launching a background scheduler loop, stop terminating it by PID, restart replacing it, and install/uninstall writing platform service files.
- I008 doctor/config/data/db commands.
- J001 Ratatui interactive shell/navigation with dashboard, jobs, runs, logs, providers, settings, and help views.
- J002 Ratatui dashboard renderer with test backend.
- J003 jobs list with selectable rows, next/last run context, text filtering, and sortable columns.
- J004 TUI job/run operational actions for enable/disable, run now, cancel, and retry.
- J005 TUI provider-backed create wizard for provider, repo, task, and timezone input.
- J006 runs list and run detail views.
- J007 logs viewer for indexed run logs.
- J008 provider setup/status screen for configured providers and capabilities.
- J009 settings and help screens.
- K001 notification sink trait and event model.
- K002 best-effort local notification adapter.
- K003 webhook notification adapter with JSON POST, timeout, failure logging, and secret redaction.
- K004 run success/failure/timeout/cancel/skipped and provider-unavailable notifications wired through daemon lifecycle helpers.
- L001 job export JSON/TOML.
- L002 import conflict handling for reject, replace, and rename.
- L003 full config export as JSON/TOML with recursive secret redaction.
- L004 database-only backup compatibility plus full directory backup/restore for database, run data, logs, and artifacts.
- L005 cleanup dry-run/execution for retained worktrees.
- M001 release workflow builds macOS and Linux archives and verifies `scheduler --version`.
- M002 shell completions generated for bash, zsh, and fish.
- M003 install script plus documented local install path.
- M004 optional macOS codesign/notarization release workflow steps gated by repository secrets.
- M005 changelog and semantic versioning policy.
- N001 README.
- N002 job spec reference.
- N003 provider guide.
- N004 troubleshooting guide.
- N005 example job spec fixture.

## Partially Implemented

None tracked in this status file.

## Not Yet Implemented

None tracked in this status file.

## Current Test Evidence

- Core unit tests cover defaults, schedule validation, branch templates, run transitions, schema generation, and semantic validation.
- Provider unit tests cover detection, prompt generation, envelope parsing, Codex/Claude/OpenCode invocation construction, custom invocation construction, process-spawn observation, stdin execution, and timeouts.
- Store unit tests cover job CRUD/edit, provider persistence and probes, schedule projections, run transitions, active run queries, process metadata, settings, run log metadata, restored log path rewriting, additive migrations, schema table coverage, integrity checks, locks, artifacts, and audit events.
- Daemon unit tests cover schedule tick/misfire behavior, queued-run start, existing scheduled-run execution, restart recovery, manual execution/log capture and log metadata, environment injection, summary ingestion, artifact indexing, repo locks, retention cleanup, daemon locking, status snapshots, notifications, and all four same-job concurrency policies.
- Git unit tests cover repo detection, non-repo rejection, base ref resolution, missing base refs, and fetch no-op behavior without remotes.
- TUI unit tests render the dashboard and multi-view app screens with Ratatui `TestBackend`.
- CLI integration tests cover setup provider probe recording, custom provider setup, shell completion generation, redacted config export, job edit/import conflict handling, active provider cancellation, manual execution, run history, logs and log following, artifacts, daemon status/tick/start/stop, non-interactive TUI rendering, database check, database-only and full backup restore flows, provider-backed spec generation, and spec-builder repair after invalid output.
