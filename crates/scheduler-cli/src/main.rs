use std::collections::HashSet;
use std::fs;
use std::io::{IsTerminal, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::generate;
use clap_complete::shells::{Bash, Fish, Zsh};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use directories::ProjectDirs;
use scheduler_core::{JobSpec, RunStatus, RunTrigger, ValidationContext, validate_job_spec};
use scheduler_daemon::{
    RunExecutor, SchedulerTickReport, acquire_daemon_lock, cleanup_retained_worktrees,
    daemon_instance_id, daemon_status_snapshot, recover_interrupted_runs, scheduler_tick,
};
use scheduler_logs::redact_secrets;
use scheduler_provider::{
    ProviderCapability, ProviderConfig, ProviderDetection, SpecBuildRequest, SpecBuilderStatus,
    SpecBuilderSummary, build_provider_spec_invocation, build_spec_prompt,
    built_in_provider_config, detect_built_in_providers, parse_spec_builder_envelope,
    run_invocation, terminate_process_group,
};
use scheduler_store::Store;
use scheduler_tui::dashboard::draw_app;
use scheduler_tui::{
    CreateField, TuiAppModel, TuiJob, TuiProvider, TuiRun, TuiSetting, TuiState, TuiView,
};

#[derive(Debug, Parser)]
#[command(
    name = "scheduler",
    version,
    about = "Agent-agnostic scheduled AI job runner"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Setup,
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Create {
        #[arg(long)]
        from_file: Option<PathBuf>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long, default_value = "UTC")]
        timezone: String,
    },
    List,
    Show {
        job: String,
    },
    Edit {
        job: String,
        #[arg(long)]
        from_file: PathBuf,
    },
    Export {
        job: String,
        #[arg(long, value_enum, default_value = "json")]
        format: ExportFormat,
    },
    Import {
        path: PathBuf,
        #[arg(long, value_enum, default_value = "reject")]
        on_conflict: ImportConflict,
    },
    Enable {
        job: String,
    },
    Disable {
        job: String,
    },
    Delete {
        job: String,
        #[arg(long)]
        yes: bool,
    },
    Run {
        job: String,
    },
    Runs {
        job: String,
    },
    Cancel {
        run_id: String,
    },
    Retry {
        run_id: String,
    },
    Logs {
        run_id: String,
        #[arg(long)]
        follow: bool,
    },
    Artifacts {
        run_id: String,
    },
    Tui,
    Doctor,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Data {
        #[command(subcommand)]
        command: PathCommand,
    },
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    Cleanup {
        #[arg(long)]
        dry_run: bool,
    },
    Completions {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    List,
    Detect,
    Enable {
        provider_id: String,
    },
    Disable {
        provider_id: String,
    },
    AddCustom {
        id: String,
        command: PathBuf,
        #[arg(long)]
        display_name: Option<String>,
    },
    Test {
        provider_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum PathCommand {
    Path,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Path,
    Export {
        #[arg(long, value_enum, default_value = "json")]
        format: ExportFormat,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Start,
    Stop,
    Restart,
    Status,
    Install,
    Uninstall,
    Tick,
    #[command(hide = true)]
    Run,
    #[command(hide = true)]
    ExecRun {
        run_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    Check,
    Migrate,
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    Create {
        output: PathBuf,
    },
    Restore {
        input: PathBuf,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExportFormat {
    Json,
    Toml,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ImportConflict {
    Reject,
    Replace,
    Rename,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::new(cli.config.as_deref())?;

    let command = cli.command.unwrap_or(Command::Tui);
    match command {
        Command::Setup => setup(&paths, cli.json),
        Command::Provider { command } => provider_command(&paths, command, cli.json),
        Command::Create {
            from_file,
            repo,
            provider,
            task,
            timezone,
        } => create_command(&paths, from_file, repo, provider, task, timezone),
        Command::List => list_command(&paths, cli.json),
        Command::Show { job } => show_command(&paths, &job, cli.json),
        Command::Edit { job, from_file } => edit_command(&paths, &job, &from_file),
        Command::Export { job, format } => export_command(&paths, &job, format),
        Command::Import { path, on_conflict } => import_command(&paths, &path, on_conflict),
        Command::Enable { job } => set_job_enabled_command(&paths, &job, true),
        Command::Disable { job } => set_job_enabled_command(&paths, &job, false),
        Command::Delete { job, yes } => delete_command(&paths, &job, yes),
        Command::Run { job } => run_command(&paths, &job),
        Command::Runs { job } => runs_command(&paths, &job, cli.json),
        Command::Cancel { run_id } => cancel_run_command(&paths, &run_id),
        Command::Retry { run_id } => retry_command(&paths, &run_id),
        Command::Logs { run_id, follow } => logs_command(&paths, &run_id, follow),
        Command::Artifacts { run_id } => artifacts_command(&paths, &run_id),
        Command::Tui => tui_command(&paths),
        Command::Doctor => doctor_command(&paths, cli.json),
        Command::Config {
            command: ConfigCommand::Path,
        } => {
            println!("{}", paths.config_dir.display());
            Ok(())
        }
        Command::Config {
            command: ConfigCommand::Export { format },
        } => config_export_command(&paths, format),
        Command::Data {
            command: PathCommand::Path,
        } => {
            println!("{}", paths.data_dir.display());
            Ok(())
        }
        Command::Daemon {
            command: DaemonCommand::Status,
        } => daemon_status(&paths, cli.json),
        Command::Daemon { command } => daemon_command(&paths, command, cli.json),
        Command::Db { command } => db_command(&paths, command, cli.json),
        Command::Backup { command } => backup_command(&paths, command),
        Command::Cleanup { dry_run } => cleanup_command(&paths, dry_run, cli.json),
        Command::Completions { shell } => completions_command(shell),
    }
}

fn setup(paths: &AppPaths, json: bool) -> Result<()> {
    let detections = bootstrap_detected_providers(paths)?;
    print_setup_result(detections, json)
}

fn bootstrap_detected_providers(paths: &AppPaths) -> Result<Vec<ProviderDetection>> {
    let detections = detect_built_in_providers();
    fs::create_dir_all(&paths.data_dir)?;
    let mut store = Store::open(&paths.database_path)?;
    for detection in &detections {
        store.record_provider_probe(detection)?;
        if let Some(config) = built_in_provider_config(detection) {
            store.upsert_provider(&config)?;
        }
    }
    Ok(detections)
}

fn print_setup_result(detections: Vec<ProviderDetection>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&detections)?);
    } else {
        println!("Detected providers:");
        for detection in detections {
            let status = if detection.available {
                "available"
            } else {
                "missing"
            };
            let version = detection.version.unwrap_or_else(|| "-".to_string());
            println!(
                "- {} ({}) [{}] {}",
                detection.display_name, detection.id, status, version
            );
        }
    }
    Ok(())
}

fn completions_command(shell: CompletionShell) -> Result<()> {
    let mut command = Cli::command();
    let mut stdout = std::io::stdout();
    match shell {
        CompletionShell::Bash => generate(Bash, &mut command, "scheduler", &mut stdout),
        CompletionShell::Zsh => generate(Zsh, &mut command, "scheduler", &mut stdout),
        CompletionShell::Fish => generate(Fish, &mut command, "scheduler", &mut stdout),
    }
    Ok(())
}

fn provider_command(paths: &AppPaths, command: ProviderCommand, json: bool) -> Result<()> {
    match command {
        ProviderCommand::Detect => setup(paths, json),
        ProviderCommand::List => {
            fs::create_dir_all(&paths.data_dir)?;
            let store = Store::open(&paths.database_path)?;
            let providers = store.list_providers()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&providers)?);
            } else if providers.is_empty() {
                println!("No providers configured. Run `scheduler setup`.");
            } else {
                for provider in providers {
                    println!(
                        "{}\t{}\t{}",
                        provider.id,
                        if provider.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        provider.command.display()
                    );
                }
            }
            Ok(())
        }
        ProviderCommand::Enable { provider_id } => {
            fs::create_dir_all(&paths.data_dir)?;
            let mut store = Store::open(&paths.database_path)?;
            ensure_detected_provider_exists(&mut store, &provider_id)?;
            store.set_provider_enabled(&provider_id, true)?;
            println!("enabled provider {provider_id}");
            Ok(())
        }
        ProviderCommand::Disable { provider_id } => {
            fs::create_dir_all(&paths.data_dir)?;
            let mut store = Store::open(&paths.database_path)?;
            store.set_provider_enabled(&provider_id, false)?;
            println!("disabled provider {provider_id}");
            Ok(())
        }
        ProviderCommand::AddCustom {
            id,
            command,
            display_name,
        } => {
            fs::create_dir_all(&paths.data_dir)?;
            let mut store = Store::open(&paths.database_path)?;
            let provider = ProviderConfig {
                display_name: display_name.unwrap_or_else(|| id.clone()),
                id,
                command,
                enabled: true,
                capabilities: ProviderCapability::default(),
            };
            store.upsert_provider(&provider)?;
            println!("added custom provider {}", provider.id);
            Ok(())
        }
        ProviderCommand::Test { provider_id } => {
            let detections = detect_built_in_providers();
            let Some(detection) = detections.into_iter().find(|item| item.id == provider_id) else {
                bail!("unknown provider `{provider_id}`");
            };
            fs::create_dir_all(&paths.data_dir)?;
            let store = Store::open(&paths.database_path)?;
            store.record_provider_probe(&detection)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&detection)?);
            } else if detection.available {
                println!(
                    "{} is available at {}",
                    detection.display_name,
                    detection
                        .binary_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
            } else {
                bail!(
                    "{} is unavailable: {}",
                    detection.display_name,
                    detection
                        .error
                        .unwrap_or_else(|| "unknown error".to_string())
                );
            }
            Ok(())
        }
    }
}

