# Agent Scheduler CLI PRD

Status: Source of truth for implementation  
Product name: `scheduler` until renamed  
Primary implementation language: Rust  
Primary UI: CLI plus Ratatui TUI  
Primary storage: local SQLite database plus file-backed logs/artifacts  
Primary execution model: local scheduled agent runs in isolated Git worktrees

## 1. Goal

Build a shippable, local-first, agent-agnostic scheduler CLI that lets users create, inspect, run, and manage scheduled AI agent jobs across repositories.

The product must:

- Detect installed coding-agent CLIs such as Codex, Claude Code, OpenCode, and custom shell providers.
- Let users select and configure which providers the scheduler may use.
- Let users describe scheduled work in plain English.
- Use a selected provider agent to convert that plain-English request into a strict structured job spec.
- Validate, store, schedule, and execute that job spec reliably.
- Run each scheduled job in an isolated Git worktree unless the job explicitly opts out.
- Capture run history, logs, generated artifacts, branch/worktree metadata, summaries, errors, and final status.
- Provide both a scriptable CLI and a full Ratatui TUI for overview, inspection, creation, editing, logs, and operational control.
- Support concurrent jobs, including multiple jobs against the same repository, while enforcing per-job concurrency policy.
- Default per-job concurrency to `skip`, with user-selectable `queue`, `parallel`, and `replace`.

This PRD is the implementation reference. Behavior that differs from this document should require an explicit PRD update.

## 2. Product Principles

- Agent agnostic: provider-specific behavior lives behind adapters.
- Strict core, flexible edges: LLMs may draft specs, but the scheduler only accepts validated structured specs.
- Local first: all job state, logs, and artifacts are usable offline on the user machine.
- Observable by default: every scheduled run has durable logs, status, timestamps, inputs, outputs, and errors.
- Safe scheduling: background jobs must not hang forever waiting for interactive approval.
- Worktree isolation by default: scheduled agent work should not mutate the user's active checkout.
- Human-editable state: job specs can be exported/imported as TOML or JSON.
- Boring scheduler: scheduling, locking, persistence, retries, and process management must be deterministic and testable without real LLM providers.

## 3. Assumptions

- The shippable product targets macOS and Linux.
- Windows support may be added later but is not required for the first shippable release.
- Provider CLIs differ and may change flags over time, so exact invocation details must be isolated inside provider adapters.
- Provider CLIs may not all support true non-interactive execution. If a provider cannot run safely in the background, the scheduler must surface that capability clearly and refuse scheduled background execution for that provider until configured.
- The scheduler should not try to understand every possible task domain. It schedules and supervises agent runs; the selected provider agent performs task-specific work.
- GitHub, Jira, Slack, email, and similar external workflows are executed by the provider agent or user-provided hooks unless the scheduler explicitly adds a first-class integration later.

## 4. Non-Goals

- Do not build a hosted cloud scheduler.
- Do not build a replacement for provider CLIs.
- Do not parse arbitrary natural language schedules with hand-written edge-case logic as the main path.
- Do not require users to adopt a specific agent provider.
- Do not hide provider limitations. If a provider cannot run non-interactively, scheduled runs must fail early with a clear reason.
- Do not implement broad remote secrets management. Use the OS keychain where possible and environment/config references otherwise.
- Do not automatically push branches, open PRs, merge code, or perform destructive repository actions unless the job spec explicitly asks for that behavior and the provider execution policy allows it.

## 5. Glossary

- Job: A saved scheduled task definition.
- Run: One execution attempt of a job.
- Provider: An adapter that invokes an installed agent CLI or custom command.
- Spec builder: A provider-backed prompt flow that converts a user request into a structured job spec.
- Job spec: The validated structured representation of a job.
- Worktree: A Git worktree created for a run.
- Artifact: Any durable output captured from a run, such as summaries, reports, patches, screenshots, or generated files.
- Log stream: Captured stdout, stderr, provider events, and scheduler events for a run.
- Concurrency policy: What to do when a job is due while an earlier run of the same job is still active.
- Repo lock policy: Whether jobs targeting the same repository are allowed to run concurrently.
- Misfire: A scheduled time that passed while the daemon was stopped or the machine was asleep.

## 6. Core User Stories

### 6.1 Provider Setup

As a user, I can run setup and have the scheduler detect installed agent CLIs so I can choose which providers to enable.

Acceptance criteria:

- `scheduler setup` scans `PATH` for known providers.
- The TUI setup screen shows detected providers, version/probe status, and capabilities.
- The user can enable or disable each provider.
- The user can add a custom provider command.
- Provider configuration is persisted.
- Disabled providers are not offered during job creation unless explicitly shown.

### 6.2 Plain-English Job Creation

As a user, I can describe a task and schedule in plain English, and the scheduler uses my selected provider to draft a validated job spec.

Acceptance criteria:

- `scheduler create` and the TUI creation flow ask for provider, repository, and task description.
- The selected provider is invoked in spec-builder mode, not execution mode.
- The provider returns a structured response envelope.
- The scheduler validates the response against the current job schema.
- The scheduler performs semantic validation beyond JSON schema.
- If the provider response is ambiguous, unsafe, invalid, or incomplete, the CLI/TUI shows targeted correction questions or errors.
- The user sees a confirmation summary before saving.
- The final saved job includes the original natural language request and the normalized structured spec.

### 6.3 Scheduled Execution

As a user, saved jobs run on schedule in the background.

Acceptance criteria:

- The daemon calculates next due runs from persisted schedules.
- The daemon survives restarts without losing job state.
- Missed schedules are handled according to `misfire_policy`.
- Each run has a durable run record before any provider process starts.
- Each run has a final status, even on timeout, cancellation, provider failure, or daemon restart recovery.
- Runs write stdout, stderr, provider events, and scheduler events to durable logs.
- Runs use isolated Git worktrees by default.

