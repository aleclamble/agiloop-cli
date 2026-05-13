# Bug Report

Generated on 2026-05-13 from a fresh-base audit of `aleclamble/agiloop-cli`.

## Validation status

- `cargo test --workspace` builds successfully, then fails in the baseline `scheduler-cli` smoke suite.
- Failure classification: baseline. The failures reproduce before this report file is added, so they were not introduced by the documentation change.
- Failing tests:
  - `cancel_terminates_active_provider_process`: provider command fails with `No such file or directory (os error 2)`.
  - `daemon_start_status_and_stop_manage_background_process`: daemon status never reports `running: true` before the 5 second test deadline.

## Current bugs found

### 1. CLI smoke test for process cancellation cannot start its provider command

- Severity: High
- Area: CLI smoke tests and provider process execution
- Evidence: `cargo test --workspace` fails `cancel_terminates_active_provider_process` with `Error: provider command failed: No such file or directory (os error 2)`.
- Impact: cancellation behavior is not currently covered by a passing regression test. A real provider command with descendants may not be terminated as expected, and the failing test blocks a clean workspace validation run.
- Suggested fix: inspect the custom provider path generated in `crates/scheduler-cli/tests/cli_smoke.rs`, ensure the provider executable path is persisted and invoked correctly, then rerun `cargo test -p scheduler-cli --test cli_smoke cancel_terminates_active_provider_process`.

### 2. CLI smoke test for daemon start does not observe a running daemon

- Severity: High
- Area: daemon lifecycle and CLI status reporting
- Evidence: `cargo test --workspace` fails `daemon_start_status_and_stop_manage_background_process`; `daemon status` reports `"running": false` with `pid: 0` while heartbeat fields continue to update.
- Impact: daemon start/status/stop behavior cannot be trusted from the test suite. The status model may be losing the daemon process id, misreporting liveness, or the daemon may be exiting before the test can observe it.
- Suggested fix: trace the `daemon start` path and pid-file/status snapshot handling, then rerun `cargo test -p scheduler-cli --test cli_smoke daemon_start_status_and_stop_manage_background_process`.

### 3. Timed-out provider runs can leave descendant processes alive

- Severity: High
- Area: provider execution and run cancellation
- Evidence: `crates/scheduler-provider/src/lib.rs` creates a Unix process group for provider invocations and exposes `terminate_process_group`, but the timeout path calls only `child.kill()`.
- Impact: if a provider launches child processes, a timeout can kill only the top-level provider process while leaving work running in the background. This can keep modifying a worktree after the scheduler has marked the run timed out.
- Suggested fix: use `terminate_process_group(child_id)` on timeout before or instead of `child.kill()`, then collect output after the group has been signaled. Add a regression test with a provider script that spawns a long-lived child.

### 4. Custom providers configured for `prompt_file` are not passed a file path

- Severity: High
- Area: custom provider invocation
- Evidence: `PromptMode::PromptFile` is a public mode, but `prompt_invocation` handles it by putting the prompt text on stdin, making it equivalent to `PromptMode::Stdin`.
- Impact: custom providers that expect a prompt file path cannot work as configured. They receive stdin instead of a filesystem path.
- Suggested fix: create a temporary prompt file, pass its path using the provider's expected argument contract, and document the exact lifecycle. If stdin is intended, remove or rename `PromptFile`.

### 5. Scheduled runs returned as `Start` are created but not transitioned to `Preparing`

- Severity: Medium
- Area: scheduler tick and run state
- Evidence: `apply_due_run_policy_with_notifier` creates a run and returns `DueRunAction::Start`, but unlike queued and skipped paths, it does not transition the run status. `start_next_queued_run_if_unblocked` does transition queued runs to `Preparing`.
- Impact: a caller that relies on tick results alone can leave due runs in their initial status instead of a runnable active state. This makes the state machine inconsistent between immediate scheduled runs and queued runs that later start.
- Suggested fix: either transition newly started scheduled runs to `Preparing` inside `apply_due_run_policy_with_notifier`, or clearly centralize that transition in the daemon caller and add tests covering scheduled start, queued start, and manual run state progression.

### 6. Secret redaction replacement is malformed for bare token patterns

- Severity: Low
- Area: log redaction
- Evidence: `crates/scheduler-logs/src/lib.rs` matches bare OpenAI and GitHub token shapes without capture groups, but the replacement always uses `$1=[REDACTED]`.
- Impact: bare secret values are redacted, but the replacement can become `=[REDACTED]`, dropping useful context and making logs harder to read.
- Suggested fix: use separate replacements per pattern, for example `"$1=[REDACTED]"` for assignment-style secrets and `"[REDACTED]"` or `"token=[REDACTED]"` for bare token patterns. Add assertions for both `sk-...` and `ghp_...` inputs.

## Notes

This report is based on code inspection plus a fresh `cargo test --workspace` run. It is not exhaustive static analysis; it captures the current bugs that were identifiable from the repo and validation output.
