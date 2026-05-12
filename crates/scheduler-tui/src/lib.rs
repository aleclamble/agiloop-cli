use scheduler_core::{JobSpec, RunRecord};
use serde::{Deserialize, Serialize};

pub mod dashboard;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DashboardViewModel {
    pub jobs: Vec<JobSpec>,
    pub recent_runs: Vec<RunRecord>,
    pub daemon_available: bool,
}

impl DashboardViewModel {
    pub fn empty() -> Self {
        Self {
            jobs: vec![],
            recent_runs: vec![],
            daemon_available: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TuiView {
    Dashboard,
    Jobs,
    JobDetail,
    Create,
    Runs,
    RunDetail,
    Logs,
    Providers,
    Settings,
    Help,
}

impl TuiView {
    pub const ALL: [Self; 10] = [
        Self::Dashboard,
        Self::Jobs,
        Self::JobDetail,
        Self::Create,
        Self::Runs,
        Self::RunDetail,
        Self::Logs,
        Self::Providers,
        Self::Settings,
        Self::Help,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Jobs => "Jobs",
            Self::JobDetail => "Job",
            Self::Create => "Create",
            Self::Runs => "Runs",
            Self::RunDetail => "Run",
            Self::Logs => "Logs",
            Self::Providers => "Providers",
            Self::Settings => "Settings",
            Self::Help => "Help",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiState {
    pub view: TuiView,
    pub selected_job: usize,
    pub selected_run: usize,
    pub selected_provider: usize,
    pub selected_setting: usize,
    pub filter: String,
    pub editing_filter: bool,
    pub job_sort: JobSort,
    pub create_field: CreateField,
    pub create_provider: String,
    pub create_repo: String,
    pub create_task: String,
    pub create_timezone: String,
    pub message: Option<String>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            view: TuiView::Dashboard,
            selected_job: 0,
            selected_run: 0,
            selected_provider: 0,
            selected_setting: 0,
            filter: String::new(),
            editing_filter: false,
            job_sort: JobSort::Name,
            create_field: CreateField::Provider,
            create_provider: String::new(),
            create_repo: String::new(),
            create_task: String::new(),
            create_timezone: "UTC".to_string(),
            message: None,
        }
    }
}

impl TuiState {
    pub fn move_next(&mut self, model: &TuiAppModel) {
        let max = self.current_len(model).saturating_sub(1);
        match self.view {
            TuiView::Jobs | TuiView::JobDetail => {
                self.selected_job = (self.selected_job + 1).min(max)
            }
            TuiView::Runs | TuiView::RunDetail | TuiView::Logs => {
                self.selected_run = (self.selected_run + 1).min(max)
            }
            TuiView::Providers => self.selected_provider = (self.selected_provider + 1).min(max),
            TuiView::Settings => self.selected_setting = (self.selected_setting + 1).min(max),
            TuiView::Dashboard | TuiView::Create | TuiView::Help => {}
        }
    }

    pub fn move_previous(&mut self) {
        match self.view {
            TuiView::Jobs | TuiView::JobDetail => {
                self.selected_job = self.selected_job.saturating_sub(1)
            }
            TuiView::Runs | TuiView::RunDetail | TuiView::Logs => {
                self.selected_run = self.selected_run.saturating_sub(1)
            }
            TuiView::Providers => self.selected_provider = self.selected_provider.saturating_sub(1),
            TuiView::Settings => self.selected_setting = self.selected_setting.saturating_sub(1),
            TuiView::Dashboard | TuiView::Create | TuiView::Help => {}
        }
    }

    fn current_len(&self, model: &TuiAppModel) -> usize {
        match self.view {
            TuiView::Jobs | TuiView::JobDetail => model.jobs.len(),
            TuiView::Runs | TuiView::RunDetail | TuiView::Logs => model.runs.len(),
            TuiView::Providers => model.providers.len(),
            TuiView::Settings => model.settings.len(),
            TuiView::Dashboard | TuiView::Create | TuiView::Help => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CreateField {
    Provider,
    Repo,
    Task,
    Timezone,
}

impl CreateField {
    pub fn next(self) -> Self {
        match self {
            Self::Provider => Self::Repo,
            Self::Repo => Self::Task,
            Self::Task => Self::Timezone,
            Self::Timezone => Self::Provider,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Provider => Self::Timezone,
            Self::Repo => Self::Provider,
            Self::Task => Self::Repo,
            Self::Timezone => Self::Task,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobSort {
    Name,
    Provider,
    Repo,
    State,
    NextRun,
    LastStatus,
}

impl JobSort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Provider,
            Self::Provider => Self::Repo,
            Self::Repo => Self::State,
            Self::State => Self::NextRun,
            Self::NextRun => Self::LastStatus,
            Self::LastStatus => Self::Name,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Provider => "provider",
            Self::Repo => "repo",
            Self::State => "state",
            Self::NextRun => "next",
            Self::LastStatus => "last",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub provider_id: String,
    pub repo_path: String,
    pub schedule: String,
    pub task: String,
    pub next_due: Option<String>,
    pub last_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiRun {
    pub id: String,
    pub job_name: String,
    pub status: String,
    pub trigger: String,
    pub provider_id: String,
    pub due_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiProvider {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub command: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiSetting {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiAppModel {
    pub daemon_online: bool,
    pub daemon_pid: Option<u32>,
    pub next_due_run: Option<String>,
    pub active_runs: usize,
    pub jobs: Vec<TuiJob>,
    pub runs: Vec<TuiRun>,
    pub providers: Vec<TuiProvider>,
    pub settings: Vec<TuiSetting>,
    pub selected_run_logs: Vec<String>,
    pub selected_run_artifacts: Vec<String>,
}
