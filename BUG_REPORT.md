# Bug Report

Generated: 2026-05-13

## Scope

This report covers bugs currently observable in the `aleclamble/agiloop-cli` repository checkout. The findings are based on repository inspection and a full workspace test run.

## Validation Run

Command:

```sh
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed. The workspace compiled, but `scheduler-cli` smoke tests reported two failures:

- `cancel_terminates_active_provider_process`
- `daemon_start_status_and_stop_manage_background_process`

`cargo clippy --workspace --all-targets -- -D warnings` could not be run because the active Rust toolchain does not have `cargo-clippy` installed.

## Bugs

### 1. Custom provider commands are invoked relative to the job working directory

- Severity: High
- Status: Reproduced by test failure
- Affected test: `crates/scheduler-cli/tests/cli_smoke.rs:115`
- Relevant implementation: `crates/scheduler-provider/src/lib.rs:438` and `crates/scheduler-provider/src/lib.rs:449`

The `cancel_terminates_active_provider_process` smoke test registers a custom provider using a path under a temporary directory, then runs a job whose working directory is also changed before provider execution. The run fails before cancellation can be tested:

```text
Error: provider command failed: No such file or directory (os error 2)
```

The provider invocation sets `current_dir` to the job working directory before spawning the provider process. If a custom provider command is stored as a relative path, that path is resolved from the job working directory instead of the CLI/config context where it was registered. This makes valid custom provider registrations fail once the job runs somewhere else.

Expected behavior: a custom provider command registered by path should resolve consistently at execution time, independent of the job working directory. The simplest durable fix is likely to canonicalize or otherwise persist absolute command paths when custom providers are registered, while preserving `PATH` lookup for bare command names.

Impact: custom provider jobs can fail with `No such file or directory` even though the provider was successfully registered and detected.

### 2. Daemon status can report `running: false` while the daemon is heartbeating

- Severity: Medium
- Status: Reproduced by test failure
- Affected test: `crates/scheduler-cli/tests/cli_smoke.rs:42`
- Relevant implementation: `crates/scheduler-cli/src/main.rs:1691`, `crates/scheduler-cli/src/main.rs:1805`, and `crates/scheduler-daemon/src/lib.rs:92`

The `daemon_start_status_and_stop_manage_background_process` smoke test starts the daemon, then waits for `daemon status` to report `running: true`. The status command timed out with the last observed output showing a live heartbeat and tick timestamp but `running: false` and `pid: 0`:

```json
{
  "pid": 0,
  "running": false,
  "active_runs": 0,
  "heartbeat_at": "2026-05-13T13:18:40.064003959Z",
  "last_tick_at": "2026-05-13T13:18:40.070906917Z",
  "last_error": ""
}
```

There is an inconsistency between persisted daemon liveness and PID-file based process detection. `daemon_status_snapshot` treats a recorded heartbeat as running, but the CLI status command overrides that result with `running: false` whenever `read_daemon_pid` returns no valid process. Because the daemon is actively updating heartbeat fields, the PID file appears to be missing, stale, unreadable, or not stable in the test environment.

Expected behavior: after `daemon start` returns successfully, `daemon status` should consistently report the spawned daemon as running while the heartbeat is current and the process is alive. The status path should avoid overriding a fresh heartbeat to `false` solely because PID-file detection fails, or it should make PID-file creation/retention reliable enough that the override is trustworthy.

Impact: operators and automation can see a false negative daemon state, which can lead to duplicate starts, failed health checks, or incorrect operational decisions.

## Additional Notes

- The workspace also emits a warning for unused `xml_escape` in `crates/scheduler-cli/src/main.rs:2142` during tests. This is not classified as a runtime bug in this report, but it would become a blocking issue if CI treats warnings as errors.
- No code fixes were made as part of this report; this file is intended to document the current reproducible bugs and point to the most relevant code paths.
