# Bug Report

Generated: 2026-05-13

This report lists the current bugs found by running the repository test suite and
checking for explicit in-repo bug markers. No `TODO`, `FIXME`, `BUG`, `todo!`, or
`unimplemented!` markers were present under `crates/`, `docs/`, or `README.md`
at the time of this audit.

## Audit Commands

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace --exclude scheduler-cli
grep -R "todo!\|unimplemented!\|FIXME\|TODO\|XXX" -n crates docs README.md
```

## Summary

| ID | Severity | Area | Status |
| --- | --- | --- | --- |
| BUG-001 | High | CLI provider execution cancellation | Reproducible test failure |
| BUG-002 | High | CLI daemon process management | Reproducible test failure |

The non-CLI workspace tests pass when `scheduler-cli` is excluded: 89 tests
passed across `scheduler-core`, `scheduler-daemon`, `scheduler-git`,
`scheduler-logs`, `scheduler-provider`, `scheduler-store`,
`scheduler-testkit`, and `scheduler-tui`.

## BUG-001: Manual Run With Custom Provider Fails Before Cancellation

**Severity:** High

**Affected test:** `cancel_terminates_active_provider_process` in
`crates/scheduler-cli/tests/cli_smoke.rs`

**Evidence:**

```text
thread 'cancel_terminates_active_provider_process' panicked at crates/scheduler-cli/tests/cli_smoke.rs:866:5:
Error: provider command failed: No such file or directory (os error 2)
```

**Expected behavior:**

A manually started job using a configured custom provider should enter
`running`, `scheduler cancel <run_id>` should terminate the active provider
process, the CLI child process should exit successfully, and the run should end
in `cancelled`.

**Actual behavior:**

The run path fails with `provider command failed: No such file or directory`
before the cancellation flow can complete.

**Impact:**

Active provider process cancellation is listed as implemented in
`docs/implementation-status.md`, but the CLI smoke test shows that the
end-to-end cancellation path is currently broken. Users may be unable to cancel
active custom-provider manual runs reliably.

**Initial investigation notes:**

The test configures a custom executable at a temporary path through
`scheduler provider add-custom sleeper <path>`, then starts `scheduler run
cancellable`. The failure indicates that the executable path used by the run
executor cannot be resolved or spawned by the time execution starts.

## BUG-002: `scheduler daemon start` Does Not Report A Running Background Daemon

**Severity:** High

**Affected test:** `daemon_start_status_and_stop_manage_background_process` in
`crates/scheduler-cli/tests/cli_smoke.rs`

**Evidence:**

```text
thread 'daemon_start_status_and_stop_manage_background_process' panicked at crates/scheduler-cli/tests/cli_smoke.rs:913:5:
daemon running state did not become `true`; last output: {
  "pid": 0,
  "running": false,
  "database_path": "/tmp/.tmpjZ5i5R/config/data/scheduler.sqlite3",
  "active_runs": 0,
  "next_due_run": null,
  "started_at": "2026-05-13T09:28:07.948075459Z",
  "heartbeat_at": "2026-05-13T09:28:12.007844878Z",
  "last_tick_at": "2026-05-13T09:28:12.018617170Z",
  "last_error": ""
}
```

**Expected behavior:**

After `scheduler daemon start`, `scheduler daemon status --json` should report
`running: true` with a non-zero daemon PID. `scheduler daemon stop` should then
terminate the daemon and status should return `running: false`.

**Actual behavior:**

The status output shows heartbeat and tick timestamps, but reports `pid: 0` and
`running: false`. The test never observes a running daemon within its five-second
polling window.

**Impact:**

Daemon lifecycle management is listed as implemented in
`docs/implementation-status.md`, but the CLI smoke test shows that `start` does
not produce the expected observable background-process state. Users may see a
daemon that appears stopped even while daemon metadata is being updated, and
`stop`/`restart` behavior may be unreliable when PID tracking fails.

**Initial investigation notes:**

The daemon status path appears to read heartbeat/tick metadata from SQLite while
the process-running check depends on a PID file. The mismatch suggests either
the background process is not persisting its PID correctly, the status command
cannot read the PID file it expects, or the daemon process exits quickly after
updating state.

## Passing Baseline Outside CLI Smoke Tests

The following validation passed:

```text
cargo test --workspace --exclude scheduler-cli
```

Result: all non-CLI unit and doc tests passed.

The full workspace validation currently fails only in `scheduler-cli` smoke
coverage because of BUG-001 and BUG-002.
