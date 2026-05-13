# Current Bug Report

Last audited: 2026-05-13

This report lists the bugs currently reproducible from the repository checkout.
It is based on local validation commands and direct source inspection of the
failing areas.

## Validation Summary

```bash
cargo fmt --all -- --check
```

Result: passed.

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace
```

Result: failed in `scheduler-cli` integration tests.

```bash
CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo clippy --workspace --all-targets -- -D warnings
```

Result: not run because the active Rust toolchain does not have
`cargo-clippy` installed.

## Open Bugs

### 1. Canceling an active custom-provider run fails to launch the provider command

- Severity: High
- Status: Reproducible
- Failing test: `cancel_terminates_active_provider_process`
- File: `crates/scheduler-cli/tests/cli_smoke.rs`
- Observed failure:

```text
Error: provider command failed: No such file or directory (os error 2)
```

The integration test creates a custom provider command in a temporary directory,
then creates and runs a job through that provider. The run does not reach the
active state because provider execution fails with `ENOENT`.

Impact: custom-provider jobs that depend on test-local or path-based executables
can fail before cancellation behavior is exercised. This also means the CLI may
not reliably execute a configured custom provider command in every supported
path shape.

Recommended investigation:

- Inspect custom provider persistence and invocation around
  `ProviderCommand::AddCustom` in `crates/scheduler-cli/src/main.rs`.
- Inspect command availability and invocation construction in
  `crates/scheduler-provider/src/lib.rs`.
- Confirm whether the stored command path is preserved as an absolute path and
  whether the process launcher resolves the executable consistently for
  cancellation-path runs.

### 2. Daemon start reports success, but status never observes the daemon process as running

- Severity: High
- Status: Reproducible
- Failing test: `daemon_start_status_and_stop_manage_background_process`
- File: `crates/scheduler-cli/tests/cli_smoke.rs`
- Observed failure:

```text
daemon running state did not become `true`
```

The last status payload included recent daemon heartbeat and tick timestamps, but
reported:

```json
{
  "pid": 0,
  "running": false
}
```

This indicates the daemon loop can update database-backed status fields while
the CLI status path cannot verify the PID from the PID file as a running
process.

Impact: `scheduler daemon start` can appear to succeed while
`scheduler daemon status` reports the daemon as offline. Users and automation
may incorrectly restart or stop the daemon, and daemon lifecycle tests remain
flaky or failing.

Recommended investigation:

- Inspect PID-file write/read logic around `start_daemon`,
  `daemon_status`, `read_daemon_pid`, and `is_process_running` in
  `crates/scheduler-cli/src/main.rs`.
- Check whether the daemon child exits early after writing heartbeat data.
- Review `daemon.log` in a reproduced test config directory for child-process
  startup errors.
- Consider making `daemon start` wait until the child process is observable or
  report a concrete startup failure when the child exits immediately.

### 3. `xml_escape` is compiled but unused

- Severity: Low
- Status: Reproducible warning
- File: `crates/scheduler-cli/src/main.rs`
- Observed warning:

```text
warning: function `xml_escape` is never used
```

The `scheduler-cli` binary emits a dead-code warning for `xml_escape`.

Impact: regular test runs pass warnings through today, but strict linting with
warnings denied will fail once `cargo-clippy` is available, and the unused code
adds maintenance noise around daemon service installation code.

Recommended investigation:

- Confirm whether platform-specific service installation paths still need
  `xml_escape`.
- If only needed on a gated platform, apply the matching `cfg` attribute.
- If obsolete, remove the function and any stale call assumptions.

## Notes

- No formatting issue was found by `cargo fmt --all -- --check`.
- The current workspace test failure is limited to two `scheduler-cli` smoke
  tests in this audit run: seven tests in `cli_smoke` passed before the two
  failures above stopped the workspace test run.
- This report captures reproducible bugs found through local validation. It does
  not claim that manual product exploration has exhausted every possible runtime
  defect.