fn ensure_detected_provider_exists(store: &mut Store, provider_id: &str) -> Result<()> {
    let detections = detect_built_in_providers();
    let Some(detection) = detections.into_iter().find(|item| item.id == provider_id) else {
        bail!("unknown built-in provider `{provider_id}`");
    };
    store.record_provider_probe(&detection)?;
    if !detection.available {
        bail!(
            "provider `{provider_id}` is unavailable: {}",
            detection.error.unwrap_or_else(|| "not found".to_string())
        );
    }
    if let Some(config) = built_in_provider_config(&detection) {
        store.upsert_provider(&config)?;
    }
    Ok(())
}

fn create_command(
    paths: &AppPaths,
    from_file: Option<PathBuf>,
    repo: Option<String>,
    provider: Option<String>,
    task: Option<String>,
    timezone: String,
) -> Result<()> {
    let (spec, summary) = if let Some(path) = from_file {
        (read_job_spec(&path)?, None)
    } else {
        build_spec_from_provider(paths, repo, provider, task, timezone)?
    };
    validate_for_cli(&spec)?;
    print_create_confirmation_summary(&spec, summary.as_ref());
    fs::create_dir_all(&paths.data_dir)?;
    let mut store = Store::open(&paths.database_path)?;
    let id = store.create_job(&spec)?;
    println!("created job {} ({})", spec.name, id);
    Ok(())
}

fn build_spec_from_provider(
    paths: &AppPaths,
    repo: Option<String>,
    provider: Option<String>,
    task: Option<String>,
    timezone: String,
) -> Result<(JobSpec, Option<SpecBuilderSummary>)> {
    let repo = repo.context("create without --from-file requires --repo")?;
    let provider_id = provider.context("create without --from-file requires --provider")?;
    let task = task.context("create without --from-file requires --task")?;
    let store = Store::open(&paths.database_path)?;
    let Some(provider) = store.get_provider(&provider_id)? else {
        bail!("provider `{provider_id}` is not configured");
    };
    if !provider.enabled {
        bail!("provider `{provider_id}` is disabled");
    }
    let mut prompt = build_spec_prompt(&SpecBuildRequest {
        provider_id,
        repo_path: repo,
        timezone,
        user_request: task,
    });
    let timeout = Duration::from_secs(provider.capabilities.default_timeout_seconds);
    let mut envelope = None;
    for attempt in 0..=2 {
        let output = run_invocation(
            &build_provider_spec_invocation(&provider, prompt.clone()),
            timeout,
        )?;
        if output.timed_out {
            bail!("spec builder provider timed out");
        }
        if output.exit_code != Some(0) {
            bail!("spec builder provider failed: {}", output.stderr.trim());
        }
        match parse_spec_builder_envelope(&output.stdout) {
            Ok(parsed) => {
                envelope = Some(parsed);
                break;
            }
            Err(error) => {
                if attempt == 2 {
                    bail!(
                        "spec builder returned invalid JSON after repair attempts: {}",
                        error
                    );
                }
                prompt = spec_builder_repair_prompt(&output.stdout, &error.to_string());
            }
        }
    }
    let envelope = envelope.context("spec builder did not return a response")?;
    match envelope.status {
        SpecBuilderStatus::Ok => {
            let summary = envelope.summary;
            let spec = envelope
                .job_spec
                .context("spec builder returned ok without job_spec")?;
            Ok((spec, summary))
        }
        SpecBuilderStatus::NeedsClarification => {
            bail!(
                "spec builder needs clarification: {}",
                envelope.questions.join("; ")
            )
        }
        SpecBuilderStatus::Unsafe => {
            bail!(
                "spec builder marked request unsafe: {}",
                envelope.warnings.join("; ")
            )
        }
        SpecBuilderStatus::Unsupported => {
            bail!(
                "spec builder marked request unsupported: {}",
                envelope.warnings.join("; ")
            )
        }
    }
}

fn spec_builder_repair_prompt(previous_output: &str, error: &str) -> String {
    format!(
        r#"Your previous scheduler spec-builder response was invalid.

Return only valid JSON matching the scheduler spec-builder envelope. Do not wrap it in Markdown.

Parse error:
{error}

Previous invalid response:
{previous_output}
"#
    )
}

fn print_create_confirmation_summary(spec: &JobSpec, summary: Option<&SpecBuilderSummary>) {
    println!("Confirmation summary:");
    if let Some(summary) = summary {
        println!("- {}", summary.human);
        println!("- Schedule: {}", summary.schedule);
        println!("- Task: {}", summary.task);
    }
    println!("- Job: {}", spec.name);
    println!("- Provider: {}", spec.provider_id);
    println!("- Repo: {}", spec.repo.path);
    println!("- Schedule: {}", schedule_summary(spec));
    println!("- Concurrency: {:?}", spec.execution.concurrency);
    println!("- Isolation: {:?}", spec.execution.isolation);
    println!(
        "- Notifications: success={}, failure={}, timeout={}",
        notification_summary(&spec.notifications.on_success),
        notification_summary(&spec.notifications.on_failure),
        notification_summary(&spec.notifications.on_timeout)
    );
}

fn schedule_summary(spec: &JobSpec) -> String {
    match &spec.schedule {
        scheduler_core::ScheduleSpec::Manual {} => "manual".to_string(),
        scheduler_core::ScheduleSpec::Once { at, timezone } => {
            format!(
                "once at {} ({})",
                at.to_rfc3339(),
                timezone.as_deref().unwrap_or("UTC")
            )
        }
        scheduler_core::ScheduleSpec::Interval {
            every,
            unit,
            timezone,
            ..
        } => {
            format!(
                "every {every} {:?} ({})",
                unit,
                timezone.as_deref().unwrap_or("UTC")
            )
        }
        scheduler_core::ScheduleSpec::Cron {
            expression,
            timezone,
            ..
        } => {
            format!("cron `{expression}` ({timezone})")
        }
    }
}

