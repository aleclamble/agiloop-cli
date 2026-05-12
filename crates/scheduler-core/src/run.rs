use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Scheduled,
    Queued,
    Skipped,
    Preparing,
    Running,
    Cancelling,
    Cancelled,
    Succeeded,
    Failed,
    TimedOut,
    Blocked,
    Lost,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Skipped
                | Self::Cancelled
                | Self::Succeeded
                | Self::Failed
                | Self::TimedOut
                | Self::Blocked
                | Self::Lost
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use RunStatus::*;
        match (self, next) {
            (Scheduled, Queued | Skipped | Preparing | Cancelled) => true,
            (Queued, Preparing | Skipped | Cancelled) => true,
            (Preparing, Running | Failed | Cancelled | Lost) => true,
            (Running, Cancelling | Succeeded | Failed | TimedOut | Blocked | Lost) => true,
            (Cancelling, Cancelled | Failed | TimedOut | Lost) => true,
            (from, to) if from == to => true,
            (from, _) if from.is_terminal() => false,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RunRecord {
    pub id: Uuid,
    pub job_id: Uuid,
    pub status: RunStatus,
    pub trigger: RunTrigger,
    pub due_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub provider_id: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunTrigger {
    Scheduled,
    Manual,
    Retry,
    Backfill,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_machine_allows_expected_transitions() {
        assert!(RunStatus::Scheduled.can_transition_to(RunStatus::Preparing));
        assert!(RunStatus::Preparing.can_transition_to(RunStatus::Running));
        assert!(RunStatus::Running.can_transition_to(RunStatus::Succeeded));
        assert!(RunStatus::Running.can_transition_to(RunStatus::TimedOut));
        assert!(RunStatus::Running.can_transition_to(RunStatus::Cancelling));
        assert!(RunStatus::Cancelling.can_transition_to(RunStatus::Cancelled));
        assert!(RunStatus::Preparing.can_transition_to(RunStatus::Lost));
        assert!(RunStatus::Cancelling.can_transition_to(RunStatus::Lost));
    }

    #[test]
    fn terminal_runs_do_not_restart() {
        assert!(!RunStatus::Succeeded.can_transition_to(RunStatus::Running));
        assert!(!RunStatus::Failed.can_transition_to(RunStatus::Preparing));
        assert!(!RunStatus::Cancelled.can_transition_to(RunStatus::Queued));
    }
}
