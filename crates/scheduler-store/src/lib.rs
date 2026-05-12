use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use scheduler_core::{JobSpec, RunRecord, RunStatus, RunTrigger};
use scheduler_provider::{ProviderCapability, ProviderConfig, ProviderDetection};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArtifact {
    pub id: Uuid,
    pub run_id: Uuid,
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationDelivery {
    pub id: Uuid,
    pub run_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub event_type: String,
    pub channel: String,
    pub status: String,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProcess {
    pub run_id: Uuid,
    pub pid: u32,
    pub process_group_id: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunLogFile {
    pub id: Uuid,
    pub run_id: Uuid,
    pub path: String,
    pub stream: String,
    pub bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Setting {
    pub key: String,
    pub value: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProbeRecord {
    pub id: i64,
    pub provider_id: String,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub available: bool,
    pub capabilities: ProviderCapability,
    pub error: Option<String>,
    pub probed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleProjection {
    pub job_id: Uuid,
    pub schedule_json: serde_json::Value,
    pub last_due_at: Option<DateTime<Utc>>,
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_run_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEvent {
    pub id: i64,
    pub event_type: String,
    pub event_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredJob {
    pub id: Uuid,
    pub spec: JobSpec,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("invalid run transition from {from:?} to {to:?}")]
    InvalidRunTransition { from: RunStatus, to: RunStatus },
    #[error("uuid parse error: {0}")]
    Uuid(#[from] uuid::Error),
}

pub struct Store {
    connection: Connection,
}

const MIGRATION_2_STATE_PROJECTIONS: &str = r#"
CREATE TABLE IF NOT EXISTS provider_probes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id TEXT NOT NULL,
    binary_path TEXT,
    version TEXT,
    available INTEGER NOT NULL,
    capabilities_json TEXT NOT NULL,
    error TEXT,
    probed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schedules (
    job_id TEXT PRIMARY KEY,
    schedule_json TEXT NOT NULL,
    last_due_at TEXT,
    next_due_at TEXT,
    last_run_id TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(job_id) REFERENCES jobs(id)
);
"#;

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS providers (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                command TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                capabilities_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS provider_probes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_id TEXT NOT NULL,
                binary_path TEXT,
                version TEXT,
                available INTEGER NOT NULL,
                capabilities_json TEXT NOT NULL,
                error TEXT,
                probed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                provider_id TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                spec_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                deleted_at TEXT
            );

            CREATE UNIQUE INDEX IF NOT EXISTS jobs_name_active_idx
                ON jobs(name)
                WHERE deleted_at IS NULL;

            CREATE TABLE IF NOT EXISTS job_versions (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                spec_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(job_id) REFERENCES jobs(id)
            );

            CREATE TABLE IF NOT EXISTS schedules (
                job_id TEXT PRIMARY KEY,
                schedule_json TEXT NOT NULL,
                last_due_at TEXT,
                next_due_at TEXT,
                last_run_id TEXT,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(job_id) REFERENCES jobs(id)
            );

            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                status TEXT NOT NULL,
                trigger TEXT NOT NULL,
                due_at TEXT,
                started_at TEXT,
                finished_at TEXT,
                provider_id TEXT NOT NULL,
                worktree_path TEXT,
                branch TEXT,
                reason TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(job_id) REFERENCES jobs(id)
            );

            CREATE TABLE IF NOT EXISTS run_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                event_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES runs(id)
            );

            CREATE TABLE IF NOT EXISTS run_artifacts (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES runs(id)
            );

            CREATE TABLE IF NOT EXISTS run_logs (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                path TEXT NOT NULL,
                stream TEXT NOT NULL,
                bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES runs(id)
            );

            CREATE TABLE IF NOT EXISTS notification_deliveries (
                id TEXT PRIMARY KEY,
                run_id TEXT,
                job_id TEXT,
                event_type TEXT NOT NULL,
                channel TEXT NOT NULL,
                status TEXT NOT NULL,
                message TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES runs(id),
                FOREIGN KEY(job_id) REFERENCES jobs(id)
            );

            CREATE TABLE IF NOT EXISTS run_processes (
                run_id TEXT PRIMARY KEY,
                pid INTEGER NOT NULL,
                process_group_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES runs(id)
            );

            CREATE TABLE IF NOT EXISTS queues (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                due_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(job_id) REFERENCES jobs(id),
                FOREIGN KEY(run_id) REFERENCES runs(id)
            );

            CREATE TABLE IF NOT EXISTS locks (
                key TEXT PRIMARY KEY,
                owner TEXT NOT NULL,
                expires_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                event_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        self.record_schema_migration(1)?;
        self.apply_migration(2, MIGRATION_2_STATE_PROJECTIONS)?;
        self.insert_default_settings()?;
        Ok(())
    }

    fn apply_migration(&self, version: i64, sql: &str) -> Result<(), StoreError> {
        if self.schema_migration_applied(version)? {
            return Ok(());
        }
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            self.connection.execute_batch(sql)?;
            self.record_schema_migration(version)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn schema_migration_applied(&self, version: i64) -> Result<bool, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
            params![version],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn record_schema_migration(&self, version: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn insert_default_settings(&self) -> Result<(), StoreError> {
        let now = Utc::now().to_rfc3339();
        for (key, value) in [
            ("retention.logs_days", "90"),
            ("retention.successful_worktree_days", "14"),
            ("retention.failed_worktree_days", "30"),
        ] {
            self.connection.execute(
                "INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
                params![key, value, now],
            )?;
        }
        Ok(())
    }

    pub fn integrity_check(&self) -> Result<String, StoreError> {
        self.connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(StoreError::from)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![key, value, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<Setting>, StoreError> {
        self.connection
            .query_row(
                "SELECT key, value, updated_at FROM settings WHERE key = ?1",
                params![key],
                decode_setting_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_settings(&self) -> Result<Vec<Setting>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT key, value, updated_at FROM settings ORDER BY key ASC")?;
        let rows = statement.query_map([], decode_setting_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn record_provider_probe(&self, detection: &ProviderDetection) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO provider_probes
                (provider_id, binary_path, version, available, capabilities_json, error, probed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &detection.id,
                detection
                    .binary_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                detection.version.as_deref(),
                bool_to_i64(detection.available),
                serde_json::to_string(&detection.capabilities)?,
                detection.error.as_deref(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn list_provider_probes(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ProviderProbeRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, provider_id, binary_path, version, available, capabilities_json, error, probed_at
             FROM provider_probes
             WHERE provider_id = ?1
             ORDER BY probed_at DESC, id DESC",
        )?;
        let rows = statement.query_map(params![provider_id], decode_provider_probe_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn upsert_schedule_projection(
        &self,
        job_id: Uuid,
        spec: &JobSpec,
        last_due_at: Option<DateTime<Utc>>,
        next_due_at: Option<DateTime<Utc>>,
        last_run_id: Option<Uuid>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO schedules
                (job_id, schedule_json, last_due_at, next_due_at, last_run_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(job_id) DO UPDATE SET
                schedule_json = excluded.schedule_json,
                last_due_at = COALESCE(excluded.last_due_at, schedules.last_due_at),
                next_due_at = excluded.next_due_at,
                last_run_id = COALESCE(excluded.last_run_id, schedules.last_run_id),
                updated_at = excluded.updated_at",
            params![
                job_id.to_string(),
                serde_json::to_string(&spec.schedule)?,
                last_due_at.map(|value| value.to_rfc3339()),
                next_due_at.map(|value| value.to_rfc3339()),
                last_run_id.map(|value| value.to_string()),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_schedule_projection(
        &self,
        job_id: Uuid,
    ) -> Result<Option<ScheduleProjection>, StoreError> {
        self.connection
            .query_row(
                "SELECT job_id, schedule_json, last_due_at, next_due_at, last_run_id, updated_at
                 FROM schedules
                 WHERE job_id = ?1",
                params![job_id.to_string()],
                decode_schedule_projection_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn create_job(&mut self, spec: &JobSpec) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let spec_json = serde_json::to_string(spec)?;
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO jobs (id, name, enabled, provider_id, repo_path, spec_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id.to_string(),
                spec.name,
                bool_to_i64(spec.enabled),
                spec.provider_id,
                spec.repo.path,
                spec_json,
                now.to_rfc3339(),
            ],
        )?;
        insert_job_version(&tx, id, spec, now)?;
        tx.commit()?;
        self.upsert_schedule_projection(id, spec, None, None, None)?;
        self.record_audit_event(
            "job.created",
            &serde_json::json!({
                "job_id": id,
                "name": spec.name,
                "provider_id": spec.provider_id,
            }),
        )?;
        Ok(id)
    }

    pub fn list_jobs(&self) -> Result<Vec<(Uuid, JobSpec)>, StoreError> {
        self.list_jobs_with_metadata().map(|jobs| {
            jobs.into_iter()
                .map(|job| (job.id, job.spec))
                .collect::<Vec<_>>()
        })
    }

    pub fn list_jobs_with_metadata(&self) -> Result<Vec<StoredJob>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, spec_json, created_at, updated_at
             FROM jobs
             WHERE deleted_at IS NULL
             ORDER BY name ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let spec_json: String = row.get(1)?;
            let created_at: String = row.get(2)?;
            let updated_at: String = row.get(3)?;
            Ok(StoredJob {
                id: Uuid::parse_str(&id).map_err(to_sql_error)?,
                spec: serde_json::from_str(&spec_json).map_err(to_sql_error)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(to_sql_error)?,
                updated_at: DateTime::parse_from_rfc3339(&updated_at)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(to_sql_error)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn get_job_by_name(&self, name: &str) -> Result<Option<(Uuid, JobSpec)>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, spec_json FROM jobs WHERE name = ?1 AND deleted_at IS NULL",
                params![name],
                |row| {
                    let id: String = row.get(0)?;
                    let spec_json: String = row.get(1)?;
                    Ok((id, spec_json))
                },
            )
            .optional()?
            .map(|(id, spec_json)| Ok((Uuid::parse_str(&id)?, serde_json::from_str(&spec_json)?)))
            .transpose()
    }

    pub fn set_job_enabled(&mut self, name: &str, enabled: bool) -> Result<bool, StoreError> {
        let Some((id, mut spec)) = self.get_job_by_name(name)? else {
            return Ok(false);
        };
        spec.enabled = enabled;
        let spec_json = serde_json::to_string(&spec)?;
        let now = Utc::now();
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE jobs SET enabled = ?1, spec_json = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                bool_to_i64(enabled),
                spec_json,
                now.to_rfc3339(),
                id.to_string()
            ],
        )?;
        insert_job_version(&tx, id, &spec, now)?;
        tx.commit()?;
        self.upsert_schedule_projection(id, &spec, None, None, None)?;
        self.record_audit_event(
            if enabled {
                "job.enabled"
            } else {
                "job.disabled"
            },
            &serde_json::json!({
                "job_id": id,
                "name": name,
            }),
        )?;
        Ok(true)
    }

    pub fn update_job(&mut self, current_name: &str, spec: &JobSpec) -> Result<bool, StoreError> {
        let Some((id, _current_spec)) = self.get_job_by_name(current_name)? else {
            return Ok(false);
        };
        let spec_json = serde_json::to_string(spec)?;
        let now = Utc::now();
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE jobs
             SET name = ?1, enabled = ?2, provider_id = ?3, repo_path = ?4, spec_json = ?5, updated_at = ?6
             WHERE id = ?7 AND deleted_at IS NULL",
            params![
                spec.name,
                bool_to_i64(spec.enabled),
                spec.provider_id,
                spec.repo.path,
                spec_json,
                now.to_rfc3339(),
                id.to_string(),
            ],
        )?;
        insert_job_version(&tx, id, spec, now)?;
        tx.commit()?;
        self.upsert_schedule_projection(id, spec, None, None, None)?;
        self.record_audit_event(
            "job.updated",
            &serde_json::json!({
                "job_id": id,
                "previous_name": current_name,
                "name": spec.name,
                "provider_id": spec.provider_id,
            }),
        )?;
        Ok(true)
    }

    pub fn delete_job(&mut self, name: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE jobs SET deleted_at = ?1, enabled = 0, updated_at = ?1 WHERE name = ?2 AND deleted_at IS NULL",
            params![Utc::now().to_rfc3339(), name],
        )?;
        if changed > 0 {
            self.connection.execute(
                "DELETE FROM schedules
                     WHERE job_id IN (SELECT id FROM jobs WHERE name = ?1)",
                params![name],
            )?;
            self.record_audit_event(
                "job.deleted",
                &serde_json::json!({
                    "name": name,
                }),
            )?;
        }
        Ok(changed > 0)
    }

    pub fn upsert_provider(&mut self, provider: &ProviderConfig) -> Result<(), StoreError> {
        let now = Utc::now().to_rfc3339();
        self.connection.execute(
            "INSERT INTO providers (id, display_name, command, enabled, capabilities_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                display_name = excluded.display_name,
                command = excluded.command,
                capabilities_json = excluded.capabilities_json,
                updated_at = excluded.updated_at",
            params![
                provider.id,
                provider.display_name,
                provider.command.display().to_string(),
                bool_to_i64(provider.enabled),
                serde_json::to_string(&provider.capabilities)?,
                now,
            ],
        )?;
        self.record_audit_event(
            "provider.upserted",
            &serde_json::json!({
                "provider_id": provider.id,
                "enabled": provider.enabled,
            }),
        )?;
        Ok(())
    }

    pub fn set_provider_enabled(
        &mut self,
        provider_id: &str,
        enabled: bool,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE providers SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![bool_to_i64(enabled), Utc::now().to_rfc3339(), provider_id],
        )?;
        self.record_audit_event(
            if enabled {
                "provider.enabled"
            } else {
                "provider.disabled"
            },
            &serde_json::json!({
                "provider_id": provider_id,
            }),
        )?;
        Ok(())
    }

    pub fn list_providers(&self) -> Result<Vec<ProviderConfig>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, display_name, command, enabled, capabilities_json FROM providers ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let capabilities_json: String = row.get(4)?;
            let capabilities: ProviderCapability =
                serde_json::from_str(&capabilities_json).map_err(to_sql_error)?;
            Ok(ProviderConfig {
                id: row.get(0)?,
                display_name: row.get(1)?,
                command: std::path::PathBuf::from(row.get::<_, String>(2)?),
                enabled: row.get::<_, i64>(3)? != 0,
                capabilities,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn get_provider(&self, provider_id: &str) -> Result<Option<ProviderConfig>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, display_name, command, enabled, capabilities_json FROM providers WHERE id = ?1",
                params![provider_id],
                |row| {
                    let capabilities_json: String = row.get(4)?;
                    let capabilities: ProviderCapability =
                        serde_json::from_str(&capabilities_json).map_err(to_sql_error)?;
                    Ok(ProviderConfig {
                        id: row.get(0)?,
                        display_name: row.get(1)?,
                        command: std::path::PathBuf::from(row.get::<_, String>(2)?),
                        enabled: row.get::<_, i64>(3)? != 0,
                        capabilities,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn create_run(
        &mut self,
        job_id: Uuid,
        spec: &JobSpec,
        trigger: RunTrigger,
        due_at: Option<DateTime<Utc>>,
    ) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        self.connection.execute(
            "INSERT INTO runs (id, job_id, status, trigger, due_at, provider_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id.to_string(),
                job_id.to_string(),
                encode_status(RunStatus::Scheduled),
                encode_trigger(trigger),
                due_at.map(|value| value.to_rfc3339()),
                spec.provider_id,
                now.to_rfc3339(),
            ],
        )?;
        if let Some(due_at) = due_at {
            self.connection.execute(
                "UPDATE schedules SET last_due_at = ?1, last_run_id = ?2, updated_at = ?3 WHERE job_id = ?4",
                params![
                    due_at.to_rfc3339(),
                    id.to_string(),
                    Utc::now().to_rfc3339(),
                    job_id.to_string(),
                ],
            )?;
        }
        self.record_audit_event(
            "run.created",
            &serde_json::json!({
                "run_id": id,
                "job_id": job_id,
                "trigger": encode_trigger(trigger),
            }),
        )?;
        Ok(id)
    }

    pub fn transition_run(
        &mut self,
        run_id: Uuid,
        next: RunStatus,
        reason: Option<&str>,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let current = get_run_status(&tx, run_id)?;
        if !current.can_transition_to(next) {
            return Err(StoreError::InvalidRunTransition {
                from: current,
                to: next,
            });
        }
        let now = Utc::now();
        tx.execute(
            "UPDATE runs SET status = ?1, reason = COALESCE(?2, reason), updated_at = ?3,
                started_at = CASE WHEN ?1 = 'running' AND started_at IS NULL THEN ?3 ELSE started_at END,
                finished_at = CASE WHEN ?4 = 1 THEN ?3 ELSE finished_at END
             WHERE id = ?5",
            params![
                encode_status(next),
                reason,
                now.to_rfc3339(),
                bool_to_i64(next.is_terminal()),
                run_id.to_string(),
            ],
        )?;
        tx.commit()?;
        self.record_audit_event(
            "run.transitioned",
            &serde_json::json!({
                "run_id": run_id,
                "status": encode_status(next),
                "reason": reason,
            }),
        )?;
        Ok(())
    }

    pub fn set_run_workspace(
        &mut self,
        run_id: Uuid,
        worktree_path: Option<&Path>,
        branch: Option<&str>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE runs SET worktree_path = ?1, branch = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                worktree_path.map(|path| path.display().to_string()),
                branch,
                Utc::now().to_rfc3339(),
                run_id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn set_run_process(
        &self,
        run_id: Uuid,
        pid: u32,
        process_group_id: u32,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO run_processes (run_id, pid, process_group_id, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(run_id) DO UPDATE SET
                pid = excluded.pid,
                process_group_id = excluded.process_group_id,
                created_at = excluded.created_at",
            params![
                run_id.to_string(),
                i64::from(pid),
                i64::from(process_group_id),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_run_process(&self, run_id: Uuid) -> Result<Option<RunProcess>, StoreError> {
        self.connection
            .query_row(
                "SELECT run_id, pid, process_group_id, created_at
                 FROM run_processes
                 WHERE run_id = ?1",
                params![run_id.to_string()],
                decode_run_process_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn clear_run_process(&self, run_id: Uuid) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "DELETE FROM run_processes WHERE run_id = ?1",
            params![run_id.to_string()],
        )?;
        Ok(changed > 0)
    }

    pub fn get_run(&self, run_id: Uuid) -> Result<Option<RunRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, job_id, status, trigger, due_at, started_at, finished_at, provider_id,
                        worktree_path, branch, reason
                 FROM runs WHERE id = ?1",
                params![run_id.to_string()],
                decode_run_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_runs_for_job(&self, job_id: Uuid) -> Result<Vec<RunRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, status, trigger, due_at, started_at, finished_at, provider_id,
                    worktree_path, branch, reason
             FROM runs WHERE job_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map(params![job_id.to_string()], decode_run_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_active_runs_for_job(&self, job_id: Uuid) -> Result<Vec<RunRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, status, trigger, due_at, started_at, finished_at, provider_id,
                    worktree_path, branch, reason
             FROM runs
             WHERE job_id = ?1 AND status IN ('preparing', 'running', 'cancelling')
             ORDER BY created_at ASC",
        )?;
        let rows = statement.query_map(params![job_id.to_string()], decode_run_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_active_runs(&self) -> Result<Vec<RunRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, status, trigger, due_at, started_at, finished_at, provider_id,
                    worktree_path, branch, reason
             FROM runs
             WHERE status IN ('preparing', 'running', 'cancelling')
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = statement.query_map([], decode_run_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_active_runs_for_repo(&self, repo_path: &str) -> Result<Vec<RunRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT runs.id, runs.job_id, runs.status, runs.trigger, runs.due_at, runs.started_at,
                    runs.finished_at, runs.provider_id, runs.worktree_path, runs.branch, runs.reason
             FROM runs
             JOIN jobs ON jobs.id = runs.job_id
             WHERE jobs.repo_path = ?1
                AND jobs.deleted_at IS NULL
                AND runs.status IN ('preparing', 'running', 'cancelling')
             ORDER BY runs.created_at ASC",
        )?;
        let rows = statement.query_map(params![repo_path], decode_run_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn last_due_at_for_job(&self, job_id: Uuid) -> Result<Option<DateTime<Utc>>, StoreError> {
        let raw: Option<String> = self.connection.query_row(
            "SELECT MAX(due_at) FROM runs WHERE job_id = ?1 AND due_at IS NOT NULL",
            params![job_id.to_string()],
            |row| row.get(0),
        )?;
        raw.map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(to_sql_error)
        })
        .transpose()
        .map_err(StoreError::from)
    }

    pub fn list_queued_runs_for_job(&self, job_id: Uuid) -> Result<Vec<RunRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, status, trigger, due_at, started_at, finished_at, provider_id,
                    worktree_path, branch, reason
             FROM runs
             WHERE job_id = ?1 AND status = 'queued'
             ORDER BY due_at ASC, created_at ASC",
        )?;
        let rows = statement.query_map(params![job_id.to_string()], decode_run_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn add_run_artifact(
        &mut self,
        run_id: Uuid,
        path: &str,
        kind: &str,
    ) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        self.connection.execute(
            "INSERT INTO run_artifacts (id, run_id, path, kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id.to_string(),
                run_id.to_string(),
                path,
                kind,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(id)
    }

    pub fn list_run_artifacts(&self, run_id: Uuid) -> Result<Vec<RunArtifact>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, run_id, path, kind FROM run_artifacts WHERE run_id = ?1 ORDER BY path ASC",
        )?;
        let rows = statement.query_map(params![run_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let run_id: String = row.get(1)?;
            Ok(RunArtifact {
                id: Uuid::parse_str(&id).map_err(to_sql_error)?,
                run_id: Uuid::parse_str(&run_id).map_err(to_sql_error)?,
                path: row.get(2)?,
                kind: row.get(3)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn add_run_log_file(
        &self,
        run_id: Uuid,
        path: &str,
        stream: &str,
        bytes: u64,
    ) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        self.connection.execute(
            "INSERT INTO run_logs (id, run_id, path, stream, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id.to_string(),
                run_id.to_string(),
                path,
                stream,
                i64::try_from(bytes).unwrap_or(i64::MAX),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(id)
    }

    pub fn list_run_log_files(&self, run_id: Uuid) -> Result<Vec<RunLogFile>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, run_id, path, stream, bytes, created_at
             FROM run_logs
             WHERE run_id = ?1
             ORDER BY created_at ASC, stream ASC",
        )?;
        let rows = statement.query_map(params![run_id.to_string()], decode_run_log_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn rewrite_run_log_path_prefix(
        &self,
        old_prefix: &Path,
        new_prefix: &Path,
    ) -> Result<usize, StoreError> {
        let mut statement = self.connection.prepare("SELECT id, path FROM run_logs")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut updates = Vec::new();
        for row in rows {
            let (id, path) = row?;
            let path = PathBuf::from(path);
            if let Ok(suffix) = path.strip_prefix(old_prefix) {
                updates.push((id, new_prefix.join(suffix).display().to_string()));
            }
        }

        for (id, path) in &updates {
            self.connection.execute(
                "UPDATE run_logs SET path = ?1 WHERE id = ?2",
                params![path, id],
            )?;
        }
        Ok(updates.len())
    }

    pub fn record_notification_delivery(
        &self,
        run_id: Option<Uuid>,
        job_id: Option<Uuid>,
        event_type: &str,
        channel: &str,
        status: &str,
        message: Option<&str>,
    ) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        self.connection.execute(
            "INSERT INTO notification_deliveries
                (id, run_id, job_id, event_type, channel, status, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.to_string(),
                run_id.map(|value| value.to_string()),
                job_id.map(|value| value.to_string()),
                event_type,
                channel,
                status,
                message,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(id)
    }

    pub fn list_notification_deliveries_for_run(
        &self,
        run_id: Uuid,
    ) -> Result<Vec<NotificationDelivery>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, run_id, job_id, event_type, channel, status, message, created_at
             FROM notification_deliveries
             WHERE run_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = statement.query_map(params![run_id.to_string()], decode_notification_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn acquire_lock(
        &mut self,
        key: &str,
        owner: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<bool, StoreError> {
        let now = Utc::now();
        let tx = self.connection.transaction()?;
        let existing: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT owner, expires_at FROM locks WHERE key = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((existing_owner, raw_expires_at)) = existing {
            if existing_owner == owner {
                tx.execute(
                    "UPDATE locks SET expires_at = ?1 WHERE key = ?2 AND owner = ?3",
                    params![expires_at.map(|value| value.to_rfc3339()), key, owner,],
                )?;
                tx.commit()?;
                return Ok(true);
            }
            let expired = raw_expires_at
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .map(|value| value.with_timezone(&Utc) <= now)
                })
                .transpose()
                .map_err(to_sql_error)?
                .unwrap_or(false);
            if !expired {
                return Ok(false);
            }
            tx.execute("DELETE FROM locks WHERE key = ?1", params![key])?;
        }
        tx.execute(
            "INSERT INTO locks (key, owner, expires_at, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                key,
                owner,
                expires_at.map(|value| value.to_rfc3339()),
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn release_lock(&mut self, key: &str, owner: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "DELETE FROM locks WHERE key = ?1 AND owner = ?2",
            params![key, owner],
        )?;
        Ok(changed > 0)
    }

    pub fn record_audit_event(
        &self,
        event_type: &str,
        event_json: &serde_json::Value,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO audit_events (event_type, event_json, created_at) VALUES (?1, ?2, ?3)",
            params![
                event_type,
                serde_json::to_string(event_json)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_audit_events(&self) -> Result<Vec<AuditEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, event_type, event_json, created_at FROM audit_events ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let event_json: String = row.get(2)?;
            let created_at: String = row.get(3)?;
            Ok(AuditEvent {
                id: row.get(0)?,
                event_type: row.get(1)?,
                event_json: serde_json::from_str(&event_json).map_err(to_sql_error)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(to_sql_error)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}

fn insert_job_version(
    tx: &Transaction<'_>,
    job_id: Uuid,
    spec: &JobSpec,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO job_versions (id, job_id, spec_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            Uuid::new_v4().to_string(),
            job_id.to_string(),
            serde_json::to_string(spec)?,
            now.to_rfc3339()
        ],
    )?;
    Ok(())
}

fn get_run_status(tx: &Transaction<'_>, run_id: Uuid) -> Result<RunStatus, StoreError> {
    let raw: String = tx.query_row(
        "SELECT status FROM runs WHERE id = ?1",
        params![run_id.to_string()],
        |row| row.get(0),
    )?;
    decode_status(&raw)
}

fn decode_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    let id: String = row.get(0)?;
    let job_id: String = row.get(1)?;
    let status: String = row.get(2)?;
    let trigger: String = row.get(3)?;
    let due_at: Option<String> = row.get(4)?;
    let started_at: Option<String> = row.get(5)?;
    let finished_at: Option<String> = row.get(6)?;
    Ok(RunRecord {
        id: Uuid::parse_str(&id).map_err(to_sql_error)?,
        job_id: Uuid::parse_str(&job_id).map_err(to_sql_error)?,
        status: decode_status(&status).map_err(to_sql_error)?,
        trigger: decode_trigger(&trigger).map_err(to_sql_error)?,
        due_at: parse_optional_datetime(due_at).map_err(to_sql_error)?,
        started_at: parse_optional_datetime(started_at).map_err(to_sql_error)?,
        finished_at: parse_optional_datetime(finished_at).map_err(to_sql_error)?,
        provider_id: row.get(7)?,
        worktree_path: row.get(8)?,
        branch: row.get(9)?,
        reason: row.get(10)?,
    })
}

fn decode_provider_probe_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProbeRecord> {
    let capabilities_json: String = row.get(5)?;
    let probed_at: String = row.get(7)?;
    Ok(ProviderProbeRecord {
        id: row.get(0)?,
        provider_id: row.get(1)?,
        binary_path: row.get(2)?,
        version: row.get(3)?,
        available: row.get::<_, i64>(4)? != 0,
        capabilities: serde_json::from_str(&capabilities_json).map_err(to_sql_error)?,
        error: row.get(6)?,
        probed_at: DateTime::parse_from_rfc3339(&probed_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(to_sql_error)?,
    })
}

fn decode_schedule_projection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleProjection> {
    let job_id: String = row.get(0)?;
    let schedule_json: String = row.get(1)?;
    let last_run_id: Option<String> = row.get(4)?;
    let updated_at: String = row.get(5)?;
    Ok(ScheduleProjection {
        job_id: Uuid::parse_str(&job_id).map_err(to_sql_error)?,
        schedule_json: serde_json::from_str(&schedule_json).map_err(to_sql_error)?,
        last_due_at: parse_optional_datetime(row.get(2)?).map_err(to_sql_error)?,
        next_due_at: parse_optional_datetime(row.get(3)?).map_err(to_sql_error)?,
        last_run_id: last_run_id
            .map(|value| Uuid::parse_str(&value))
            .transpose()
            .map_err(to_sql_error)?,
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(to_sql_error)?,
    })
}

fn decode_notification_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationDelivery> {
    let id: String = row.get(0)?;
    let run_id: Option<String> = row.get(1)?;
    let job_id: Option<String> = row.get(2)?;
    let created_at: String = row.get(7)?;
    Ok(NotificationDelivery {
        id: Uuid::parse_str(&id).map_err(to_sql_error)?,
        run_id: run_id
            .map(|value| Uuid::parse_str(&value))
            .transpose()
            .map_err(to_sql_error)?,
        job_id: job_id
            .map(|value| Uuid::parse_str(&value))
            .transpose()
            .map_err(to_sql_error)?,
        event_type: row.get(3)?,
        channel: row.get(4)?,
        status: row.get(5)?,
        message: row.get(6)?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(to_sql_error)?,
    })
}

fn decode_run_process_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunProcess> {
    let run_id: String = row.get(0)?;
    let pid: i64 = row.get(1)?;
    let process_group_id: i64 = row.get(2)?;
    let created_at: String = row.get(3)?;
    Ok(RunProcess {
        run_id: Uuid::parse_str(&run_id).map_err(to_sql_error)?,
        pid: u32::try_from(pid).map_err(to_sql_error)?,
        process_group_id: u32::try_from(process_group_id).map_err(to_sql_error)?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(to_sql_error)?,
    })
}

fn decode_run_log_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunLogFile> {
    let id: String = row.get(0)?;
    let run_id: String = row.get(1)?;
    let bytes: i64 = row.get(4)?;
    let created_at: String = row.get(5)?;
    Ok(RunLogFile {
        id: Uuid::parse_str(&id).map_err(to_sql_error)?,
        run_id: Uuid::parse_str(&run_id).map_err(to_sql_error)?,
        path: row.get(2)?,
        stream: row.get(3)?,
        bytes: u64::try_from(bytes).map_err(to_sql_error)?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(to_sql_error)?,
    })
}

fn decode_setting_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Setting> {
    let updated_at: String = row.get(2)?;
    Ok(Setting {
        key: row.get(0)?,
        value: row.get(1)?,
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(to_sql_error)?,
    })
}

fn parse_optional_datetime(
    value: Option<String>,
) -> Result<Option<DateTime<Utc>>, chrono::ParseError> {
    value
        .map(|value| DateTime::parse_from_rfc3339(&value).map(|value| value.with_timezone(&Utc)))
        .transpose()
}

fn encode_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Scheduled => "scheduled",
        RunStatus::Queued => "queued",
        RunStatus::Skipped => "skipped",
        RunStatus::Preparing => "preparing",
        RunStatus::Running => "running",
        RunStatus::Cancelling => "cancelling",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::TimedOut => "timed_out",
        RunStatus::Blocked => "blocked",
        RunStatus::Lost => "lost",
    }
}

fn decode_status(value: &str) -> Result<RunStatus, StoreError> {
    Ok(match value {
        "scheduled" => RunStatus::Scheduled,
        "queued" => RunStatus::Queued,
        "skipped" => RunStatus::Skipped,
        "preparing" => RunStatus::Preparing,
        "running" => RunStatus::Running,
        "cancelling" => RunStatus::Cancelling,
        "cancelled" => RunStatus::Cancelled,
        "succeeded" => RunStatus::Succeeded,
        "failed" => RunStatus::Failed,
        "timed_out" => RunStatus::TimedOut,
        "blocked" => RunStatus::Blocked,
        "lost" => RunStatus::Lost,
        _ => {
            return Err(StoreError::Sqlite(rusqlite::Error::InvalidColumnType(
                0,
                "status".to_string(),
                rusqlite::types::Type::Text,
            )));
        }
    })
}

fn encode_trigger(trigger: RunTrigger) -> &'static str {
    match trigger {
        RunTrigger::Scheduled => "scheduled",
        RunTrigger::Manual => "manual",
        RunTrigger::Retry => "retry",
        RunTrigger::Backfill => "backfill",
    }
}

fn decode_trigger(value: &str) -> Result<RunTrigger, StoreError> {
    Ok(match value {
        "scheduled" => RunTrigger::Scheduled,
        "manual" => RunTrigger::Manual,
        "retry" => RunTrigger::Retry,
        "backfill" => RunTrigger::Backfill,
        _ => {
            return Err(StoreError::Sqlite(rusqlite::Error::InvalidColumnType(
                0,
                "trigger".to_string(),
                rusqlite::types::Type::Text,
            )));
        }
    })
}

fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn to_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use scheduler_core::schedule::{MisfirePolicy, ScheduleSpec};
    use scheduler_core::{ExecutionSpec, RepoSpec, TaskSpec};

    use super::*;

    fn spec() -> JobSpec {
        JobSpec {
            schema_version: "scheduler.job.v1".to_string(),
            name: "daily-report".to_string(),
            enabled: true,
            provider_id: "codex".to_string(),
            repo: RepoSpec {
                path: "/tmp/repo".to_string(),
                base_ref: "main".to_string(),
                fetch_before_run: true,
            },
            schedule: ScheduleSpec::Cron {
                expression: "0 8 * * *".to_string(),
                timezone: "Africa/Johannesburg".to_string(),
                misfire_policy: MisfirePolicy::RunOnce,
            },
            task: TaskSpec {
                prompt: "Create report".to_string(),
                success_criteria: vec![],
            },
            execution: ExecutionSpec::default(),
            delivery: Default::default(),
            notifications: Default::default(),
            metadata: Default::default(),
        }
    }

    fn table_exists(store: &Store, table: &str) -> bool {
        let count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .unwrap();
        count == 1
    }

    #[test]
    fn creates_and_lists_jobs() {
        let mut store = Store::in_memory().unwrap();
        let id = store.create_job(&spec()).unwrap();
        let jobs = store.list_jobs().unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, id);
        assert_eq!(jobs[0].1.name, "daily-report");
        let jobs = store.list_jobs_with_metadata().unwrap();
        assert_eq!(jobs[0].id, id);
        assert!(jobs[0].created_at <= Utc::now());
    }

    #[test]
    fn migration_records_schema_version_and_integrity_check_passes() {
        let store = Store::in_memory().unwrap();

        assert_eq!(store.integrity_check().unwrap(), "ok");
        assert!(store.schema_migration_applied(1).unwrap());
        assert!(store.schema_migration_applied(2).unwrap());
    }

    #[test]
    fn migration_applies_additive_versions_to_existing_database() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL
                );
                INSERT INTO schema_migrations (version, applied_at)
                VALUES (1, '2026-05-12T00:00:00Z');
                "#,
            )
            .unwrap();
        let store = Store { connection };

        store.migrate().unwrap();

        assert!(store.schema_migration_applied(2).unwrap());
        assert!(table_exists(&store, "provider_probes"));
        assert!(table_exists(&store, "schedules"));
    }

    #[test]
    fn schema_contains_required_state_tables() {
        let store = Store::in_memory().unwrap();
        for table in [
            "settings",
            "providers",
            "provider_probes",
            "jobs",
            "job_versions",
            "schedules",
            "runs",
            "run_events",
            "run_artifacts",
            "run_logs",
            "notification_deliveries",
            "run_processes",
            "queues",
            "locks",
            "audit_events",
        ] {
            assert!(table_exists(&store, table), "{table}");
        }
    }

    #[test]
    fn settings_have_defaults_and_can_be_updated() {
        let store = Store::in_memory().unwrap();

        assert_eq!(
            store
                .get_setting("retention.logs_days")
                .unwrap()
                .unwrap()
                .value,
            "90"
        );
        store.set_setting("retention.logs_days", "30").unwrap();
        assert_eq!(
            store
                .get_setting("retention.logs_days")
                .unwrap()
                .unwrap()
                .value,
            "30"
        );
        assert!(
            store
                .list_settings()
                .unwrap()
                .iter()
                .any(|setting| setting.key == "retention.failed_worktree_days")
        );
    }

    #[test]
    fn provider_probes_can_be_recorded() {
        let store = Store::in_memory().unwrap();
        let probe_id = store
            .record_provider_probe(&ProviderDetection {
                id: "codex".to_string(),
                display_name: "Codex".to_string(),
                binary_path: Some("/usr/local/bin/codex".into()),
                version: Some("codex 1.2.3".to_string()),
                available: true,
                capabilities: ProviderCapability {
                    supports_non_interactive: true,
                    ..ProviderCapability::default()
                },
                error: None,
            })
            .unwrap();

        let probes = store.list_provider_probes("codex").unwrap();

        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].id, probe_id);
        assert_eq!(probes[0].provider_id, "codex");
        assert!(probes[0].available);
        assert!(probes[0].capabilities.supports_non_interactive);
    }

    #[test]
    fn schedule_projection_tracks_job_and_due_runs() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let projection = store.get_schedule_projection(job_id).unwrap().unwrap();

        assert_eq!(projection.job_id, job_id);
        assert_eq!(projection.schedule_json["kind"], "cron");
        assert!(projection.last_due_at.is_none());
        assert!(projection.last_run_id.is_none());

        let due_at = Utc::now();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Scheduled, Some(due_at))
            .unwrap();
        let projection = store.get_schedule_projection(job_id).unwrap().unwrap();

        assert_eq!(projection.last_due_at, Some(due_at));
        assert_eq!(projection.last_run_id, Some(run_id));
    }

    #[test]
    fn run_transitions_are_transactional_and_validated() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();

        store
            .transition_run(run_id, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Running, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Succeeded, Some("done"))
            .unwrap();

        let run = store.get_run(run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Succeeded);
        assert_eq!(run.reason.as_deref(), Some("done"));
        assert!(matches!(
            store.transition_run(run_id, RunStatus::Running, None),
            Err(StoreError::InvalidRunTransition { .. })
        ));
    }

    #[test]
    fn providers_can_be_persisted_and_enabled() {
        let mut store = Store::in_memory().unwrap();
        store
            .upsert_provider(&ProviderConfig {
                id: "codex".to_string(),
                display_name: "Codex".to_string(),
                command: "/usr/local/bin/codex".into(),
                enabled: false,
                capabilities: ProviderCapability::default(),
            })
            .unwrap();
        store.set_provider_enabled("codex", true).unwrap();

        let providers = store.list_providers().unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "codex");
        assert!(providers[0].enabled);
    }

    #[test]
    fn jobs_can_be_enabled_disabled_and_deleted() {
        let mut store = Store::in_memory().unwrap();
        store.create_job(&spec()).unwrap();

        assert!(store.set_job_enabled("daily-report", false).unwrap());
        let (_, disabled) = store.get_job_by_name("daily-report").unwrap().unwrap();
        assert!(!disabled.enabled);

        assert!(store.delete_job("daily-report").unwrap());
        assert!(store.get_job_by_name("daily-report").unwrap().is_none());
    }

    #[test]
    fn jobs_can_be_updated_with_version_history() {
        let mut store = Store::in_memory().unwrap();
        store.create_job(&spec()).unwrap();
        let mut edited = spec();
        edited.name = "edited-report".to_string();
        edited.task.prompt = "Create edited report".to_string();

        assert!(store.update_job("daily-report", &edited).unwrap());

        assert!(store.get_job_by_name("daily-report").unwrap().is_none());
        let (_id, stored) = store.get_job_by_name("edited-report").unwrap().unwrap();
        assert_eq!(stored.task.prompt, "Create edited report");
        let audit = store.list_audit_events().unwrap();
        assert!(audit.iter().any(|event| event.event_type == "job.updated"));
    }

    #[test]
    fn lists_runs_for_job() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();

        let runs = store.list_runs_for_job(job_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, run_id);
    }

    #[test]
    fn lists_only_active_runs_for_job() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let active_run = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active_run, RunStatus::Preparing, None)
            .unwrap();
        let done_run = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(done_run, RunStatus::Skipped, Some("test"))
            .unwrap();

        let active = store.list_active_runs_for_job(job_id).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, active_run);
    }

    #[test]
    fn lists_all_active_runs() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let active_run = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active_run, RunStatus::Preparing, None)
            .unwrap();
        store
            .transition_run(active_run, RunStatus::Running, None)
            .unwrap();
        let done_run = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(done_run, RunStatus::Skipped, None)
            .unwrap();

        let active = store.list_active_runs().unwrap();

        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, active_run);
    }

    #[test]
    fn lists_active_runs_for_repo() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let active_run = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(active_run, RunStatus::Preparing, None)
            .unwrap();

        let active = store.list_active_runs_for_repo("/tmp/repo").unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, active_run);
    }

    #[test]
    fn tracks_last_due_and_queued_runs_for_job() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let due_at = Utc::now();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Scheduled, Some(due_at))
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Queued, Some("test"))
            .unwrap();

        assert_eq!(store.last_due_at_for_job(job_id).unwrap(), Some(due_at));
        let queued = store.list_queued_runs_for_job(job_id).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, run_id);
    }

    #[test]
    fn records_run_artifacts() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();

        let artifact_id = store
            .add_run_artifact(run_id, "artifacts/report.md", "report")
            .unwrap();
        let artifacts = store.list_run_artifacts(run_id).unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id, artifact_id);
        assert_eq!(artifacts[0].path, "artifacts/report.md");
        assert_eq!(artifacts[0].kind, "report");
    }

    #[test]
    fn records_run_log_files() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();

        let log_id = store
            .add_run_log_file(run_id, "runs/1/provider_stdout.log", "stdout", 42)
            .unwrap();
        let logs = store.list_run_log_files(run_id).unwrap();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, log_id);
        assert_eq!(logs[0].path, "runs/1/provider_stdout.log");
        assert_eq!(logs[0].stream, "stdout");
        assert_eq!(logs[0].bytes, 42);
    }

    #[test]
    fn rewrites_run_log_path_prefixes_for_restores() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .add_run_log_file(run_id, "/old/data/runs/1/stdout.log", "stdout", 42)
            .unwrap();

        assert_eq!(
            store
                .rewrite_run_log_path_prefix(Path::new("/old/data"), Path::new("/new/data"))
                .unwrap(),
            1
        );

        let logs = store.list_run_log_files(run_id).unwrap();
        assert_eq!(logs[0].path, "/new/data/runs/1/stdout.log");
    }

    #[test]
    fn records_notification_deliveries() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();

        let delivery_id = store
            .record_notification_delivery(
                Some(run_id),
                Some(job_id),
                "run_succeeded",
                "webhook",
                "delivered",
                Some("ok"),
            )
            .unwrap();
        let deliveries = store.list_notification_deliveries_for_run(run_id).unwrap();

        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].id, delivery_id);
        assert_eq!(deliveries[0].run_id, Some(run_id));
        assert_eq!(deliveries[0].job_id, Some(job_id));
        assert_eq!(deliveries[0].event_type, "run_succeeded");
        assert_eq!(deliveries[0].channel, "webhook");
        assert_eq!(deliveries[0].status, "delivered");
        assert_eq!(deliveries[0].message.as_deref(), Some("ok"));
    }

    #[test]
    fn tracks_run_process_metadata() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();

        store.set_run_process(run_id, 1234, 1234).unwrap();
        let process = store.get_run_process(run_id).unwrap().unwrap();
        assert_eq!(process.run_id, run_id);
        assert_eq!(process.pid, 1234);
        assert_eq!(process.process_group_id, 1234);
        assert!(store.clear_run_process(run_id).unwrap());
        assert!(store.get_run_process(run_id).unwrap().is_none());
    }

    #[test]
    fn locks_prevent_duplicate_owners_until_released_or_expired() {
        let mut store = Store::in_memory().unwrap();
        let future = Utc::now() + chrono::Duration::minutes(5);

        assert!(
            store
                .acquire_lock("daemon", "owner-a", Some(future))
                .unwrap()
        );
        assert!(
            !store
                .acquire_lock("daemon", "owner-b", Some(future))
                .unwrap()
        );
        assert!(!store.release_lock("daemon", "owner-b").unwrap());
        assert!(store.release_lock("daemon", "owner-a").unwrap());
        assert!(
            store
                .acquire_lock("daemon", "owner-b", Some(future))
                .unwrap()
        );
    }

    #[test]
    fn lock_owner_can_refresh_existing_lock() {
        let mut store = Store::in_memory().unwrap();
        let future = Utc::now() + chrono::Duration::minutes(5);
        let later = Utc::now() + chrono::Duration::minutes(10);

        assert!(
            store
                .acquire_lock("daemon", "owner-a", Some(future))
                .unwrap()
        );
        assert!(
            store
                .acquire_lock("daemon", "owner-a", Some(later))
                .unwrap()
        );
        assert!(
            !store
                .acquire_lock("daemon", "owner-b", Some(later))
                .unwrap()
        );
    }

    #[test]
    fn expired_locks_can_be_reacquired() {
        let mut store = Store::in_memory().unwrap();
        let past = Utc::now() - chrono::Duration::minutes(5);
        let future = Utc::now() + chrono::Duration::minutes(5);

        assert!(store.acquire_lock("daemon", "owner-a", Some(past)).unwrap());
        assert!(
            store
                .acquire_lock("daemon", "owner-b", Some(future))
                .unwrap()
        );
    }

    #[test]
    fn mutations_record_audit_events() {
        let mut store = Store::in_memory().unwrap();
        let job_id = store.create_job(&spec()).unwrap();
        let run_id = store
            .create_run(job_id, &spec(), RunTrigger::Manual, None)
            .unwrap();
        store
            .transition_run(run_id, RunStatus::Skipped, Some("test"))
            .unwrap();

        let events = store.list_audit_events().unwrap();
        assert!(events.iter().any(|event| event.event_type == "job.created"));
        assert!(events.iter().any(|event| event.event_type == "run.created"));
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "run.transitioned")
        );
    }
}
