# Bug Report

Audit date: 2026-05-13

This report captures the current reproducible bugs found in the repository during a local test audit.

## Summary

| ID | Severity | Area | Status |
| --- | --- | --- | --- |
| BUG-001 | High | CLI run cancellation | Reproduced |
| BUG-002 | High | Daemon lifecycle | Reproduced |

## BUG-001: Cancelling an active provider run can fail with `No such file or directory`

Severity: High

Affected test: `crates/scheduler-cli/tests/cli_smoke.rs::cancel_terminates_active_provider_process`

Observed behavior:

Running the workspace test suite fails while cancelling an active custom-provider run:

```text
thread 'cancel_terminates_active_provider_process' panicked at crates/scheduler-cli/tests/cli_smoke.rs:866:5:
Error: provider command failed: No such file or directory (os error 2)
```

Expected behavior:

The `scheduler cancel <run_id>` command should terminate the registered provider process group, allow the foreground `scheduler run cancellable` process to exit, and transition the run to `cancelled`.

Evidence:

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed with 7 passing CLI smoke tests and this cancellation failure.

Likely affected code:

- `crates/scheduler-cli/src/main.rs`, `cancel_command`
- `crates/scheduler-cli/src/main.rs`, `terminate_registered_process`
- `crates/scheduler-provider/src/lib.rs`, `terminate_process_group`
- `crates/scheduler-daemon/src/lib.rs`, run process registration through `set_run_process`

Impact:

Operators may be unable to reliably stop a stuck or long-running provider process. This also weakens the documented cancellation behavior for active provider runs.

## BUG-002: `daemon start` records heartbeat state but status never reports `running: true`

Severity: High

Affected test: `crates/scheduler-cli/tests/cli_smoke.rs::daemon_start_status_and_stop_manage_background_process`

Observed behavior:

After `scheduler daemon start`, repeated `scheduler --json daemon status` checks never report `running: true`. The last observed status includes fresh daemon timestamps but a zero PID and `running: false`:

```json
{
  "pid": 0,
  "running": false,
  "database_path": "/tmp/.tmpLRwcn4/config/data/scheduler.sqlite3",
  "active_runs": 0,
  "next_due_run": null,
  "started_at": "2026-05-13T01:35:28.757670708Z",
  "heartbeat_at": "2026-05-13T01:35:32.813946502Z",
  "last_tick_at": "2026-05-13T01:35:32.822698794Z",
  "last_error": ""
}
```

Expected behavior:

After `daemon start`, status should report the live daemon PID and `running: true` until `daemon stop` terminates it.

Evidence:

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed with 7 passing CLI smoke tests and this daemon lifecycle failure.

Likely affected code:

- `crates/scheduler-cli/src/main.rs`, `start_daemon`
- `crates/scheduler-cli/src/main.rs`, `daemon_status`
- `crates/scheduler-daemon/src/lib.rs`, `daemon_status_snapshot`
- PID file and `daemon.pid` setting synchronization

Impact:

Automation and users cannot trust `daemon status` after startup. This can break service health checks, restart logic, and scripts that wait for the daemon to become available.

## Validation Notes

Command run:

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Overall result:

- `scheduler-cli` integration tests: 7 passed, 2 failed.
- Failures: `cancel_terminates_active_provider_process`, `daemon_start_status_and_stop_manage_background_process`.
- Non-fatal warning: `xml_escape` is unused in `crates/scheduler-cli/src/main.rs`.

