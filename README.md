# Scheduler CLI

`scheduler` is a local-first, agent-agnostic scheduler for recurring AI agent work.

The product goal is defined in [PRD.md](PRD.md). The current implementation provides typed job specs, validation, provider detection/configuration, provider-backed spec creation, SQLite-backed jobs/runs/logs/artifacts, scheduled daemon execution, worktree isolation, notifications, import/export/backup, a scriptable CLI, and a Ratatui TUI.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

## Local Usage

Open the scheduler:

```bash
cargo run -p scheduler-cli --
```

On first launch, Scheduler auto-detects installed providers such as Codex,
Claude Code, and OpenCode. Use the Providers view to choose one, then describe
the task and schedule in plain English in the Create view.

Install the binary locally:

```bash
cargo install --path crates/scheduler-cli
scheduler
```

Create a job from a spec:

```bash
cargo run -p scheduler-cli -- create --from-file fixtures/jobs/manual-shell.json
```

Run a job manually:

```bash
cargo run -p scheduler-cli -- run manual-shell
```

Use isolated test state:

```bash
cargo run -p scheduler-cli -- --config /tmp/scheduler-dev list
```

Generate shell completions:

```bash
cargo run -p scheduler-cli -- completions zsh
```

## User Docs

- [Job spec reference](docs/job-spec-reference.md)
- [Provider guide](docs/provider-guide.md)
- [Release and install](docs/release.md)
- [Troubleshooting](docs/troubleshooting.md)

## Architecture

The workspace follows the PRD crate boundaries:

- `scheduler-core`: job spec, schedule, validation, branch templates, run state machine.
- `scheduler-provider`: provider detection, prompt contracts, invocation execution.
- `scheduler-store`: SQLite migrations and repositories.
- `scheduler-git`: Git repository and worktree helpers.
- `scheduler-logs`: log redaction utilities.
- `scheduler-daemon`: scheduling/execution primitives.
- `scheduler-cli`: command-line entry point.
- `scheduler-tui`: Ratatui views and view models.
- `scheduler-testkit`: shared test helpers.

## Release Notes

See [docs/implementation-status.md](docs/implementation-status.md) for the PRD status map. macOS signing and notarization run in the release workflow when the required Apple Developer secrets are configured.
