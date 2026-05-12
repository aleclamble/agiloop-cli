# Job Spec Reference

Scheduler jobs are serialized as JSON or TOML. The current schema version is
`scheduler.job.v1`.

## Required Fields

- `schema_version`: must be `scheduler.job.v1`.
- `name`: unique active job name.
- `provider_id`: configured provider to run.
- `repo.path`: repository or working directory path.
- `schedule`: `manual`, `once`, `interval`, or `cron`.
- `task.prompt`: provider prompt for each run.

## Schedule

Manual jobs never run on their own:

```json
{ "kind": "manual" }
```

One-time jobs run once when `at` is due:

```json
{ "kind": "once", "at": "2026-05-12T08:00:00Z" }
```

Interval jobs require `every`, `unit`, and optional `start_at`:

```json
{
  "kind": "interval",
  "every": 1,
  "unit": "hours",
  "timezone": "UTC",
  "start_at": "2026-05-12T08:00:00Z",
  "misfire_policy": "run_once"
}
```

Cron jobs use five-field cron plus an explicit timezone:

```json
{
  "kind": "cron",
  "expression": "0 8 * * *",
  "timezone": "Africa/Johannesburg",
  "misfire_policy": "run_once"
}
```

Misfire policies:

- `skip`: only run if the due time is exactly current.
- `run_once`: run the latest missed execution.
- `backfill`: create missed runs up to the daemon backfill limit.

## Execution

Defaults are intentionally conservative:

- `isolation`: `git_worktree`
- `concurrency`: `skip`
- `repo_lock`: `none`
- `timeout_seconds`: `3600`
- `approval_policy`: `non_interactive`
- `branch_template`: `scheduler/{job_slug}/{run_id}`

Concurrency:

- `skip`: record a skipped run if the same job is active.
- `queue`: persist a queued run.
- `parallel`: start another run immediately.
- `replace`: cancel active runs and create a replacement run.

Repo lock:

- `none`: jobs in the same repo may overlap.
- `exclusive`: only one active run may target the repo.

## Notifications

Jobs can route terminal run events to channels:

```json
{
  "notifications": {
    "on_success": ["local"],
    "on_failure": ["webhook"],
    "on_timeout": ["local"]
  }
}
```

Supported channel names are `local`, `webhook`, and `none`. The `webhook`
channel reads `SCHEDULER_WEBHOOK_URL`; `webhook:https://...` is supported for
explicit per-job URLs, but environment configuration is preferred because job
specs should not contain secrets.

## Validation

Run `scheduler create --from-file job.json` to validate and store a job. Use
`scheduler export <job> --format json` or `--format toml` to inspect the stored
normalized spec.
