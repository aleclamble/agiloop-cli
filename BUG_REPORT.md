# Bug Report

Generated: 2026-05-12

This report lists the current bugs found during a local repository audit of
`aleclamble/agiloop-cli`. The audit used `cargo test --workspace` with writable
Cargo paths in the Escalate workspace.

## Validation Summary

Command:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed.

- 7 `scheduler-cli` smoke tests passed.
- 2 `scheduler-cli` smoke tests failed.
- A compiler warning was also emitted for an unused helper in
  `crates/scheduler-cli/src/main.rs`.

## Current Bugs

### 1. Cancelling an active provider-backed run fails before the provider starts

- Severity: High
- Failing test: `cancel_terminates_active_provider_process`
- Location: `crates/scheduler-cli/tests/cli_smoke.rs`
- Observed failure:

```text
Error: provider command failed: No such file or directory (os error 2)
```

Expected behavior: a custom provider-backed job should reach `running`, accept a
cancel request, terminate the active provider process, and transition the run to
`cancelled`.

Actual behavior: the run fails while trying to launch the provider command, so
the cancellation path is never exercised.

Likely area to inspect:

- `crates/scheduler-provider/src/lib.rs`, especially
  `run_invocation_with_observer`, which constructs and spawns the provider
  command.
- `crates/scheduler-daemon/src/lib.rs`, where daemon execution reconstructs and
  runs provider-backed jobs.
- `crates/scheduler-cli/tests/cli_smoke.rs`, where the test creates a temporary
  executable provider and expects that exact path to remain usable by the
  daemon execution process.

### 2. Background daemon start reports success, but status never becomes running

- Severity: High
- Failing test: `daemon_start_status_and_stop_manage_background_process`
- Location: `crates/scheduler-cli/tests/cli_smoke.rs`
- Observed failure:

```text
daemon running state did not become `true`
```

The last status payload showed a stored daemon heartbeat and start time, but
reported:

```json
{
  "pid": 0,
  "running": false,
  "active_runs": 0,
  "next_due_run": null,
  "last_error": ""
}
```

Expected behavior: after `scheduler daemon start`, `scheduler daemon status`
should report `running: true` with a live daemon process until `scheduler daemon
stop` is called.

Actual behavior: startup writes daemon metadata, but the subsequent status
polling never observes a live daemon process.

Likely area to inspect:

- `crates/scheduler-cli/src/main.rs`, especially `start_daemon`,
  `daemon_status_snapshot`, and pid file handling.
- Background process lifecycle around `std::env::current_exe()` and process
  detachment in `start_daemon`.
- Daemon metadata consistency between the pid file and the persisted
  `daemon.pid` setting.

## Non-Blocking Finding

### Unused `xml_escape` helper warning

- Severity: Low
- Location: `crates/scheduler-cli/src/main.rs`
- Warning:

```text
function `xml_escape` is never used
```

This does not fail the test suite, but it should be resolved by either removing
the helper or using it from the code path that formats XML-like output.

## Notes

- No fixes are included in this report.
- The report is based on the current local checkout and the failing tests above.
- The full workspace test command should be rerun after fixes to confirm both
  smoke-test failures are resolved.
