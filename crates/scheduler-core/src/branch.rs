use chrono::{DateTime, Utc};
use regex::Regex;
use slug::slugify;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct BranchTemplateContext {
    pub job_name: String,
    pub run_id: Uuid,
    pub scheduled_at: DateTime<Utc>,
}

pub fn render_branch_template(template: &str, context: &BranchTemplateContext) -> String {
    let mut rendered = template.to_string();
    rendered = rendered.replace("{job_slug}", &slugify(&context.job_name));
    rendered = rendered.replace("{run_id}", &context.run_id.to_string());
    rendered = rendered.replace("{date}", &context.scheduled_at.format("%Y%m%d").to_string());
    rendered = rendered.replace(
        "{datetime}",
        &context.scheduled_at.format("%Y%m%dT%H%M%SZ").to_string(),
    );
    sanitize_branch_name(&rendered)
}

pub fn validate_branch_template(template: &str) -> Result<(), String> {
    if template.trim().is_empty() {
        return Err("branch template must not be empty".to_string());
    }
    let allowed = Regex::new(r"\{(job_slug|run_id|date|datetime)\}").unwrap();
    let stripped = allowed.replace_all(template, "");
    if stripped.contains('{') || stripped.contains('}') {
        return Err("branch template contains unknown placeholder".to_string());
    }
    Ok(())
}

fn sanitize_branch_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut previous_slash = false;

    for ch in value.chars() {
        let next = if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.') {
            ch
        } else {
            '-'
        };

        if next == '/' {
            if previous_slash {
                continue;
            }
            previous_slash = true;
        } else {
            previous_slash = false;
        }
        out.push(next);
    }

    out.trim_matches('/')
        .trim_matches('.')
        .replace("..", ".")
        .trim_end_matches(".lock")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn renders_supported_placeholders() {
        let run_id = Uuid::parse_str("018f0000-0000-7000-8000-000000000001").unwrap();
        let context = BranchTemplateContext {
            job_name: "Nightly Issue Worker".to_string(),
            run_id,
            scheduled_at: Utc.with_ymd_and_hms(2026, 5, 12, 18, 0, 0).unwrap(),
        };

        assert_eq!(
            render_branch_template("scheduler/{job_slug}/{datetime}/{run_id}", &context),
            "scheduler/nightly-issue-worker/20260512T180000Z/018f0000-0000-7000-8000-000000000001"
        );
    }

    #[test]
    fn rejects_unknown_placeholders() {
        assert_eq!(
            validate_branch_template("scheduler/{unknown}"),
            Err("branch template contains unknown placeholder".to_string())
        );
    }
}