fn notification_summary(channels: &[String]) -> String {
    if channels.is_empty() {
        "none".to_string()
    } else {
        channels.join(",")
    }
}

fn list_command(paths: &AppPaths, json: bool) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let jobs = store.list_jobs()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
    } else if jobs.is_empty() {
        println!("No jobs configured.");
    } else {
        for (id, spec) in jobs {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                id,
                if spec.enabled { "enabled" } else { "disabled" },
                spec.provider_id,
                spec.repo.path,
                spec.name
            );
        }
    }
    Ok(())
}

fn show_command(paths: &AppPaths, job: &str, json: bool) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let Some((id, spec)) = store.get_job_by_name(job)? else {
        bail!("job `{job}` not found");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&spec)?);
    } else {
        println!("Job: {} ({})", spec.name, id);
        println!("Provider: {}", spec.provider_id);
        println!("Repo: {}", spec.repo.path);
        println!("Enabled: {}", spec.enabled);
        println!("Task: {}", spec.task.prompt);
    }
    Ok(())
}

fn edit_command(paths: &AppPaths, job: &str, from_file: &Path) -> Result<()> {
    let spec = read_job_spec(from_file)?;
    validate_for_cli(&spec)?;
    fs::create_dir_all(&paths.data_dir)?;
    let mut store = Store::open(&paths.database_path)?;
    if !store.update_job(job, &spec)? {
        bail!("job `{job}` not found");
    }
    if spec.name == job {
        println!("updated job {job}");
    } else {
        println!("updated job {job} as {}", spec.name);
    }
    Ok(())
}

fn export_command(paths: &AppPaths, job: &str, format: ExportFormat) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let Some((_id, spec)) = store.get_job_by_name(job)? else {
        bail!("job `{job}` not found");
    };
    match format {
        ExportFormat::Json => println!("{}", serde_json::to_string_pretty(&spec)?),
        ExportFormat::Toml => println!("{}", toml::to_string_pretty(&spec)?),
    }
    Ok(())
}

fn config_export_command(paths: &AppPaths, format: ExportFormat) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let store = Store::open(&paths.database_path)?;
    let providers = store.list_providers()?;
    let settings = store.list_settings()?;
    let jobs = store
        .list_jobs()?
        .into_iter()
        .map(|(id, spec)| {
            serde_json::json!({
                "id": id,
                "spec": spec,
            })
        })
        .collect::<Vec<_>>();
    let mut export = serde_json::json!({
        "schema_version": "scheduler.config.v1",
        "generated_at": chrono::Utc::now(),
        "config_dir": paths.config_dir,
        "data_dir": paths.data_dir,
        "database_path": paths.database_path,
        "settings": settings,
        "providers": providers,
        "jobs": jobs,
    });
    redact_json_strings(&mut export);

    match format {
        ExportFormat::Json => println!("{}", serde_json::to_string_pretty(&export)?),
        ExportFormat::Toml => {
            remove_json_nulls(&mut export);
            println!("{}", toml::to_string_pretty(&export)?);
        }
    }
    Ok(())
}

fn redact_json_strings(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => {
            *value = redact_secrets(value);
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json_strings(value);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                redact_json_strings(value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn remove_json_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            values.retain(|value| !value.is_null());
            for value in values {
                remove_json_nulls(value);
            }
        }
        serde_json::Value::Object(values) => {
            values.retain(|_, value| !value.is_null());
            for value in values.values_mut() {
                remove_json_nulls(value);
            }
        }
        serde_json::Value::String(_)
        | serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_) => {}
    }
}

fn import_command(paths: &AppPaths, path: &Path, on_conflict: ImportConflict) -> Result<()> {
    let mut spec = read_job_spec(path)?;
    validate_for_cli(&spec)?;
    fs::create_dir_all(&paths.data_dir)?;
    let mut store = Store::open(&paths.database_path)?;
    if store.get_job_by_name(&spec.name)?.is_some() {
        match on_conflict {
            ImportConflict::Reject => {
                bail!(
                    "job `{}` already exists; use --on-conflict replace|rename",
                    spec.name
                );
            }
            ImportConflict::Replace => {
                store.delete_job(&spec.name)?;
            }
            ImportConflict::Rename => {
                spec.name = next_available_job_name(&store, &spec.name)?;
            }
        }
    }
    let id = store.create_job(&spec)?;
    println!("imported job {} ({})", spec.name, id);
    Ok(())
}

