use std::process::Command;
use std::time::{Duration, Instant};

fn scheduler() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scheduler"))
}

#[test]
fn completions_are_generated_for_supported_shells() {
    for shell in ["bash", "zsh", "fish"] {
        let output = scheduler().args(["completions", shell]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("scheduler"), "{stdout}");
    }
}

#[test]
fn setup_records_provider_probe_history() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");

    assert_command_ok(
        scheduler()
            .args(["--config", config.to_str().unwrap(), "setup"])
            .output()
            .unwrap(),
    );

    let store = scheduler_store::Store::open(config.join("data/scheduler.sqlite3")).unwrap();
    for provider_id in ["claude", "codex", "opencode"] {
        let probes = store.list_provider_probes(provider_id).unwrap();
        assert_eq!(probes.len(), 1, "{provider_id}");
    }
}

#[test]
fn daemon_start_status_and_stop_manage_background_process() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");

    assert_command_ok(
        scheduler()
            .env("SCHEDULER_DAEMON_INTERVAL_SECONDS", "1")
            .args(["--config", config.to_str().unwrap(), "daemon", "start"])
            .output()
            .unwrap(),
    );

    let status = wait_for_daemon_running(config.to_str().unwrap(), true);
    assert!(status["pid"].as_u64().unwrap_or_default() > 0);

    assert_command_ok(
        scheduler()
            .args(["--config", config.to_str().unwrap(), "daemon", "stop"])
            .output()
            .unwrap(),
    );

    let status = wait_for_daemon_running(config.to_str().unwrap(), false);
    assert_eq!(status["running"], serde_json::json!(false));
}

#[test]
fn tui_renders_snapshot_when_stdout_is_not_a_tty() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "tui"])
        .output()
        .unwrap();

    assert_command_ok(output);
}

#[cfg(unix)]
#[test]
fn default_launch_detects_providers_and_renders_tui_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let codex = bin.join("codex");
    std::fs::write(&codex, "#!/bin/sh\necho codex 1.2.3\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&codex).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&codex, permissions).unwrap();
    }

    let output = scheduler()
        .env("PATH", bin)
        .args(["--config", config.to_str().unwrap()])
        .output()
        .unwrap();

    assert_command_ok(output);
    let store = scheduler_store::Store::open(config.join("data/scheduler.sqlite3")).unwrap();
    let providers = store.list_providers().unwrap();
    let codex_provider = providers
        .iter()
        .find(|provider| provider.id == "codex")
        .expect("codex provider should be configured");
    assert!(!codex_provider.enabled);
    assert_eq!(store.list_provider_probes("codex").unwrap().len(), 1);
}

