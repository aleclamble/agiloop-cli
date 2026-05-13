# Bug Report

Generated: 2026-05-13

Scope: current repository state for `aleclamble/agiloop-cli`.

Validation command:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed in `scheduler-cli` integration tests. The rest of the completed test targets passed before the integration test failure stopped the workspace run.

## Bugs

### 1. Cancelling an active custom provider run fails before the run can be cancelled

- Status: open
- Evidence: `cargo test --workspace` fails `cancel_terminates_active_provider_process`.
- Failing test: `crates/scheduler-cli/tests/cli_smoke.rs`
- Runtime area: provider execution and cancellation.
- Observed failure:

```text
Error: provider command failed: No such file or directory (os error 2)
```

Expected behavior: the custom provider run should enter `running`, `scheduler cancel <run_id>` should terminate the provider process group, the CLI child should exit successfully, the run should end in `cancelled`, and the provider marker file should not be created.

Impact: cancellation cannot be trusted for custom provider jobs when the provider process fails to spawn or resolve correctly from the run context. This undermines the CLI's ability to stop active work and may leave run state inconsistent with process state.

Likely investigation points:

- `crates/scheduler-provider/src/lib.rs` builds and spawns `ProviderInvocation` values with a `working_dir`.
- `crates/scheduler-daemon/src/lib.rs` prepares the run context, records process IDs, and transitions runs through `Preparing` and `Running`.
- `crates/scheduler-cli/tests/cli_smoke.rs` creates an executable temporary provider script and expects that exact command to be runnable during `scheduler run cancellable`.

### 2. `daemon start` reports success, but `daemon status` never reports the daemon as running

- Status: open
- Evidence: `cargo test --workspace` fails `daemon_start_status_and_stop_manage_background_process`.
- Failing test: `crates/scheduler-cli/tests/cli_smoke.rs`
- Runtime area: daemon start/status process tracking.
- Observed failure:

```text
daemon running state did not become `true`; last output:
{
  "pid": 0,
  "running": false,
  "database_path": ".../scheduler.sqlite3",
  "active_runs": 0,
  "next_due_run": null,
  "started_at": "2026-05-13T10:06:26.157928218Z",
  "heartbeat_at": "2026-05-13T10:06:30.222609845Z",
  "last_tick_at": "2026-05-13T10:06:30.230903845Z",
  "last_error": ""
}
```

Expected behavior: after `scheduler daemon start`, `scheduler daemon status --json` should report `running: true` and a non-zero `pid` while the daemon loop is alive. After `scheduler daemon stop`, status should report `running: false`.

Impact: daemon health is misreported. The persisted heartbeat and tick timestamps show daemon activity, but process-based status returns `pid: 0` and `running: false`, so operators and scripts may incorrectly conclude the daemon is down.

Likely investigation points:

- `crates/scheduler-cli/src/main.rs` writes and reads the daemon PID file with `write_daemon_pid`, `read_daemon_pid`, and `daemon_pid_path`.
- `daemon_status` trusts the PID file plus `is_process_running(pid)` to set `running`.
- `run_daemon_loop` also writes the PID file after the background child starts, so there may be a race or PID-file overwrite/visibility problem between `start_daemon`, the child process, and status checks.

## Additional Notes

- The workspace test run also reports a non-fatal warning: `xml_escape` is unused in `crates/scheduler-cli/src/main.rs`.
- No source fix is included in this report; this file documents the current reproducible bugs found by the repository's test suite.
