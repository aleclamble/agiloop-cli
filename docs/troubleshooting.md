# Troubleshooting

## Provider Missing

Symptoms:

- `scheduler provider detect` shows `missing`.
- `scheduler run <job>` says the provider is not configured or disabled.

Checks:

```bash
scheduler setup
scheduler provider list
scheduler provider test <provider-id>
```

Fixes:

- Install the provider CLI and ensure it is on `PATH`.
- Re-run `scheduler setup`.
- Enable the provider with `scheduler provider enable <provider-id>`.
- For custom providers, verify the command path is executable.

## Daemon Stopped

`scheduler daemon start` launches a background scheduler loop and writes a PID
file and log under the scheduler data directory. `scheduler daemon stop`
terminates that background process.

Checks:

```bash
scheduler daemon status
scheduler daemon tick
```

If scheduled work did not run, inspect `scheduler daemon status --json`, check
`daemon.log` in the data directory, and verify the job is enabled.

## Git Failure

Worktree-backed jobs require a valid Git repository and a resolvable `base_ref`.

Checks:

```bash
git -C /path/to/repo status
git -C /path/to/repo rev-parse --verify main
scheduler show <job>
```

Fixes:

- Set `execution.isolation` to `none` only for jobs that do not need a worktree.
- Correct `repo.path`.
- Correct `repo.base_ref`.
- Remove or change a conflicting generated branch name.

## Invalid Spec

Symptoms:

- `scheduler create --from-file` fails validation.
- Provider-backed creation fails after repair attempts.

Checks:

```bash
scheduler export <existing-job> --format json
```

Common fixes:

- Use `schema_version = "scheduler.job.v1"`.
- Use a supported schedule `kind`.
- Use a valid five-field cron expression.
- Use an enabled provider.
- Avoid `approval_policy = "provider_default"` for enabled scheduled jobs.

## Stuck Run

Active runs should be `preparing`, `running`, or `cancelling`. On daemon
start/restart, Scheduler marks interrupted active runs as `lost`.

Checks:

```bash
scheduler runs <job>
scheduler daemon restart
scheduler runs <job>
```

Manual recovery:

```bash
scheduler cancel <run-id>
scheduler retry <run-id>
```

Inspect logs and artifacts:

```bash
scheduler logs <run-id>
scheduler artifacts <run-id>
```

## Notification Failure

Webhook and local notifications are best-effort. A notification failure is
recorded, but the run status remains based on provider execution.

For webhook notifications:

- Verify `SCHEDULER_WEBHOOK_URL`.
- Check that the URL is reachable.
- Avoid storing secrets directly in job specs.