fn next_available_job_name(store: &Store, base_name: &str) -> Result<String> {
    for index in 2..10_000 {
        let candidate = format!("{base_name}-{index}");
        if store.get_job_by_name(&candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    bail!("could not find available name for `{base_name}`")
}

fn set_job_enabled_command(paths: &AppPaths, job: &str, enabled: bool) -> Result<()> {
    let mut store = Store::open(&paths.database_path)?;
    if !store.set_job_enabled(job, enabled)? {
        bail!("job `{job}` not found");
    }
    println!("{} job {job}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

fn delete_command(paths: &AppPaths, job: &str, yes: bool) -> Result<()> {
    if !yes {
        bail!("delete requires --yes");
    }
    let mut store = Store::open(&paths.database_path)?;
    if !store.delete_job(job)? {
        bail!("job `{job}` not found");
    }
    println!("deleted job {job}");
    Ok(())
}

fn run_command(paths: &AppPaths, job: &str) -> Result<()> {
    let mut store = Store::open(&paths.database_path)?;
    let Some((job_id, spec)) = store.get_job_by_name(job)? else {
        bail!("job `{job}` not found");
    };
    let Some(provider) = store.get_provider(&spec.provider_id)? else {
        bail!("provider `{}` is not configured", spec.provider_id);
    };
    let executor = RunExecutor::new(&paths.data_dir);
    let run_id = executor.execute_once(&mut store, job_id, &spec, &provider, RunTrigger::Manual)?;
    println!("completed manual run {run_id} for job {job}");
    Ok(())
}

fn runs_command(paths: &AppPaths, job: &str, json: bool) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let Some((job_id, _spec)) = store.get_job_by_name(job)? else {
        bail!("job `{job}` not found");
    };
    let runs = store.list_runs_for_job(job_id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
    } else if runs.is_empty() {
        println!("No runs for {job}.");
    } else {
        for run in runs {
            println!(
                "{}\t{:?}\t{:?}\t{}",
                run.id, run.status, run.trigger, run.provider_id
            );
        }
    }
    Ok(())
}

fn cancel_run_command(paths: &AppPaths, run_id: &str) -> Result<()> {
    let run_id = run_id.parse()?;
    let mut store = Store::open(&paths.database_path)?;
    let Some(run) = store.get_run(run_id)? else {
        bail!("run `{run_id}` not found");
    };

    match run.status {
        RunStatus::Scheduled | RunStatus::Queued | RunStatus::Preparing => {
            store.transition_run(
                run_id,
                RunStatus::Cancelled,
                Some("manual operator request"),
            )?;
            println!("cancelled run {run_id}");
        }
        RunStatus::Running => {
            store.transition_run(
                run_id,
                RunStatus::Cancelling,
                Some("manual operator request"),
            )?;
            terminate_registered_process(&store, run_id)?;
            store.transition_run(
                run_id,
                RunStatus::Cancelled,
                Some("manual operator request"),
            )?;
            println!("cancelled active run {run_id}");
        }
        RunStatus::Cancelling => {
            terminate_registered_process(&store, run_id)?;
            store.transition_run(
                run_id,
                RunStatus::Cancelled,
                Some("manual operator request"),
            )?;
            println!("cancelled active run {run_id}");
        }
        status @ (RunStatus::Skipped
        | RunStatus::Cancelled
        | RunStatus::Succeeded
        | RunStatus::Failed
        | RunStatus::TimedOut
        | RunStatus::Blocked
        | RunStatus::Lost) => {
            println!("run {run_id} is already {:?}", status);
        }
    }
    Ok(())
}

fn terminate_registered_process(store: &Store, run_id: uuid::Uuid) -> Result<()> {
    if let Some(process) = store.get_run_process(run_id)? {
        terminate_process_group(process.process_group_id)?;
    }
    Ok(())
}

fn retry_command(paths: &AppPaths, run_id: &str) -> Result<()> {
    let run_id = run_id.parse()?;
    let mut store = Store::open(&paths.database_path)?;
    let Some(run) = store.get_run(run_id)? else {
        bail!("run `{run_id}` not found");
    };
    let jobs = store.list_jobs()?;
    let Some((_job_id, spec)) = jobs.into_iter().find(|(job_id, _)| *job_id == run.job_id) else {
        bail!("job for run `{run_id}` not found");
    };
    let retry_id = store.create_run(run.job_id, &spec, RunTrigger::Retry, run.due_at)?;
    println!("created retry run {retry_id} for {run_id}");
    Ok(())
}

fn logs_command(paths: &AppPaths, run_id: &str, follow: bool) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let parsed_run_id = run_id.parse()?;
    if follow {
        return follow_logs(paths, parsed_run_id, run_id);
    }
    let indexed_logs = store.list_run_log_files(parsed_run_id)?;
    if !indexed_logs.is_empty() {
        let mut printed = false;
        for log in indexed_logs {
            let path = PathBuf::from(&log.path);
            if path.exists() {
                println!("== {} ({}) ==", log.stream, log.path);
                print!("{}", fs::read_to_string(path)?);
                printed = true;
            }
        }
        if printed {
            return Ok(());
        }
    }

    let run_dir = paths.data_dir.join("runs").join(run_id);
    let stdout = run_dir.join("provider_stdout.log");
    let stderr = run_dir.join("provider_stderr.log");
    if !stdout.exists() && !stderr.exists() {
        bail!("no logs found for run {run_id}");
    }
    if stdout.exists() {
        println!("== stdout ==");
        print!("{}", fs::read_to_string(stdout)?);
    }
    if stderr.exists() {
        println!("== stderr ==");
        print!("{}", fs::read_to_string(stderr)?);
    }
    Ok(())
}

fn follow_logs(paths: &AppPaths, parsed_run_id: uuid::Uuid, run_id: &str) -> Result<()> {
    let mut offsets = std::collections::HashMap::<PathBuf, u64>::new();
    loop {
        let store = Store::open(&paths.database_path)?;
        let log_paths = log_paths_for_run(&store, paths, parsed_run_id, run_id)?;
        for (stream, path) in log_paths {
            if !path.exists() {
                continue;
            }
            let offset = offsets.entry(path.clone()).or_insert(0);
            let mut file = fs::File::open(&path)?;
            file.seek(SeekFrom::Start(*offset))?;
            let mut chunk = String::new();
            file.read_to_string(&mut chunk)?;
            let next_offset = file.stream_position()?;
            if !chunk.is_empty() {
                println!("== {} ({}) ==", stream, path.display());
                print!("{chunk}");
                *offset = next_offset;
            }
        }
        if let Some(run) = store.get_run(parsed_run_id)?
            && run.status.is_terminal()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn log_paths_for_run(
    store: &Store,
    paths: &AppPaths,
    parsed_run_id: uuid::Uuid,
    run_id: &str,
) -> Result<Vec<(String, PathBuf)>> {
    let indexed_logs = store.list_run_log_files(parsed_run_id)?;
    if !indexed_logs.is_empty() {
        return Ok(indexed_logs
            .into_iter()
            .map(|log| (log.stream, PathBuf::from(log.path)))
            .collect());
    }
    let run_dir = paths.data_dir.join("runs").join(run_id);
    Ok(vec![
        ("stdout".to_string(), run_dir.join("provider_stdout.log")),
        ("stderr".to_string(), run_dir.join("provider_stderr.log")),
        (
            "scheduler".to_string(),
            run_dir.join("scheduler_events.jsonl"),
        ),
    ])
}

fn artifacts_command(paths: &AppPaths, run_id: &str) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let parsed_run_id = run_id.parse()?;
    let indexed = store.list_run_artifacts(parsed_run_id)?;
    if !indexed.is_empty() {
        for artifact in indexed {
            println!("{}\t{}", artifact.kind, artifact.path);
        }
        return Ok(());
    }

    let artifacts_dir = paths.data_dir.join("runs").join(run_id).join("artifacts");
    if !artifacts_dir.exists() {
        bail!("no artifacts directory found for run {run_id}");
    }
    let mut entries = fs::read_dir(artifacts_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    if entries.is_empty() {
        println!("No artifacts for run {run_id}.");
    } else {
        for entry in entries {
            println!("{}", entry.display());
        }
    }
    Ok(())
}

fn tui_command(paths: &AppPaths) -> Result<()> {
    let mut state = initial_tui_state(paths)?;
    if !std::io::stdout().is_terminal() {
        let model = load_tui_model(paths, &state)?;
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend)?;
        terminal.draw(|frame| draw_app(frame, &model, &state))?;
        print_test_backend(terminal.backend());
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    let result = run_tui_loop(paths, &mut terminal, &mut state);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn print_test_backend(backend: &ratatui::backend::TestBackend) {
    let buffer = backend.buffer();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        println!("{}", line.trim_end());
    }
}

fn initial_tui_state(paths: &AppPaths) -> Result<TuiState> {
    bootstrap_detected_providers(paths)?;
    let store = Store::open(&paths.database_path)?;
    let providers = ordered_provider_configs(store.list_providers()?);
    let mut state = TuiState {
        create_repo: std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
            .to_string(),
        ..TuiState::default()
    };

    if providers.is_empty() {
        state.view = TuiView::Providers;
        state.message = Some("No supported providers were found on PATH.".to_string());
        return Ok(state);
    }

    if let Some((index, provider)) = providers
        .iter()
        .enumerate()
        .find(|(_, provider)| provider.enabled)
    {
        state.selected_provider = index;
        state.create_provider = provider.id.clone();
        if store.list_jobs()?.is_empty() {
            state.view = TuiView::Create;
            state.create_field = CreateField::Task;
            state.message = Some(
                "Describe the task and schedule in plain English, then press Enter.".to_string(),
            );
        }
        return Ok(state);
    }

    let index = providers
        .iter()
        .position(|provider| provider.id == "codex")
        .unwrap_or(0);
    state.view = TuiView::Providers;
    state.selected_provider = index;
    state.create_provider = providers[index].id.clone();
    state.message = Some(
        "Select a provider with Space, or press Enter to enable it and create a job.".to_string(),
    );
    Ok(state)
}

fn run_tui_loop(
    paths: &AppPaths,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    state: &mut TuiState,
) -> Result<()> {
    let mut create_child = None;
    loop {
        poll_tui_create_child(state, &mut create_child)?;
        let model = load_tui_model(paths, state)?;
        terminal.draw(|frame| draw_app(frame, &model, state))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if state.view == TuiView::Create
                && handle_tui_create_key(paths, state, &key, &mut create_child)?
            {
                continue;
            }
            if state.editing_filter {
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => state.editing_filter = false,
                    KeyCode::Backspace => {
                        state.filter.pop();
                    }
                    KeyCode::Char(ch) => state.filter.push(ch),
                    _ => {}
                }
                continue;
            }
            if handle_tui_action(paths, state, &model, &key)? {
                continue;
            }
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('/') => state.editing_filter = true,
                KeyCode::Char('s') => state.job_sort = state.job_sort.next(),
                KeyCode::Tab => state.view = state.view.next(),
                KeyCode::BackTab => state.view = state.view.previous(),
                KeyCode::Right => state.view = state.view.next(),
                KeyCode::Left => state.view = state.view.previous(),
                KeyCode::Down | KeyCode::Char('j') => state.move_next(&model),
                KeyCode::Up | KeyCode::Char('k') => state.move_previous(),
                KeyCode::Enter => match state.view {
                    TuiView::Jobs => state.view = TuiView::JobDetail,
                    TuiView::Runs => state.view = TuiView::RunDetail,
                    TuiView::RunDetail => state.view = TuiView::Logs,
                    _ => {}
                },
                KeyCode::Esc => {
                    state.view = match state.view {
                        TuiView::JobDetail => TuiView::Jobs,
                        TuiView::RunDetail | TuiView::Logs => TuiView::Runs,
                        view => view,
                    };
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn poll_tui_create_child(
    state: &mut TuiState,
    create_child: &mut Option<std::process::Child>,
) -> Result<()> {
    let Some(child) = create_child.as_mut() else {
        return Ok(());
    };
    let Some(status) = child.try_wait()? else {
        return Ok(());
    };
    *create_child = None;
    if status.success() {
        state.message = Some("Job created. Review it in Jobs or create another.".to_string());
        state.create_task.clear();
        state.view = TuiView::Jobs;
    } else {
        state.message = Some(
            "Job creation failed. Run the same create command in a shell for details.".to_string(),
        );
    }
    Ok(())
}

fn handle_tui_create_key(
    paths: &AppPaths,
    state: &mut TuiState,
    key: &crossterm::event::KeyEvent,
    create_child: &mut Option<std::process::Child>,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            state.view = TuiView::Dashboard;
            Ok(true)
        }
        KeyCode::Tab | KeyCode::Down => {
            state.create_field = state.create_field.next();
            Ok(true)
        }
        KeyCode::BackTab | KeyCode::Up => {
            state.create_field = state.create_field.previous();
            Ok(true)
        }
        KeyCode::Backspace => {
            create_field_value_mut(state).pop();
            Ok(true)
        }
        KeyCode::Enter => {
            submit_tui_create(paths, state, create_child)?;
            Ok(true)
        }
        KeyCode::Char(ch) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            create_field_value_mut(state).push(ch);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn create_field_value_mut(state: &mut TuiState) -> &mut String {
    match state.create_field {
        CreateField::Provider => &mut state.create_provider,
        CreateField::Repo => &mut state.create_repo,
        CreateField::Task => &mut state.create_task,
        CreateField::Timezone => &mut state.create_timezone,
    }
}

fn submit_tui_create(
    paths: &AppPaths,
    state: &mut TuiState,
    create_child: &mut Option<std::process::Child>,
) -> Result<()> {
    if create_child.is_some() {
        state.message = Some("Job creation is already running in the background.".to_string());
        return Ok(());
    }
    if state.create_provider.trim().is_empty()
        || state.create_repo.trim().is_empty()
        || state.create_task.trim().is_empty()
        || state.create_timezone.trim().is_empty()
    {
        state.message = Some("provider, repo, task, and timezone are required".to_string());
        return Ok(());
    }
    let args = vec![
        "create".to_string(),
        "--repo".to_string(),
        state.create_repo.trim().to_string(),
        "--provider".to_string(),
        state.create_provider.trim().to_string(),
        "--task".to_string(),
        state.create_task.trim().to_string(),
        "--timezone".to_string(),
        state.create_timezone.trim().to_string(),
    ];
    *create_child = Some(spawn_scheduler_child(paths, args)?);
    state.message = Some(format!(
        "Creating job with {} in the background. You can keep navigating.",
        state.create_provider.trim()
    ));
    Ok(())
}

fn handle_tui_action(
    paths: &AppPaths,
    state: &mut TuiState,
    model: &TuiAppModel,
    key: &crossterm::event::KeyEvent,
) -> Result<bool> {
    match key.code {
        KeyCode::Char(' ') if matches!(state.view, TuiView::Providers) => {
            toggle_selected_tui_provider(paths, state, model)?;
            Ok(true)
        }
        KeyCode::Enter if matches!(state.view, TuiView::Providers) => {
            enable_selected_tui_provider_for_create(paths, state, model)?;
            Ok(true)
        }
        KeyCode::Char(' ') if matches!(state.view, TuiView::Jobs | TuiView::JobDetail) => {
            if let Some(job) = model.jobs.get(state.selected_job) {
                let mut store = Store::open(&paths.database_path)?;
                store.set_job_enabled(&job.name, !job.enabled)?;
            }
            Ok(true)
        }
        KeyCode::Char('n') if matches!(state.view, TuiView::Jobs | TuiView::JobDetail) => {
            if let Some(job) = model.jobs.get(state.selected_job) {
                spawn_scheduler_background(paths, vec!["run".to_string(), job.name.clone()])?;
            }
            Ok(true)
        }
        KeyCode::Char('c') if matches!(state.view, TuiView::Runs | TuiView::RunDetail) => {
            if let Some(run) = model.runs.get(state.selected_run) {
                spawn_scheduler_background(paths, vec!["cancel".to_string(), run.id.clone()])?;
            }
            Ok(true)
        }
        KeyCode::Char('r') if matches!(state.view, TuiView::Runs | TuiView::RunDetail) => {
            if let Some(run) = model.runs.get(state.selected_run) {
                spawn_scheduler_background(paths, vec!["retry".to_string(), run.id.clone()])?;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn toggle_selected_tui_provider(
    paths: &AppPaths,
    state: &mut TuiState,
    model: &TuiAppModel,
) -> Result<()> {
    let Some(provider) = model.providers.get(state.selected_provider) else {
        state.message = Some("No provider selected.".to_string());
        return Ok(());
    };
    let mut store = Store::open(&paths.database_path)?;
    let enabled = !provider.enabled;
    store.set_provider_enabled(&provider.id, enabled)?;
    if enabled {
        state.create_provider = provider.id.clone();
    } else if state.create_provider == provider.id {
        state.create_provider.clear();
    }
    state.message = Some(format!(
        "{} provider {}",
        if enabled { "enabled" } else { "disabled" },
        provider.id
    ));
    Ok(())
}

fn enable_selected_tui_provider_for_create(
    paths: &AppPaths,
    state: &mut TuiState,
    model: &TuiAppModel,
) -> Result<()> {
    let Some(provider) = model.providers.get(state.selected_provider) else {
        state.message = Some("No provider selected.".to_string());
        return Ok(());
    };
    if !provider.enabled {
        let mut store = Store::open(&paths.database_path)?;
        store.set_provider_enabled(&provider.id, true)?;
    }
    state.create_provider = provider.id.clone();
    if state.create_repo.trim().is_empty() {
        state.create_repo = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
            .to_string();
    }
    state.view = TuiView::Create;
    state.create_field = CreateField::Task;
    state.message = Some(format!(
        "Using provider {}. Describe the scheduled task.",
        provider.id
    ));
    Ok(())
}

fn spawn_scheduler_background(paths: &AppPaths, args: Vec<String>) -> Result<()> {
    spawn_scheduler_child(paths, args)?;
    Ok(())
}

fn spawn_scheduler_child(paths: &AppPaths, args: Vec<String>) -> Result<std::process::Child> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(daemon_log_path(paths))?;
    let err = log.try_clone()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .arg("--config")
        .arg(&paths.config_dir)
        .args(args)
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err))
        .spawn()?;
    Ok(child)
}

fn load_tui_model(paths: &AppPaths, state: &TuiState) -> Result<TuiAppModel> {
    let store = Store::open(&paths.database_path)?;
    let mut status = daemon_status_snapshot(
        &store,
        paths.database_path.display().to_string(),
        chrono::Utc::now(),
    )?;
    match read_daemon_pid(paths)? {
        Some(pid) if is_process_running(pid) => {
            status.pid = pid;
            status.running = true;
        }
        _ => {
            status.pid = 0;
            status.running = false;
        }
    }

    let jobs_with_metadata = store.list_jobs_with_metadata()?;
    let mut jobs = Vec::new();
    let mut runs = Vec::new();
    for job in &jobs_with_metadata {
        let job_runs = store.list_runs_for_job(job.id)?;
        let last_status = job_runs.first().map(|run| format!("{:?}", run.status));
        let next_due = job
            .spec
            .schedule
            .next_after(chrono::Utc::now())
            .ok()
            .flatten()
            .map(|value| value.to_rfc3339());
        jobs.push(TuiJob {
            id: job.id.to_string(),
            name: job.spec.name.clone(),
            enabled: job.spec.enabled,
            provider_id: job.spec.provider_id.clone(),
            repo_path: job.spec.repo.path.clone(),
            schedule: schedule_summary(&job.spec),
            task: job.spec.task.prompt.clone(),
            next_due,
            last_status,
        });
        for run in job_runs {
            runs.push(TuiRun {
                id: run.id.to_string(),
                job_name: job.spec.name.clone(),
                status: format!("{:?}", run.status),
                trigger: format!("{:?}", run.trigger),
                provider_id: run.provider_id,
                due_at: run.due_at.map(|value| value.to_rfc3339()),
                started_at: run.started_at.map(|value| value.to_rfc3339()),
                finished_at: run.finished_at.map(|value| value.to_rfc3339()),
                worktree_path: run.worktree_path,
                branch: run.branch,
                reason: run.reason,
            });
        }
    }
    apply_tui_job_filter_and_sort(&mut jobs, state);
    runs.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then(right.id.cmp(&left.id))
    });

    let providers = ordered_provider_configs(store.list_providers()?)
        .into_iter()
        .map(|provider| TuiProvider {
            id: provider.id,
            display_name: provider.display_name,
            enabled: provider.enabled,
            command: provider.command.display().to_string(),
            capabilities: provider_capability_labels(&provider.capabilities),
        })
        .collect::<Vec<_>>();
    let settings = store
        .list_settings()?
        .into_iter()
        .map(|setting| TuiSetting {
            key: setting.key,
            value: redact_secrets(&setting.value),
        })
        .collect::<Vec<_>>();
    let selected_run_id = runs
        .get(state.selected_run)
        .and_then(|run| run.id.parse::<uuid::Uuid>().ok());
    let selected_run_logs = if let Some(run_id) = selected_run_id {
        load_tui_logs(&store, run_id)?
    } else {
        vec![]
    };
    let selected_run_artifacts = if let Some(run_id) = selected_run_id {
        store
            .list_run_artifacts(run_id)?
            .into_iter()
            .map(|artifact| format!("{}\t{}", artifact.kind, artifact.path))
            .collect()
    } else {
        vec![]
    };

    Ok(TuiAppModel {
        daemon_online: status.running,
        daemon_pid: (status.pid != 0).then_some(status.pid),
        next_due_run: status.next_due_run.map(|value| value.to_rfc3339()),
        active_runs: status.active_runs,
        jobs,
        runs,
        providers,
        settings,
        selected_run_logs,
        selected_run_artifacts,
    })
}

fn ordered_provider_configs(mut providers: Vec<ProviderConfig>) -> Vec<ProviderConfig> {
    providers.sort_by(|left, right| {
        provider_sort_key(left)
            .cmp(&provider_sort_key(right))
            .then(left.id.cmp(&right.id))
    });
    providers
}

fn provider_sort_key(provider: &ProviderConfig) -> (u8, u8) {
    let enabled_rank = if provider.enabled { 0 } else { 1 };
    let provider_rank = match provider.id.as_str() {
        "codex" => 0,
        "claude" => 1,
        "opencode" => 2,
        _ => 3,
    };
    (enabled_rank, provider_rank)
}

fn apply_tui_job_filter_and_sort(jobs: &mut Vec<TuiJob>, state: &TuiState) {
    if !state.filter.trim().is_empty() {
        let filter = state.filter.to_ascii_lowercase();
        jobs.retain(|job| {
            job.name.to_ascii_lowercase().contains(&filter)
                || job.provider_id.to_ascii_lowercase().contains(&filter)
                || job.repo_path.to_ascii_lowercase().contains(&filter)
                || job
                    .last_status
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&filter)
                || if job.enabled { "enabled" } else { "disabled" }.contains(&filter)
        });
    }
    match state.job_sort {
        scheduler_tui::JobSort::Name => jobs.sort_by(|left, right| left.name.cmp(&right.name)),
        scheduler_tui::JobSort::Provider => {
            jobs.sort_by(|left, right| left.provider_id.cmp(&right.provider_id))
        }
        scheduler_tui::JobSort::Repo => {
            jobs.sort_by(|left, right| left.repo_path.cmp(&right.repo_path))
        }
        scheduler_tui::JobSort::State => jobs.sort_by(|left, right| {
            left.enabled
                .cmp(&right.enabled)
                .reverse()
                .then(left.name.cmp(&right.name))
        }),
        scheduler_tui::JobSort::NextRun => {
            jobs.sort_by(|left, right| left.next_due.cmp(&right.next_due))
        }
        scheduler_tui::JobSort::LastStatus => {
            jobs.sort_by(|left, right| left.last_status.cmp(&right.last_status))
        }
    }
}

fn load_tui_logs(store: &Store, run_id: uuid::Uuid) -> Result<Vec<String>> {
    let mut logs = Vec::new();
    for log in store.list_run_log_files(run_id)? {
        let path = PathBuf::from(&log.path);
        if path.exists() {
            logs.push(format!(
                "== {} ({}) ==\n{}",
                log.stream,
                log.path,
                fs::read_to_string(path)?
            ));
        }
    }
    Ok(logs)
}

fn provider_capability_labels(capabilities: &ProviderCapability) -> Vec<String> {
    let mut labels = Vec::new();
    if capabilities.supports_spec_builder {
        labels.push("spec-builder".to_string());
    }
    if capabilities.supports_task_execution {
        labels.push("execute".to_string());
    }
    if capabilities.supports_non_interactive {
        labels.push("non-interactive".to_string());
    }
    if capabilities.supports_stdin_prompt {
        labels.push("stdin".to_string());
    }
    if capabilities.supports_structured_output {
        labels.push("structured".to_string());
    }
    labels
}

fn doctor_command(paths: &AppPaths, json: bool) -> Result<()> {
    let detections = detect_built_in_providers();
    let report = serde_json::json!({
        "config_dir": paths.config_dir,
        "data_dir": paths.data_dir,
        "database_path": paths.database_path,
        "providers": detections,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Config: {}", paths.config_dir.display());
        println!("Data: {}", paths.data_dir.display());
        println!("Database: {}", paths.database_path.display());
        println!("Providers:");
        for provider in report["providers"].as_array().unwrap() {
            println!(
                "- {}: {}",
                provider["id"].as_str().unwrap_or("-"),
                if provider["available"].as_bool().unwrap_or(false) {
                    "available"
                } else {
                    "missing"
                }
            );
        }
    }
    Ok(())
}

fn daemon_status(paths: &AppPaths, json: bool) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let store = Store::open(&paths.database_path)?;
    let mut status = daemon_status_snapshot(
        &store,
        paths.database_path.display().to_string(),
        chrono::Utc::now(),
    )?;
    match read_daemon_pid(paths)? {
        Some(pid) if is_process_running(pid) => {
            status.pid = pid;
            status.running = true;
        }
        _ => {
            status.pid = 0;
            status.running = false;
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("Database: {}", status.database_path);
        println!("Running: {}", status.running);
        println!(
            "PID: {}",
            if status.pid == 0 {
                "-".to_string()
            } else {
                status.pid.to_string()
            }
        );
        println!("Active runs: {}", status.active_runs);
        println!(
            "Next due run: {}",
            status
                .next_due_run
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "Last heartbeat: {}",
            status
                .heartbeat_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "Last tick: {}",
            status
                .last_tick_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string())
        );
        if let Some(error) = status.last_error {
            println!("Last error: {error}");
        }
    }
    Ok(())
}

fn daemon_command(paths: &AppPaths, command: DaemonCommand, json: bool) -> Result<()> {
    match command {
        DaemonCommand::Tick => {
            fs::create_dir_all(&paths.data_dir)?;
            let mut store = Store::open(&paths.database_path)?;
            let report = scheduler_tick(&mut store, chrono::Utc::now(), 100)?;
            spawn_due_run_executors(paths, &report)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Due actions: {}", report.due_actions.len());
                println!("Queued runs started: {}", report.queued_started.len());
            }
        }
        DaemonCommand::Start => start_daemon(paths, json)?,
        DaemonCommand::Restart => {
            stop_daemon(paths, true, json)?;
            start_daemon(paths, json)?;
        }
        DaemonCommand::Stop => stop_daemon(paths, false, json)?,
        DaemonCommand::Install => install_daemon_service(paths)?,
        DaemonCommand::Uninstall => uninstall_daemon_service()?,
        DaemonCommand::Status => daemon_status(paths, json)?,
        DaemonCommand::Run => run_daemon_loop(paths)?,
        DaemonCommand::ExecRun { run_id } => execute_scheduled_run_command(paths, &run_id)?,
    }
    Ok(())
}

fn start_daemon(paths: &AppPaths, json: bool) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    if let Some(pid) = read_daemon_pid(paths)?
        && is_process_running(pid)
    {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "started": false,
                    "already_running": true,
                    "pid": pid,
                }))?
            );
        } else {
            println!("daemon already running with pid {pid}");
        }
        return Ok(());
    }

    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(daemon_log_path(paths))?;
    let err = log.try_clone()?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("--config")
        .arg(&paths.config_dir)
        .arg("daemon")
        .arg("run")
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err));
    let child = command.spawn()?;
    write_daemon_pid(paths, child.id())?;
    let store = Store::open(&paths.database_path)?;
    let now = chrono::Utc::now().to_rfc3339();
    store.set_setting("daemon.pid", &child.id().to_string())?;
    store.set_setting("daemon.started_at", &now)?;
    store.set_setting("daemon.heartbeat_at", &now)?;
    store.set_setting("daemon.last_error", "")?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "started": true,
                "pid": child.id(),
                "log": daemon_log_path(paths),
            }))?
        );
    } else {
        println!("started daemon pid {}", child.id());
        println!("Log: {}", daemon_log_path(paths).display());
    }
    Ok(())
}

