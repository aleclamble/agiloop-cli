use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use scheduler_core::{
    BranchTemplateContext, CleanupPolicy, ConcurrencyDecision, IsolationMode, JobSpec, RunStatus,
    RunTrigger, ScheduleSpec, decide_concurrency, render_branch_template,
};
use scheduler_git::{GitError, canonicalize_repo, create_worktree, fetch_repo, resolve_base_ref};
use scheduler_logs::redact_secrets;
use scheduler_provider::{
    ProviderConfig, RunExecutionRequest, build_provider_run_invocation,
    run_invocation_with_observer_and_cancellation,
};
use scheduler_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatus {
    pub pid: u32,
    pub running: bool,
    pub database_path: String,
    pub active_runs: usize,
    pub next_due_run: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub last_tick_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub fn next_due(
    schedule: &ScheduleSpec,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    schedule.next_after(after)
}

pub fn concurrency_decision(
    policy: scheduler_core::ConcurrencyPolicy,
    active_runs_for_same_job: usize,
) -> scheduler_core::ConcurrencyDecision {
    decide_concurrency(policy, active_runs_for_same_job)
}

pub fn daemon_instance_id() -> Uuid {
    Uuid::new_v4()
}

pub fn daemon_status_snapshot(
    store: &Store,
    database_path: impl Into<String>,
    now: DateTime<Utc>,
) -> Result<DaemonStatus, ExecutionError> {
    let mut active_runs = 0;
    let mut next_due_run = None;
    for job in store.list_jobs_with_metadata()? {
        active_runs += store.list_active_runs_for_job(job.id)?.len();
        if !job.spec.enabled {
            continue;
        }
        if let Some(next) = job
            .spec
            .schedule
            .next_after(now)
            .map_err(ExecutionError::Schedule)?
            && next_due_run.map(|current| next < current).unwrap_or(true)
        {
            next_due_run = Some(next);
        }
    }
    let pid = store
        .get_setting("daemon.pid")?
        .and_then(|setting| setting.value.parse::<u32>().ok())
        .unwrap_or_else(std::process::id);
    let started_at = store
        .get_setting("daemon.started_at")?
        .and_then(|setting| parse_rfc3339_utc(&setting.value).ok());
    let heartbeat_at = store
        .get_setting("daemon.heartbeat_at")?
        .and_then(|setting| parse_rfc3339_utc(&setting.value).ok());
    let last_tick_at = store
        .get_setting("daemon.last_tick_at")?
        .and_then(|setting| parse_rfc3339_utc(&setting.value).ok());
    let last_error = store
        .get_setting("daemon.last_error")?
        .map(|setting| setting.value);

    Ok(DaemonStatus {
        pid,
        running: heartbeat_at.is_some(),
        database_path: database_path.into(),
        active_runs,
        next_due_run,
        started_at,
        heartbeat_at,
        last_tick_at,
        last_error,
    })
}

fn parse_rfc3339_utc(value: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(value).map(|value| value.with_timezone(&Utc))
}

pub fn acquire_daemon_lock(
    store: &mut Store,
    owner: &str,
    ttl_seconds: i64,
) -> Result<bool, StoreError> {
    let expires_at = Utc::now() + chrono::Duration::seconds(ttl_seconds);
    store.acquire_lock("daemon", owner, Some(expires_at))
}

pub fn release_daemon_lock(store: &mut Store, owner: &str) -> Result<bool, StoreError> {
    store.release_lock("daemon", owner)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RestartRecoveryReport {
    pub recovered_runs: Vec<Uuid>,
}

pub fn recover_interrupted_runs(
    store: &mut Store,
) -> Result<RestartRecoveryReport, ExecutionError> {
    let notifier = SystemNotifier::from_env();
    recover_interrupted_runs_with_notifier(store, &notifier)
}

pub fn recover_interrupted_runs_with_notifier<N: NotificationSink + ?Sized>(
    store: &mut Store,
    notifier: &N,
) -> Result<RestartRecoveryReport, ExecutionError> {
    let jobs = store.list_jobs_with_metadata()?;
    let mut report = RestartRecoveryReport::default();
    for run in store.list_active_runs()? {
        let Some(job) = jobs.iter().find(|job| job.id == run.job_id) else {
            continue;
        };
        store.transition_run(
            run.id,
            RunStatus::Lost,
            Some("marked lost during daemon restart recovery"),
        )?;
        dispatch_run_status_notification_by_id(store, &job.spec, run.id, notifier);
        report.recovered_runs.push(run.id);
    }
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueRunAction {
    Start(Uuid),
    Skipped(Uuid),
    Queued(Uuid),
    Replace {
        cancelled_run_ids: Vec<Uuid>,
        new_run_id: Uuid,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SchedulerTickReport {
    pub due_actions: Vec<DueRunActionSummary>,
    pub queued_started: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DueRunActionSummary {
    pub job_id: Uuid,
    pub due_at: DateTime<Utc>,
    pub action: String,
    pub run_id: Option<Uuid>,
}

pub fn scheduler_tick(
    store: &mut Store,
    now: DateTime<Utc>,
    max_backfill: usize,
) -> Result<SchedulerTickReport, ExecutionError> {
    let notifier = SystemNotifier::from_env();
    scheduler_tick_with_notifier(store, now, max_backfill, &notifier)
}

pub fn scheduler_tick_with_notifier<N: NotificationSink + ?Sized>(
    store: &mut Store,
    now: DateTime<Utc>,
    max_backfill: usize,
    notifier: &N,
) -> Result<SchedulerTickReport, ExecutionError> {
    let mut report = SchedulerTickReport::default();
    for job in store.list_jobs_with_metadata()? {
        if !job.spec.enabled {
            continue;
        }
        let last_due_at = store.last_due_at_for_job(job.id)?;
        let due_times =
            due_times_for_job(&job.spec, job.created_at, last_due_at, now, max_backfill)
                .map_err(ExecutionError::Schedule)?;
        for due_at in due_times {
            let action =
                apply_due_run_policy_with_notifier(store, job.id, &job.spec, due_at, notifier)?;
            report.due_actions.push(DueRunActionSummary {
                job_id: job.id,
                due_at,
                action: due_action_name(&action).to_string(),
                run_id: due_action_run_id(&action),
            });
        }
        if let Some(run_id) = start_next_queued_run_if_unblocked(store, job.id, &job.spec)? {
            report.queued_started.push(run_id);
        }
    }
    Ok(report)
}

pub fn due_times_for_job(
    spec: &JobSpec,
    job_created_at: DateTime<Utc>,
    last_due_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    max_backfill: usize,
) -> Result<Vec<DateTime<Utc>>, String> {
    use scheduler_core::ScheduleSpec;

    match &spec.schedule {
        ScheduleSpec::Manual {} => Ok(vec![]),
        ScheduleSpec::Once { at, .. } => {
            if last_due_at.is_none() && *at <= now {
                Ok(vec![*at])
            } else {
                Ok(vec![])
            }
        }
        ScheduleSpec::Cron { misfire_policy, .. }
        | ScheduleSpec::Interval { misfire_policy, .. } => {
            let mut cursor = last_due_at
                .unwrap_or_else(|| initial_schedule_cursor(&spec.schedule, job_created_at));
            let mut due = Vec::new();
            while let Some(next) = spec.schedule.next_after(cursor)? {
                if next > now {
                    break;
                }
                due.push(next);
                cursor = next;
                if due.len() >= max_backfill.max(1) {
                    break;
                }
            }
            Ok(apply_misfire_policy(due, *misfire_policy, now))
        }
    }
}

fn initial_schedule_cursor(
    schedule: &ScheduleSpec,
    job_created_at: DateTime<Utc>,
) -> DateTime<Utc> {
    match schedule {
        ScheduleSpec::Interval {
            start_at: Some(start_at),
            ..
        } => *start_at - chrono::Duration::seconds(1),
        _ => job_created_at,
    }
}

fn apply_misfire_policy(
    mut due: Vec<DateTime<Utc>>,
    policy: scheduler_core::MisfirePolicy,
    now: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    match policy {
        scheduler_core::MisfirePolicy::Skip => due
            .pop()
            .filter(|due_at| *due_at == now)
            .into_iter()
            .collect(),
        scheduler_core::MisfirePolicy::RunOnce => due.pop().into_iter().collect(),
        scheduler_core::MisfirePolicy::Backfill => due,
    }
}

fn start_next_queued_run_if_unblocked(
    store: &mut Store,
    job_id: Uuid,
    spec: &JobSpec,
) -> Result<Option<Uuid>, StoreError> {
    if !store.list_active_runs_for_job(job_id)?.is_empty() {
        return Ok(None);
    }
    if spec.execution.repo_lock == scheduler_core::RepoLockPolicy::Exclusive
        && !store.list_active_runs_for_repo(&spec.repo.path)?.is_empty()
    {
        return Ok(None);
    }
    let Some(run) = store.list_queued_runs_for_job(job_id)?.into_iter().next() else {
        return Ok(None);
    };
    store.transition_run(run.id, RunStatus::Preparing, Some("started from queue"))?;
    Ok(Some(run.id))
}

fn due_action_name(action: &DueRunAction) -> &'static str {
    match action {
        DueRunAction::Start(_) => "start",
        DueRunAction::Skipped(_) => "skipped",
        DueRunAction::Queued(_) => "queued",
        DueRunAction::Replace { .. } => "replace",
    }
}

fn due_action_run_id(action: &DueRunAction) -> Option<Uuid> {
    match action {
        DueRunAction::Start(run_id)
        | DueRunAction::Skipped(run_id)
        | DueRunAction::Queued(run_id) => Some(*run_id),
        DueRunAction::Replace { new_run_id, .. } => Some(*new_run_id),
    }
}

pub fn apply_due_run_policy(
    store: &mut Store,
    job_id: Uuid,
    spec: &JobSpec,
    due_at: DateTime<Utc>,
) -> Result<DueRunAction, StoreError> {
    let notifier = SystemNotifier::from_env();
    apply_due_run_policy_with_notifier(store, job_id, spec, due_at, &notifier)
}

pub fn apply_due_run_policy_with_notifier<N: NotificationSink + ?Sized>(
    store: &mut Store,
    job_id: Uuid,
    spec: &JobSpec,
    due_at: DateTime<Utc>,
    notifier: &N,
) -> Result<DueRunAction, StoreError> {
    let active_runs = store.list_active_runs_for_job(job_id)?;
    match decide_concurrency(spec.execution.concurrency, active_runs.len()) {
        ConcurrencyDecision::Start => {
            if spec.execution.repo_lock == scheduler_core::RepoLockPolicy::Exclusive
                && !store.list_active_runs_for_repo(&spec.repo.path)?.is_empty()
            {
                let run_id = store.create_run(job_id, spec, RunTrigger::Scheduled, Some(due_at))?;
                store.transition_run(
                    run_id,
                    RunStatus::Queued,
                    Some("queued by exclusive repo lock"),
                )?;
                return Ok(DueRunAction::Queued(run_id));
            }
            let run_id = store.create_run(job_id, spec, RunTrigger::Scheduled, Some(due_at))?;
            Ok(DueRunAction::Start(run_id))
        }
        ConcurrencyDecision::Skip => {
            let run_id = store.create_run(job_id, spec, RunTrigger::Scheduled, Some(due_at))?;
            store.transition_run(
                run_id,
                RunStatus::Skipped,
                Some("skipped by concurrency policy"),
            )?;
            dispatch_run_status_notification_by_id(store, spec, run_id, notifier);
            Ok(DueRunAction::Skipped(run_id))
        }
        ConcurrencyDecision::Queue => {
            let run_id = store.create_run(job_id, spec, RunTrigger::Scheduled, Some(due_at))?;
            store.transition_run(
                run_id,
                RunStatus::Queued,
                Some("queued by concurrency policy"),
            )?;
            Ok(DueRunAction::Queued(run_id))
        }
        ConcurrencyDecision::Replace => {
            let mut cancelled_run_ids = Vec::new();
            for run in active_runs {
                if run.status == RunStatus::Running {
                    store.transition_run(
                        run.id,
                        RunStatus::Cancelling,
                        Some("replaced by new scheduled run"),
                    )?;
                    store.transition_run(
                        run.id,
                        RunStatus::Cancelled,
                        Some("replaced by new scheduled run"),
                    )?;
                } else {
                    store.transition_run(
                        run.id,
                        RunStatus::Cancelled,
                        Some("replaced by new scheduled run"),
                    )?;
                }
                cancelled_run_ids.push(run.id);
                dispatch_run_status_notification_by_id(store, spec, run.id, notifier);
            }
            let new_run_id = store.create_run(job_id, spec, RunTrigger::Scheduled, Some(due_at))?;
            Ok(DueRunAction::Replace {
                cancelled_run_ids,
                new_run_id,
            })
        }
    }
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("provider error: {0}")]
    Provider(#[from] scheduler_provider::ProviderError),
    #[error("filesystem error: {0}")]
    Filesystem(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("schedule error: {0}")]
    Schedule(String),
    #[error("provider `{0}` is disabled")]
    ProviderDisabled(String),
    #[error("run `{0}` was not found")]
    RunNotFound(Uuid),
    #[error("run `{run_id}` belongs to job `{actual_job_id}`, expected `{expected_job_id}`")]
    RunJobMismatch {
        run_id: Uuid,
        expected_job_id: Uuid,
        actual_job_id: Uuid,
    },
    #[error("run `{run_id}` is {status:?} and cannot be executed")]
    InvalidRunStatus { run_id: Uuid, status: RunStatus },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEventKind {
    RunSucceeded,
    RunFailed,
    RunTimedOut,
    RunCancelled,
    JobSkipped,
    ProviderUnavailable,
    DaemonError,
}

impl NotificationEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunSucceeded => "run_succeeded",
            Self::RunFailed => "run_failed",
            Self::RunTimedOut => "run_timed_out",
            Self::RunCancelled => "run_cancelled",
            Self::JobSkipped => "job_skipped",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::DaemonError => "daemon_error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationEvent {
    pub kind: NotificationEventKind,
    pub job_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub job_name: String,
    pub provider_id: String,
    pub status: Option<RunStatus>,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl NotificationEvent {
    fn for_run(
        spec: &JobSpec,
        run: &scheduler_core::RunRecord,
        kind: NotificationEventKind,
    ) -> Self {
        Self {
            kind,
            job_id: Some(run.job_id),
            run_id: Some(run.id),
            job_name: spec.name.clone(),
            provider_id: run.provider_id.clone(),
            status: Some(run.status),
            message: run.reason.clone(),
            created_at: Utc::now(),
        }
    }

    fn provider_unavailable(job_id: Uuid, spec: &JobSpec, provider_id: &str) -> Self {
        Self {
            kind: NotificationEventKind::ProviderUnavailable,
            job_id: Some(job_id),
            run_id: None,
            job_name: spec.name.clone(),
            provider_id: provider_id.to_string(),
            status: None,
            message: Some(format!("provider `{provider_id}` is unavailable")),
            created_at: Utc::now(),
        }
    }

    fn title(&self) -> String {
        match self.kind {
            NotificationEventKind::RunSucceeded => format!("{} succeeded", self.job_name),
            NotificationEventKind::RunFailed => format!("{} failed", self.job_name),
            NotificationEventKind::RunTimedOut => format!("{} timed out", self.job_name),
            NotificationEventKind::RunCancelled => format!("{} cancelled", self.job_name),
            NotificationEventKind::JobSkipped => format!("{} skipped", self.job_name),
            NotificationEventKind::ProviderUnavailable => {
                format!("{} provider unavailable", self.job_name)
            }
            NotificationEventKind::DaemonError => "scheduler daemon error".to_string(),
        }
    }

    fn body(&self) -> String {
        self.message
            .clone()
            .unwrap_or_else(|| self.kind.as_str().replace('_', " "))
    }
}

#[derive(Debug, Error)]
pub enum NotificationError {
    #[error("unknown notification channel `{0}`")]
    UnknownChannel(String),
    #[error("webhook URL is not configured")]
    MissingWebhookUrl,
    #[error("local notifications are unsupported: {0}")]
    UnsupportedLocal(String),
    #[error("local notification command failed: {0}")]
    LocalCommand(String),
    #[error("webhook request failed: {0}")]
    Webhook(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

pub trait NotificationSink {
    fn notify(&self, channel: &str, event: &NotificationEvent) -> Result<(), NotificationError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopNotifier;

impl NotificationSink for NoopNotifier {
    fn notify(&self, _channel: &str, _event: &NotificationEvent) -> Result<(), NotificationError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct LocalNotifier {
    command_override: Option<PathBuf>,
}

impl LocalNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_command(command: impl Into<PathBuf>) -> Self {
        Self {
            command_override: Some(command.into()),
        }
    }

    pub fn notify_event(&self, event: &NotificationEvent) -> Result<(), NotificationError> {
        let title = event.title();
        let body = event.body();
        let mut command = self.local_command(&title, &body)?;
        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(NotificationError::LocalCommand(format!(
                "command exited with status {status}"
            )))
        }
    }

    fn local_command(&self, title: &str, body: &str) -> Result<Command, NotificationError> {
        if let Some(command) = &self.command_override {
            let mut command = Command::new(command);
            command.arg(title).arg(body);
            return Ok(command);
        }

        #[cfg(target_os = "macos")]
        {
            let mut command = Command::new("osascript");
            command.arg("-e").arg(format!(
                "display notification \"{}\" with title \"{}\"",
                escape_applescript(body),
                escape_applescript(title)
            ));
            Ok(command)
        }

        #[cfg(target_os = "linux")]
        {
            let mut command = Command::new("notify-send");
            command.arg(title).arg(body);
            Ok(command)
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = title;
            let _ = body;
            Err(NotificationError::UnsupportedLocal(
                "no local adapter for this platform".to_string(),
            ))
        }
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Clone)]
pub struct WebhookNotifier {
    url: String,
    timeout: Duration,
}

impl WebhookNotifier {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_timeout(url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            url: url.into(),
            timeout,
        }
    }

    pub fn notify_event(&self, event: &NotificationEvent) -> Result<(), NotificationError> {
        let payload = serde_json::json!({
            "type": event.kind.as_str(),
            "event": event,
        });
        let body = serde_json::to_string(&payload)?;
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        agent
            .post(&self.url)
            .set("content-type", "application/json")
            .send_string(&body)
            .map(|_| ())
            .map_err(|error| NotificationError::Webhook(redact_secrets(&error.to_string())))
    }
}

#[derive(Debug, Clone)]
pub struct SystemNotifier {
    local: LocalNotifier,
    webhook_url: Option<String>,
}

impl SystemNotifier {
    pub fn from_env() -> Self {
        Self {
            local: LocalNotifier::new(),
            webhook_url: std::env::var("SCHEDULER_WEBHOOK_URL").ok(),
        }
    }

    pub fn with_webhook_url(url: impl Into<String>) -> Self {
        Self {
            local: LocalNotifier::new(),
            webhook_url: Some(url.into()),
        }
    }
}

impl Default for SystemNotifier {
    fn default() -> Self {
        Self::from_env()
    }
}

impl NotificationSink for SystemNotifier {
    fn notify(&self, channel: &str, event: &NotificationEvent) -> Result<(), NotificationError> {
        let channel = channel.trim();
        match channel {
            "" | "none" => Ok(()),
            "local" => self.local.notify_event(event),
            "webhook" => {
                let url = self
                    .webhook_url
                    .as_deref()
                    .ok_or(NotificationError::MissingWebhookUrl)?;
                WebhookNotifier::new(url).notify_event(event)
            }
            value if value.starts_with("webhook:") => {
                WebhookNotifier::new(value.trim_start_matches("webhook:")).notify_event(event)
            }
            value => Err(NotificationError::UnknownChannel(value.to_string())),
        }
    }
}

fn dispatch_notifications<N: NotificationSink + ?Sized>(
    store: &Store,
    channels: &[String],
    event: &NotificationEvent,
    notifier: &N,
) {
    for channel in normalized_notification_channels(channels) {
        let result = notifier.notify(&channel, event);
        let (status, message) = match result {
            Ok(()) => ("delivered", None),
            Err(error) => ("failed", Some(redact_secrets(&error.to_string()))),
        };
        let _ = store.record_notification_delivery(
            event.run_id,
            event.job_id,
            event.kind.as_str(),
            &redact_notification_channel(&channel),
            status,
            message.as_deref(),
        );
    }
}

fn normalized_notification_channels(channels: &[String]) -> Vec<String> {
    if channels
        .iter()
        .any(|channel| channel.trim().eq_ignore_ascii_case("none"))
    {
        return Vec::new();
    }
    let mut normalized = Vec::new();
    for channel in channels {
        let trimmed = channel.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == trimmed) {
            normalized.push(trimmed.to_string());
        }
    }
    normalized
}

fn redact_notification_channel(channel: &str) -> String {
    if let Some(url) = channel.strip_prefix("webhook:") {
        format!("webhook:{}", redact_secrets(url))
    } else {
        redact_secrets(channel)
    }
}

fn notification_kind_for_status(status: RunStatus) -> Option<NotificationEventKind> {
    match status {
        RunStatus::Succeeded => Some(NotificationEventKind::RunSucceeded),
        RunStatus::Failed | RunStatus::Blocked | RunStatus::Lost => {
            Some(NotificationEventKind::RunFailed)
        }
        RunStatus::TimedOut => Some(NotificationEventKind::RunTimedOut),
        RunStatus::Cancelled => Some(NotificationEventKind::RunCancelled),
        RunStatus::Skipped => Some(NotificationEventKind::JobSkipped),
        RunStatus::Scheduled
        | RunStatus::Queued
        | RunStatus::Preparing
        | RunStatus::Running
        | RunStatus::Cancelling => None,
    }
}

fn notification_channels_for_status(spec: &JobSpec, status: RunStatus) -> &[String] {
    match status {
        RunStatus::Succeeded => &spec.notifications.on_success,
        RunStatus::TimedOut => &spec.notifications.on_timeout,
        RunStatus::Failed
        | RunStatus::Blocked
        | RunStatus::Lost
        | RunStatus::Cancelled
        | RunStatus::Skipped => &spec.notifications.on_failure,
        RunStatus::Scheduled
        | RunStatus::Queued
        | RunStatus::Preparing
        | RunStatus::Running
        | RunStatus::Cancelling => &[],
    }
}

fn dispatch_run_status_notification<N: NotificationSink + ?Sized>(
    store: &Store,
    spec: &JobSpec,
    run: &scheduler_core::RunRecord,
    notifier: &N,
) {
    let Some(kind) = notification_kind_for_status(run.status) else {
        return;
    };
    let event = NotificationEvent::for_run(spec, run, kind);
    dispatch_notifications(
        store,
        notification_channels_for_status(spec, run.status),
        &event,
        notifier,
    );
}

fn dispatch_run_status_notification_by_id<N: NotificationSink + ?Sized>(
    store: &Store,
    spec: &JobSpec,
    run_id: Uuid,
    notifier: &N,
) {
    if let Ok(Some(run)) = store.get_run(run_id) {
        dispatch_run_status_notification(store, spec, &run, notifier);
    }
}

fn mark_run_failed_and_notify<N: NotificationSink + ?Sized>(
    store: &mut Store,
    spec: &JobSpec,
    run_id: Uuid,
    reason: &str,
    notifier: &N,
) {
    let _ = store.clear_run_process(run_id);
    let _ = store.transition_run(run_id, RunStatus::Failed, Some(reason));
    dispatch_run_status_notification_by_id(store, spec, run_id, notifier);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CleanupReport {
    pub removed_worktrees: Vec<PathBuf>,
    pub kept_worktrees: Vec<PathBuf>,
    pub missing_worktrees: Vec<PathBuf>,
}

pub fn cleanup_retained_worktrees(
    store: &Store,
    now: DateTime<Utc>,
    dry_run: bool,
) -> Result<CleanupReport, ExecutionError> {
    let mut report = CleanupReport::default();
    for (job_id, spec) in store.list_jobs()? {
        for run in store.list_runs_for_job(job_id)? {
            let Some(worktree_path) = run.worktree_path.as_ref().map(PathBuf::from) else {
                continue;
            };
            if !run.status.is_terminal() || run.status == RunStatus::Skipped {
                report.kept_worktrees.push(worktree_path);
                continue;
            }
            if !worktree_path.exists() {
                report.missing_worktrees.push(worktree_path);
                continue;
            }
            if should_remove_worktree(&spec, &run, now) {
                if !dry_run {
                    fs::remove_dir_all(&worktree_path)?;
                }
                report.removed_worktrees.push(worktree_path);
            } else {
                report.kept_worktrees.push(worktree_path);
            }
        }
    }
    Ok(report)
}

fn should_remove_worktree(
    spec: &JobSpec,
    run: &scheduler_core::RunRecord,
    now: DateTime<Utc>,
) -> bool {
    let policy = if run.status == RunStatus::Succeeded {
        spec.execution.worktree_cleanup.on_success
    } else {
        spec.execution.worktree_cleanup.on_failure
    };
    match policy {
        CleanupPolicy::Keep => false,
        CleanupPolicy::RemoveImmediately => true,
        CleanupPolicy::AfterRetention => run
            .finished_at
            .map(|finished_at| {
                finished_at
                    + chrono::Duration::days(spec.execution.worktree_cleanup.retention_days as i64)
                    <= now
            })
            .unwrap_or(false),
    }
}

#[derive(Debug, Clone)]
pub struct RunExecutor {
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunContextPaths {
    pub root: PathBuf,
    pub artifacts_dir: PathBuf,
    pub context_json: PathBuf,
    pub execution_prompt: PathBuf,
    pub provider_stdout: PathBuf,
    pub provider_stderr: PathBuf,
    pub scheduler_events: PathBuf,
    pub summary_json: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunSummary {
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<RunSummaryArtifact>,
    #[serde(default)]
    pub files_changed: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub pull_request_url: Option<String>,
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunSummaryArtifact {
    pub path: String,
    pub kind: String,
}

impl RunExecutor {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    pub fn execute_once(
        &self,
        store: &mut Store,
        job_id: Uuid,
        spec: &JobSpec,
        provider: &ProviderConfig,
        trigger: RunTrigger,
    ) -> Result<Uuid, ExecutionError> {
        let notifier = SystemNotifier::from_env();
        self.execute_once_with_notifier(store, job_id, spec, provider, trigger, &notifier)
    }

    pub fn execute_once_with_notifier<N: NotificationSink + ?Sized>(
        &self,
        store: &mut Store,
        job_id: Uuid,
        spec: &JobSpec,
        provider: &ProviderConfig,
        trigger: RunTrigger,
        notifier: &N,
    ) -> Result<Uuid, ExecutionError> {
        if !provider.enabled {
            let event = NotificationEvent::provider_unavailable(job_id, spec, &provider.id);
            dispatch_notifications(store, &spec.notifications.on_failure, &event, notifier);
            return Err(ExecutionError::ProviderDisabled(provider.id.clone()));
        }

        let run_id = store.create_run(job_id, spec, trigger, None)?;
        self.execute_existing_run_with_notifier(store, run_id, job_id, spec, provider, notifier)
    }

    pub fn execute_existing_run(
        &self,
        store: &mut Store,
        run_id: Uuid,
        job_id: Uuid,
        spec: &JobSpec,
        provider: &ProviderConfig,
    ) -> Result<Uuid, ExecutionError> {
        let notifier = SystemNotifier::from_env();
        self.execute_existing_run_with_notifier(store, run_id, job_id, spec, provider, &notifier)
    }

    pub fn execute_existing_run_with_notifier<N: NotificationSink + ?Sized>(
        &self,
        store: &mut Store,
        run_id: Uuid,
        job_id: Uuid,
        spec: &JobSpec,
        provider: &ProviderConfig,
        notifier: &N,
    ) -> Result<Uuid, ExecutionError> {
        let Some(run) = store.get_run(run_id)? else {
            return Err(ExecutionError::RunNotFound(run_id));
        };
        if run.job_id != job_id {
            return Err(ExecutionError::RunJobMismatch {
                run_id,
                expected_job_id: job_id,
                actual_job_id: run.job_id,
            });
        }
        match run.status {
            RunStatus::Scheduled | RunStatus::Queued => {
                store.transition_run(run_id, RunStatus::Preparing, None)?;
            }
            RunStatus::Preparing => {}
            status => {
                return Err(ExecutionError::InvalidRunStatus { run_id, status });
            }
        }
        if !provider.enabled {
            let reason = format!("provider `{}` is disabled", provider.id);
            mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
            return Err(ExecutionError::ProviderDisabled(provider.id.clone()));
        }
        let paths = match self.create_run_context(run_id, spec) {
            Ok(paths) => paths,
            Err(error) => {
                let reason = error.to_string();
                mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
                return Err(error);
            }
        };
        let prompt = execution_prompt(spec, &paths);
        if let Err(error) = fs::write(&paths.execution_prompt, &prompt) {
            let error = ExecutionError::Filesystem(error);
            let reason = error.to_string();
            mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
            return Err(error);
        }

        let working_dir = match self.prepare_working_dir(store, run_id, spec) {
            Ok(working_dir) => working_dir,
            Err(error) => {
                let reason = error.to_string();
                mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
                return Err(error);
            }
        };
        let mut invocation = build_provider_run_invocation(
            provider,
            &RunExecutionRequest {
                provider_id: provider.id.clone(),
                prompt,
                working_dir: working_dir.clone(),
                context_path: paths.root.clone(),
                approval_policy: spec.execution.approval_policy,
            },
        );
        invocation.env = scheduler_env(job_id, run_id, spec, &paths, &working_dir);
        let cancellation_database_path = self.data_dir.join("scheduler.sqlite3");
        let mut next_cancel_check = Instant::now();
        let output = match run_invocation_with_observer_and_cancellation(
            &invocation,
            Duration::from_secs(spec.execution.timeout_seconds),
            |process_id| {
                store
                    .set_run_process(run_id, process_id, process_id)
                    .map_err(|error| {
                        scheduler_provider::ProviderError::Command(error.to_string())
                    })?;
                store
                    .transition_run(run_id, RunStatus::Running, None)
                    .map_err(|error| scheduler_provider::ProviderError::Command(error.to_string()))
            },
            move || {
                let now = Instant::now();
                if now < next_cancel_check {
                    return Ok(false);
                }
                next_cancel_check = now + Duration::from_millis(100);
                let store = Store::open(&cancellation_database_path).map_err(|error| {
                    scheduler_provider::ProviderError::Command(error.to_string())
                })?;
                store
                    .get_run(run_id)
                    .map(|run| {
                        run.is_some_and(|run| {
                            matches!(run.status, RunStatus::Cancelling | RunStatus::Cancelled)
                        })
                    })
                    .map_err(|error| scheduler_provider::ProviderError::Command(error.to_string()))
            },
        ) {
            Ok(output) => output,
            Err(error) => {
                let error = ExecutionError::Provider(error);
                let reason = error.to_string();
                mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
                return Err(error);
            }
        };

        if let Err(error) = fs::write(&paths.provider_stdout, redact_secrets(&output.stdout)) {
            let error = ExecutionError::Filesystem(error);
            let reason = error.to_string();
            mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
            return Err(error);
        }
        if let Err(error) = fs::write(&paths.provider_stderr, redact_secrets(&output.stderr)) {
            let error = ExecutionError::Filesystem(error);
            let reason = error.to_string();
            mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
            return Err(error);
        }
        if let Err(error) = record_run_log_files(store, run_id, &paths) {
            let reason = error.to_string();
            mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
            return Err(error);
        }

        store.clear_run_process(run_id)?;
        if let Some(run) = store.get_run(run_id)?
            && matches!(run.status, RunStatus::Cancelling | RunStatus::Cancelled)
        {
            if run.status == RunStatus::Cancelling {
                store.transition_run(
                    run_id,
                    RunStatus::Cancelled,
                    Some("manual cancellation requested"),
                )?;
            }
            dispatch_run_status_notification_by_id(store, spec, run_id, notifier);
            return Ok(run_id);
        }

        let summary = match read_run_summary(&paths.summary_json) {
            Ok(summary) => summary,
            Err(error) => {
                let reason = error.to_string();
                mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
                return Err(error);
            }
        };
        if let Err(error) = index_artifacts(store, run_id, &paths, summary.as_ref()) {
            let reason = error.to_string();
            mark_run_failed_and_notify(store, spec, run_id, &reason, notifier);
            return Err(error);
        }

        let final_status =
            final_status_from_output(output.timed_out, output.exit_code, summary.as_ref());
        let reason = summary.as_ref().and_then(|summary| {
            summary
                .blocked_reason
                .as_deref()
                .or(summary.summary.as_deref())
        });
        store.transition_run(run_id, final_status, reason)?;
        dispatch_run_status_notification_by_id(store, spec, run_id, notifier);
        Ok(run_id)
    }

    pub fn create_run_context(
        &self,
        run_id: Uuid,
        spec: &JobSpec,
    ) -> Result<RunContextPaths, ExecutionError> {
        let root = self.data_dir.join("runs").join(run_id.to_string());
        let artifacts_dir = root.join("artifacts");
        fs::create_dir_all(&artifacts_dir)?;
        let paths = RunContextPaths {
            context_json: root.join("context.json"),
            execution_prompt: root.join("execution_prompt.md"),
            provider_stdout: root.join("provider_stdout.log"),
            provider_stderr: root.join("provider_stderr.log"),
            scheduler_events: root.join("scheduler_events.jsonl"),
            summary_json: root.join("summary.json"),
            root,
            artifacts_dir,
        };
        fs::write(
            &paths.context_json,
            serde_json::to_string_pretty(&serde_json::json!({
                "run_id": run_id,
                "job_name": spec.name,
                "provider_id": spec.provider_id,
                "repo_path": spec.repo.path,
            }))?,
        )?;
        fs::write(&paths.scheduler_events, "")?;
        Ok(paths)
    }

    fn prepare_working_dir(
        &self,
        store: &mut Store,
        run_id: Uuid,
        spec: &JobSpec,
    ) -> Result<PathBuf, ExecutionError> {
        match spec.execution.isolation {
            IsolationMode::None => Ok(PathBuf::from(&spec.repo.path)),
            IsolationMode::GitWorktree => {
                let repo_path = canonicalize_repo(Path::new(&spec.repo.path))?;
                if spec.repo.fetch_before_run {
                    fetch_repo(&repo_path)?;
                }
                let base_ref = resolve_base_ref(&repo_path, &spec.repo.base_ref)?;
                let branch = render_branch_template(
                    &spec.execution.branch_template,
                    &BranchTemplateContext {
                        job_name: spec.name.clone(),
                        run_id,
                        scheduled_at: Utc::now(),
                    },
                );
                let worktree_path = self
                    .data_dir
                    .join("worktrees")
                    .join(slug_for_path(&spec.name))
                    .join(run_id.to_string());
                if let Some(parent) = worktree_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                create_worktree(&repo_path, &worktree_path, &branch, &base_ref)?;
                store.set_run_workspace(run_id, Some(&worktree_path), Some(&branch))?;
                Ok(worktree_path)
            }
        }
    }
}

pub fn read_run_summary(path: &Path) -> Result<Option<RunSummary>, ExecutionError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    let summary = serde_json::from_str(&content)?;
    Ok(Some(summary))
}

fn final_status_from_output(
    timed_out: bool,
    exit_code: Option<i32>,
    summary: Option<&RunSummary>,
) -> RunStatus {
    if timed_out {
        return RunStatus::TimedOut;
    }
    if summary
        .and_then(|summary| summary.blocked_reason.as_ref())
        .is_some()
    {
        return RunStatus::Blocked;
    }
    match exit_code {
        Some(0) => RunStatus::Succeeded,
        _ => RunStatus::Failed,
    }
}

fn index_artifacts(
    store: &mut Store,
    run_id: Uuid,
    paths: &RunContextPaths,
    summary: Option<&RunSummary>,
) -> Result<(), ExecutionError> {
    if let Some(summary) = summary {
        for artifact in &summary.artifacts {
            store.add_run_artifact(run_id, &artifact.path, &artifact.kind)?;
        }
    }

    for artifact_path in discover_artifact_files(&paths.artifacts_dir)? {
        let relative = artifact_path
            .strip_prefix(&paths.root)
            .unwrap_or(&artifact_path)
            .display()
            .to_string();
        let already_indexed = summary
            .map(|summary| {
                summary
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.path == relative)
            })
            .unwrap_or(false);
        if !already_indexed {
            store.add_run_artifact(run_id, &relative, &artifact_kind(&artifact_path))?;
        }
    }
    Ok(())
}

fn record_run_log_files(
    store: &Store,
    run_id: Uuid,
    paths: &RunContextPaths,
) -> Result<(), ExecutionError> {
    for (stream, path) in [
        ("stdout", &paths.provider_stdout),
        ("stderr", &paths.provider_stderr),
        ("scheduler", &paths.scheduler_events),
    ] {
        let bytes = fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        store.add_run_log_file(run_id, &path.display().to_string(), stream, bytes)?;
    }
    Ok(())
}

fn discover_artifact_files(dir: &Path) -> Result<Vec<PathBuf>, ExecutionError> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(discover_artifact_files(&path)?);
        } else if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn artifact_kind(path: &Path) -> String {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("md") | Some("txt") => "report",
        Some("json") => "data",
        _ => "file",
    }
    .to_string()
}

fn scheduler_env(
    job_id: Uuid,
    run_id: Uuid,
    spec: &JobSpec,
    paths: &RunContextPaths,
    working_dir: &Path,
) -> Vec<(String, String)> {
    vec![
        ("SCHEDULER_JOB_ID".to_string(), job_id.to_string()),
        ("SCHEDULER_JOB_NAME".to_string(), spec.name.clone()),
        ("SCHEDULER_RUN_ID".to_string(), run_id.to_string()),
        ("SCHEDULER_REPO_PATH".to_string(), spec.repo.path.clone()),
        (
            "SCHEDULER_WORKTREE_PATH".to_string(),
            working_dir.display().to_string(),
        ),
        (
            "SCHEDULER_CONTEXT_PATH".to_string(),
            paths.root.display().to_string(),
        ),
        (
            "SCHEDULER_SUMMARY_PATH".to_string(),
            paths.summary_json.display().to_string(),
        ),
        (
            "SCHEDULER_ARTIFACTS_DIR".to_string(),
            paths.artifacts_dir.display().to_string(),
        ),
        (
            "SCHEDULER_PROVIDER_ID".to_string(),
            spec.provider_id.clone(),
        ),
    ]
}

pub fn execution_prompt(spec: &JobSpec, paths: &RunContextPaths) -> String {
    format!(
        r#"Execute this scheduled task in the current working directory.

Perform exactly one bounded pass for this run. The scheduler owns recurrence, so do not sleep,
watch for future intervals, or run your own loop. If the task is broad, inspect the highest-signal
files and commands you can within this run and summarize any remaining scope.

Task:
{task}

Success criteria:
{success_criteria}

Delivery mode: {delivery_mode:?}

Write a machine-readable summary to:
{summary_path}

Put durable outputs in:
{artifacts_dir}

Do not ask interactive questions during scheduled runs. If blocked, write a clear blocked reason and exit non-zero.

If this run is specifically remediating a blocker from another task, issue, branch, or pull request:
- Do not mutate the original task branch in place.
- Create the fix on this run's isolated branch/worktree.
- Commit and push the remediation branch only when delivery mode allows code delivery.
- Report the original blocked item, remediation branch, validation performed, and what the user must merge.
- After the remediation is merged by a human, the original blocked item should be rechecked rather than creating a duplicate original task.
"#,
        task = spec.task.prompt,
        success_criteria = spec
            .task
            .success_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        delivery_mode = spec.delivery.mode,
        summary_path = paths.summary_json.display(),
        artifacts_dir = paths.artifacts_dir.display()
    )
}

fn slug_for_path(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;

    use chrono::TimeZone;
    use scheduler_core::schedule::{IntervalUnit, MisfirePolicy, ScheduleSpec};
    use scheduler_core::{
        CleanupPolicy, ConcurrencyPolicy, DeliverySpec, ExecutionSpec, IsolationMode,
        NotificationSpec, RepoLockPolicy, RepoSpec, TaskSpec, WorktreeCleanupSpec,
    };
    use scheduler_provider::ProviderCapability;

    use super::*;

    #[derive(Clone, Default)]
    struct RecordingNotifier {
        entries: Arc<Mutex<Vec<(String, NotificationEventKind)>>>,
    }

    impl RecordingNotifier {
        fn entries(&self) -> Vec<(String, NotificationEventKind)> {
            self.entries.lock().unwrap().clone()
        }
    }

    impl NotificationSink for RecordingNotifier {
        fn notify(
            &self,
            channel: &str,
            event: &NotificationEvent,
        ) -> Result<(), NotificationError> {
            self.entries
                .lock()
                .unwrap()
                .push((channel.to_string(), event.kind));
            Ok(())
        }
    }

    struct FailingNotifier;

    impl NotificationSink for FailingNotifier {
        fn notify(
            &self,
            _channel: &str,
            _event: &NotificationEvent,
        ) -> Result<(), NotificationError> {
            Err(NotificationError::LocalCommand(
                "test notification failure token=secret-value".to_string(),
            ))
        }
    }

    fn spec(repo_path: String) -> JobSpec {
        JobSpec {
            schema_version: "scheduler.job.v1".to_string(),
            name: "manual-report".to_string(),
            enabled: true,
            provider_id: "shell".to_string(),
            repo: RepoSpec {
                path: repo_path,
                base_ref: "main".to_string(),
                fetch_before_run: false,
            },
            schedule: ScheduleSpec::Manual {},
            task: TaskSpec {
                prompt: "Say hello".to_string(),
                success_criteria: vec!["stdout contains hello".to_string()],
            },
            execution: ExecutionSpec {
                isolation: IsolationMode::None,
                concurrency: Default::default(),
                repo_lock: Default::default(),
                timeout_seconds: 2,
                approval_policy: Default::default(),
                branch_template: "scheduler/{job_slug}/{run_id}".to_string(),
                worktree_cleanup: WorktreeCleanupSpec::default(),
            },
            delivery: DeliverySpec::default(),
            notifications: Default::default(),
            metadata: Default::default(),
        }
    }

    fn spec_with_policy(repo_path: String, concurrency: ConcurrencyPolicy) -> JobSpec {
        let mut spec = spec(repo_path);
        spec.execution.concurrency = concurrency;
        spec
    }

    fn interval_spec(
        repo_path: String,
        start_at: DateTime<Utc>,
        misfire_policy: MisfirePolicy,
    ) -> JobSpec {
        let mut spec = spec(repo_path);
        spec.schedule = ScheduleSpec::Interval {
            every: 1,
            unit: IntervalUnit::Hours,
            timezone: None,
            start_at: Some(start_at),
            misfire_policy,
        };
        spec
    }

    #[test]
    fn executes_provider_and_captures_logs() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.execution.timeout_seconds = 10;
        let job_id = store.create_job(&spec).unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: PathBuf::from("/bin/cat"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));

        let run_id = executor
            .execute_once(&mut store, job_id, &spec, &provider, RunTrigger::Manual)
            .unwrap();

        let run = store.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Succeeded);
        let stdout = fs::read_to_string(
            temp.path()
                .join("data")
                .join("runs")
                .join(run_id.to_string())
                .join("provider_stdout.log"),
        )
        .unwrap();
        assert!(stdout.contains("Say hello"));
    }

    #[test]
    fn executes_existing_scheduled_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.execution.timeout_seconds = 10;
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Scheduled, Some(Utc::now()))
            .unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: PathBuf::from("/bin/cat"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));

        executor
            .execute_existing_run(&mut store, run_id, job_id, &spec, &provider)
            .unwrap();

        let run = store.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Succeeded);
        assert_eq!(run.trigger, RunTrigger::Scheduled);
    }

    #[test]
    fn execution_passes_env_ingests_summary_and_indexes_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("provider.sh");
        fs::write(
            &script,
            r#"#!/bin/sh
cat > /dev/null
printf "%s" "$SCHEDULER_RUN_ID" > "$SCHEDULER_ARTIFACTS_DIR/run-id.txt"
printf "{}" > "$SCHEDULER_ARTIFACTS_DIR/declared.json"
cat > "$SCHEDULER_SUMMARY_PATH" <<JSON
{
  "status": "succeeded",
  "summary": "provider wrote summary",
  "artifacts": [
    {
      "path": "artifacts/declared.json",
      "kind": "data"
    }
  ],
  "files_changed": [],
  "branch": null,
  "commit": null,
  "pull_request_url": null,
  "blocked_reason": null
}
JSON
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
        }
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.execution.timeout_seconds = 10;
        let job_id = store.create_job(&spec).unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: script,
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));

        let run_id = executor
            .execute_once(&mut store, job_id, &spec, &provider, RunTrigger::Manual)
            .unwrap();

        let run = store.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Succeeded);
        assert_eq!(run.reason.as_deref(), Some("provider wrote summary"));
        let artifacts = store.list_run_artifacts(run_id).unwrap();
        assert_eq!(artifacts.len(), 2);
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.path == "artifacts/declared.json"
                    && artifact.kind == "data")
        );
        assert!(
            artifacts.iter().any(
                |artifact| artifact.path == "artifacts/run-id.txt" && artifact.kind == "report"
            )
        );
        let logs = store.list_run_log_files(run_id).unwrap();
        assert_eq!(logs.len(), 3);
        assert!(logs.iter().any(|log| log.stream == "stdout"));
        assert!(logs.iter().any(|log| log.stream == "stderr"));
        assert!(logs.iter().any(|log| log.stream == "scheduler"));
    }

    #[test]
    fn git_worktree_run_resolves_base_ref_and_records_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo_with_commit(&repo);
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(repo.display().to_string());
        spec.repo.base_ref = "HEAD".to_string();
        spec.repo.fetch_before_run = true;
        spec.execution.isolation = IsolationMode::GitWorktree;
        spec.execution.timeout_seconds = 10;
        let job_id = store.create_job(&spec).unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: PathBuf::from("/bin/cat"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));

        let run_id = executor
            .execute_once(&mut store, job_id, &spec, &provider, RunTrigger::Manual)
            .unwrap();

        let run = store.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Succeeded);
        assert!(PathBuf::from(run.worktree_path.unwrap()).exists());
        assert!(run.branch.unwrap().starts_with("scheduler/manual-report/"));
    }

    #[test]
    fn missing_base_ref_fails_run_before_provider_launch() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo_with_commit(&repo);
        let provider_marker = temp.path().join("provider-ran");
        let script = temp.path().join("provider.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", provider_marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
        }
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(repo.display().to_string());
        spec.repo.base_ref = "missing-branch".to_string();
        spec.repo.fetch_before_run = false;
        spec.execution.isolation = IsolationMode::GitWorktree;
        let job_id = store.create_job(&spec).unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: script,
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));

        let error = executor
            .execute_once(&mut store, job_id, &spec, &provider, RunTrigger::Manual)
            .unwrap_err();

        assert!(matches!(
            error,
            ExecutionError::Git(GitError::BaseRefNotFound(_))
        ));
        assert!(!provider_marker.exists());
        let runs = store.list_runs_for_job(job_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Failed);
    }

    #[test]
    fn run_success_notification_records_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.execution.timeout_seconds = 10;
        spec.notifications = NotificationSpec {
            on_success: vec!["local".to_string()],
            on_failure: vec![],
            on_timeout: vec![],
        };
        let job_id = store.create_job(&spec).unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: PathBuf::from("/bin/cat"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));
        let notifier = RecordingNotifier::default();

        let run_id = executor
            .execute_once_with_notifier(
                &mut store,
                job_id,
                &spec,
                &provider,
                RunTrigger::Manual,
                &notifier,
            )
            .unwrap();

        assert_eq!(
            notifier.entries(),
            vec![("local".to_string(), NotificationEventKind::RunSucceeded)]
        );
        let deliveries = store.list_notification_deliveries_for_run(run_id).unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].event_type, "run_succeeded");
        assert_eq!(deliveries[0].status, "delivered");
    }

    #[test]
    fn run_failure_and_timeout_notifications_trigger_channels() {
        let temp = tempfile::tempdir().unwrap();
        let fail_script = temp.path().join("fail.sh");
        fs::write(&fail_script, "#!/bin/sh\nexit 7\n").unwrap();
        let timeout_script = temp.path().join("timeout.sh");
        fs::write(&timeout_script, "#!/bin/sh\nsleep 2\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in [&fail_script, &timeout_script] {
                let mut permissions = fs::metadata(script).unwrap().permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(script, permissions).unwrap();
            }
        }

        let mut store = Store::in_memory().unwrap();
        let mut failing_spec = spec(temp.path().display().to_string());
        failing_spec.name = "failing-job".to_string();
        failing_spec.execution.timeout_seconds = 10;
        failing_spec.notifications = NotificationSpec {
            on_success: vec![],
            on_failure: vec!["webhook".to_string()],
            on_timeout: vec!["local".to_string()],
        };
        let failing_job_id = store.create_job(&failing_spec).unwrap();
        let mut timeout_spec = failing_spec.clone();
        timeout_spec.name = "timeout-job".to_string();
        timeout_spec.execution.timeout_seconds = 1;
        let timeout_job_id = store.create_job(&timeout_spec).unwrap();
        let executor = RunExecutor::new(temp.path().join("data"));
        let notifier = RecordingNotifier::default();

        let failed_run = executor
            .execute_once_with_notifier(
                &mut store,
                failing_job_id,
                &failing_spec,
                &ProviderConfig {
                    id: "shell".to_string(),
                    display_name: "Shell".to_string(),
                    command: fail_script,
                    enabled: true,
                    capabilities: ProviderCapability::default(),
                },
                RunTrigger::Manual,
                &notifier,
            )
            .unwrap();
        let timeout_run = executor
            .execute_once_with_notifier(
                &mut store,
                timeout_job_id,
                &timeout_spec,
                &ProviderConfig {
                    id: "shell".to_string(),
                    display_name: "Shell".to_string(),
                    command: timeout_script,
                    enabled: true,
                    capabilities: ProviderCapability::default(),
                },
                RunTrigger::Manual,
                &notifier,
            )
            .unwrap();

        assert_eq!(
            notifier.entries(),
            vec![
                ("webhook".to_string(), NotificationEventKind::RunFailed),
                ("local".to_string(), NotificationEventKind::RunTimedOut),
            ]
        );
        assert_eq!(
            store
                .list_notification_deliveries_for_run(failed_run)
                .unwrap()[0]
                .event_type,
            "run_failed"
        );
        assert_eq!(
            store
                .list_notification_deliveries_for_run(timeout_run)
                .unwrap()[0]
                .event_type,
            "run_timed_out"
        );
    }

    #[test]
    fn skipped_and_cancelled_notifications_trigger_channels() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut skip_spec =
            spec_with_policy(temp.path().display().to_string(), ConcurrencyPolicy::Skip);
        skip_spec.notifications.on_failure = vec!["local".to_string()];
        let skip_job_id = store.create_job(&skip_spec).unwrap();
        let skip_active = store
            .create_run(skip_job_id, &skip_spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(skip_active, RunStatus::Preparing, None)
            .unwrap();

        let notifier = RecordingNotifier::default();
        let action = apply_due_run_policy_with_notifier(
            &mut store,
            skip_job_id,
            &skip_spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
            &notifier,
        )
        .unwrap();
        let DueRunAction::Skipped(skipped_id) = action else {
            panic!("expected skipped action");
        };

        let mut replace_spec = spec_with_policy(
            temp.path().display().to_string(),
            ConcurrencyPolicy::Replace,
        );
        replace_spec.name = "replace-job".to_string();
        replace_spec.notifications.on_failure = vec!["webhook".to_string()];
        let replace_job_id = store.create_job(&replace_spec).unwrap();
        let replace_active = store
            .create_run(replace_job_id, &replace_spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(replace_active, RunStatus::Preparing, None)
            .unwrap();
        apply_due_run_policy_with_notifier(
            &mut store,
            replace_job_id,
            &replace_spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
            &notifier,
        )
        .unwrap();

        assert_eq!(
            notifier.entries(),
            vec![
                ("local".to_string(), NotificationEventKind::JobSkipped),
                ("webhook".to_string(), NotificationEventKind::RunCancelled),
            ]
        );
        assert_eq!(
            store
                .list_notification_deliveries_for_run(skipped_id)
                .unwrap()[0]
                .event_type,
            "job_skipped"
        );
        assert_eq!(
            store
                .list_notification_deliveries_for_run(replace_active)
                .unwrap()[0]
                .event_type,
            "run_cancelled"
        );
    }

    #[test]
    fn notification_failures_are_logged_without_failing_runs_and_redact_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.execution.timeout_seconds = 10;
        spec.notifications.on_success =
            vec!["webhook:http://example.invalid/hook?token=secret-value".to_string()];
        let job_id = store.create_job(&spec).unwrap();
        let provider = ProviderConfig {
            id: "shell".to_string(),
            display_name: "Shell".to_string(),
            command: PathBuf::from("/bin/cat"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let executor = RunExecutor::new(temp.path().join("data"));

        let run_id = executor
            .execute_once_with_notifier(
                &mut store,
                job_id,
                &spec,
                &provider,
                RunTrigger::Manual,
                &FailingNotifier,
            )
            .unwrap();

        assert_eq!(
            store.get_run(run_id).unwrap().unwrap().status,
            RunStatus::Succeeded
        );
        let deliveries = store.list_notification_deliveries_for_run(run_id).unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].status, "failed");
        assert!(deliveries[0].channel.contains("token=[REDACTED]"));
        assert!(!deliveries[0].channel.contains("secret-value"));
        assert!(
            !deliveries[0]
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("secret-value")
        );
    }

    #[test]
    fn webhook_notification_posts_expected_json() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0_u8; 8192];
            let mut request_bytes = Vec::new();
            loop {
                let size = stream.read(&mut buffer).unwrap_or(0);
                if size == 0 {
                    break;
                }
                request_bytes.extend_from_slice(&buffer[..size]);
                if request_bytes
                    .windows(b"\"job_name\"".len())
                    .any(|window| window == b"\"job_name\"")
                {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request_bytes).to_string();
            sender.send(request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let event = NotificationEvent {
            kind: NotificationEventKind::RunSucceeded,
            job_id: None,
            run_id: None,
            job_name: "manual-report".to_string(),
            provider_id: "shell".to_string(),
            status: Some(RunStatus::Succeeded),
            message: Some("done".to_string()),
            created_at: Utc::now(),
        };

        WebhookNotifier::with_timeout(
            format!("http://{address}/notify?token=secret-value"),
            Duration::from_secs(2),
        )
        .notify_event(&event)
        .unwrap();

        let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        assert!(request.starts_with("POST /notify?token=secret-value HTTP/1.1"));
        assert!(request.contains("\"type\":\"run_succeeded\""));
        assert!(request.contains("\"job_name\":\"manual-report\""));
    }

    #[test]
    fn due_policy_skip_records_skipped_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec_with_policy(temp.path().display().to_string(), ConcurrencyPolicy::Skip);
        let job_id = store.create_job(&spec).unwrap();
        let active = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active, RunStatus::Preparing, None)
            .unwrap();

        let action = apply_due_run_policy(
            &mut store,
            job_id,
            &spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
        )
        .unwrap();

        let DueRunAction::Skipped(skipped_id) = action else {
            panic!("expected skipped action");
        };
        assert_eq!(
            store.get_run(skipped_id).unwrap().unwrap().status,
            RunStatus::Skipped
        );
    }

    #[test]
    fn due_times_run_once_returns_latest_missed_time() {
        let start_at = Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
        let spec = interval_spec("/tmp/repo".to_string(), start_at, MisfirePolicy::RunOnce);

        let due = due_times_for_job(&spec, start_at, None, now, 10).unwrap();

        assert_eq!(due, vec![now]);
    }

    #[test]
    fn due_times_backfill_returns_bounded_missed_times() {
        let start_at = Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
        let spec = interval_spec("/tmp/repo".to_string(), start_at, MisfirePolicy::Backfill);

        let due = due_times_for_job(&spec, start_at, None, now, 2).unwrap();

        assert_eq!(
            due,
            vec![
                Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            ]
        );
    }

    #[test]
    fn due_times_skip_ignores_missed_times() {
        let start_at = Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 12, 11, 30, 0).unwrap();
        let spec = interval_spec("/tmp/repo".to_string(), start_at, MisfirePolicy::Skip);

        let due = due_times_for_job(&spec, start_at, None, now, 10).unwrap();

        assert!(due.is_empty());
    }

    #[test]
    fn scheduler_tick_creates_due_runs() {
        let temp = tempfile::tempdir().unwrap();
        let start_at = Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = interval_spec(
            temp.path().display().to_string(),
            start_at,
            MisfirePolicy::RunOnce,
        );
        let job_id = store.create_job(&spec).unwrap();

        let report = scheduler_tick(&mut store, now, 10).unwrap();

        assert_eq!(report.due_actions.len(), 1);
        assert_eq!(report.due_actions[0].job_id, job_id);
        assert_eq!(report.due_actions[0].action, "start");
        let runs = store.list_runs_for_job(job_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].due_at, Some(now));
    }

    #[test]
    fn scheduler_tick_starts_queued_run_when_unblocked() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec(temp.path().display().to_string());
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Scheduled, Some(Utc::now()))
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Queued, Some("test"))
            .unwrap();

        let report = scheduler_tick(&mut store, Utc::now(), 10).unwrap();

        assert_eq!(report.queued_started, vec![run_id]);
        assert_eq!(
            store.get_run(run_id).unwrap().unwrap().status,
            RunStatus::Preparing
        );
    }

    #[test]
    fn daemon_lock_allows_single_owner() {
        let mut store = Store::in_memory().unwrap();

        assert!(acquire_daemon_lock(&mut store, "owner-a", 60).unwrap());
        assert!(!acquire_daemon_lock(&mut store, "owner-b", 60).unwrap());
        assert!(release_daemon_lock(&mut store, "owner-a").unwrap());
        assert!(acquire_daemon_lock(&mut store, "owner-b", 60).unwrap());
    }

    #[test]
    fn daemon_status_reports_active_runs_and_next_due() {
        let temp = tempfile::tempdir().unwrap();
        let start_at = Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 12, 8, 30, 0).unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = interval_spec(
            temp.path().display().to_string(),
            start_at,
            MisfirePolicy::RunOnce,
        );
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Preparing, None)
            .unwrap();

        let status = daemon_status_snapshot(&store, "/tmp/db.sqlite3", now).unwrap();

        assert_eq!(status.active_runs, 1);
        assert_eq!(
            status.next_due_run,
            Some(Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap())
        );
    }

    #[test]
    fn restart_recovery_marks_active_runs_lost_and_notifies() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.notifications.on_failure = vec!["local".to_string()];
        let job_id = store.create_job(&spec).unwrap();
        let preparing = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(preparing, RunStatus::Preparing, None)
            .unwrap();
        let running = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(running, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(running, RunStatus::Running, None)
            .unwrap();
        let notifier = RecordingNotifier::default();

        let report = recover_interrupted_runs_with_notifier(&mut store, &notifier).unwrap();

        assert_eq!(report.recovered_runs.len(), 2);
        assert!(report.recovered_runs.contains(&preparing));
        assert!(report.recovered_runs.contains(&running));
        assert_eq!(
            store.get_run(preparing).unwrap().unwrap().status,
            RunStatus::Lost
        );
        assert_eq!(
            store.get_run(running).unwrap().unwrap().status,
            RunStatus::Lost
        );
        assert_eq!(
            notifier.entries(),
            vec![
                ("local".to_string(), NotificationEventKind::RunFailed),
                ("local".to_string(), NotificationEventKind::RunFailed),
            ]
        );
    }

    #[test]
    fn due_policy_queue_records_queued_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec_with_policy(temp.path().display().to_string(), ConcurrencyPolicy::Queue);
        let job_id = store.create_job(&spec).unwrap();
        let active = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active, RunStatus::Preparing, None)
            .unwrap();

        let action = apply_due_run_policy(
            &mut store,
            job_id,
            &spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
        )
        .unwrap();

        let DueRunAction::Queued(queued_id) = action else {
            panic!("expected queued action");
        };
        assert_eq!(
            store.get_run(queued_id).unwrap().unwrap().status,
            RunStatus::Queued
        );
    }

    #[test]
    fn due_policy_parallel_starts_new_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec_with_policy(
            temp.path().display().to_string(),
            ConcurrencyPolicy::Parallel,
        );
        let job_id = store.create_job(&spec).unwrap();
        let active = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active, RunStatus::Preparing, None)
            .unwrap();

        let action = apply_due_run_policy(
            &mut store,
            job_id,
            &spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
        )
        .unwrap();

        assert!(matches!(action, DueRunAction::Start(_)));
    }

    #[test]
    fn due_policy_replace_cancels_active_and_starts_new_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec_with_policy(
            temp.path().display().to_string(),
            ConcurrencyPolicy::Replace,
        );
        let job_id = store.create_job(&spec).unwrap();
        let active = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active, RunStatus::Preparing, None)
            .unwrap();

        let action = apply_due_run_policy(
            &mut store,
            job_id,
            &spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
        )
        .unwrap();

        let DueRunAction::Replace {
            cancelled_run_ids,
            new_run_id,
        } = action
        else {
            panic!("expected replace action");
        };
        assert_eq!(cancelled_run_ids, vec![active]);
        assert_eq!(
            store.get_run(active).unwrap().unwrap().status,
            RunStatus::Cancelled
        );
        assert_eq!(
            store.get_run(new_run_id).unwrap().unwrap().status,
            RunStatus::Scheduled
        );
    }

    #[test]
    fn exclusive_repo_lock_queues_when_same_repo_has_active_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let active_spec = spec(temp.path().display().to_string());
        let active_job_id = store.create_job(&active_spec).unwrap();
        let active = store
            .create_run(active_job_id, &active_spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active, RunStatus::Preparing, None)
            .unwrap();

        let mut locked_spec = spec(temp.path().display().to_string());
        locked_spec.name = "locked-job".to_string();
        locked_spec.execution.repo_lock = RepoLockPolicy::Exclusive;
        let locked_job_id = store.create_job(&locked_spec).unwrap();

        let action = apply_due_run_policy(
            &mut store,
            locked_job_id,
            &locked_spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
        )
        .unwrap();

        let DueRunAction::Queued(run_id) = action else {
            panic!("expected queued action");
        };
        let run = store.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Queued);
        assert_eq!(run.reason.as_deref(), Some("queued by exclusive repo lock"));
    }

    #[test]
    fn repo_lock_none_allows_same_repo_parallel_jobs() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::in_memory().unwrap();
        let active_spec = spec(temp.path().display().to_string());
        let active_job_id = store.create_job(&active_spec).unwrap();
        let active = store
            .create_run(active_job_id, &active_spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active, RunStatus::Preparing, None)
            .unwrap();

        let mut unlocked_spec = spec(temp.path().display().to_string());
        unlocked_spec.name = "unlocked-job".to_string();
        unlocked_spec.execution.repo_lock = RepoLockPolicy::None;
        let unlocked_job_id = store.create_job(&unlocked_spec).unwrap();

        let action = apply_due_run_policy(
            &mut store,
            unlocked_job_id,
            &unlocked_spec,
            Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap(),
        )
        .unwrap();

        assert!(matches!(action, DueRunAction::Start(_)));
    }

    #[test]
    fn cleanup_removes_successful_worktree_after_retention() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec(temp.path().display().to_string());
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .set_run_workspace(run_id, Some(&worktree), Some("test-branch"))
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Running, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Succeeded, None)
            .unwrap();

        let report =
            cleanup_retained_worktrees(&store, Utc::now() + chrono::Duration::days(15), false)
                .unwrap();

        assert_eq!(report.removed_worktrees, vec![worktree.clone()]);
        assert!(!worktree.exists());
    }

    #[test]
    fn cleanup_keeps_worktree_inside_retention_window() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec(temp.path().display().to_string());
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .set_run_workspace(run_id, Some(&worktree), Some("test-branch"))
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Running, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Succeeded, None)
            .unwrap();

        let report =
            cleanup_retained_worktrees(&store, Utc::now() + chrono::Duration::days(1), false)
                .unwrap();

        assert_eq!(report.kept_worktrees, vec![worktree.clone()]);
        assert!(worktree.exists());
    }

    #[test]
    fn cleanup_respects_keep_policy_for_failures() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let mut store = Store::in_memory().unwrap();
        let mut spec = spec(temp.path().display().to_string());
        spec.execution.worktree_cleanup.on_failure = CleanupPolicy::Keep;
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .set_run_workspace(run_id, Some(&worktree), Some("test-branch"))
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Failed, None)
            .unwrap();

        let report =
            cleanup_retained_worktrees(&store, Utc::now() + chrono::Duration::days(365), false)
                .unwrap();

        assert_eq!(report.kept_worktrees, vec![worktree.clone()]);
        assert!(worktree.exists());
    }

    #[test]
    fn cleanup_dry_run_reports_without_removing() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let mut store = Store::in_memory().unwrap();
        let spec = spec(temp.path().display().to_string());
        let job_id = store.create_job(&spec).unwrap();
        let run_id = store
            .create_run(job_id, &spec, RunTrigger::Manual, None)
            .unwrap();
        store
            .set_run_workspace(run_id, Some(&worktree), Some("test-branch"))
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Running, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Succeeded, None)
            .unwrap();

        let report =
            cleanup_retained_worktrees(&store, Utc::now() + chrono::Duration::days(15), true)
                .unwrap();

        assert_eq!(report.removed_worktrees, vec![worktree.clone()]);
        assert!(worktree.exists());
    }

    fn init_repo_with_commit(path: &Path) {
        let status = Command::new("git")
            .arg("init")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(path.join("README.md"), "test\n").unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("add")
            .arg("README.md")
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("-c")
            .arg("user.name=Scheduler Test")
            .arg("-c")
            .arg("user.email=scheduler@example.invalid")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
