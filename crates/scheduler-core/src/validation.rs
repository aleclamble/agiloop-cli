use std::collections::HashSet;
use std::path::Path;

use crate::branch::validate_branch_template;
use crate::error::ValidationError;
use crate::job::{ApprovalPolicy, IsolationMode, JobSpec};

#[derive(Debug, Clone, Default)]
pub struct ValidationContext {
    pub enabled_provider_ids: HashSet<String>,
    pub require_enabled_provider: bool,
    pub require_existing_repo: bool,
}

pub fn validate_job_spec(
    spec: &JobSpec,
    context: &ValidationContext,
) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if spec.schema_version != "scheduler.job.v1" {
        errors.push(ValidationError::field(
            "schema_version",
            "must be scheduler.job.v1",
        ));
    }
    if spec.name.trim().is_empty() {
        errors.push(ValidationError::field("name", "must not be empty"));
    }
    if spec.provider_id.trim().is_empty() {
        errors.push(ValidationError::field("provider_id", "must not be empty"));
    }
    if context.require_enabled_provider && !context.enabled_provider_ids.contains(&spec.provider_id)
    {
        errors.push(ValidationError::field(
            "provider_id",
            format!("provider `{}` is not enabled", spec.provider_id),
        ));
    }
    if spec.repo.path.trim().is_empty() {
        errors.push(ValidationError::field("repo.path", "must not be empty"));
    } else if context.require_existing_repo {
        let repo_path = Path::new(&spec.repo.path);
        if !repo_path.exists() {
            errors.push(ValidationError::field(
                "repo.path",
                "repository path does not exist",
            ));
        } else if !repo_path.join(".git").exists() {
            errors.push(ValidationError::field(
                "repo.path",
                "path is not a Git repository",
            ));
        }
    }
    if let Err(error) = spec.schedule.validate() {
        errors.push(ValidationError::field("schedule", error));
    }
    if spec.task.prompt.trim().is_empty() {
        errors.push(ValidationError::field("task.prompt", "must not be empty"));
    }
    if spec.execution.timeout_seconds == 0 {
        errors.push(ValidationError::field(
            "execution.timeout_seconds",
            "must be greater than zero",
        ));
    }
    if let Err(error) = validate_branch_template(&spec.execution.branch_template) {
        errors.push(ValidationError::field("execution.branch_template", error));
    }
    if matches!(spec.execution.isolation, IsolationMode::GitWorktree)
        && spec.repo.base_ref.trim().is_empty()
    {
        errors.push(ValidationError::field(
            "repo.base_ref",
            "must not be empty when using git_worktree isolation",
        ));
    }
    if spec.enabled
        && matches!(
            spec.execution.approval_policy,
            ApprovalPolicy::ProviderDefault
        )
    {
        errors.push(ValidationError::field(
            "execution.approval_policy",
            "scheduled enabled jobs cannot use provider_default approval policy",
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::job::{ExecutionSpec, RepoSpec, TaskSpec};
    use crate::schedule::{MisfirePolicy, ScheduleSpec};

    use super::*;

    fn valid_spec() -> JobSpec {
        JobSpec {
            schema_version: "scheduler.job.v1".to_string(),
            name: "overnight report".to_string(),
            enabled: true,
            provider_id: "codex".to_string(),
            repo: RepoSpec {
                path: "/tmp/example".to_string(),
                base_ref: "main".to_string(),
                fetch_before_run: true,
            },
            schedule: ScheduleSpec::Cron {
                expression: "0 8 * * *".to_string(),
                timezone: "Africa/Johannesburg".to_string(),
                misfire_policy: MisfirePolicy::RunOnce,
            },
            task: TaskSpec {
                prompt: "Create a report".to_string(),
                success_criteria: vec![],
            },
            execution: ExecutionSpec::default(),
            delivery: Default::default(),
            notifications: Default::default(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn accepts_valid_spec_without_filesystem_checks() {
        let spec = valid_spec();
        let context = ValidationContext {
            enabled_provider_ids: HashSet::from(["codex".to_string()]),
            require_enabled_provider: true,
            require_existing_repo: false,
        };

        assert!(validate_job_spec(&spec, &context).is_ok());
    }

    #[test]
    fn rejects_disabled_provider() {
        let spec = valid_spec();
        let context = ValidationContext {
            enabled_provider_ids: HashSet::from(["claude".to_string()]),
            require_enabled_provider: true,
            require_existing_repo: false,
        };

        let errors = validate_job_spec(&spec, &context).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("codex"))
        );
    }

    #[test]
    fn rejects_provider_default_for_enabled_scheduled_job() {
        let mut spec = valid_spec();
        spec.execution.approval_policy = ApprovalPolicy::ProviderDefault;

        let errors = validate_job_spec(&spec, &ValidationContext::default()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("provider_default"))
        );
    }

    #[test]
    fn serializes_to_toml_with_defaults() {
        let spec = valid_spec();
        let encoded = toml::to_string(&spec).unwrap();
        let decoded: JobSpec = toml::from_str(&encoded).unwrap();

        assert_eq!(decoded.execution.concurrency, spec.execution.concurrency);
        assert_eq!(decoded.provider_id, "codex");
    }

    #[test]
    fn generates_job_spec_json_schema() {
        let schema = crate::job_spec_json_schema();

        assert!(schema.schema.metadata.is_some());
        assert!(
            schema
                .schema
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.title.as_deref())
                .is_some_and(|title| title.contains("JobSpec"))
        );
    }
}