fn stop_daemon(paths: &AppPaths, allow_not_running: bool, json: bool) -> Result<()> {
    let Some(pid) = read_daemon_pid(paths)? else {
        if allow_not_running {
            return Ok(());
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "stopped": false,
                    "running": false,
                }))?
            );
        } else {
            println!("daemon is not running");
        }
        return Ok(());
    };
    if is_process_running(pid) {
        terminate_process(pid)?;
    }
    clear_daemon_pid(paths)?;
    if paths.database_path.exists() {
        let store = Store::open(&paths.database_path)?;
        store.set_setting("daemon.last_error", "")?;
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "stopped": true,
                "pid": pid,
            }))?
        );
    } else if !allow_not_running {
        println!("stopped daemon pid {pid}");
    }
    Ok(())
}

fn run_daemon_loop(paths: &AppPaths) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let owner = daemon_instance_id().to_string();
    let interval = daemon_interval();
    write_daemon_pid(paths, std::process::id())?;
    let mut store = Store::open(&paths.database_path)?;
    let now = chrono::Utc::now().to_rfc3339();
    store.set_setting("daemon.pid", &std::process::id().to_string())?;
    store.set_setting("daemon.started_at", &now)?;
    store.set_setting("daemon.heartbeat_at", &now)?;
    store.set_setting("daemon.last_error", "")?;
    if !acquire_daemon_lock(&mut store, &owner, (interval.as_secs() as i64).max(1) * 3)? {
        bail!("another daemon owns the scheduler lock");
    }
    recover_interrupted_runs(&mut store)?;

    loop {
        let tick_started = chrono::Utc::now();
        store.set_setting("daemon.heartbeat_at", &tick_started.to_rfc3339())?;
        if !acquire_daemon_lock(&mut store, &owner, (interval.as_secs() as i64).max(1) * 3)? {
            store.set_setting(
                "daemon.last_error",
                "another daemon owns the scheduler lock",
            )?;
            std::thread::sleep(interval);
            continue;
        }
        match scheduler_tick(&mut store, tick_started, 100) {
            Ok(report) => {
                store.set_setting("daemon.last_tick_at", &chrono::Utc::now().to_rfc3339())?;
                store.set_setting("daemon.last_error", "")?;
                spawn_due_run_executors(paths, &report)?;
            }
            Err(error) => {
                store.set_setting("daemon.last_error", &error.to_string())?;
            }
        }
        std::thread::sleep(interval);
    }
}

