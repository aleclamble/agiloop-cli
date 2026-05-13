# Bug Report

Generated on 2026-05-13 from local validation of the current repository state.

## Summary

The repository currently has three CI-relevant defects:

1. `cargo test --workspace` fails in `scheduler-cli` smoke tests.
2. `cargo clippy --workspace --all-targets -- -D warnings` is expected to fail because `scheduler-cli` has a dead-code warning.
3. The local validation environment cannot run Clippy because the `clippy` component is not installed for the active Rust toolchain.

## Current Bugs

### 1. `cancel_terminates_active_provider_process` cannot start the custom provider

- **Location:** `crates/scheduler-cli/tests/cli_smoke.rs:115`
- **Validation command:** `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace`
- **Observed failure:**

```text
test cancel_terminates_active_provider_process ... FAILED
thread 'cancel_terminates_active_provider_process' panicked at crates/scheduler-cli/tests/cli_smoke.rs:866:5:
Error: provider command failed: No such file or directory (os error 2)
```

- **Impact:** The workspace test suite exits with status 101, so the CI `Test` job cannot pass.
- **Notes:** The test registers a temporary `provider.sh`, creates a job using provider id `sleeper`, then runs the job and expects the provider process to be cancellable. The runtime instead reports that the provider command cannot be found.

### 2. `daemon_start_status_and_stop_manage_background_process` reports a non-running daemon with heartbeat timestamps

- **Location:** `crates/scheduler-cli/tests/cli_smoke.rs:30`
- **Validation command:** `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace`
- **Observed failure:**

```text
test daemon_start_status_and_stop_manage_background_process ... FAILED
thread 'daemon_start_status_and_stop_manage_background_process' panicked at crates/scheduler-cli/tests/cli_smoke.rs:913:5:
daemon running state did not become `true`; last output: {
  "pid": 0,
  "running": false,
  "active_runs": 0,
  "next_due_run": null,
  "last_error": ""
}
```

- **Impact:** The workspace test suite exits with status 101, so the CI `Test` job cannot pass.
- **Notes:** The status output includes `started_at`, `heartbeat_at`, and `last_tick_at`, but `pid` remains `0` and `running` remains `false`. This points to daemon start/status bookkeeping rather than an assertion-only issue.

### 3. `scheduler-cli` has a dead-code warning that is promoted to an error by CI Clippy

- **Location:** `crates/scheduler-cli/src/main.rs:2142`
- **Validation command:** `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo check --workspace`
- **Observed warning:**

```text
warning: function `xml_escape` is never used
    --> crates/scheduler-cli/src/main.rs:2142:4
     |
2142 | fn xml_escape(value: &str) -> String {
     |    ^^^^^^^^^^
```

- **Impact:** CI runs `cargo clippy --workspace --all-targets -- -D warnings`, so this warning is expected to fail the CI `Clippy` job.
- **Notes:** The function should either be used by the XML-emitting path or removed if no longer needed.

## Validation Evidence

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo check --workspace` | Passed with one `dead_code` warning |
| `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo test --workspace` | Failed: 7 passed, 2 failed in `scheduler-cli` smoke tests |
| `CARGO_HOME="$PWD/.escalate-cargo-home" CARGO_TARGET_DIR="$PWD/target" cargo clippy --workspace --all-targets -- -D warnings` | Not runnable locally: `cargo-clippy` is not installed for toolchain `1.91.0-aarch64-unknown-linux-gnu` |

## Recommended Fix Order

1. Fix provider command resolution for custom providers used by job execution.
2. Fix daemon start/status persistence so a successfully started daemon records a real PID and reports `running: true`.
3. Remove or wire up `xml_escape`, then rerun Clippy in an environment with the Clippy component installed.