#[test]
fn cancel_terminates_active_provider_process() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let provider = temp.path().join("provider.sh");
    let marker = temp.path().join("provider-finished");
    std::fs::write(
        &provider,
        format!(
            r#"#!/bin/sh
trap 'exit 0' TERM INT
while :; do sleep 1; done
touch "{}"
"#,
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&provider, permissions).unwrap();
    }
    let job_spec = temp.path().join("cancellable.json");
    std::fs::write(
        &job_spec,
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": "scheduler.job.v1",
            "name": "cancellable",
            "enabled": true,
            "provider_id": "sleeper",
            "repo": {
                "path": temp.path().display().to_string(),
                "base_ref": "main",
                "fetch_before_run": false
            },
            "schedule": {
                "kind": "manual"
            },
            "task": {
                "prompt": "Sleep until cancelled.",
                "success_criteria": []
            },
            "execution": {
                "isolation": "none",
                "concurrency": "skip",
                "repo_lock": "none",
                "timeout_seconds": 30,
                "approval_policy": "non_interactive",
                "branch_template": "scheduler/{job_slug}/{run_id}",
                "worktree_cleanup": {
                    "on_success": "after_retention",
                    "on_failure": "keep",
                    "retention_days": 14
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    assert_command_ok(
        scheduler()
            .args([
                "--config",
                config.to_str().unwrap(),
                "provider",
                "add-custom",
                "sleeper",
                provider.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
    );
    assert_command_ok(
        scheduler()
            .args([
                "--config",
                config.to_str().unwrap(),
                "create",
                "--from-file",
                job_spec.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
    );

    let mut child = scheduler()
        .args(["--config", config.to_str().unwrap(), "run", "cancellable"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let run_id = wait_for_run_status(config.to_str().unwrap(), "cancellable", "running");

    assert_command_ok(
        scheduler()
            .args(["--config", config.to_str().unwrap(), "cancel", &run_id])
            .output()
            .unwrap(),
    );
    wait_for_child_exit(&mut child, Duration::from_secs(15));

    let final_status = wait_for_run_status(config.to_str().unwrap(), "cancellable", "cancelled");
    assert_eq!(final_status, run_id);
    assert!(!marker.exists());
}

#[test]
fn custom_provider_job_can_be_created_run_and_listed() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/jobs/manual-shell.json");
    let provider = temp.path().join("provider.sh");
    std::fs::write(
        &provider,
        r#"#!/bin/sh
cat
printf "artifact" > "$SCHEDULER_ARTIFACTS_DIR/report.md"
cat > "$SCHEDULER_SUMMARY_PATH" <<JSON
{
  "status": "succeeded",
  "summary": "provider completed",
  "artifacts": [
    {
      "path": "artifacts/report.md",
      "kind": "report"
    }
  ],
  "files_changed": []
}
JSON
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&provider, permissions).unwrap();
    }

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "provider",
            "add-custom",
            "shell",
            provider.to_str().unwrap(),
            "--display-name",
            "Shell",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "create",
            "--from-file",
            fixture.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let edited_job = temp.path().join("manual-shell-edited.json");
    let mut edited_spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture).unwrap()).unwrap();
    edited_spec["task"]["prompt"] = serde_json::json!("Edited prompt");
    std::fs::write(
        &edited_job,
        serde_json::to_string_pretty(&edited_spec).unwrap(),
    )
    .unwrap();
    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "edit",
            "manual-shell",
            "--from-file",
            edited_job.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "show", "manual-shell"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Edited prompt"));

    let secret_job = temp.path().join("secret-webhook.json");
    let secret_spec = serde_json::json!({
        "schema_version": "scheduler.job.v1",
        "name": "secret-webhook",
        "enabled": true,
        "provider_id": "shell",
        "repo": {
            "path": temp.path().display().to_string(),
            "base_ref": "main",
            "fetch_before_run": false
        },
        "schedule": {
            "kind": "manual"
        },
        "task": {
            "prompt": "No-op",
            "success_criteria": []
        },
        "execution": {
            "isolation": "none",
            "concurrency": "skip",
            "repo_lock": "none",
            "timeout_seconds": 10,
            "approval_policy": "non_interactive",
            "branch_template": "scheduler/{job_slug}/{run_id}",
            "worktree_cleanup": {
                "on_success": "after_retention",
                "on_failure": "keep",
                "retention_days": 14
            }
        },
        "notifications": {
            "on_success": ["webhook:http://example.invalid/hook?token=secret-value"],
            "on_failure": [],
            "on_timeout": []
        }
    });
    std::fs::write(
        &secret_job,
        serde_json::to_string_pretty(&secret_spec).unwrap(),
    )
    .unwrap();
    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "create",
            "--from-file",
            secret_job.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "config",
            "export",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("scheduler.config.v1"), "{stdout}");
    assert!(stdout.contains("token=[REDACTED]"), "{stdout}");
    assert!(!stdout.contains("secret-value"), "{stdout}");

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "config",
            "export",
            "--format",
            "toml",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "import",
            fixture.to_str().unwrap(),
            "--on-conflict",
            "rename",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("manual-shell-2"));

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "import",
            fixture.to_str().unwrap(),
            "--on-conflict",
            "replace",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "run", "manual-shell"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "runs",
            "manual-shell",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("succeeded"), "{stdout}");
    let runs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let run_id = runs[0]["id"].as_str().unwrap();

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "logs", run_id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Print this prompt back"));

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "logs",
            run_id,
            "--follow",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "artifacts", run_id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("artifacts/report.md"));

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "db", "check"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "daemon", "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Active runs"));

    let output = scheduler()
        .args(["--config", config.to_str().unwrap(), "daemon", "tick"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Due actions"));

    let backup = temp.path().join("backup.sqlite3");
    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "backup",
            "create",
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(backup.exists());

    let full_backup = temp.path().join("full-backup");
    assert_command_ok(
        scheduler()
            .args([
                "--config",
                config.to_str().unwrap(),
                "backup",
                "create",
                full_backup.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
    );
    assert!(full_backup.join("scheduler.sqlite3").exists());
    assert!(full_backup.join("data").exists());

    let restored_config = temp.path().join("restored-config");
    assert_command_ok(
        scheduler()
            .args([
                "--config",
                restored_config.to_str().unwrap(),
                "backup",
                "restore",
                full_backup.to_str().unwrap(),
                "--yes",
            ])
            .output()
            .unwrap(),
    );
    let output = scheduler()
        .args(["--config", restored_config.to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert_command_ok(output);
    let output = scheduler()
        .args([
            "--config",
            restored_config.to_str().unwrap(),
            "logs",
            run_id,
        ])
        .output()
        .unwrap();
    assert_command_ok(output);
}

#[test]
fn create_can_use_provider_backed_spec_builder() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let builder = temp.path().join("builder");
    std::fs::write(
        &builder,
        r#"#!/bin/sh
cat <<'JSON'
{
  "status": "ok",
  "questions": [],
  "warnings": [],
  "summary": {
    "human": "Manual generated job",
    "schedule": "manual",
    "task": "Generated from provider"
  },
  "job_spec": {
    "schema_version": "scheduler.job.v1",
    "name": "generated-provider-job",
    "enabled": true,
    "provider_id": "builder",
    "repo": {
      "path": "/tmp",
      "base_ref": "main",
      "fetch_before_run": false
    },
    "schedule": {
      "kind": "manual"
    },
    "task": {
      "prompt": "Generated from provider",
      "success_criteria": ["Spec is stored"]
    },
    "execution": {
      "isolation": "none",
      "concurrency": "skip",
      "repo_lock": "none",
      "timeout_seconds": 10,
      "approval_policy": "non_interactive",
      "branch_template": "scheduler/{job_slug}/{run_id}",
      "worktree_cleanup": {
        "on_success": "after_retention",
        "on_failure": "keep",
        "retention_days": 14
      }
    }
  }
}
JSON
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&builder).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&builder, permissions).unwrap();
    }

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "provider",
            "add-custom",
            "builder",
            builder.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "create",
            "--repo",
            "/tmp",
            "--provider",
            "builder",
            "--task",
            "make a manual job",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Confirmation summary:"), "{stdout}");
    assert!(stdout.contains("Manual generated job"), "{stdout}");

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "show",
            "generated-provider-job",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Generated from provider"));
}

#[test]
fn create_repairs_invalid_spec_builder_output() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let builder = temp.path().join("repair-builder");
    let state = temp.path().join("builder-state");
    let script = r#"#!/bin/sh
STATE="__STATE__"
if [ ! -f "$STATE" ]; then
  touch "$STATE"
  printf '```json\n{"status":'
  exit 0
fi
cat <<'JSON'
{
  "status": "ok",
  "questions": [],
  "warnings": [],
  "summary": {
    "human": "Repaired generated job",
    "schedule": "manual",
    "task": "Generated after repair"
  },
  "job_spec": {
    "schema_version": "scheduler.job.v1",
    "name": "repaired-provider-job",
    "enabled": true,
    "provider_id": "repair-builder",
    "repo": {
      "path": "/tmp",
      "base_ref": "main",
      "fetch_before_run": false
    },
    "schedule": {
      "kind": "manual"
    },
    "task": {
      "prompt": "Generated after repair",
      "success_criteria": ["Spec is stored"]
    },
    "execution": {
      "isolation": "none",
      "concurrency": "skip",
      "repo_lock": "none",
      "timeout_seconds": 10,
      "approval_policy": "non_interactive",
      "branch_template": "scheduler/{job_slug}/{run_id}",
      "worktree_cleanup": {
        "on_success": "after_retention",
        "on_failure": "keep",
        "retention_days": 14
      }
    }
  }
}
JSON
"#
    .replace("__STATE__", state.to_str().unwrap());
    std::fs::write(&builder, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&builder).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&builder, permissions).unwrap();
    }

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "provider",
            "add-custom",
            "repair-builder",
            builder.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "create",
            "--repo",
            "/tmp",
            "--provider",
            "repair-builder",
            "--task",
            "make a manual job",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = scheduler()
        .args([
            "--config",
            config.to_str().unwrap(),
            "show",
            "repaired-provider-job",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Generated after repair"));
}

fn assert_command_ok(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for_run_status(config: &str, job: &str, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_stdout = String::new();
    while Instant::now() < deadline {
        let output = scheduler()
            .args(["--config", config, "--json", "runs", job])
            .output()
            .unwrap();
        if output.status.success() {
            last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let runs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            if let Some(run) = runs.as_array().and_then(|runs| runs.first())
                && run["status"].as_str() == Some(expected)
                && let Some(run_id) = run["id"].as_str()
            {
                return run_id.to_string();
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("run `{job}` did not reach status `{expected}`; last output: {last_stdout}");
}

fn wait_for_daemon_running(config: &str, expected: bool) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_stdout = String::new();
    while Instant::now() < deadline {
        let output = scheduler()
            .args(["--config", config, "--json", "daemon", "status"])
            .output()
            .unwrap();
        if output.status.success() {
            last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            if status["running"].as_bool() == Some(expected) {
                return status;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("daemon running state did not become `{expected}`; last output: {last_stdout}");
}

fn wait_for_child_exit(child: &mut std::process::Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "child exited with status {status}");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("child did not exit within {timeout:?}");
}
