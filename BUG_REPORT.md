# Bug Report

Generated on 2026-05-13 from a local audit of `aleclamble/agiloop-cli`.

## Validation status

- `cargo fmt --all -- --check` passes.
- `cargo test --workspace` could not complete in this workspace because Cargo could not fetch dependencies from `index.crates.io` (`SSL connect error` after DNS/TLS retry warnings).
- `cargo test --workspace --offline` could not complete because the local Cargo cache is missing dependencies such as `chrono`.

## Current bugs found

### 1. Timed-out provider runs can leave descendant processes alive

- Severity: High
- Area: provider execution and run cancellation
- Evidence: `crates/scheduler-provider/src/lib.rs:444` creates a Unix process group for provider invocations, and `crates/scheduler-provider/src/lib.rs:507` has `terminate_process_group`, but the timeout path at `crates/scheduler-provider/src/lib.rs:491` only calls `child.kill()`.
- Impact: if a provider launches child processes, a timeout can kill only the top-level provider process while leaving work running in the background. This can keep modifying a worktree after the scheduler has marked the run timed out.
- Suggested fix: use `terminate_process_group(child_id)` on timeout before or instead of `child.kill()`, then collect output after the group has been signaled. Add a regression test with a provider script that spawns a long-lived child.

### 2. Custom providers configured for `prompt_file` are not passed a file path

- Severity: High
- Area: custom provider invocation
- Evidence: `PromptMode::PromptFile` is a public mode, but `prompt_invocation` at `crates/scheduler-provider/src/lib.rs:708` handles it by putting the prompt text on stdin at `crates/scheduler-provider/src/lib.rs:722`.
- Impact: custom providers that expect a prompt file path cannot work as configured. They receive stdin instead of a filesystem path, making `prompt_file` behavior indistinguishable from `stdin`.
- Suggested fix: create a temporary prompt file, pass its path using the provider's expected argument contract, and document the exact lifecycle. If the intended behavior is stdin, remove or rename `PromptFile` to avoid a broken public configuration option.

### 3. Scheduled runs returned as `Start` are created but not transitioned to `Preparing`

- Severity: Medium
- Area: scheduler tick and run state
- Evidence: `apply_due_run_policy_with_notifier` creates a run and returns `DueRunAction::Start` at `crates/scheduler-daemon/src/lib.rs:357`, but unlike queued and skipped paths, it does not transition the run status. `start_next_queued_run_if_unblocked` does transition queued runs to `Preparing` at `crates/scheduler-daemon/src/lib.rs:336`.
- Impact: a caller that relies on tick results alone can leave due runs in their initial status instead of a runnable active state. This makes the state machine inconsistent between immediate scheduled runs and queued runs that later start.
- Suggested fix: either transition newly started scheduled runs to `Preparing` inside `apply_due_run_policy_with_notifier`, or clearly centralize that transition in the daemon caller and add tests covering scheduled start, queued start, and manual run state progression.

### 4. Secret redaction replacement is malformed for bare key patterns

- Severity: Low
- Area: log redaction
- Evidence: `crates/scheduler-logs/src/lib.rs:6` and `crates/scheduler-logs/src/lib.rs:7` match bare OpenAI and GitHub token shapes without capture groups, but the replacement at `crates/scheduler-logs/src/lib.rs:12` always uses `$1=[REDACTED]`.
- Impact: bare secret values are redacted, but the replacement can become `=[REDACTED]`, dropping useful context and making logs harder to read. It also hides which redaction rule fired.
- Suggested fix: use separate replacements per pattern, for example `"$1=[REDACTED]"` for assignment-style secrets and `"[REDACTED]"` or `"token=[REDACTED]"` for bare token patterns. Add assertions for both `sk-...` and `ghp_...` inputs.

## Notes

This report is based on code inspection plus the validation commands listed above. A full test run should be retried in an environment with Cargo dependency access before treating this as exhaustive.
