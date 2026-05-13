# Current Bug Report

Generated on 2026-05-13 from the local `aleclamble/agiloop-cli` workspace.

## Validation Summary

Command run:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed in `scheduler-cli` smoke tests. Unit tests that ran before the smoke suite passed.

## Bugs

### 1. Custom provider runs can fail immediately with `Broken pipe`

- Severity: High
- Evidence: `cargo test --workspace` failed `cancel_terminates_active_provider_process`.
- Failing output: the `cancellable` run reached `failed` instead of `running`, with reason `provider error: provider command failed: Broken pipe (os error 32)`.
- Affected areas:
  - `crates/scheduler-provider/src/lib.rs`
  - `crates/scheduler-daemon/src/lib.rs`
  - `crates/scheduler-cli/tests/cli_smoke.rs`
- Suspected cause: `run_invocation_with_observer_and_cancellation` writes the full prompt to provider stdin before entering the cancellation/status polling loop. If the provider command exits early or does not read stdin, that write is treated as a hard provider error, so the run fails before cancellation can observe the process as running.
- User impact: custom providers that ignore stdin, exit before reading stdin, or close stdin early can make manual runs fail immediately. This also blocks reliable cancellation behavior for active provider processes.
- Reproduction:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test -p scheduler-cli --test cli_smoke cancel_terminates_active_provider_process
```

### 2. Daemon status reports `running: false` while heartbeat data is present

- Severity: High
- Evidence: `cargo test --workspace` failed both `daemon_start_status_and_stop_manage_background_process` and `scheduled_job_creation_starts_daemon`.
- Failing output: `daemon status` returned `running: false` and `pid: 0` even though `started_at`, `heartbeat_at`, and `last_tick_at` were populated.
- Affected areas:
  - `crates/scheduler-cli/src/main.rs`
  - `crates/scheduler-daemon/src/lib.rs`
  - `crates/scheduler-cli/tests/cli_smoke.rs`
- Suspected cause: daemon liveness is split between the PID file/process check in the CLI and heartbeat-derived status in `daemon_status_snapshot`. The stored heartbeat can indicate daemon activity, but the CLI status path can still emit `pid: 0` / `running: false`, likely when the background daemon exits quickly, the PID file is missing/stale, or the process check does not align with stored daemon state.
- User impact: `scheduler daemon status` can tell users the daemon is not running immediately after `daemon start` or after creating an enabled scheduled job, even while the database contains fresh daemon heartbeat/tick data.
- Reproduction:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test -p scheduler-cli --test cli_smoke daemon_start_status_and_stop_manage_background_process
```

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test -p scheduler-cli --test cli_smoke scheduled_job_creation_starts_daemon
```

### 3. Enabled scheduled job creation does not reliably leave the daemon observable as running

- Severity: High
- Evidence: `scheduled_job_creation_starts_daemon` failed after job creation. Status output had `running: false`, `pid: 0`, and a future `next_due_run` of `2099-01-01T00:00:00Z`.
- Affected areas:
  - `crates/scheduler-cli/src/main.rs`
  - `crates/scheduler-daemon/src/lib.rs`
  - `crates/scheduler-cli/tests/cli_smoke.rs`
- Suspected cause: `ensure_daemon_running_for_schedules` only calls `start_daemon_process` and prints the child PID when a scheduled enabled job exists. It does not verify that the spawned daemon remains alive and visible through `daemon status` after startup.
- User impact: creating or enabling a scheduled job can appear successful while leaving automation in a state where the daemon is not reported as running, so scheduled work may not execute.
- Reproduction:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test -p scheduler-cli --test cli_smoke scheduled_job_creation_starts_daemon
```

## Notes

- No `TODO`, `FIXME`, `BUG`, `todo!`, or `unimplemented!` markers were found under `crates`, `docs`, `README.md`, or `PRD.md`.
- The three bugs above are the currently confirmed bugs from the workspace validation run. The daemon-related failures may share one root cause, but they are listed separately because they affect distinct user workflows: explicit daemon lifecycle management and scheduled job creation.