fn execute_scheduled_run_command(paths: &AppPaths, run_id: &str) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let run_id = run_id.parse()?;
    let mut store = Store::open(&paths.database_path)?;
    let Some(run) = store.get_run(run_id)? else {
        bail!("run `{run_id}` not found");
    };
    let Some((job_id, spec)) = store
        .list_jobs()?
        .into_iter()
        .find(|(job_id, _)| *job_id == run.job_id)
    else {
        bail!("job for run `{run_id}` not found");
    };
    let Some(provider) = store.get_provider(&spec.provider_id)? else {
        bail!("provider `{}` is not configured", spec.provider_id);
    };
    let executor = RunExecutor::new(&paths.data_dir);
    executor.execute_existing_run(&mut store, run_id, job_id, &spec, &provider)?;
    Ok(())
}

fn spawn_due_run_executors(paths: &AppPaths, report: &SchedulerTickReport) -> Result<()> {
    let mut run_ids = report.queued_started.clone();
    for action in &report.due_actions {
        if matches!(action.action.as_str(), "start" | "replace")
            && let Some(run_id) = action.run_id
        {
            run_ids.push(run_id);
        }
    }
    run_ids.sort();
    run_ids.dedup();

    for run_id in run_ids {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(daemon_log_path(paths))?;
        let err = log.try_clone()?;
        std::process::Command::new(std::env::current_exe()?)
            .arg("--config")
            .arg(&paths.config_dir)
            .arg("daemon")
            .arg("exec-run")
            .arg(run_id.to_string())
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(err))
            .spawn()?;
    }
    Ok(())
}

