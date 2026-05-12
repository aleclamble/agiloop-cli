use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::{CreateField, DashboardViewModel, TuiAppModel, TuiState, TuiView};

pub fn draw_dashboard(frame: &mut Frame<'_>, model: &DashboardViewModel) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(55),
            Constraint::Percentage(45),
        ])
        .split(frame.area());

    let daemon = if model.daemon_available {
        "daemon online"
    } else {
        "daemon offline"
    };
    frame.render_widget(
        Paragraph::new(Line::from(format!("Scheduler  |  {daemon}")))
            .block(Block::default().title("Overview").borders(Borders::ALL)),
        chunks[0],
    );

    let job_rows = model.jobs.iter().map(|job| {
        Row::new(vec![
            Cell::from(job.name.clone()),
            Cell::from(job.provider_id.clone()),
            Cell::from(job.repo.path.clone()),
            Cell::from(if job.enabled { "enabled" } else { "disabled" }),
        ])
    });
    let jobs = Table::new(
        job_rows,
        [
            Constraint::Percentage(25),
            Constraint::Length(12),
            Constraint::Percentage(45),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Job", "Provider", "Repo", "State"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title("Jobs").borders(Borders::ALL));
    frame.render_widget(jobs, chunks[1]);

    let run_rows = model.recent_runs.iter().map(|run| {
        Row::new(vec![
            Cell::from(run.id.to_string()),
            Cell::from(format!("{:?}", run.status)),
            Cell::from(run.provider_id.clone()),
            Cell::from(run.reason.clone().unwrap_or_default()),
        ])
    });
    let runs = Table::new(
        run_rows,
        [
            Constraint::Percentage(38),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Percentage(38),
        ],
    )
    .header(
        Row::new(vec!["Run", "Status", "Provider", "Reason"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title("Recent Runs").borders(Borders::ALL));
    frame.render_widget(runs, chunks[2]);
}

pub fn draw_app(frame: &mut Frame<'_>, model: &TuiAppModel, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(frame.area());
    draw_tabs(frame, chunks[0], state.view);
    match state.view {
        TuiView::Dashboard => draw_overview(frame, chunks[1], model),
        TuiView::Jobs => draw_jobs(frame, chunks[1], model, state),
        TuiView::JobDetail => draw_job_detail(frame, chunks[1], model, state),
        TuiView::Create => draw_create(frame, chunks[1], state),
        TuiView::Runs => draw_runs(frame, chunks[1], model, state),
        TuiView::RunDetail => draw_run_detail(frame, chunks[1], model, state),
        TuiView::Logs => draw_logs(frame, chunks[1], model),
        TuiView::Providers => draw_providers(frame, chunks[1], model, state),
        TuiView::Settings => draw_settings(frame, chunks[1], model, state),
        TuiView::Help => draw_help(frame, chunks[1]),
    }
    let filter = if state.filter.is_empty() {
        "-".to_string()
    } else {
        state.filter.clone()
    };
    let mode = if state.editing_filter {
        "filter"
    } else {
        "navigate"
    };
    let footer_text = footer_text(state, mode, &filter);
    let footer = Paragraph::new(footer_text)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, chunks[2]);
}

fn footer_text(state: &TuiState, mode: &str, filter: &str) -> String {
    let message = state.message.as_deref().unwrap_or("");
    let controls = if state.view == TuiView::Create {
        "Views Left/Right  Fields Tab/Up/Down  Enter create in background  Esc dashboard  q quit"
    } else {
        "Views Left/Right or Tab/Shift-Tab  Move Up/Down  Enter select  / filter  Space toggle  n run  q quit"
    };
    format!(
        "Mode {mode}  Sort {}  Filter {filter}  {controls}  {message}",
        state.job_sort.title()
    )
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, active: TuiView) {
    let spans = TuiView::ALL
        .iter()
        .map(|view| {
            let style = if *view == active {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Span::styled(format!(" {} ", view.title()), style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .block(Block::default().title("Views").borders(Borders::ALL)),
        area,
    );
}

fn draw_overview(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ])
        .split(area);
    let daemon = if model.daemon_online {
        format!("online pid {}", model.daemon_pid.unwrap_or_default())
    } else {
        "offline".to_string()
    };
    let metrics = Paragraph::new(vec![
        Line::from(format!("Daemon: {daemon}")),
        Line::from(format!("Active runs: {}", model.active_runs)),
        Line::from(format!(
            "Next due: {}",
            model.next_due_run.as_deref().unwrap_or("-")
        )),
    ])
    .block(Block::default().title("Overview").borders(Borders::ALL));
    frame.render_widget(metrics, chunks[0]);
    draw_jobs(frame, chunks[1], model, &TuiState::default());
    draw_runs(frame, chunks[2], model, &TuiState::default());
}

fn draw_jobs(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel, state: &TuiState) {
    let rows = model.jobs.iter().enumerate().map(|(index, job)| {
        let style = selected_style(index == state.selected_job);
        Row::new(vec![
            Cell::from(job.name.clone()),
            Cell::from(job.provider_id.clone()),
            Cell::from(job.repo_path.clone()),
            Cell::from(job.next_due.clone().unwrap_or_else(|| "-".to_string())),
            Cell::from(job.last_status.clone().unwrap_or_else(|| "-".to_string())),
            Cell::from(if job.enabled { "enabled" } else { "disabled" }),
        ])
        .style(style)
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Length(12),
            Constraint::Percentage(25),
            Constraint::Percentage(18),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(vec![
        "Job", "Provider", "Repo", "Next", "Last", "State",
    ]))
    .block(Block::default().title("Jobs").borders(Borders::ALL));
    frame.render_widget(table, area);
}

fn draw_job_detail(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel, state: &TuiState) {
    let Some(job) = model.jobs.get(state.selected_job) else {
        draw_empty(frame, area, "Job");
        return;
    };
    let text = vec![
        Line::from(format!("Name: {}", job.name)),
        Line::from(format!("ID: {}", job.id)),
        Line::from(format!("Enabled: {}", job.enabled)),
        Line::from(format!("Provider: {}", job.provider_id)),
        Line::from(format!("Repo: {}", job.repo_path)),
        Line::from(format!("Schedule: {}", job.schedule)),
        Line::from(format!(
            "Next due: {}",
            job.next_due.as_deref().unwrap_or("-")
        )),
        Line::from(""),
        Line::from("Task"),
        Line::from(job.task.clone()),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().title("Job Detail").borders(Borders::ALL)),
        area,
    );
}

fn draw_create(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let field = |current: CreateField, label: &str, value: &str| {
        let marker = if state.create_field == current {
            ">"
        } else {
            " "
        };
        Line::from(format!("{marker} {label}: {value}"))
    };
    let mut lines = vec![
        field(CreateField::Provider, "Provider", &state.create_provider),
        field(CreateField::Repo, "Repo", &state.create_repo),
        field(CreateField::Task, "Task", &state.create_task),
        field(CreateField::Timezone, "Timezone", &state.create_timezone),
    ];
    if let Some(message) = &state.message {
        lines.push(Line::from(""));
        lines.push(Line::from(message.clone()));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().title("Create Job").borders(Borders::ALL)),
        area,
    );
}

fn draw_runs(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel, state: &TuiState) {
    let rows = model.runs.iter().enumerate().map(|(index, run)| {
        Row::new(vec![
            Cell::from(run.job_name.clone()),
            Cell::from(run.status.clone()),
            Cell::from(run.trigger.clone()),
            Cell::from(run.provider_id.clone()),
            Cell::from(run.started_at.clone().unwrap_or_else(|| "-".to_string())),
            Cell::from(run.reason.clone().unwrap_or_default()),
        ])
        .style(selected_style(index == state.selected_run))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Percentage(20),
            Constraint::Percentage(26),
        ],
    )
    .header(Row::new(vec![
        "Job", "Status", "Trigger", "Provider", "Started", "Reason",
    ]))
    .block(Block::default().title("Runs").borders(Borders::ALL));
    frame.render_widget(table, area);
}

fn draw_run_detail(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel, state: &TuiState) {
    let Some(run) = model.runs.get(state.selected_run) else {
        draw_empty(frame, area, "Run");
        return;
    };
    let text = vec![
        Line::from(format!("Run: {}", run.id)),
        Line::from(format!("Job: {}", run.job_name)),
        Line::from(format!("Status: {}", run.status)),
        Line::from(format!("Trigger: {}", run.trigger)),
        Line::from(format!("Provider: {}", run.provider_id)),
        Line::from(format!("Due: {}", run.due_at.as_deref().unwrap_or("-"))),
        Line::from(format!(
            "Started: {}",
            run.started_at.as_deref().unwrap_or("-")
        )),
        Line::from(format!(
            "Finished: {}",
            run.finished_at.as_deref().unwrap_or("-")
        )),
        Line::from(format!("Branch: {}", run.branch.as_deref().unwrap_or("-"))),
        Line::from(format!(
            "Worktree: {}",
            run.worktree_path.as_deref().unwrap_or("-")
        )),
        Line::from(format!("Reason: {}", run.reason.as_deref().unwrap_or("-"))),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().title("Run Detail").borders(Borders::ALL)),
        area,
    );
}

fn draw_logs(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel) {
    let text = if model.selected_run_logs.is_empty() {
        vec![Line::from("No logs indexed for the selected run.")]
    } else {
        model
            .selected_run_logs
            .iter()
            .flat_map(|entry| entry.lines().map(|line| Line::from(line.to_string())))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().title("Logs").borders(Borders::ALL)),
        area,
    );
}

