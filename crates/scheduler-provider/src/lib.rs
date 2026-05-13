use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use scheduler_core::{ApprovalPolicy, JobSpec};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider `{0}` was not found on PATH")]
    NotFound(String),
    #[error("provider command failed: {0}")]
    Command(String),
    #[error("invalid provider response: {0}")]
    InvalidResponse(String),
    #[error("provider timed out after {0:?}")]
    Timeout(Duration),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCapability {
    pub supports_spec_builder: bool,
    pub supports_task_execution: bool,
    pub supports_non_interactive: bool,
    pub supports_working_directory: bool,
    pub supports_prompt_file: bool,
    pub supports_stdin_prompt: bool,
    pub supports_streaming_output: bool,
    pub supports_structured_output: bool,
    pub supports_cancellation: bool,
    pub default_timeout_seconds: u64,
}

impl Default for ProviderCapability {
    fn default() -> Self {
        Self {
            supports_spec_builder: true,
            supports_task_execution: true,
            supports_non_interactive: false,
            supports_working_directory: true,
            supports_prompt_file: true,
            supports_stdin_prompt: true,
            supports_streaming_output: true,
            supports_structured_output: false,
            supports_cancellation: true,
            default_timeout_seconds: 3_600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderDetection {
    pub id: String,
    pub display_name: String,
    pub binary_path: Option<PathBuf>,
    pub version: Option<String>,
    pub available: bool,
    pub capabilities: ProviderCapability,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConfig {
    pub id: String,
    pub display_name: String,
    pub command: PathBuf,
    pub enabled: bool,
    pub capabilities: ProviderCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderInvocation {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomProviderDefinition {
    pub id: String,
    pub display_name: String,
    pub command: PathBuf,
    #[serde(default)]
    pub spec_builder_args: Vec<String>,
    #[serde(default)]
    pub execute_args: Vec<String>,
    pub prompt_mode: PromptMode,
    #[serde(default)]
    pub capabilities: ProviderCapability,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Stdin,
    Argument,
    PromptFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecBuildRequest {
    pub provider_id: String,
    pub repo_path: String,
    pub timezone: String,
    pub user_request: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunExecutionRequest {
    pub provider_id: String,
    pub prompt: String,
    pub working_dir: PathBuf,
    pub context_path: PathBuf,
    pub approval_policy: ApprovalPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecBuilderEnvelope {
    pub status: SpecBuilderStatus,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub summary: Option<SpecBuilderSummary>,
    pub job_spec: Option<JobSpec>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpecBuilderStatus {
    Ok,
    NeedsClarification,
    Unsafe,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecBuilderSummary {
    pub human: String,
    pub schedule: String,
    pub task: String,
}

pub trait ProviderAdapter {
    fn id(&self) -> &str;
    fn detect(&self) -> ProviderDetection;
    fn build_spec(&self, request: &SpecBuildRequest) -> ProviderInvocation;
    fn execute_run(&self, request: &RunExecutionRequest) -> ProviderInvocation;
}

#[derive(Debug, Clone)]
pub struct CommandProviderAdapter {
    definition: CustomProviderDefinition,
}

impl CommandProviderAdapter {
    pub fn new(definition: CustomProviderDefinition) -> Self {
        Self { definition }
    }
}

impl ProviderAdapter for CommandProviderAdapter {
    fn id(&self) -> &str {
        &self.definition.id
    }

    fn detect(&self) -> ProviderDetection {
        let available = self.definition.command.is_file()
            || find_on_path(&self.definition.command.to_string_lossy()).is_some();
        ProviderDetection {
            id: self.definition.id.clone(),
            display_name: self.definition.display_name.clone(),
            binary_path: Some(self.definition.command.clone()),
            version: None,
            available,
            capabilities: self.definition.capabilities.clone(),
            error: (!available).then(|| "custom provider command not found".to_string()),
        }
    }

    fn build_spec(&self, request: &SpecBuildRequest) -> ProviderInvocation {
        let prompt = build_spec_prompt(request);
        prompt_invocation(
            &self.definition.command,
            &self.definition.spec_builder_args,
            self.definition.prompt_mode,
            prompt,
            None,
        )
    }

    fn execute_run(&self, request: &RunExecutionRequest) -> ProviderInvocation {
        prompt_invocation(
            &self.definition.command,
            &self.definition.execute_args,
            self.definition.prompt_mode,
            request.prompt.clone(),
            Some(request.working_dir.clone()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltInProviderKind {
    Codex,
    Claude,
    OpenCode,
}

impl BuiltInProviderKind {
    fn from_id(id: &str) -> Option<Self> {
        match id {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "opencode" => Some(Self::OpenCode),
            _ => None,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::OpenCode => "OpenCode",
        }
    }

    fn version_args(self) -> &'static [&'static str] {
        match self {
            Self::Codex | Self::Claude | Self::OpenCode => &["--version"],
        }
    }

    fn capabilities(self) -> ProviderCapability {
        let mut capabilities = ProviderCapability {
            supports_non_interactive: true,
            supports_prompt_file: false,
            supports_structured_output: matches!(self, Self::Codex | Self::Claude),
            ..ProviderCapability::default()
        };
        if matches!(self, Self::Claude | Self::OpenCode) {
            capabilities.supports_stdin_prompt = false;
        }
        capabilities
    }
}

#[derive(Debug, Clone)]
pub struct BuiltInProviderAdapter {
    kind: BuiltInProviderKind,
    command: PathBuf,
}

impl BuiltInProviderAdapter {
    pub fn from_config(config: &ProviderConfig) -> Option<Self> {
        Some(Self {
            kind: BuiltInProviderKind::from_id(&config.id)?,
            command: config.command.clone(),
        })
    }

    fn prompt_invocation(
        &self,
        prompt: String,
        working_dir: Option<PathBuf>,
        approval_policy: ApprovalPolicy,
    ) -> ProviderInvocation {
        match self.kind {
            BuiltInProviderKind::Codex => {
                codex_invocation(&self.command, prompt, working_dir, approval_policy)
            }
            BuiltInProviderKind::Claude => {
                claude_invocation(&self.command, prompt, working_dir, approval_policy)
            }
            BuiltInProviderKind::OpenCode => {
                opencode_invocation(&self.command, prompt, working_dir)
            }
        }
    }
}

impl ProviderAdapter for BuiltInProviderAdapter {
    fn id(&self) -> &str {
        self.kind.id()
    }

    fn detect(&self) -> ProviderDetection {
        let available =
            self.command.is_file() || find_on_path(&self.command.to_string_lossy()).is_some();
        ProviderDetection {
            id: self.kind.id().to_string(),
            display_name: self.kind.display_name().to_string(),
            binary_path: available.then(|| self.command.clone()),
            version: available
                .then(|| probe_version(&self.command, self.kind.version_args()).ok())
                .flatten(),
            available,
            capabilities: self.kind.capabilities(),
            error: (!available).then(|| format!("{} not found", self.command.display())),
        }
    }

    fn build_spec(&self, request: &SpecBuildRequest) -> ProviderInvocation {
        self.prompt_invocation(
            build_spec_prompt(request),
            None,
            ApprovalPolicy::ProviderDefault,
        )
    }

    fn execute_run(&self, request: &RunExecutionRequest) -> ProviderInvocation {
        self.prompt_invocation(
            request.prompt.clone(),
            Some(request.working_dir.clone()),
            request.approval_policy,
        )
    }
}

#[derive(Debug, Clone)]
pub struct BuiltInProvider {
    pub id: &'static str,
    pub display_name: &'static str,
    pub command: &'static str,
    pub version_args: &'static [&'static str],
    pub non_interactive: bool,
}

pub fn built_in_providers() -> Vec<BuiltInProvider> {
    vec![
        BuiltInProvider {
            id: "codex",
            display_name: "Codex",
            command: "codex",
            version_args: &["--version"],
            non_interactive: true,
        },
        BuiltInProvider {
            id: "claude",
            display_name: "Claude Code",
            command: "claude",
            version_args: &["--version"],
            non_interactive: true,
        },
        BuiltInProvider {
            id: "opencode",
            display_name: "OpenCode",
            command: "opencode",
            version_args: &["--version"],
            non_interactive: true,
        },
    ]
}

pub fn detect_built_in_providers() -> Vec<ProviderDetection> {
    built_in_providers()
        .into_iter()
        .map(|provider| detect_provider(&provider))
        .collect()
}

pub fn detect_provider(provider: &BuiltInProvider) -> ProviderDetection {
    match find_on_path(provider.command) {
        Some(binary_path) => {
            let version = probe_version(&binary_path, provider.version_args).ok();
            let mut capabilities = BuiltInProviderKind::from_id(provider.id)
                .map(BuiltInProviderKind::capabilities)
                .unwrap_or_default();
            capabilities.supports_non_interactive = provider.non_interactive;
            ProviderDetection {
                id: provider.id.to_string(),
                display_name: provider.display_name.to_string(),
                binary_path: Some(binary_path),
                version,
                available: true,
                capabilities,
                error: None,
            }
        }
        None => ProviderDetection {
            id: provider.id.to_string(),
            display_name: provider.display_name.to_string(),
            binary_path: None,
            version: None,
            available: false,
            capabilities: ProviderCapability::default(),
            error: Some(format!("{} not found on PATH", provider.command)),
        },
    }
}

pub fn parse_spec_builder_envelope(input: &str) -> Result<SpecBuilderEnvelope, ProviderError> {
    serde_json::from_str(input).map_err(|error| ProviderError::InvalidResponse(error.to_string()))
}

pub fn run_invocation(
    invocation: &ProviderInvocation,
    timeout: Duration,
) -> Result<ProviderOutput, ProviderError> {
    run_invocation_with_observer(invocation, timeout, |_| Ok(()))
}

pub fn run_invocation_with_observer<F>(
    invocation: &ProviderInvocation,
    timeout: Duration,
    on_spawn: F,
) -> Result<ProviderOutput, ProviderError>
where
    F: FnOnce(u32) -> Result<(), ProviderError>,
{
    let mut command = Command::new(&invocation.command);
    command.args(&invocation.args);
    for (key, value) in &invocation.env {
        command.env(key, value);
    }
    if let Some(working_dir) = &invocation.working_dir {
        command.current_dir(working_dir);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if invocation.stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = spawn_provider_command(command, invocation)?;
    let child_id = child.id();
    if let Err(error) = on_spawn(child_id) {
        let _ = terminate_process_group(child_id);
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    if let Some(stdin) = &invocation.stdin {
        use std::io::Write;
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Command("failed to open provider stdin".to_string()))?;
        if let Err(error) = child_stdin.write_all(stdin.as_bytes()) {
            if error.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(ProviderError::Command(error.to_string()));
            }
        }
        if let Err(error) = child_stdin.flush()
            && error.kind() != std::io::ErrorKind::BrokenPipe
        {
            return Err(ProviderError::Command(error.to_string()));
        }
        drop(child_stdin);
    }

    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| ProviderError::Command(error.to_string()))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .map_err(|error| ProviderError::Command(error.to_string()))?;
            return Ok(ProviderOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                timed_out: false,
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|error| ProviderError::Command(error.to_string()))?;
            return Ok(ProviderOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                timed_out: true,
            });
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn spawn_provider_command(
    mut command: Command,
    invocation: &ProviderInvocation,
) -> Result<std::process::Child, ProviderError> {
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && invocation.command.exists() =>
        {
            let mut fallback = Command::new("sh");
            fallback.arg(&invocation.command).args(&invocation.args);
            for (key, value) in &invocation.env {
                fallback.env(key, value);
            }
            if let Some(working_dir) = &invocation.working_dir {
                fallback.current_dir(working_dir);
            }
            fallback.stdout(Stdio::piped()).stderr(Stdio::piped());
            if invocation.stdin.is_some() {
                fallback.stdin(Stdio::piped());
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                fallback.process_group(0);
            }
            fallback
                .spawn()
                .map_err(|fallback_error| ProviderError::Command(fallback_error.to_string()))
        }
        Err(error) => Err(ProviderError::Command(format!(
            "{}: {}",
            invocation.command.display(),
            error
        ))),
    }
}

pub fn terminate_process_group(process_group_id: u32) -> Result<(), ProviderError> {
    #[cfg(unix)]
    {
        signal_process_group(process_group_id, "TERM")?;
        std::thread::sleep(Duration::from_millis(250));
        let _ = signal_process_group(process_group_id, "KILL");
        Ok(())
    }

    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .arg("/PID")
            .arg(process_group_id.to_string())
            .arg("/T")
            .arg("/F")
            .status()
            .map_err(|error| ProviderError::Command(error.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(ProviderError::Command(format!(
                "taskkill exited with status {status}"
            )))
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = process_group_id;
        Err(ProviderError::Command(
            "process cancellation is not supported on this platform".to_string(),
        ))
    }
}

#[cfg(unix)]
fn signal_process_group(process_group_id: u32, signal: &str) -> Result<(), ProviderError> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -{signal} -{process_group_id}"))
        .status()
        .map_err(|error| ProviderError::Command(error.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(ProviderError::Command(format!(
            "kill -{signal} -{process_group_id} exited with status {status}"
        )))
    }
}

pub fn build_spec_prompt(request: &SpecBuildRequest) -> String {
    format!(
        r#"You are creating a scheduler job spec. Do not execute the task.
Return only valid JSON matching the scheduler spec-builder envelope.

Defaults:
- provider_id: {provider_id}
- repo.path: {repo_path}
- schedule.timezone: {timezone}
- execution.concurrency: skip
- execution.isolation: git_worktree
- execution.repo_lock: none
- execution.approval_policy: non_interactive

Rules:
- Do not invent repository paths.
- Do not invent provider IDs.
- Ask clarification if schedule wording is ambiguous.
- Keep the task prompt faithful to the user's request.

User request:
{user_request}
"#,
        provider_id = request.provider_id,
        repo_path = request.repo_path,
        timezone = request.timezone,
        user_request = request.user_request
    )
}

pub fn build_provider_spec_invocation(
    provider: &ProviderConfig,
    prompt: String,
) -> ProviderInvocation {
    if let Some(adapter) = BuiltInProviderAdapter::from_config(provider) {
        return adapter.prompt_invocation(prompt, None, ApprovalPolicy::ProviderDefault);
    }

    ProviderInvocation {
        command: provider.command.clone(),
        args: vec![],
        stdin: Some(prompt),
        working_dir: None,
        env: vec![],
    }
}

pub fn build_provider_run_invocation(
    provider: &ProviderConfig,
    request: &RunExecutionRequest,
) -> ProviderInvocation {
    if let Some(adapter) = BuiltInProviderAdapter::from_config(provider) {
        return adapter.execute_run(request);
    }

    ProviderInvocation {
        command: provider.command.clone(),
        args: vec![],
        stdin: Some(request.prompt.clone()),
        working_dir: Some(request.working_dir.clone()),
        env: vec![],
    }
}

pub fn built_in_provider_config(detection: &ProviderDetection) -> Option<ProviderConfig> {
    Some(ProviderConfig {
        id: detection.id.clone(),
        display_name: detection.display_name.clone(),
        command: detection.binary_path.clone()?,
        enabled: false,
        capabilities: detection.capabilities.clone(),
    })
}

fn codex_invocation(
    command: &Path,
    prompt: String,
    working_dir: Option<PathBuf>,
    approval_policy: ApprovalPolicy,
) -> ProviderInvocation {
    let mut args = vec!["exec".to_string(), "--skip-git-repo-check".to_string()];
    if approval_policy == ApprovalPolicy::NonInteractive {
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    } else {
        args.push("--sandbox".to_string());
        args.push("workspace-write".to_string());
    }
    if let Some(working_dir) = &working_dir {
        args.push("--cd".to_string());
        args.push(working_dir.display().to_string());
    }
    args.push("-".to_string());
    ProviderInvocation {
        command: command.to_path_buf(),
        args,
        stdin: Some(prompt),
        working_dir,
        env: vec![],
    }
}

fn claude_invocation(
    command: &Path,
    prompt: String,
    working_dir: Option<PathBuf>,
    approval_policy: ApprovalPolicy,
) -> ProviderInvocation {
    let mut args = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "text".to_string(),
        "--input-format".to_string(),
        "text".to_string(),
        "--no-session-persistence".to_string(),
    ];
    if approval_policy == ApprovalPolicy::NonInteractive {
        args.push("--dangerously-skip-permissions".to_string());
    } else {
        args.push("--permission-mode".to_string());
        args.push("dontAsk".to_string());
    }
    if let Some(working_dir) = &working_dir {
        args.push("--add-dir".to_string());
        args.push(working_dir.display().to_string());
    }
    args.push(prompt);
    ProviderInvocation {
        command: command.to_path_buf(),
        args,
        stdin: None,
        working_dir,
        env: vec![],
    }
}

fn opencode_invocation(
    command: &Path,
    prompt: String,
    working_dir: Option<PathBuf>,
) -> ProviderInvocation {
    ProviderInvocation {
        command: command.to_path_buf(),
        args: vec!["run".to_string(), prompt],
        stdin: None,
        working_dir,
        env: vec![],
    }
}

fn prompt_invocation(
    command: &Path,
    base_args: &[String],
    prompt_mode: PromptMode,
    prompt: String,
    working_dir: Option<PathBuf>,
) -> ProviderInvocation {
    let mut args = base_args.to_vec();
    let stdin = match prompt_mode {
        PromptMode::Stdin => Some(prompt),
        PromptMode::Argument => {
            args.push(prompt);
            None
        }
        PromptMode::PromptFile => Some(prompt),
    };

    ProviderInvocation {
        command: command.to_path_buf(),
        args,
        stdin,
        working_dir,
        env: vec![],
    }
}

fn probe_version(binary: &Path, args: &[&str]) -> Result<String, ProviderError> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|error| ProviderError::Command(error.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let text = if stdout.is_empty() { stderr } else { stdout };
    if text.is_empty() {
        Ok("unknown".to_string())
    } else {
        Ok(text.lines().next().unwrap_or("unknown").to_string())
    }
}

fn find_on_path(command: &str) -> Option<PathBuf> {
    if command.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(command);
        return path.is_file().then_some(path);
    }

    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn with_path<T>(path: OsString, f: impl FnOnce() -> T) -> T {
        let previous = env::var_os("PATH");
        unsafe {
            env::set_var("PATH", path);
        }
        let result = f();
        unsafe {
            match previous {
                Some(value) => env::set_var("PATH", value),
                None => env::remove_var("PATH"),
            }
        }
        result
    }

    #[test]
    fn parses_spec_builder_envelope_status() {
        let parsed = parse_spec_builder_envelope(
            r#"{"status":"needs_clarification","questions":["Which repo?"],"warnings":[],"summary":null,"job_spec":null}"#,
        )
        .unwrap();

        assert_eq!(parsed.status, SpecBuilderStatus::NeedsClarification);
        assert_eq!(parsed.questions, vec!["Which repo?"]);
    }

    #[test]
    fn spec_prompt_contains_defaults_and_request() {
        let prompt = build_spec_prompt(&SpecBuildRequest {
            provider_id: "codex".to_string(),
            repo_path: "/tmp/repo".to_string(),
            timezone: "Africa/Johannesburg".to_string(),
            user_request: "Run daily".to_string(),
        });

        assert!(prompt.contains("execution.concurrency: skip"));
        assert!(prompt.contains("provider_id: codex"));
        assert!(prompt.contains("Run daily"));
    }

    #[test]
    fn custom_provider_builds_stdin_invocation() {
        let adapter = CommandProviderAdapter::new(CustomProviderDefinition {
            id: "custom".to_string(),
            display_name: "Custom".to_string(),
            command: PathBuf::from("/bin/cat"),
            spec_builder_args: vec!["--json".to_string()],
            execute_args: vec!["run".to_string()],
            prompt_mode: PromptMode::Stdin,
            capabilities: ProviderCapability::default(),
        });
        let invocation = adapter.build_spec(&SpecBuildRequest {
            provider_id: "custom".to_string(),
            repo_path: "/tmp/repo".to_string(),
            timezone: "UTC".to_string(),
            user_request: "daily report".to_string(),
        });

        assert_eq!(invocation.command, PathBuf::from("/bin/cat"));
        assert_eq!(invocation.args, vec!["--json"]);
        assert!(invocation.stdin.unwrap().contains("daily report"));
    }

    #[test]
    fn codex_adapter_builds_expected_invocations() {
        let provider = ProviderConfig {
            id: "codex".to_string(),
            display_name: "Codex".to_string(),
            command: PathBuf::from("/tmp/codex"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let spec_invocation = build_provider_spec_invocation(&provider, "make a spec".to_string());

        assert_eq!(spec_invocation.command, PathBuf::from("/tmp/codex"));
        assert_eq!(
            spec_invocation.args,
            vec![
                "exec",
                "--skip-git-repo-check",
                "--sandbox",
                "workspace-write",
                "-"
            ]
        );
        assert_eq!(spec_invocation.stdin.as_deref(), Some("make a spec"));

        let run_invocation = build_provider_run_invocation(
            &provider,
            &RunExecutionRequest {
                provider_id: "codex".to_string(),
                prompt: "do the task".to_string(),
                working_dir: PathBuf::from("/tmp/worktree"),
                context_path: PathBuf::from("/tmp/context"),
                approval_policy: ApprovalPolicy::NonInteractive,
            },
        );

        assert_eq!(
            run_invocation.args,
            vec![
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "--cd",
                "/tmp/worktree",
                "-"
            ]
        );
        assert_eq!(
            run_invocation.working_dir,
            Some(PathBuf::from("/tmp/worktree"))
        );
        assert_eq!(run_invocation.stdin.as_deref(), Some("do the task"));
    }

    #[test]
    fn claude_adapter_builds_expected_invocations() {
        let provider = ProviderConfig {
            id: "claude".to_string(),
            display_name: "Claude Code".to_string(),
            command: PathBuf::from("/tmp/claude"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let invocation = build_provider_run_invocation(
            &provider,
            &RunExecutionRequest {
                provider_id: "claude".to_string(),
                prompt: "do the task".to_string(),
                working_dir: PathBuf::from("/tmp/worktree"),
                context_path: PathBuf::from("/tmp/context"),
                approval_policy: ApprovalPolicy::NonInteractive,
            },
        );

        assert_eq!(invocation.command, PathBuf::from("/tmp/claude"));
        assert_eq!(
            invocation.args,
            vec![
                "--print",
                "--output-format",
                "text",
                "--input-format",
                "text",
                "--no-session-persistence",
                "--dangerously-skip-permissions",
                "--add-dir",
                "/tmp/worktree",
                "do the task"
            ]
        );
        assert!(invocation.stdin.is_none());
        assert_eq!(invocation.working_dir, Some(PathBuf::from("/tmp/worktree")));
    }

    #[test]
    fn opencode_adapter_builds_expected_invocations() {
        let provider = ProviderConfig {
            id: "opencode".to_string(),
            display_name: "OpenCode".to_string(),
            command: PathBuf::from("/tmp/opencode"),
            enabled: true,
            capabilities: ProviderCapability::default(),
        };
        let invocation = build_provider_run_invocation(
            &provider,
            &RunExecutionRequest {
                provider_id: "opencode".to_string(),
                prompt: "do the task".to_string(),
                working_dir: PathBuf::from("/tmp/worktree"),
                context_path: PathBuf::from("/tmp/context"),
                approval_policy: ApprovalPolicy::NonInteractive,
            },
        );

        assert_eq!(invocation.command, PathBuf::from("/tmp/opencode"));
        assert_eq!(invocation.args, vec!["run", "do the task"]);
        assert!(invocation.stdin.is_none());
        assert_eq!(invocation.working_dir, Some(PathBuf::from("/tmp/worktree")));
    }

    #[test]
    fn detects_fake_provider_on_path() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("codex");
        std::fs::write(&bin, "#!/bin/sh\necho codex 1.2.3\n").unwrap();
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bin, permissions).unwrap();
        }

        let result = with_path(temp.path().as_os_str().to_os_string(), || {
            detect_provider(&BuiltInProvider {
                id: "codex",
                display_name: "Codex",
                command: "codex",
                version_args: &["--version"],
                non_interactive: true,
            })
        });

        assert!(result.available);
        assert_eq!(result.version.as_deref(), Some("codex 1.2.3"));
    }

    #[test]
    fn runs_invocation_with_stdin() {
        let output = run_invocation(
            &ProviderInvocation {
                command: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_string(), "cat".to_string()],
                stdin: Some("hello".to_string()),
                working_dir: None,
                env: vec![],
            },
            Duration::from_secs(2),
        )
        .unwrap();

        assert_eq!(output.stdout, "hello");
        assert!(!output.timed_out);
    }

    #[test]
    fn invocation_observer_receives_process_id() {
        let mut observed = None;
        let output = run_invocation_with_observer(
            &ProviderInvocation {
                command: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_string(), "printf ok".to_string()],
                stdin: None,
                working_dir: None,
                env: vec![],
            },
            Duration::from_secs(2),
            |process_id| {
                observed = Some(process_id);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(output.stdout, "ok");
        assert!(observed.is_some_and(|process_id| process_id > 0));
    }

    #[test]
    fn times_out_invocation() {
        let output = run_invocation(
            &ProviderInvocation {
                command: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_string(), "while :; do :; done".to_string()],
                stdin: None,
                working_dir: None,
                env: vec![],
            },
            Duration::from_millis(50),
        )
        .unwrap();

        assert!(output.timed_out);
    }

    #[test]
    fn passes_environment_to_invocation() {
        let output = run_invocation(
            &ProviderInvocation {
                command: PathBuf::from("/bin/sh"),
                args: vec![
                    "-c".to_string(),
                    "printf %s \"$SCHEDULER_TEST_VALUE\"".to_string(),
                ],
                stdin: None,
                working_dir: None,
                env: vec![("SCHEDULER_TEST_VALUE".to_string(), "present".to_string())],
            },
            Duration::from_secs(2),
        )
        .unwrap();

        assert_eq!(output.stdout, "present");
    }
}
