# Provider Guide

Providers are local agent CLIs that Scheduler can detect, configure, and invoke.
Built-in adapters currently cover Codex, Claude Code, and OpenCode. Custom
providers are supported with any executable that accepts the scheduler prompt on
stdin.

## Setup

Run `scheduler` to open the TUI. On launch, Scheduler detects installed
built-in providers and shows them in the Providers view. Press Space to enable
or disable the selected provider, or Enter to enable it and start creating a job.

The same setup flow is available from the CLI:

```bash
scheduler setup
scheduler provider list
scheduler provider detect
```

Detected providers are stored disabled by default. Enable one explicitly:

```bash
scheduler provider enable codex
```

Add a custom command provider:

```bash
scheduler provider add-custom shell /path/to/provider --display-name Shell
```

## Provider Expectations

For task execution, Scheduler builds a provider-specific non-interactive
invocation and sets environment variables:

- Codex non-interactive runs: `codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -`
- Claude Code non-interactive runs: `claude --print --output-format text --input-format text --no-session-persistence --dangerously-skip-permissions`
- OpenCode: `opencode run <prompt>`

Custom providers receive the prompt on stdin.

- `SCHEDULER_JOB_NAME`
- `SCHEDULER_RUN_ID`
- `SCHEDULER_REPO_PATH`
- `SCHEDULER_WORKTREE_PATH`
- `SCHEDULER_CONTEXT_PATH`
- `SCHEDULER_ARTIFACTS_DIR`
- `SCHEDULER_SUMMARY_PATH`
- `SCHEDULER_PROVIDER_ID`

Providers should write artifacts under `SCHEDULER_ARTIFACTS_DIR`. They may write
a JSON summary to `SCHEDULER_SUMMARY_PATH`:

```json
{
  "status": "succeeded",
  "summary": "Created the report.",
  "artifacts": [
    { "path": "artifacts/report.md", "kind": "report" }
  ],
  "files_changed": [],
  "branch": null,
  "commit": null,
  "pull_request_url": null,
  "blocked_reason": null
}
```

## Natural Language Job Creation

`scheduler create --repo <path> --provider <id> --task "..."`
invokes the selected provider as a spec builder. The provider must return the
spec-builder envelope JSON. Invalid responses are repaired with targeted retry
prompts before the CLI fails.

## Non-Interactive Safety

Scheduled jobs should use providers configured for non-interactive execution.
The job validator rejects `approval_policy = provider_default` for enabled
scheduled jobs because background runs cannot wait for interactive approval.
For built-in Codex and Claude Code providers, `approval_policy =
non_interactive` maps to the providers' unattended/bypass flags for actual job
execution. Spec creation remains on the normal, sandboxed provider invocation.

## Webhook Notifications

For the `webhook` notification channel, set:

```bash
export SCHEDULER_WEBHOOK_URL="https://example.com/scheduler"
```

Webhook failures are recorded as notification delivery failures and do not fail
the run.