fn draw_providers(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel, state: &TuiState) {
    let rows = model.providers.iter().enumerate().map(|(index, provider)| {
        Row::new(vec![
            Cell::from(provider.id.clone()),
            Cell::from(provider.display_name.clone()),
            Cell::from(if provider.enabled {
                "enabled"
            } else {
                "disabled"
            }),
            Cell::from(provider.command.clone()),
            Cell::from(provider.capabilities.join(",")),
        ])
        .style(selected_style(index == state.selected_provider))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Percentage(18),
            Constraint::Length(10),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ],
    )
    .header(Row::new(vec![
        "ID",
        "Name",
        "State",
        "Command",
        "Capabilities",
    ]))
    .block(Block::default().title("Providers").borders(Borders::ALL));
    frame.render_widget(table, area);
}

fn draw_settings(frame: &mut Frame<'_>, area: Rect, model: &TuiAppModel, state: &TuiState) {
    let items = model
        .settings
        .iter()
        .enumerate()
        .map(|(index, setting)| {
            ListItem::new(format!("{} = {}", setting.key, setting.value))
                .style(selected_style(index == state.selected_setting))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().title("Settings").borders(Borders::ALL)),
        area,
    );
}

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Views"),
            Line::from(
                "Left/Right moves between tabs. Tab/Shift-Tab also moves tabs outside Create.",
            ),
            Line::from(
                "Dashboard, jobs, job detail, create, runs, run detail, logs, providers, settings.",
            ),
            Line::from(""),
            Line::from("Actions"),
            Line::from(
                "On Providers, Space toggles a provider and Enter uses it for the Create view.",
            ),
            Line::from(
                "On Create, describe the task and schedule in plain English. Enter starts background creation.",
            ),
        ])
        .wrap(Wrap { trim: false })
        .block(Block::default().title("Help").borders(Borders::ALL)),
        area,
    );
}