### 6.4 Run Inspection

As a user, I can inspect all jobs, run history, logs, summaries, generated artifacts, branches, and failures.

Acceptance criteria:

- `scheduler list` shows jobs, enabled state, next run, last run, provider, repo, and status.
- `scheduler runs <job>` shows historical runs.
- `scheduler logs <run>` streams or prints logs.
- The TUI dashboard shows active jobs and recent runs.
- The TUI run detail view supports live log tailing.
- Run summaries and artifacts are discoverable from both CLI and TUI.

### 6.5 Operational Control

As a user, I can pause, resume, run now, cancel, retry, edit, clone, disable, or delete jobs.

Acceptance criteria:

- Jobs can be enabled and disabled without deleting history.
- `run now` respects concurrency policy unless forced.
- Active runs can be cancelled.
- Failed runs can be retried.
- Jobs can be edited via a structured editor or by exporting/editing/importing spec files.
- Deleting a job requires confirmation and does not delete run history unless requested.

### 6.6 Concurrency

As a user, I can decide what happens when a job is scheduled again before its previous run finished.

Acceptance criteria:

- Every job has a `concurrency` field.
- Default value is `skip`.
- Allowed values are `skip`, `queue`, `parallel`, and `replace`.
- The scheduler enforces the policy before creating a worktree or launching a provider.
- Skipped runs are recorded as skipped with reason.
- Queued runs are persisted and survive daemon restarts.
- Parallel runs use distinct run IDs, worktrees, branches, and log files.
- Replace cancels active runs, records cancellation reason, then starts the new run.

### 6.7 Same-Repository Parallelism

As a user, I can run multiple jobs against the same repository at the same time when they use isolated worktrees.

Acceptance criteria:

- The default repo lock policy is `none`.
- Distinct jobs targeting the same repo may run at the same time by default.
- Each run gets a unique worktree path.
- Each run gets a unique branch name unless the job explicitly opts into a fixed branch.
- The scheduler can optionally enforce `repo_lock = exclusive` for jobs that must not share a repo with other active runs.

## 7. Product Surface

### 7.1 CLI Commands

All CLI commands must support `--help`.

Required commands:

```text
scheduler setup
scheduler provider list
scheduler provider detect
scheduler provider enable <provider-id>
scheduler provider disable <provider-id>
scheduler provider add-custom
scheduler provider test <provider-id>

scheduler create
scheduler create --from-file <path>
scheduler create --repo <path> --provider <id> --task "<text>"
scheduler list
scheduler show <job-id-or-name>
scheduler edit <job-id-or-name>
scheduler export <job-id-or-name> --format toml|json
scheduler import <path>
scheduler enable <job-id-or-name>
scheduler disable <job-id-or-name>
scheduler delete <job-id-or-name>

scheduler run <job-id-or-name>
scheduler run <job-id-or-name> --force
scheduler cancel <run-id>
scheduler retry <run-id>
scheduler runs <job-id-or-name>
scheduler logs <run-id>
scheduler logs <run-id> --follow
scheduler artifacts <run-id>

scheduler daemon start
scheduler daemon stop
scheduler daemon restart
scheduler daemon status
scheduler daemon install
scheduler daemon uninstall

scheduler tui
scheduler doctor
scheduler config path
scheduler data path
scheduler db check
scheduler db migrate
```

CLI behavior requirements:

- Use stable human-readable output by default.
- Support `--json` on list/show/status commands.
- Support `--config <path>` for tests and advanced usage.
- Exit non-zero on validation, provider, scheduler, storage, and execution failures.
- Never require the TUI for core workflows.

### 7.2 TUI Screens

Required Ratatui screens:

- Dashboard: job overview, active runs, recent failures, next due runs.
- Jobs list: filter/search/sort by provider, repo, status, enabled, next run.
- Job detail: schedule, provider, repo, task, concurrency, history, actions.
- Create job wizard: provider, repo, prompt, generated spec, confirmation.
- Edit job: structured form fields and raw spec view.
- Runs list: status, duration, start/end, trigger, branch, provider.
- Run detail: live status, summary, logs, artifacts, worktree, branch, errors.
- Logs viewer: tail, search, wrap, source filter, copy path.
- Provider setup: detection results, enabled providers, capability probes.
- Settings: paths, daemon status, retention, notifications, defaults.
- Help: keybindings and command equivalents.

TUI behavior requirements:

- Keyboard-first navigation.
- Clear loading, empty, error, and offline-daemon states.
- No hidden destructive actions; require confirmation for delete/cancel/replace.
- Long logs must stream without freezing the UI.
- The TUI must continue to work when the daemon is stopped, with read-only database views where possible.

## 8. Job Spec Schema

The scheduler stores a versioned job spec. JSON is the canonical internal schema. TOML is supported for human editing.

### 8.1 Job Spec v1

Required top-level fields:

```json
{
  "schema_version": "scheduler.job.v1",
  "name": "overnight-commit-report",
  "enabled": true,
  "provider_id": "codex",
  "repo": {
    "path": "/Users/alec/projects/example",
    "base_ref": "main",
    "fetch_before_run": true
  },
  "schedule": {
    "kind": "cron",
    "expression": "0 8 * * *",
    "timezone": "Africa/Johannesburg",
    "misfire_policy": "run_once"
  },
  "task": {
    "prompt": "Create a report of all commits pushed since the previous run.",
    "success_criteria": [
      "A report artifact is created.",
      "The report includes commit hashes, authors, timestamps, and summaries."
    ]
  },
  "execution": {
    "isolation": "git_worktree",
    "concurrency": "skip",
    "repo_lock": "none",
    "timeout_seconds": 3600,
    "approval_policy": "non_interactive",
    "branch_template": "scheduler/{job_slug}/{run_id}",
    "worktree_cleanup": {
      "on_success": "after_retention",
      "on_failure": "keep",
      "retention_days": 14
    }
  },
  "notifications": {
    "on_success": [],
    "on_failure": ["local"],
    "on_timeout": ["local"]
  },
  "metadata": {
    "source": "plain_english",
    "source_text": "Every morning at 8 create a report of all commits pushed overnight.",
    "created_by_provider_id": "codex"
  }
}
```