fn daemon_interval() -> Duration {
    std::env::var("SCHEDULER_DAEMON_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.max(1)))
        .unwrap_or_else(|| Duration::from_secs(60))
}

fn daemon_pid_path(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("daemon.pid")
}

fn daemon_log_path(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("daemon.log")
}

fn read_daemon_pid(paths: &AppPaths) -> Result<Option<u32>> {
    let path = daemon_pid_path(paths);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    Ok(raw.trim().parse().ok())
}

fn write_daemon_pid(paths: &AppPaths, pid: u32) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    fs::write(daemon_pid_path(paths), pid.to_string())?;
    Ok(())
}

fn clear_daemon_pid(paths: &AppPaths) -> Result<()> {
    let path = daemon_pid_path(paths);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn is_process_running(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> Result<()> {
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    for _ in 0..25 {
        if !is_process_running(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::process::Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
    Ok(())
}

#[cfg(windows)]
fn terminate_process(pid: u32) -> Result<()> {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
    Ok(())
}

fn install_daemon_service(paths: &AppPaths) -> Result<()> {
    let service_path = daemon_service_path()?;
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let executable = std::env::current_exe()?;
    fs::write(
        &service_path,
        daemon_service_definition(&executable, &paths.config_dir)?,
    )?;
    println!("installed daemon service file {}", service_path.display());
    Ok(())
}

fn uninstall_daemon_service() -> Result<()> {
    let service_path = daemon_service_path()?;
    if service_path.exists() {
        fs::remove_file(&service_path)?;
    }
    println!("removed daemon service file {}", service_path.display());
    Ok(())
}

fn daemon_service_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from(home).join("Library/LaunchAgents/dev.scheduler.scheduler.plist"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(PathBuf::from(home).join(".config/systemd/user/scheduler.service"))
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        bail!("daemon service install is not supported on this platform")
    }
}

fn daemon_service_definition(executable: &Path, config_dir: &Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.scheduler.scheduler</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--config</string>
    <string>{}</string>
    <string>daemon</string>
    <string>run</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#,
            xml_escape(&executable.display().to_string()),
            xml_escape(&config_dir.display().to_string())
        ))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(format!(
            r#"[Unit]
Description=Scheduler CLI daemon

[Service]
ExecStart={} --config {} daemon run
Restart=always

[Install]
WantedBy=default.target
"#,
            executable.display(),
            config_dir.display()
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        let _ = config_dir;
        bail!("daemon service install is not supported on this platform")
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn db_command(paths: &AppPaths, command: DbCommand, json: bool) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let store = Store::open(&paths.database_path)?;
    match command {
        DbCommand::Check => {
            let result = store.integrity_check()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "database_path": paths.database_path,
                        "integrity_check": result,
                    }))?
                );
            } else {
                println!("Database: {}", paths.database_path.display());
                println!("Integrity: {result}");
            }
        }
        DbCommand::Migrate => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "database_path": paths.database_path,
                        "migrated": true,
                    }))?
                );
            } else {
                println!("Migrations applied for {}", paths.database_path.display());
            }
        }
    }
    Ok(())
}