fn draw_empty(frame: &mut Frame<'_>, area: Rect, title: &str) {
    frame.render_widget(
        Paragraph::new("No data.").block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use scheduler_core::schedule::ScheduleSpec;
    use scheduler_core::{ExecutionSpec, JobSpec, RepoSpec, TaskSpec};

    use crate::{TuiAppModel, TuiJob, TuiProvider, TuiRun, TuiSetting, TuiState};

    use super::*;

    #[test]
    fn dashboard_renders_core_sections() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let model = DashboardViewModel {
            daemon_available: false,
            jobs: vec![JobSpec {
                schema_version: "scheduler.job.v1".to_string(),
                name: "daily-report".to_string(),
                enabled: true,
                provider_id: "codex".to_string(),
                repo: RepoSpec {
                    path: "/tmp/repo".to_string(),
                    base_ref: "main".to_string(),
                    fetch_before_run: true,
                },
                schedule: ScheduleSpec::Manual {},
                task: TaskSpec {
                    prompt: "Report".to_string(),
                    success_criteria: vec![],
                },
                execution: ExecutionSpec::default(),
                delivery: Default::default(),
                notifications: Default::default(),
                metadata: Default::default(),
            }],
            recent_runs: vec![],
        };

        terminal
            .draw(|frame| draw_dashboard(frame, &model))
            .unwrap();
        let buffer = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(buffer.contains("Scheduler"));
        assert!(buffer.contains("Jobs"));
        assert!(buffer.contains("daily-report"));
        assert!(buffer.contains("Recent Runs"));
    }

    #[test]
    fn app_renders_provider_logs_and_help_views() {
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let model = TuiAppModel {
            daemon_online: true,
            daemon_pid: Some(42),
            next_due_run: Some("2026-05-12T08:00:00Z".to_string()),
            active_runs: 1,
            jobs: vec![TuiJob {
                id: "job-1".to_string(),
                name: "daily-report".to_string(),
                enabled: true,
                provider_id: "codex".to_string(),
                repo_path: "/tmp/repo".to_string(),
                schedule: "manual".to_string(),
                task: "Report".to_string(),
                next_due: None,
                last_status: Some("succeeded".to_string()),
            }],
            runs: vec![TuiRun {
                id: "run-1".to_string(),
                job_name: "daily-report".to_string(),
                status: "running".to_string(),
                trigger: "scheduled".to_string(),
                provider_id: "codex".to_string(),
                due_at: None,
                started_at: None,
                finished_at: None,
                worktree_path: None,
                branch: None,
                reason: None,
            }],
            providers: vec![TuiProvider {
                id: "codex".to_string(),
                display_name: "Codex".to_string(),
                enabled: true,
                command: "/usr/local/bin/codex".to_string(),
                capabilities: vec!["non-interactive".to_string()],
            }],
            settings: vec![TuiSetting {
                key: "retention.logs_days".to_string(),
                value: "90".to_string(),
            }],
            selected_run_logs: vec!["provider log line".to_string()],
            selected_run_artifacts: vec!["artifacts/report.md".to_string()],
        };

        for view in [TuiView::Providers, TuiView::Logs, TuiView::Help] {
            let state = TuiState {
                view,
                ..TuiState::default()
            };
            terminal
                .draw(|frame| draw_app(frame, &model, &state))
                .unwrap();
        }

        let buffer = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(buffer.contains("Help"));
    }

    #[test]
    fn footer_explains_view_navigation_per_mode() {
        let default_state = TuiState::default();
        let default_footer = footer_text(&default_state, "navigate", "-");
        assert!(default_footer.contains("Views Left/Right or Tab/Shift-Tab"));

        let create_state = TuiState {
            view: TuiView::Create,
            ..TuiState::default()
        };
        let create_footer = footer_text(&create_state, "navigate", "-");
        assert!(create_footer.contains("Views Left/Right"));
        assert!(create_footer.contains("Fields Tab/Up/Down"));
    }
}