### 8.2 Schedule Kinds

Supported schedule kinds:

- `cron`: cron expression plus timezone.
- `interval`: every N seconds/minutes/hours/days from a start time.
- `once`: one scheduled date-time.
- `manual`: not scheduled; can be run on demand.

Cron requirements:

- Store expressions normalized to five-field cron unless the implementation intentionally supports seconds.
- Store timezone explicitly.
- Validate invalid dates and impossible schedules.

Interval requirements:

- Minimum interval defaults to 60 seconds unless config allows lower.
- Interval schedules must define `every` and `unit`.
- Interval schedules must support optional `start_at`.

Misfire policy values:

- `skip`: do not run missed executions.
- `run_once`: run once after daemon resumes if at least one execution was missed.
- `backfill`: enqueue each missed execution, subject to a configured maximum.

Default misfire policy: `run_once`.

### 8.3 Concurrency Values

`skip`:

- If the same job has an active run, record the due run as skipped.
- Do not create a worktree.
- Do not launch provider.

`queue`:

- If the same job has an active run, persist a queued run.
- Start queued runs in due-time order when the active run completes.
- Queue depth is configurable per job and globally.

`parallel`:

- Start a new run even if the same job has active runs.
- Generate unique run ID, branch, worktree, and log path.

`replace`:

- Cancel active runs for the same job.
- Wait for cancellation cleanup up to a configured grace period.
- Start the new run.
- Record replaced runs as cancelled with reason `replaced_by_new_run`.

### 8.4 Repo Lock Values

`none`:

- Default.
- Jobs targeting the same repository may run concurrently.

`exclusive`:

- Only one run targeting the same canonical repository path may be active.
- Other due runs follow their own concurrency or queue behavior after repo lock evaluation.

### 8.5 Approval Policy Values

`non_interactive`:

- Default for scheduled jobs.
- Provider must not block waiting for user input.
- If provider cannot guarantee non-interactive behavior, the run fails during preflight.

`interactive_attach`:

- Allowed only for manual `run now` unless explicitly enabled for scheduled jobs.
- The user can attach to the provider process.

`provider_default`:

- Uses provider defaults.
- Must be blocked for scheduled jobs unless the user confirms the risk during job creation.

### 8.6 Delivery Options

The scheduler does not assume every job should commit or push code. The job spec may include optional delivery instructions:

```json
{
  "delivery": {
    "mode": "artifact_only",
    "require_summary": true,
    "require_clean_worktree": false
  }
}
```

Supported delivery modes:

- `artifact_only`: default; capture outputs and summaries.
- `leave_changes`: allow files to remain changed in the run worktree.
- `commit`: require the provider agent to create a commit.
- `push_branch`: require a branch push.
- `pull_request`: require branch push and PR creation.

The scheduler validates observable outcomes where possible, such as commit existence or generated summary file, but the provider agent performs task-specific delivery work.

## 9. Spec Builder

The spec builder is the LLM-backed flow that converts plain English into a structured job spec.

### 9.1 Spec Builder Output Envelope

Providers must return a JSON object matching this envelope:

```json
{
  "status": "ok",
  "questions": [],
  "warnings": [],
  "summary": {
    "human": "Run every day at 08:00 Africa/Johannesburg using Codex in a Git worktree.",
    "schedule": "0 8 * * *",
    "task": "Create a report of commits pushed since the previous run."
  },
  "job_spec": {}
}
```

Allowed statuses:

- `ok`: `job_spec` is present.
- `needs_clarification`: `questions` is non-empty and `job_spec` may be partial.
- `unsafe`: the request asks for behavior requiring explicit user approval.
- `unsupported`: the request cannot be represented by the current schema.

### 9.2 Spec Builder Prompt Contract

The scheduler injects a provider-specific prompt/skill with these rules:

- You are only creating a job spec; do not execute the requested task.
- Return only valid JSON matching the output envelope.
- Use the provided provider ID, repo path, timezone, and defaults.
- Do not invent repository paths.
- Do not invent provider IDs.
- Do not choose destructive behavior unless the user explicitly requested it.
- Default `execution.concurrency` to `skip`.
- Default `execution.isolation` to `git_worktree`.
- Default `execution.repo_lock` to `none`.
- Default `execution.approval_policy` to `non_interactive`.
- If schedule wording is ambiguous, return `needs_clarification`.
- If the request requires credentials, external services, pushing branches, or opening PRs, include warnings and explicit delivery fields.
- Keep the task prompt faithful to the user's request.
- Add success criteria that are observable after the run.

### 9.3 Validation and Repair

The scheduler must:

- Validate JSON syntax.
- Validate schema version.
- Validate JSON schema.
- Validate semantic constraints.
- Validate provider exists and is enabled.
- Validate repo path exists and is a Git repository when `isolation = git_worktree`.
- Validate schedule can compute a next run.
- Validate branch template variables.
- Validate timeout and retention bounds.
- Attempt at most two provider repair prompts for invalid spec-builder output.
- Show the raw provider error and validation error when repair fails.

## 10. Provider System

### 10.1 Provider Detection

Built-in provider detection must check for:

- Codex CLI
- Claude Code CLI
- OpenCode CLI
- Custom shell provider definitions

Detection requirements:

- Search `PATH`.
- Probe version.
- Probe whether command can run non-interactively.
- Probe whether command accepts prompt via stdin, argument, or file.
- Store binary path, version string, capabilities, and last probe time.
- Never enable a provider silently without user confirmation during setup.

### 10.2 Provider Capabilities

Each provider adapter exposes:

```text
id
display_name
binary_path
version
supports_spec_builder
supports_task_execution
supports_non_interactive
supports_working_directory
supports_prompt_file
supports_stdin_prompt
supports_streaming_output
supports_structured_output
supports_cancellation
default_timeout_seconds
```

### 10.3 Provider Invocation

The scheduler invokes providers through a single internal interface:

```rust
trait ProviderAdapter {
    fn detect(&self) -> ProviderDetection;
    fn build_spec(&self, request: SpecBuildRequest) -> ProviderInvocation;
    fn execute_run(&self, request: RunExecutionRequest) -> ProviderInvocation;
    fn cancel(&self, run: ActiveRun) -> CancellationResult;
}
```

Implementation requirements:

- Provider-specific command flags stay inside adapters.
- Provider invocations are logged with redaction.
- Provider stdout and stderr are captured separately.
- Provider process group is tracked for cancellation.
- Providers run with the run worktree as current working directory when supported.
- Providers receive scheduler context via environment variables and prompt injection.

### 10.4 Custom Providers

Users can add a custom provider:

```toml
[providers.my-agent]
display_name = "My Agent"
command = "/usr/local/bin/my-agent"
spec_builder_args = ["--json"]
execute_args = ["run", "--non-interactive"]
prompt_mode = "stdin"
working_directory_mode = "process_cwd"
```

Custom provider requirements:

- Validate command exists.
- Support prompt via stdin, argument template, or prompt file.
- Support optional environment variables.
- Support capability declarations.
- Include a test command to run a fake spec build and fake execution.

## 11. Run Execution Lifecycle

### 11.1 Run State Machine

Run statuses:

- `scheduled`
- `queued`
- `skipped`
- `preparing`
- `running`
- `cancelling`
- `cancelled`
- `succeeded`
- `failed`
- `timed_out`
- `blocked`
- `lost`

Allowed transitions must be encoded and tested. Invalid transitions should fail loudly.

### 11.2 Run Preparation

Before launching provider:

- Create run record.
- Resolve job spec and provider config.
- Check daemon lock ownership.
- Apply concurrency policy.
- Apply repo lock policy.
- Validate provider capabilities.
- Validate schedule trigger.
- Create log files.
- Prepare worktree if required.
- Write execution prompt/context files.
- Mark run as `running` only after provider process starts.

### 11.3 Git Worktree Isolation

Default isolation: `git_worktree`.

Worktree requirements:

- Canonicalize repo path.
- Verify repo is valid Git repository.
- Determine base ref.
- Optionally fetch before run.
- Generate unique branch name from template.
- Generate unique worktree path under scheduler data directory unless overridden.
- Create worktree with `git worktree add`.
- Record worktree path and branch.
- Never mutate the user's active checkout.
- On failure to create worktree, fail run before provider invocation.
- Cleanup according to retention policy.

### 11.4 Execution Context

For every run, create a run context directory containing:

```text
context.json
execution_prompt.md
provider_stdout.log
provider_stderr.log
scheduler_events.jsonl
artifacts/
summary.json
```

Environment variables passed to provider:

```text
SCHEDULER_JOB_ID
SCHEDULER_JOB_NAME
SCHEDULER_RUN_ID
SCHEDULER_REPO_PATH
SCHEDULER_WORKTREE_PATH
SCHEDULER_CONTEXT_PATH
SCHEDULER_SUMMARY_PATH
SCHEDULER_ARTIFACTS_DIR
SCHEDULER_PROVIDER_ID
```

### 11.5 Execution Prompt Contract

The scheduler injects an execution prompt that tells the provider agent:

- Execute the user's scheduled task in the current worktree.
- Stay within the provided worktree unless the task explicitly requires external paths.
- Respect the delivery mode.
- Write a machine-readable summary to `SCHEDULER_SUMMARY_PATH`.
- Put durable outputs in `SCHEDULER_ARTIFACTS_DIR`.
- Report final status, files changed, commands run at high level, and follow-up recommendations.
- Do not ask interactive questions during scheduled runs.
- If blocked, write a clear blocked reason and exit non-zero.

### 11.6 Timeouts and Cancellation

Requirements:

- Every run has a timeout.
- Timeout sends graceful termination first.
- After grace period, kill process group.
- Cancellation records who/what cancelled the run.
- Cancelled and timed-out runs keep logs and worktrees according to policy.
- Replace concurrency uses the same cancellation path.

## 12. Persistence

### 12.1 Storage Locations

Use OS-appropriate directories through a standard directories library.

Required logical paths:

```text
config_dir
data_dir
cache_dir
log_dir
database_path
worktrees_dir
runs_dir
provider_prompts_dir
```

Expose paths through:

```text
scheduler config path
scheduler data path
```

### 12.2 SQLite Database

Required tables:

- `schema_migrations`
- `settings`
- `providers`
- `provider_probes`
- `jobs`
- `job_versions`
- `schedules`
- `runs`
- `run_events`
- `run_artifacts`
- `run_logs`
- `queues`
- `locks`
- `notifications`
- `audit_events`

Database requirements:

- All migrations are versioned.
- Migrations are idempotent.
- Migrations are tested against empty and previous-version databases.
- Writes that create or transition runs are transactional.
- The daemon uses a process lock to prevent multiple active schedulers for the same database.
- Corruption or migration failures produce actionable errors.

### 12.3 Retention

Retention settings:

- Keep run metadata indefinitely by default.
- Keep logs for 90 days by default.
- Keep successful worktrees for 14 days by default.
- Keep failed worktrees for 30 days by default.
- Allow per-job override.
- Provide `scheduler cleanup --dry-run` and `scheduler cleanup`.

## 13. Scheduler Daemon

### 13.1 Responsibilities

The daemon:

- Owns schedule evaluation.
- Starts due runs.
- Recovers active runs after restart.
- Maintains queue.
- Enforces concurrency and repo locks.
- Streams run events to the TUI.
- Performs retention cleanup.
- Emits notifications.

### 13.2 Installation

Required platforms:

- macOS: install as launchd user agent.
- Linux: install as systemd user service.

Commands:

```text
scheduler daemon install
scheduler daemon uninstall
scheduler daemon start
scheduler daemon stop
scheduler daemon status
```

Daemon requirements:

- Install command writes a user-scoped service.
- Start command does not require root.
- Status reports PID, database path, uptime, active runs, next due run, and last error.
- If a daemon is already running, starting another should fail clearly.

### 13.3 Clock and Scheduling

Requirements:

- Use timezone-aware schedule calculation.
- Use monotonic timers for sleeps.
- Recompute schedules after clock changes or daemon resume.
- Persist `last_due_at`, `next_due_at`, and `last_run_id`.
- Unit tests must use a fake clock.

## 14. Notifications

Required notification channels:

- `local`: OS desktop notification where available.
- `webhook`: POST JSON to configured URL.
- `none`: explicit disabled state.

Notification events:

- run success
- run failure
- run timeout
- run cancelled
- job skipped due to concurrency
- provider unavailable
- daemon error

Requirements:

- Notifications are best-effort and must not crash the daemon.
- Webhook failures are logged.
- Secrets in webhook URLs are redacted in logs.

## 15. Security and Safety

### 15.1 Secrets

Requirements:

- Do not store provider API keys directly in job specs.
- Prefer provider CLI's existing auth/session.
- Support environment variable references.
- Support OS keychain integration where practical.
- Redact known secret patterns in logs and provider invocation displays.

### 15.2 Background Execution Safety

Requirements:

- Scheduled runs default to `approval_policy = non_interactive`.
- Providers that may block for approval must be rejected for scheduled runs unless explicitly configured.
- Jobs that push, open PRs, delete files, or run external automations must show warnings during creation.
- Destructive scheduler actions require confirmation in CLI/TUI unless `--yes` is passed.

### 15.3 Audit Events

Record audit events for:

- provider enabled/disabled
- job created/edited/deleted
- job enabled/disabled
- manual run triggered
- run cancelled/retried
- daemon installed/uninstalled
- config import/export

## 16. Error Handling

Error categories:

- configuration
- validation
- provider detection
- provider invocation
- schedule calculation
- database
- filesystem
- git
- daemon lock
- timeout/cancellation
- notification

Requirements:

- CLI errors should include a concise message and a suggested next step.
- TUI errors should include details view and command equivalent where useful.
- Run failures must preserve logs and context.
- Provider failures must include exit code, signal if available, and log paths.

## 17. Observability

Required:

- Structured scheduler events as JSONL.
- Human-readable provider logs.
- Run summary JSON.
- `scheduler doctor` for environment checks.
- `scheduler db check` for database integrity.
- `scheduler daemon status --json` for automation.

Run summary schema:

```json
{
  "status": "succeeded",
  "summary": "Created overnight commit report.",
  "artifacts": [
    {
      "path": "artifacts/report.md",
      "kind": "report"
    }
  ],
  "files_changed": [],
  "branch": "scheduler/overnight-commit-report/20260512T080000Z",
  "commit": null,
  "pull_request_url": null,
  "blocked_reason": null
}
```

## 18. Import, Export, and Backup

Requirements:

- Export individual jobs as JSON or TOML.
- Import jobs with validation and conflict handling.
- Export all scheduler configuration excluding secrets.
- Backup database and config with `scheduler backup create`.
- Restore backup with explicit confirmation.
- Preserve job version history when editing through CLI/TUI.

## 19. Documentation

Required docs:

- README with install, quick start, and concept overview.
- Provider setup guide.
- Job spec reference.
- Scheduling reference.
- Worktree and branch behavior guide.
- Daemon installation guide.
- Troubleshooting guide.
- Security model.
- Examples:
  - daily commit report
  - nightly issue triage
  - weekly dependency update
  - recurring documentation review
  - manual-only agent workflow

## 20. Suggested Rust Architecture

Use a Cargo workspace.

Suggested crates:

```text
crates/scheduler-cli        CLI binary and command routing
crates/scheduler-tui        Ratatui application
crates/scheduler-core       domain types, validation, state machines
crates/scheduler-store      SQLite migrations and repositories
crates/scheduler-daemon     scheduling loop and process supervision
crates/scheduler-provider   provider traits and built-in adapters
crates/scheduler-git        worktree and Git helpers
crates/scheduler-logs       log capture, tailing, redaction
crates/scheduler-testkit    fake providers, fake clock, temp repo helpers
```

Suggested dependencies by category:

- CLI: `clap`
- TUI: `ratatui`, `crossterm`
- Serialization: `serde`, `serde_json`, `toml`
- Validation/schema: `schemars` or equivalent JSON schema tooling
- Time: timezone-aware date/time library
- Scheduling: cron/recurrence library plus wrapper validation
- Storage: SQLite library with migrations
- Async/process: `tokio`
- Git operations: shell out to `git` first for predictable behavior, wrap carefully
- Testing: temp directories, snapshot tests, fake provider binaries

Architecture requirements:

- Core domain types must not depend on TUI or CLI.
- Provider adapters must be testable with fake binaries.
- Scheduler must be testable with fake clock and fake provider.
- Store layer must expose explicit transactions for run lifecycle operations.
- TUI must consume application services rather than directly mutating database tables.