fn backup_command(paths: &AppPaths, command: BackupCommand) -> Result<()> {
    match command {
        BackupCommand::Create { output } => {
            fs::create_dir_all(&paths.data_dir)?;
            let _store = Store::open(&paths.database_path)?;
            if backup_output_is_database_file(&output) {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&paths.database_path, &output)?;
            } else {
                create_full_backup(paths, &output)?;
            }
            println!("created backup {}", output.display());
        }
        BackupCommand::Restore { input, yes } => {
            if !yes {
                bail!("restore requires --yes");
            }
            if !input.exists() {
                bail!("backup file does not exist: {}", input.display());
            }
            fs::create_dir_all(&paths.data_dir)?;
            if input.is_dir() {
                restore_full_backup(paths, &input)?;
            } else {
                fs::copy(&input, &paths.database_path)?;
            }
            println!("restored backup from {}", input.display());
        }
    }
    Ok(())
}

fn backup_output_is_database_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("db" | "sqlite" | "sqlite3")
    )
}

fn create_full_backup(paths: &AppPaths, output: &Path) -> Result<()> {
    if output.starts_with(&paths.data_dir) {
        bail!("full backup output must be outside the scheduler data directory");
    }
    fs::create_dir_all(output)?;
    fs::copy(&paths.database_path, output.join("scheduler.sqlite3"))?;
    let data_output = output.join("data");
    if data_output.exists() {
        fs::remove_dir_all(&data_output)?;
    }
    copy_dir_recursive(
        &paths.data_dir,
        &data_output,
        &HashSet::from([
            paths.database_path.clone(),
            daemon_pid_path(paths),
            daemon_log_path(paths),
        ]),
    )?;
    fs::write(
        output.join("scheduler-backup.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": "scheduler.backup.v1",
            "created_at": chrono::Utc::now(),
            "database": "scheduler.sqlite3",
            "data": "data",
            "source_data_dir": paths.data_dir,
        }))?,
    )?;
    Ok(())
}

fn restore_full_backup(paths: &AppPaths, input: &Path) -> Result<()> {
    let database = input.join("scheduler.sqlite3");
    if !database.exists() {
        bail!("full backup is missing scheduler.sqlite3");
    }
    fs::copy(database, &paths.database_path)?;
    let data_input = input.join("data");
    if data_input.exists() {
        copy_dir_recursive(&data_input, &paths.data_dir, &HashSet::new())?;
    }
    let manifest_path = input.join("scheduler-backup.json");
    if manifest_path.exists() {
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
        if let Some(source_data_dir) = manifest["source_data_dir"].as_str() {
            let store = Store::open(&paths.database_path)?;
            store.rewrite_run_log_path_prefix(Path::new(source_data_dir), &paths.data_dir)?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(
    source: &Path,
    destination: &Path,
    excluded_paths: &HashSet<PathBuf>,
) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if excluded_paths.contains(&path) {
            continue;
        }
        let target = destination.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target, excluded_paths)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, target)?;
        }
    }
    Ok(())
}

fn cleanup_command(paths: &AppPaths, dry_run: bool, json: bool) -> Result<()> {
    let store = Store::open(&paths.database_path)?;
    let report = cleanup_retained_worktrees(&store, chrono::Utc::now(), dry_run)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let action = if dry_run { "Would remove" } else { "Removed" };
        for path in &report.removed_worktrees {
            println!("{action}: {}", path.display());
        }
        for path in &report.kept_worktrees {
            println!("Kept: {}", path.display());
        }
        for path in &report.missing_worktrees {
            println!("Missing: {}", path.display());
        }
    }
    Ok(())
}

fn read_job_spec(path: &Path) -> Result<JobSpec> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read job spec {}", path.display()))?;
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::from_str(&content).context("failed to parse TOML job spec"),
        Some("json") => serde_json::from_str(&content).context("failed to parse JSON job spec"),
        _ => serde_json::from_str(&content)
            .or_else(|_| toml::from_str(&content))
            .context("failed to parse job spec as JSON or TOML"),
    }
}

fn validate_for_cli(spec: &JobSpec) -> Result<()> {
    let enabled_providers = detect_built_in_providers()
        .into_iter()
        .filter(|provider| provider.available)
        .map(|provider| provider.id)
        .collect::<HashSet<_>>();
    let context = ValidationContext {
        enabled_provider_ids: enabled_providers,
        require_enabled_provider: false,
        require_existing_repo: false,
    };
    if let Err(errors) = validate_job_spec(spec, &context) {
        for error in &errors {
            eprintln!("validation error: {error}");
        }
        bail!("job spec validation failed");
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct AppPaths {
    config_dir: PathBuf,
    data_dir: PathBuf,
    database_path: PathBuf,
}

impl AppPaths {
    fn new(config_override: Option<&Path>) -> Result<Self> {
        let (config_dir, data_dir) = if let Some(config_dir) = config_override {
            let config_dir = config_dir.to_path_buf();
            let data_dir = config_dir.join("data");
            (config_dir, data_dir)
        } else {
            let dirs = ProjectDirs::from("dev", "scheduler", "scheduler")
                .context("failed to resolve platform directories")?;
            (
                dirs.config_dir().to_path_buf(),
                dirs.data_dir().to_path_buf(),
            )
        };
        let database_path = data_dir.join("scheduler.sqlite3");
        Ok(Self {
            config_dir,
            data_dir,
            database_path,
        })
    }
}

#[allow(dead_code)]
fn parse_builder_output_for_future_cli_use(input: &str) -> Result<()> {
    parse_spec_builder_envelope(input)?;
    Ok(())
}
