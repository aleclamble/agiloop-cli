use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct JobSpec {
    pub schema_version: String,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub provider_id: String,
    pub repo: RepoSpec,
    pub schedule: crate::schedule::ScheduleSpec,
    pub task: TaskSpec,
    #[serde(default)]
    pub execution: ExecutionSpec,
    #[serde(default)]
    pub delivery: DeliverySpec,
    #[serde(default)]
    pub notifications: NotificationSpec,
    #[serde(default)]
    pub metadata: MetadataSpec,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RepoSpec {
    pub path: String,
    #[serde(default = "default_base_ref")]
    pub base_ref: String,
    #[serde(default = "default_fetch_before_run")]
    pub fetch_before_run: bool,
}

fn default_base_ref() -> String {
    "main".to_string()
}

fn default_fetch_before_run() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TaskSpec {
    pub prompt: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExecutionSpec {
    #[serde(default)]
    pub isolation: IsolationMode,
    #[serde(default)]
    pub concurrency: ConcurrencyPolicy,
    #[serde(default)]
    pub repo_lock: RepoLockPolicy,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub approval_policy: ApprovalPolicy,
    #[serde(default = "default_branch_template")]
    pub branch_template: String,
    #[serde(default)]
    pub worktree_cleanup: WorktreeCleanupSpec,
}

impl Default for ExecutionSpec {
    fn default() -> Self {
        Self {
            isolation: IsolationMode::default(),
            concurrency: ConcurrencyPolicy::default(),
            repo_lock: RepoLockPolicy::default(),
            timeout_seconds: default_timeout_seconds(),
            approval_policy: ApprovalPolicy::default(),
            branch_template: default_branch_template(),
            worktree_cleanup: WorktreeCleanupSpec::default(),
        }
    }
}

fn default_timeout_seconds() -> u64 {
    3_600
}

fn default_branch_template() -> String {
    "scheduler/{job_slug}/{run_id}".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    #[default]
    GitWorktree,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyPolicy {
    #[default]
    Skip,
    Queue,
    Parallel,
    Replace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepoLockPolicy {
    #[default]
    None,
    Exclusive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    #[default]
    NonInteractive,
    InteractiveAttach,
    ProviderDefault,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorktreeCleanupSpec {
    #[serde(default)]
    pub on_success: CleanupPolicy,
    #[serde(default = "default_failure_cleanup")]
    pub on_failure: CleanupPolicy,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for WorktreeCleanupSpec {
    fn default() -> Self {
        Self {
            on_success: CleanupPolicy::default(),
            on_failure: default_failure_cleanup(),
            retention_days: default_retention_days(),
        }
    }
}

fn default_failure_cleanup() -> CleanupPolicy {
    CleanupPolicy::Keep
}

fn default_retention_days() -> u32 {
    14
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    #[default]
    AfterRetention,
    Keep,
    RemoveImmediately,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeliverySpec {
    #[serde(default)]
    pub mode: DeliveryMode,
    #[serde(default = "default_require_summary")]
    pub require_summary: bool,
    #[serde(default)]
    pub require_clean_worktree: bool,
}

impl Default for DeliverySpec {
    fn default() -> Self {
        Self {
            mode: DeliveryMode::default(),
            require_summary: default_require_summary(),
            require_clean_worktree: false,
        }
    }
}

fn default_require_summary() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    #[default]
    ArtifactOnly,
    LeaveChanges,
    Commit,
    PushBranch,
    PullRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub struct NotificationSpec {
    #[serde(default)]
    pub on_success: Vec<String>,
    #[serde(default)]
    pub on_failure: Vec<String>,
    #[serde(default)]
    pub on_timeout: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub struct MetadataSpec {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub source_text: Option<String>,
    #[serde(default)]
    pub created_by_provider_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrencyDecision {
    Start,
    Skip,
    Queue,
    Replace,
}

pub fn decide_concurrency(
    policy: ConcurrencyPolicy,
    active_runs_for_same_job: usize,
) -> ConcurrencyDecision {
    if active_runs_for_same_job == 0 {
        return ConcurrencyDecision::Start;
    }

    match policy {
        ConcurrencyPolicy::Skip => ConcurrencyDecision::Skip,
        ConcurrencyPolicy::Queue => ConcurrencyDecision::Queue,
        ConcurrencyPolicy::Parallel => ConcurrencyDecision::Start,
        ConcurrencyPolicy::Replace => ConcurrencyDecision::Replace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_execution_is_safe_for_scheduled_runs() {
        let execution = ExecutionSpec::default();

        assert_eq!(execution.isolation, IsolationMode::GitWorktree);
        assert_eq!(execution.concurrency, ConcurrencyPolicy::Skip);
        assert_eq!(execution.repo_lock, RepoLockPolicy::None);
        assert_eq!(execution.approval_policy, ApprovalPolicy::NonInteractive);
    }

    #[test]
    fn concurrency_decisions_match_prd() {
        assert_eq!(
            decide_concurrency(ConcurrencyPolicy::Skip, 1),
            ConcurrencyDecision::Skip
        );
        assert_eq!(
            decide_concurrency(ConcurrencyPolicy::Queue, 1),
            ConcurrencyDecision::Queue
        );
        assert_eq!(
            decide_concurrency(ConcurrencyPolicy::Parallel, 1),
            ConcurrencyDecision::Start
        );
        assert_eq!(
            decide_concurrency(ConcurrencyPolicy::Replace, 1),
            ConcurrencyDecision::Replace
        );
        assert_eq!(
            decide_concurrency(ConcurrencyPolicy::Skip, 0),
            ConcurrencyDecision::Start
        );
    }
}
