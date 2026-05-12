# Bug Report

Generated: 2026-05-12

This report lists the current bugs found by inspecting the repository and running the workspace validation commands. The Rust workspace compiles, but the full test suite currently fails in the CLI smoke tests.

## Validation Summary

| Command | Result | Notes |
| --- | --- | --- |
| `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo check --workspace` | Passes with warnings | `scheduler-cli` reports one dead-code warning for `xml_escape` in `crates/scheduler-cli/src/main.rs`. |
| `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace` | Fails | `crates/scheduler-cli/tests/cli_smoke.rs` has 2 failing tests and 7 passing tests before the suite stops. |

## Current Bugs

### 1. Custom provider run cancellation fails before the provider process starts

- **Status:** Open
- **Severity:** High
- **Area:** CLI provider execution and cancellation
- **Failing test:** `cancel_terminates_active_provider_process` in `crates/scheduler-cli/tests/cli_smoke.rs`
- **Observed failure:**

```text
Error: provider command failed: No such file or directory (os error 2)
```

- **Expected behavior:** A custom provider added with an executable path should start successfully, the run should enter `running`, `scheduler cancel <run_id>` should terminate the active provider process, and the run should end in `cancelled`.
- **Actual behavior:** The run command exits with a provider command lookup failure before cancellation can complete.
- **Evidence:** The failing test registers a custom provider from an absolute temp-file path through `scheduler provider add-custom sleeper <provider-path>`, creates a job that uses provider `sleeper`, then starts `scheduler run cancellable`. The command fails with `No such file or directory`.
- **Likely impact:** Users who configure custom providers can create jobs that cannot be executed or cancelled, even when the provider executable exists and is executable.
- **Likely investigation path:** Check persistence and retrieval of `ProviderConfig.command` across `provider add-custom`, `Store::upsert_provider`, and provider invocation. The failing path suggests the stored command is not being resolved back to the intended executable when `RunExecutor` launches the provider.

### 2. `scheduler daemon start` returns success while the daemon is not running

- **Status:** Open
- **Severity:** High
- **Area:** Daemon lifecycle
- **Failing test:** `daemon_start_status_and_stop_manage_background_process` in `crates/scheduler-cli/tests/cli_smoke.rs`
- **Observed failure:**

```json
{
  "pid": 0,
  "running": false,
  "database_path": "/tmp/.tmpddIpBz/config/data/scheduler.sqlite3",
  "active_runs": 0,
  "next_due_run": null,
  "started_at": "2026-05-12T20:50:55.041608833Z",
  "heartbeat_at": "2026-05-12T20:50:59.128637377Z",
  "last_tick_at": "2026-05-12T20:50:59.140029460Z",
  "last_error": ""
}
```

- **Expected behavior:** After `scheduler daemon start`, `scheduler --json daemon status` should report `running: true` with a non-zero daemon PID until `scheduler daemon stop` is called.
- **Actual behavior:** The start command succeeds, the status record has recent heartbeat data, but status reports `running: false` and `pid: 0`.
- **Evidence:** `start_daemon` spawns `current_exe() --config <dir> daemon run` and writes the child PID to both the pid file and the store. The status path reports the daemon as not running while database heartbeat fields continue to update.
- **Likely impact:** Users and automation cannot reliably tell whether the background daemon is alive. Stop/restart behavior may also be unreliable when status and PID tracking disagree.
- **Likely investigation path:** Inspect `daemon_status` and its interaction with `daemon_status_snapshot`, `read_daemon_pid`, and `is_process_running`. The snapshot can report `running: true` from heartbeat data, but the CLI status path appears to override or clear that state when PID validation fails.

## Non-Failing Issue

### 3. Unused `xml_escape` helper

- **Status:** Open
- **Severity:** Low
- **Area:** CLI code hygiene
- **Location:** `crates/scheduler-cli/src/main.rs`
- **Observed warning:**

```text
warning: function `xml_escape` is never used
```

- **Expected behavior:** The workspace should build without dead-code warnings, or unused helpers should be removed.
- **Actual behavior:** `cargo check --workspace` succeeds but emits the warning.
- **Likely impact:** Low runtime impact, but it adds noise to validation output and can hide more important warnings over time.