## 21. Implementation Task Breakdown

The task list is intentionally structured for `/goal`: each item has a clear deliverable and verification target.

### Epic A: Repository and Workspace Foundation

- [ ] A001 Create Cargo workspace with the crate layout in Section 20.
  - Verify: `cargo metadata` succeeds.
- [ ] A002 Add shared formatting, lint, and test configuration.
  - Verify: `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] A003 Add root README with product summary and development commands.
  - Verify: README includes setup, test, and run commands.
- [ ] A004 Add CI workflow for format, lint, unit tests, integration tests, and build.
  - Verify: CI passes on a clean checkout.

### Epic B: Domain Model and Validation

- [ ] B001 Implement job spec v1 Rust types.
  - Verify: serialization round-trips JSON and TOML fixtures.
- [ ] B002 Implement schedule types for cron, interval, once, and manual.
  - Verify: unit tests cover valid and invalid schedules.
- [ ] B003 Implement concurrency enum and policy decision function.
  - Verify: tests cover `skip`, `queue`, `parallel`, and `replace`.
- [ ] B004 Implement repo lock enum and lock evaluation model.
  - Verify: tests cover same repo and different repo cases.
- [ ] B005 Implement semantic validation for job specs.
  - Verify: invalid provider, repo, timeout, branch template, schedule, and approval policy fixtures fail with useful errors.
- [ ] B006 Generate or maintain JSON schema for job spec v1.
  - Verify: schema validates canonical examples.

### Epic C: Storage and Migrations

- [ ] C001 Implement SQLite migration runner.
  - Verify: migrations apply to empty database.
- [ ] C002 Create tables listed in Section 12.2.
  - Verify: database schema snapshot test.
- [ ] C003 Implement job repository with create/read/update/delete/version history.
  - Verify: unit/integration tests cover job CRUD and version creation.
- [ ] C004 Implement run repository with transactional state transitions.
  - Verify: invalid run transitions are rejected.
- [ ] C005 Implement queue and lock persistence.
  - Verify: queued runs and locks survive process restart in tests.
- [ ] C006 Implement audit event persistence.
  - Verify: job/provider/daemon actions write audit rows.

### Epic D: Provider Detection and Adapters

- [ ] D001 Define provider adapter trait and capability model.
  - Verify: fake adapter implements all required methods.
- [ ] D002 Implement provider detection framework.
  - Verify: fake binaries on temp `PATH` are detected.
- [ ] D003 Implement Codex provider adapter.
  - Verify: adapter builds expected spec-builder and execution invocations using fake Codex binary.
- [ ] D004 Implement Claude Code provider adapter.
  - Verify: adapter builds expected invocations using fake Claude binary.
- [ ] D005 Implement OpenCode provider adapter.
  - Verify: adapter builds expected invocations using fake OpenCode binary.
- [ ] D006 Implement custom shell provider.
  - Verify: custom provider can build spec and execute task through stdin or prompt file.
- [ ] D007 Implement provider test command.
  - Verify: `scheduler provider test <id>` reports capabilities and failures.
- [ ] D008 Implement provider config persistence.
  - Verify: enabled/disabled/custom providers persist across process restarts.

### Epic E: Spec Builder Flow

- [ ] E001 Add spec-builder prompt template and provider-specific injection layer.
  - Verify: generated prompt includes schema, defaults, repo, provider, timezone, and user request.
- [ ] E002 Implement spec-builder invocation service.
  - Verify: fake provider response creates a pending confirmation.
- [ ] E003 Implement output envelope parser.
  - Verify: `ok`, `needs_clarification`, `unsafe`, and `unsupported` fixtures parse correctly.
- [ ] E004 Implement schema and semantic validation after provider response.
  - Verify: invalid provider output is rejected with targeted errors.
- [ ] E005 Implement repair prompt loop with max two attempts.
  - Verify: fake invalid-then-valid provider succeeds; always-invalid provider fails clearly.
- [ ] E006 Implement confirmation summary generation.
  - Verify: CLI and TUI receive the same normalized summary data.

### Epic F: Git Worktree Management

- [ ] F001 Implement Git repository discovery and canonicalization.
  - Verify: temp repo tests identify valid, invalid, bare, and nested repos.
- [ ] F002 Implement base ref resolution.
  - Verify: default branch, explicit branch, and commit SHA cases work.
- [ ] F003 Implement branch template rendering.
  - Verify: run ID, job slug, date/time variables render uniquely.
- [ ] F004 Implement worktree creation.
  - Verify: provider runs in temp worktree and user checkout remains unchanged.
- [ ] F005 Implement worktree cleanup according to retention policy.
  - Verify: dry-run and actual cleanup tests cover success/failure retention.
- [ ] F006 Implement worktree failure handling.
  - Verify: invalid branch, existing path, and Git command failure mark run failed before provider launch.

### Epic G: Run Execution Engine

- [ ] G001 Implement run context directory creation.
  - Verify: required files and directories are created before provider launch.
- [ ] G002 Implement provider process launch and log capture.
  - Verify: stdout/stderr are captured separately and live-tailable.
- [ ] G003 Implement environment variable injection.
  - Verify: fake provider sees all required `SCHEDULER_*` variables.
- [ ] G004 Implement execution prompt generation.
  - Verify: prompt includes task, delivery mode, worktree path, summary path, and non-interactive instruction.
- [ ] G005 Implement timeout handling.
  - Verify: long fake provider is terminated and run is `timed_out`.
- [ ] G006 Implement cancellation.
  - Verify: manual cancel terminates process group and records cancellation reason.
- [ ] G007 Implement run summary ingestion.
  - Verify: valid `summary.json` is parsed; invalid/missing summary creates warning without losing run status.
- [ ] G008 Implement artifact discovery and persistence.
  - Verify: files in artifacts directory are listed through CLI and TUI service.

### Epic H: Scheduler Daemon

- [ ] H001 Implement daemon process lock.
  - Verify: second daemon fails clearly against same database.
- [ ] H002 Implement schedule evaluation with fake clock.
  - Verify: cron, interval, once, manual, timezone, and misfire cases.
- [ ] H003 Implement due-run creation.
  - Verify: due jobs create run records transactionally.
- [ ] H004 Enforce same-job concurrency policies.
  - Verify: `skip`, `queue`, `parallel`, `replace` integration tests.
- [ ] H005 Enforce repo lock policies.
  - Verify: `repo_lock = exclusive` blocks same-repo parallel runs.
- [ ] H006 Implement restart recovery.
  - Verify: active runs after simulated crash become `lost` or are reattached according to implementation policy.
- [ ] H007 Implement daemon status API/service.
  - Verify: status reports active runs, next due run, PID, uptime, and last error.
- [ ] H008 Implement retention cleanup loop.
  - Verify: cleanup respects retention settings.

### Epic I: CLI

- [ ] I001 Implement command parser and global flags.
  - Verify: all commands in Section 7.1 expose `--help`.
- [ ] I002 Implement setup and provider commands.
  - Verify: detection, enable, disable, custom add, and test work with fake providers.
- [ ] I003 Implement job create flow.
  - Verify: interactive and flag-driven creation produce same stored spec.
- [ ] I004 Implement job list/show/edit/export/import.
  - Verify: JSON and human outputs have snapshot tests.
- [ ] I005 Implement enable/disable/delete with confirmations.
  - Verify: destructive commands require confirmation unless `--yes`.
- [ ] I006 Implement run/cancel/retry/runs/logs/artifacts.
  - Verify: commands work against fake completed and active runs.
- [ ] I007 Implement daemon commands.
  - Verify: start/stop/status paths work in integration tests without installing OS service.
- [ ] I008 Implement doctor/config/data/db commands.
  - Verify: doctor reports provider, database, daemon, Git, and path status.

### Epic J: TUI

- [ ] J001 Implement TUI application shell and navigation.
  - Verify: smoke test renders all main screens.
- [ ] J002 Implement dashboard.
  - Verify: active, failed, and upcoming run fixtures render correctly.
- [ ] J003 Implement jobs list and filters.
  - Verify: snapshot tests for empty, populated, filtered, and error states.
- [ ] J004 Implement job detail and actions.
  - Verify: enable/disable/run/cancel/edit actions call application services.
- [ ] J005 Implement create job wizard.
  - Verify: fake provider spec-builder flow reaches confirmation and save.
- [ ] J006 Implement runs list and run detail.
  - Verify: completed, active, failed, skipped, and queued statuses render correctly.
- [ ] J007 Implement live logs viewer.
  - Verify: large log fixture scrolls and searches without blocking.
- [ ] J008 Implement provider setup screen.
  - Verify: detected, missing, disabled, and misconfigured providers are distinguishable.
- [ ] J009 Implement settings and help screens.
  - Verify: keybindings and paths are visible.

### Epic K: Notifications

- [ ] K001 Implement notification trait.
  - Verify: fake notifier records events.
- [ ] K002 Implement local notification adapter.
  - Verify: adapter is best-effort and failures are logged.
- [ ] K003 Implement webhook notification adapter.
  - Verify: fake HTTP server receives expected JSON and secret redaction works.
- [ ] K004 Wire notifications to run lifecycle events.
  - Verify: success/failure/timeout/cancel/skipped events trigger configured channels.

### Epic L: Import, Export, Backup, Cleanup

- [ ] L001 Implement job export JSON/TOML.
  - Verify: exported files re-import without changes.
- [ ] L002 Implement job import conflict handling.
  - Verify: rename, replace, and reject paths are tested.
- [ ] L003 Implement full config export excluding secrets.
  - Verify: secret-like values are redacted or omitted.
- [ ] L004 Implement backup create/restore.
  - Verify: restored backup contains jobs, runs, providers, and settings.
- [ ] L005 Implement cleanup dry-run and cleanup execution.
  - Verify: cleanup never deletes records outside configured retention scope.

### Epic M: Packaging and Release

- [ ] M001 Implement release builds for macOS and Linux.
  - Verify: binaries run `scheduler --version`.
- [ ] M002 Add shell completions.
  - Verify: completions generated for zsh, bash, and fish.
- [ ] M003 Add install script or documented package manager path.
  - Verify: clean machine install can run setup.
- [ ] M004 Add signed/notarized macOS release if distribution requires it.
  - Verify: macOS binary launches without quarantine issues in release process.
- [ ] M005 Add changelog and versioning policy.
  - Verify: release notes include migrations and compatibility notes.

### Epic N: Documentation and Examples

- [ ] N001 Write user README.
  - Verify: new user can create and run first job from docs.
- [ ] N002 Write job spec reference.
  - Verify: every schema field is documented.
- [ ] N003 Write provider guide.
  - Verify: built-in and custom providers are documented.
- [ ] N004 Write troubleshooting guide.
  - Verify: covers provider missing, daemon stopped, Git failure, invalid spec, and stuck run.
- [ ] N005 Add example job specs.
  - Verify: examples pass schema validation in tests.

## 22. Unit Test Requirements

Required unit test areas:

- Job spec serialization and deserialization.
- JSON schema generation/validation.
- Semantic validation.
- Schedule parsing and next-run calculation.
- Timezone behavior.
- Misfire policies.
- Concurrency policy decisions.
- Repo lock decisions.
- Run state machine transitions.
- Branch template rendering.
- Path canonicalization.
- Secret redaction.
- Provider capability parsing.
- Provider command construction.
- Spec-builder envelope parsing.
- Repair-loop stopping conditions.
- Notification event routing.
- Retention cutoff calculation.

Each pure domain function should have table-driven tests.

## 23. Integration Test Requirements

Required integration tests:

- Setup detects fake providers on a temporary `PATH`.
- Create job from fake provider valid JSON.
- Create job from invalid provider output fails clearly.
- Create job repair loop succeeds on second attempt.
- Daemon starts due interval job with fake clock.
- Daemon handles cron schedule in a non-UTC timezone.
- `skip` concurrency records skipped run.
- `queue` concurrency starts queued run after active run completes.
- `parallel` concurrency starts two runs with distinct worktrees.
- `replace` concurrency cancels active run and starts new run.
- Same-repo different-job parallel runs both succeed with unique worktrees.
- Exclusive repo lock blocks same-repo parallel run.
- Provider timeout marks run timed out and preserves logs.
- Manual cancel marks run cancelled and kills fake provider.
- Worktree creation failure fails run before provider launch.
- Failed run retry creates a new run linked to original.
- Export/import roundtrip preserves job behavior.
- Backup/restore preserves jobs and run history.
- TUI smoke renders dashboard, jobs, run detail, logs, and provider setup using fixture database.

## 24. Regression Test Suite

Add regression fixtures for every bug fixed after initial implementation. Initial regression cases must include:

- Invalid cron expression does not panic.
- Ambiguous natural language schedule returns clarification, not invented schedule.
- Provider returns markdown-wrapped JSON and parser rejects or repairs deterministically.
- Provider returns valid JSON with unknown fields and validation handles according to schema policy.
- Missing provider binary after job creation fails preflight clearly.
- Repo path deleted after job creation fails preflight clearly.
- Branch name collision is resolved or fails clearly before provider launch.
- Daemon restart while run is active does not leave run permanently `running`.
- Log file larger than memory budget can still be tailed.
- Webhook notification failure does not mark run failed.
- Cleanup dry-run does not delete anything.
- `replace` does not start new run until cancellation path has completed or timed out.
- TUI does not freeze when daemon is unavailable.

## 25. End-to-End Acceptance Scenarios

### Scenario 1: Daily Report

1. User runs `scheduler setup`.
2. Scheduler detects Codex.
3. User enables Codex.
4. User runs `scheduler create`.
5. User selects repo.
6. User enters: "Every day at 8am, create a report of all commits pushed since the previous run."
7. Spec builder returns job spec.
8. User confirms.
9. Daemon runs job at the scheduled time.
10. Run completes with report artifact.
11. User opens TUI and views report artifact and logs.

Pass criteria:

- Job has next run before execution.
- Run has correct worktree.
- Report artifact is captured.
- Logs and summary are visible.

### Scenario 2: GitHub Issue Worker

1. User creates job: "Every weekday at 6pm, find GitHub issues starting with [agent] and work on one in a new branch."
2. Spec includes warnings about GitHub credentials and branch/PR delivery.
3. User confirms delivery mode.
4. Run creates isolated worktree and branch.
5. Provider agent performs work.
6. Scheduler captures branch, summary, logs, and artifacts.

Pass criteria:

- Scheduler does not need first-class GitHub issue logic.
- Provider task prompt contains the user's GitHub workflow.
- Branch/worktree metadata is recorded.
- Failure to authenticate with GitHub is captured as provider failure or blocked summary.

### Scenario 3: Overlapping Run

1. Job interval is every 5 minutes.
2. First run takes 10 minutes.
3. Concurrency is `skip`.
4. Second due time occurs during first run.

Pass criteria:

- Second run is recorded as skipped.
- No second provider process starts.
- No second worktree is created.

### Scenario 4: Parallel Same Repo

1. Two distinct jobs target the same repo.
2. Both are due at the same time.
3. Both use `repo_lock = none`.

Pass criteria:

- Both runs start.
- Each has unique worktree and branch.
- Logs remain separate.

### Scenario 5: Provider Unavailable

1. User creates job with a provider.
2. Provider binary is later removed.
3. Job becomes due.

Pass criteria:

- Run fails during preflight.
- Error says provider binary is missing.
- Logs and run record exist.
- Daemon continues processing other jobs.

## 26. Quality Gates

Before a release:

- `cargo fmt --check` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `cargo test --workspace` passes.
- Integration test suite passes.
- TUI smoke tests pass.
- Example job specs validate.
- Database migration tests pass.
- Packaging smoke test passes on macOS and Linux.
- README quick start is manually verified.
- No known run state can get stuck without an operator-visible recovery path.

## 27. Open Design Decisions

These must be resolved during implementation and recorded in the PRD or architecture docs:

- Exact public binary name.
- Exact provider CLI flags for Codex, Claude Code, and OpenCode.
- SQLite crate choice.
- Time/scheduling crate choice.
- Whether real provider smoke tests run in CI or only locally.
- Whether the TUI talks to daemon over local socket or directly reads database plus command service.
- Whether run log indexing stores every line in SQLite or stores only file offsets and metadata.
- Exact OS keychain support in first release.

## 28. Definition of Done

The product is shippable when:

- A new user can install the binary, run setup, enable a detected provider, create a plain-English scheduled job, and see it run on schedule.
- Jobs run in isolated worktrees by default.
- The scheduler enforces `skip`, `queue`, `parallel`, and `replace`.
- The daemon can be installed, started, stopped, and inspected.
- CLI and TUI both cover creation, listing, inspection, logs, run history, and operational actions.
- Provider failures, Git failures, invalid specs, missed schedules, timeouts, and cancellations are visible and recoverable.
- All tests and quality gates in this PRD pass.
- Documentation includes quick start, provider setup, job spec reference, daemon guide, and troubleshooting.
