# Bug Report

Generated: 2026-05-12

This report captures the current confirmed bugs found by running the workspace test suite and inspecting the implicated code paths. It is intentionally limited to reproducible failures in the current checkout.

## Summary

| ID | Severity | Area | Status |
| --- | --- | --- | --- |
| BUG-001 | High | CLI run cancellation | Confirmed by failing test |
| BUG-002 | High | Daemon lifecycle status | Confirmed by failing test |

## BUG-001: Cancelling an active custom-provider run fails with `No such file or directory`

### Impact

Users may be unable to cancel a running job backed by a custom provider. The active run command can fail with a provider command error before cancellation completes, leaving the operator without reliable cancellation behavior.

### Evidence

Command:

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Failure:

```text
cancel_terminates_active_provider_process ... FAILED

thread 'cancel_terminates_active_provider_process' panicked at crates/scheduler-cli/tests/cli_smoke.rs:866:5:
Error: provider command failed: No such file or directory (os error 2)
```

Relevant files:

- `crates/scheduler-cli/tests/cli_smoke.rs`
- `crates/scheduler-cli/src/main.rs`
- `crates/scheduler-daemon/src/lib.rs`
- `crates/scheduler-provider/src/lib.rs`

### Notes

The failing smoke test registers a temporary executable custom provider, starts `scheduler run cancellable`, waits for the run to reach `running`, then invokes `scheduler cancel <run_id>`. The provider invocation path creates a process group and records the process id so the cancel command can terminate it. The observed `No such file or directory` error comes from the provider command execution path rather than a clean cancellation transition.

### Expected Behavior

The cancel command should terminate the active provider process group, the run should end in `cancelled`, and the provider script should not complete its post-loop marker write.

## BUG-002: Started daemon is reported as not running even while heartbeat metadata is present

### Impact

The CLI reports the daemon as stopped after `scheduler daemon start`, even though daemon metadata such as `started_at`, `heartbeat_at`, and `last_tick_at` is being written. This can break status checks, scripts, health monitoring, and operator confidence in background scheduling.

### Evidence

Command:

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Failure:

```text
daemon_start_status_and_stop_manage_background_process ... FAILED

daemon running state did not become `true`; last output: {
  "pid": 0,
  "running": false,
  "database_path": "/tmp/.tmp7MMt5o/config/data/scheduler.sqlite3",
  "active_runs": 0,
  "next_due_run": null,
  "started_at": "2026-05-12T23:52:44.777370966Z",
  "heartbeat_at": "2026-05-12T23:52:48.830857177Z",
  "last_tick_at": "2026-05-12T23:52:48.839567093Z",
  "last_error": ""
}
```

Relevant files:

- `crates/scheduler-cli/tests/cli_smoke.rs`
- `crates/scheduler-cli/src/main.rs`
- `crates/scheduler-daemon/src/lib.rs`

### Notes

The status output shows fresh daemon heartbeat and tick timestamps, but `pid` is `0` and `running` is `false`. The CLI status path appears to be unable to correlate the live daemon metadata with a valid running pid, or the pid file/process check is becoming invalid while the daemon loop still updates the store.

### Expected Behavior

After `scheduler daemon start`, `scheduler --json daemon status` should report `running: true` with a non-zero pid until the daemon is stopped.

## Additional Observation

The test run also emits a warning:

```text
warning: function `xml_escape` is never used
```

This is not classified as a functional bug because it does not currently fail the workspace test suite, but it will become build-breaking if CI runs clippy or compiler warnings as errors.

## Validation

The workspace test suite was run on 2026-05-12 with writable Cargo directories:

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed with 7 passing CLI smoke tests and 2 failing CLI smoke tests:

- `cancel_terminates_active_provider_process`
- `daemon_start_status_and_stop_manage_background_process`
