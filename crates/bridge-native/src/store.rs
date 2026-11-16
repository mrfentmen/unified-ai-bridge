use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GENESIS_HASH: &str = "GENESIS";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub agent: String,
    pub provider: String,
    pub tab_id: i64,
    pub window_id: i64,
    pub url: String,
    pub conversation: Option<String>,
    pub extension_version: Option<String>,
    #[serde(default)]
    pub workspace_id: String,
    pub created_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub id: i64,
    pub created_at: String,
    pub agent: String,
    pub session_id: String,
    pub tool: String,
    pub kind: String,
    pub path: Option<String>,
    pub summary: String,
    pub decision: String,
    #[serde(default)]
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AuditInput {
    pub agent: String,
    pub session_id: String,
    pub action: String,
    pub path: Option<String>,
    pub decision: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditBody {
    pub seq: i64,
    pub created_at: String,
    pub agent: String,
    pub session_id: String,
    pub action: String,
    pub path: Option<String>,
    pub decision: String,
    pub detail: String,
    pub prev_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFileEntry {
    #[serde(flatten)]
    pub body: AuditBody,
    pub entry_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditVerification {
    pub valid: bool,
    pub database_entries: usize,
    pub file_entries: usize,
    pub first_bad_seq: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum RequestClaim {
    New,
    Completed(String),
    InProgress,
    Rejected(String),
}

pub struct ResponseRequest<'a> {
    pub scope: &'a str,
    pub response: &'a str,
    pub task: &'a str,
    pub index: u32,
    pub tool: &'a str,
    pub arguments: &'a serde_json::Value,
    pub response_limit: u32,
    pub task_limit: u32,
    pub timeout_secs: u64,
}

pub struct Store {
    conn: Mutex<Connection>,
    audit_path: PathBuf,
    audit_lock: PathBuf,
}

impl Store {
    /// Serialize first-start migration with all subsequent audit appends. The
    /// legacy workspace log is only read and never removed or rewritten.
    pub fn open_migrating(
        database: &Path,
        audit_path: &Path,
        legacy_audit: &Path,
        lock_dir: &Path,
    ) -> Result<Self> {
        fs::create_dir_all(lock_dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_dir.join("audit.lock"))?;
        lock.lock_exclusive()?;
        let store = Self::open(database, audit_path, lock_dir)?;
        if !audit_path.exists() && legacy_audit.exists() {
            let legacy = Self::open_readonly(database, legacy_audit)?;
            let verification = legacy.verify_audit()?;
            if !verification.valid {
                bail!("legacy audit migration refused: {}", verification.message);
            }
            crate::config::atomic_write(audit_path, &fs::read(legacy_audit)?)?;
        }
        let verification = store.verify_audit()?;
        if !verification.valid {
            bail!("audit validation failed: {}", verification.message);
        }
        FileExt::unlock(&lock)?;
        Ok(store)
    }

    /// Diagnostic access never creates directories, tables, or audit files.
    pub fn open_readonly(database: &Path, audit_path: &Path) -> Result<Self> {
        let conn =
            Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            conn: Mutex::new(conn),
            audit_path: audit_path.into(),
            audit_lock: PathBuf::new(),
        })
    }

    pub fn integrity_check(&self) -> Result<String> {
        let conn = self.conn.lock().expect("store mutex");
        let mut stmt = conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.join("; "))
    }
    pub fn open(database: &Path, audit_path: &Path, lock_dir: &Path) -> Result<Self> {
        if let Some(parent) = database.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        if let Some(parent) = audit_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create audit directory {}", parent.display()))?;
        }
        fs::create_dir_all(lock_dir)
            .with_context(|| format!("create lock directory {}", lock_dir.display()))?;
        let conn = Connection::open(database)
            .with_context(|| format!("open SQLite database {}", database.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                agent TEXT NOT NULL,
                provider TEXT NOT NULL,
                tab_id INTEGER NOT NULL,
                window_id INTEGER NOT NULL DEFAULT -1,
                url TEXT NOT NULL,
                conversation TEXT,
                extension_version TEXT,
                created_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS sessions_active ON sessions(last_seen_at);
            CREATE TABLE IF NOT EXISTS activity (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                kind TEXT NOT NULL,
                path TEXT,
                summary TEXT NOT NULL,
                decision TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS activity_recent ON activity(id DESC);
            CREATE TABLE IF NOT EXISTS audit (
                seq INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                action TEXT NOT NULL,
                path TEXT,
                decision TEXT NOT NULL,
                detail TEXT NOT NULL,
                prev_hash TEXT NOT NULL,
                entry_hash TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS requests (
                session_id TEXT NOT NULL,
                request_key TEXT NOT NULL,
                tool TEXT NOT NULL,
                status TEXT NOT NULL,
                result_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(session_id, request_key)
            );
            CREATE TABLE IF NOT EXISTS response_requests (
                scope TEXT NOT NULL,
                response_id TEXT NOT NULL,
                call_index INTEGER NOT NULL,
                task_id TEXT NOT NULL,
                payload TEXT NOT NULL,
                status TEXT NOT NULL,
                result_json TEXT,
                deadline INTEGER NOT NULL,
                PRIMARY KEY(scope, response_id, call_index)
            );
            CREATE INDEX IF NOT EXISTS response_request_tasks ON response_requests(scope, task_id);
            CREATE TABLE IF NOT EXISTS workspaces (id TEXT PRIMARY KEY, definition TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS bridge_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS activity_results (activity_id INTEGER PRIMARY KEY REFERENCES activity(id), duration_ms INTEGER NOT NULL, result_json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS file_changes (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, created_at TEXT NOT NULL, record TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS artifacts (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, download_id INTEGER NOT NULL, record TEXT NOT NULL, UNIQUE(workspace_id, session_id, download_id));
            CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, owner TEXT NOT NULL, created_at TEXT NOT NULL, record TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS write_grants (workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, record TEXT NOT NULL, PRIMARY KEY(workspace_id,session_id));
            CREATE INDEX IF NOT EXISTS jobs_owner ON jobs(workspace_id,owner,created_at);
            CREATE TABLE IF NOT EXISTS job_output (job_id TEXT NOT NULL REFERENCES jobs(id), seq INTEGER NOT NULL, stream TEXT NOT NULL, text TEXT NOT NULL, PRIMARY KEY(job_id,seq));
            "#,
        )?;
        let has_workspace: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='workspace_id'",
            [],
            |row| row.get(0),
        )?;
        if has_workspace == 0 {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN workspace_id TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            audit_path: audit_path.to_path_buf(),
            audit_lock: lock_dir.join("audit.lock"),
        })
    }

    fn now() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    pub fn insert_job(&self, job: &crate::jobs::Job) -> Result<()> {
        self.conn.lock().expect("store mutex").execute(
            "INSERT INTO jobs VALUES (?1,?2,?3,?4,?5)",
            params![
                job.id,
                job.workspace_id,
                job.owner,
                job.created_at,
                serde_json::to_string(job)?
            ],
        )?;
        Ok(())
    }

    pub fn save_write_grant(&self, grant: &crate::grants::Grant) -> Result<()> {
        self.conn.lock().expect("store mutex").execute("INSERT INTO write_grants VALUES (?1,?2,?3) ON CONFLICT(workspace_id,session_id) DO UPDATE SET record=excluded.record",params![grant.workspace_id,grant.session_id,serde_json::to_string(grant)?])?;
        Ok(())
    }
    pub fn write_grant(
        &self,
        workspace: &str,
        session: &str,
    ) -> Result<Option<crate::grants::Grant>> {
        let value: Option<String> = self
            .conn
            .lock()
            .expect("store mutex")
            .query_row(
                "SELECT record FROM write_grants WHERE workspace_id=?1 AND session_id=?2",
                params![workspace, session],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub fn write_grants(&self, workspace: &str) -> Result<Vec<crate::grants::Grant>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut q = conn.prepare(
            "SELECT record FROM write_grants WHERE workspace_id=?1 ORDER BY rowid DESC LIMIT 100",
        )?;
        let rows = q.query_map([workspace], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn job(&self, id: &str) -> Result<Option<crate::jobs::Job>> {
        let conn = self.conn.lock().expect("store mutex");
        let value: Option<String> = conn
            .query_row("SELECT record FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }

    pub fn jobs(&self, workspace: &str, owner: Option<&str>) -> Result<Vec<crate::jobs::Job>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut query=conn.prepare("SELECT record FROM jobs WHERE workspace_id=?1 AND (?2 IS NULL OR owner=?2) ORDER BY (json_extract(record,'$.state') NOT IN ('completed','failed','cancelled','interrupted')) DESC, created_at DESC LIMIT 100")?;
        let rows = query.query_map(params![workspace, owner], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn update_job(
        &self,
        id: &str,
        update: impl FnOnce(&mut crate::jobs::Job) -> Result<()>,
    ) -> Result<crate::jobs::Job> {
        let mut conn = self.conn.lock().expect("store mutex");
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let raw: String =
            tx.query_row("SELECT record FROM jobs WHERE id=?1", [id], |r| r.get(0))?;
        let mut job: crate::jobs::Job = serde_json::from_str(&raw)?;
        update(&mut job)?;
        tx.execute(
            "UPDATE jobs SET record=?2 WHERE id=?1",
            params![id, serde_json::to_string(&job)?],
        )?;
        tx.commit()?;
        Ok(job)
    }

    pub fn append_job_output(&self, id: &str, stream: &str, text: &str) -> Result<()> {
        let mut conn = self.conn.lock().expect("store mutex");
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO job_output(job_id,seq,stream,text) SELECT ?1,COALESCE(MAX(seq),0)+1,?2,?3 FROM job_output WHERE job_id=?1",params![id,stream,text])?;
        tx.commit()?;
        Ok(())
    }

    pub fn job_output(&self, id: &str, after: u64) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut query=conn.prepare("SELECT seq,stream,text FROM job_output WHERE job_id=?1 AND seq>?2 ORDER BY seq LIMIT 8")?;
        let rows=query.query_map(params![id,after.min(i64::MAX as u64) as i64],|r|Ok(serde_json::json!({"cursor":r.get::<_,i64>(0)?,"stream":r.get::<_,String>(1)?,"text":r.get::<_,String>(2)?})))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn save_file_change(&self, record: &crate::tools::recovery::Change) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute("INSERT INTO file_changes VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET record=excluded.record",
            params![record.id,record.workspace_id,record.created_at,serde_json::to_string(record)?])?;
        Ok(())
    }

    pub fn save_artifact(&self, artifact: &crate::tools::artifacts::Artifact) -> Result<()> {
        self.conn.lock().expect("store mutex").execute(
            "INSERT INTO artifacts VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET record=excluded.record",
            params![artifact.id, artifact.workspace_id, artifact.session_id, artifact.download_id, serde_json::to_string(artifact)?],
        )?;
        Ok(())
    }

    pub fn artifacts(
        &self,
        workspace: &str,
        session: &str,
    ) -> Result<Vec<crate::tools::artifacts::Artifact>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut query = conn.prepare("SELECT record FROM artifacts WHERE workspace_id=?1 AND session_id=?2 ORDER BY rowid DESC LIMIT 100")?;
        let rows = query.query_map(params![workspace, session], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn file_change(
        &self,
        id: &str,
        workspace_id: &str,
    ) -> Result<Option<crate::tools::recovery::Change>> {
        let conn = self.conn.lock().expect("store mutex");
        let raw: Option<String> = conn
            .query_row(
                "SELECT record FROM file_changes WHERE id=?1 AND workspace_id=?2",
                params![id, workspace_id],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }

    pub fn file_changes(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::tools::recovery::Change>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut statement = conn.prepare("SELECT record FROM file_changes WHERE workspace_id=?1 ORDER BY created_at DESC, id DESC LIMIT ?2")?;
        let rows = statement.query_map(params![workspace_id, limit.min(100) as i64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn insert_session(&self, session: &SessionRecord) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute(
            "DELETE FROM sessions WHERE provider=?1 AND tab_id=?2",
            params![session.provider, session.tab_id],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO sessions
             (id, agent, provider, tab_id, window_id, url, conversation, extension_version, created_at, last_seen_at, workspace_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                session.id,
                session.agent,
                session.provider,
                session.tab_id,
                session.window_id,
                session.url,
                session.conversation,
                session.extension_version,
                session.created_at,
                session.last_seen_at,
                session.workspace_id
            ],
        )?;
        Ok(())
    }

    pub fn touch_session(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute(
            "UPDATE sessions SET last_seen_at = ?1 WHERE id = ?2",
            params![Self::now(), id],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRecord>> {
        let conn = self.conn.lock().expect("store mutex");
        conn.query_row(
            "SELECT id, agent, provider, tab_id, window_id, url, conversation, extension_version, created_at, last_seen_at, workspace_id
             FROM sessions WHERE id = ?1",
            params![id],
            row_to_session,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut stmt = conn.prepare(
            "SELECT id, agent, provider, tab_id, window_id, url, conversation, extension_version, created_at, last_seen_at, workspace_id
             FROM sessions ORDER BY last_seen_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_session)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn prune_sessions_older_than(&self, hours: i64) -> Result<usize> {
        let cutoff = (Utc::now() - chrono::Duration::hours(hours))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let conn = self.conn.lock().expect("store mutex");
        let changed = conn.execute(
            "DELETE FROM sessions WHERE last_seen_at < ?1",
            params![cutoff],
        )?;
        Ok(changed)
    }

    pub fn record_activity(
        &self,
        agent: &str,
        session_id: &str,
        tool: &str,
        kind: &str,
        path: Option<&str>,
        summary: &str,
        decision: &str,
    ) -> Result<ActivityRecord> {
        let conn = self.conn.lock().expect("store mutex");
        let created_at = Self::now();
        conn.execute(
            "INSERT INTO activity (created_at, agent, session_id, tool, kind, path, summary, decision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![created_at, agent, session_id, tool, kind, path, summary, decision],
        )?;
        Ok(ActivityRecord {
            id: conn.last_insert_rowid(),
            created_at,
            agent: agent.to_string(),
            session_id: session_id.to_string(),
            tool: tool.to_string(),
            kind: kind.to_string(),
            path: path.map(str::to_string),
            summary: summary.to_string(),
            decision: decision.to_string(),
            duration_ms: 0,
        })
    }

    pub fn recent_activity(&self, limit: usize) -> Result<Vec<ActivityRecord>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut stmt = conn.prepare(
            "SELECT a.id, created_at, agent, session_id, tool, kind, path, summary, decision, COALESCE(r.duration_ms,0)
             FROM activity a LEFT JOIN activity_results r ON r.activity_id=a.id ORDER BY a.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(ActivityRecord {
                id: row.get(0)?,
                created_at: row.get(1)?,
                agent: row.get(2)?,
                session_id: row.get(3)?,
                tool: row.get(4)?,
                kind: row.get(5)?,
                path: row.get(6)?,
                summary: row.get(7)?,
                decision: row.get(8)?,
                duration_ms: row.get::<_, i64>(9)?.max(0) as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn save_activity_result(
        &self,
        id: i64,
        duration_ms: u64,
        result: &crate::wire::ToolResultMessage,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute(
            "INSERT INTO activity_results(activity_id,duration_ms,result_json) VALUES (?1,?2,?3)",
            params![
                id,
                i64::try_from(duration_ms)?,
                serde_json::to_string(result)?
            ],
        )?;
        Ok(())
    }

    pub fn activity_result(&self, id: i64) -> Result<serde_json::Value> {
        let conn = self.conn.lock().expect("store mutex");
        let raw: Option<String> = conn
            .query_row(
                "SELECT result_json FROM activity_results WHERE activity_id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(raw) => Ok(serde_json::from_str(&raw)?),
            None => Ok(
                serde_json::json!({"note":"Full result is unavailable for this older activity or non-tool event."}),
            ),
        }
    }

    /// Append a hash-chained audit entry. The file is the external append-only
    /// record; SQLite mirrors it for fast reads. A cross-process file lock
    /// keeps the chain linear when MCP and the browser host run together.
    pub fn append_audit(&self, input: AuditInput) -> Result<AuditFileEntry> {
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.audit_lock)
            .with_context(|| format!("open audit lock {}", self.audit_lock.display()))?;
        lock_file.lock_exclusive().context("lock audit chain")?;

        let result = self.append_audit_locked(input);
        let _ = FileExt::unlock(&lock_file);
        result
    }

    fn append_audit_locked(&self, input: AuditInput) -> Result<AuditFileEntry> {
        let conn = self.conn.lock().expect("store mutex");
        let (seq, prev_hash): (i64, String) = conn
            .query_row(
                "SELECT seq, entry_hash FROM audit ORDER BY seq DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, GENESIS_HASH.to_string()));
        let body = AuditBody {
            seq: seq + 1,
            created_at: Self::now(),
            agent: input.agent,
            session_id: input.session_id,
            action: input.action,
            path: input.path,
            decision: input.decision,
            detail: input.detail,
            prev_hash,
        };
        let canonical = serde_json::to_vec(&body).context("serialize audit body")?;
        let entry_hash = hex::encode(Sha256::digest(&canonical));
        let entry = AuditFileEntry {
            body,
            entry_hash: entry_hash.clone(),
        };
        let line = serde_json::to_string(&entry).context("serialize audit line")?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
            .with_context(|| format!("open audit log {}", self.audit_path.display()))?;
        writeln!(file, "{line}").context("append audit log")?;
        file.sync_data().context("fsync audit log")?;

        conn.execute(
            "INSERT INTO audit (seq, created_at, agent, session_id, action, path, decision, detail, prev_hash, entry_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.body.seq,
                entry.body.created_at,
                entry.body.agent,
                entry.body.session_id,
                entry.body.action,
                entry.body.path,
                entry.body.decision,
                entry.body.detail,
                entry.body.prev_hash,
                entry.entry_hash
            ],
        )
        .context("insert audit row")?;
        Ok(entry)
    }

    pub fn verify_audit(&self) -> Result<AuditVerification> {
        let conn = self.conn.lock().expect("store mutex");
        let mut stmt = conn.prepare(
            "SELECT seq, created_at, agent, session_id, action, path, decision, detail, prev_hash, entry_hash
             FROM audit ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AuditFileEntry {
                body: AuditBody {
                    seq: row.get(0)?,
                    created_at: row.get(1)?,
                    agent: row.get(2)?,
                    session_id: row.get(3)?,
                    action: row.get(4)?,
                    path: row.get(5)?,
                    decision: row.get(6)?,
                    detail: row.get(7)?,
                    prev_hash: row.get(8)?,
                },
                entry_hash: row.get(9)?,
            })
        })?;
        let entries = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut previous = GENESIS_HASH.to_string();
        let mut expected_seq = 1i64;
        for entry in &entries {
            let canonical = serde_json::to_vec(&entry.body)?;
            let expected_hash = hex::encode(Sha256::digest(&canonical));
            if entry.body.seq != expected_seq
                || entry.body.prev_hash != previous
                || entry.entry_hash != expected_hash
            {
                return Ok(AuditVerification {
                    valid: false,
                    database_entries: entries.len(),
                    file_entries: 0,
                    first_bad_seq: Some(entry.body.seq),
                    message: "audit hash chain is broken".into(),
                });
            }
            previous = entry.entry_hash.clone();
            expected_seq += 1;
        }

        let file_entries = match fs::read_to_string(&self.audit_path) {
            Ok(raw) => {
                let mut count = 0usize;
                let mut previous = GENESIS_HASH.to_string();
                for (index, line) in raw
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .enumerate()
                {
                    let entry: AuditFileEntry = match serde_json::from_str(line) {
                        Ok(entry) => entry,
                        Err(_) => {
                            return Ok(AuditVerification {
                                valid: false,
                                database_entries: entries.len(),
                                file_entries: count,
                                first_bad_seq: Some(index as i64 + 1),
                                message: "audit file contains invalid JSON".into(),
                            })
                        }
                    };
                    let canonical = serde_json::to_vec(&entry.body)?;
                    let expected_hash = hex::encode(Sha256::digest(&canonical));
                    if entry.body.prev_hash != previous
                        || entry.entry_hash != expected_hash
                        || entry.body.seq != index as i64 + 1
                        || entries.get(index).map(|row| &row.entry_hash) != Some(&entry.entry_hash)
                    {
                        return Ok(AuditVerification {
                            valid: false,
                            database_entries: entries.len(),
                            file_entries: count,
                            first_bad_seq: Some(entry.body.seq),
                            message: "audit file hash chain is broken".into(),
                        });
                    }
                    previous = entry.entry_hash;
                    count += 1;
                }
                count
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err.into()),
        };

        let valid = entries.len() == file_entries;
        Ok(AuditVerification {
            valid,
            database_entries: entries.len(),
            file_entries,
            first_bad_seq: if valid {
                None
            } else {
                Some((entries.len().min(file_entries) + 1) as i64)
            },
            message: if valid {
                "audit chain verified".into()
            } else {
                "audit database and append-only file have different entry counts".into()
            },
        })
    }

    /// Reserve execution and both budgets in one SQLite transaction. The scope
    /// is host-derived from workspace/provider/conversation, not the session ID.
    pub fn claim_response(&self, request: ResponseRequest<'_>) -> Result<RequestClaim> {
        let ResponseRequest {
            scope,
            response,
            task,
            index,
            tool,
            arguments,
            response_limit,
            task_limit,
            timeout_secs,
        } = request;
        let payload = serde_json::to_string(&(tool, arguments))?;
        let mut conn = self.conn.lock().expect("store mutex");
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<(String, String, Option<String>, i64)> = tx
            .query_row(
                "SELECT payload, status, result_json, deadline FROM response_requests
             WHERE scope=?1 AND response_id=?2 AND call_index=?3",
                params![scope, response, index],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((previous, status, result, deadline)) = existing {
            if previous != payload {
                return Ok(RequestClaim::Rejected(serde_json::to_string(&crate::wire::ToolResultMessage::error(
                    tool, "request_conflict", "This response slot already contains a different tool call; it will not execute again.".into(),
                ))?));
            }
            if let Some(result) = result {
                return Ok(RequestClaim::Completed(result));
            }
            if status == "running" && deadline > Utc::now().timestamp() {
                return Ok(RequestClaim::InProgress);
            }
            let result = serde_json::to_string(&crate::wire::ToolResultMessage::error(
                tool, "request_timeout", "The original execution expired or its host stopped. Its outcome may be uncertain; it will not be executed again.".into(),
            ))?;
            tx.execute(
                "UPDATE response_requests SET status='error', result_json=?4
                WHERE scope=?1 AND response_id=?2 AND call_index=?3",
                params![scope, response, index, result],
            )?;
            tx.commit()?;
            return Ok(RequestClaim::Completed(result));
        }
        let response_count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM response_requests WHERE scope=?1 AND response_id=?2 AND status!='rejected'",
            params![scope, response], |row| row.get(0))?;
        let task_count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM response_requests WHERE scope=?1 AND task_id=?2 AND status!='rejected'",
            params![scope, task], |row| row.get(0))?;
        let rejection = if index >= response_limit || response_count >= response_limit {
            Some(("response_budget_exceeded", format!("This assistant response reached its {response_limit}-call limit. Stop and ask the user before continuing.")))
        } else if task_count >= task_limit {
            Some(("task_budget_exceeded", format!("This user task reached its {task_limit}-call limit. Wait for a new explicit user task.")))
        } else {
            None
        };
        let result = rejection
            .map(|(code, reason)| {
                serde_json::to_string(&crate::wire::ToolResultMessage::error(tool, code, reason))
            })
            .transpose()?;
        tx.execute(
            "INSERT INTO response_requests
            (scope,response_id,call_index,task_id,payload,status,result_json,deadline)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                scope,
                response,
                index,
                task,
                payload,
                if result.is_some() {
                    "rejected"
                } else {
                    "running"
                },
                result,
                Utc::now().timestamp() + timeout_secs as i64
            ],
        )?;
        tx.commit()?;
        Ok(match result {
            Some(result) => RequestClaim::Rejected(result),
            None => RequestClaim::New,
        })
    }

    pub fn finish_response(
        &self,
        scope: &str,
        response: &str,
        index: u32,
        result_json: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("store mutex");
        Ok(conn.execute(
            "UPDATE response_requests SET status='complete', result_json=?4
            WHERE scope=?1 AND response_id=?2 AND call_index=?3 AND status='running'",
            params![scope, response, index, result_json],
        )? == 1)
    }

    pub fn claim_request(&self, session_id: &str, key: &str, tool: &str) -> Result<RequestClaim> {
        let conn = self.conn.lock().expect("store mutex");
        let now = Self::now();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO requests (session_id, request_key, tool, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'running', ?4, ?4)",
            params![session_id, key, tool, now],
        )?;
        if inserted == 1 {
            return Ok(RequestClaim::New);
        }
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT status, result_json FROM requests WHERE session_id = ?1 AND request_key = ?2",
                params![session_id, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((status, Some(result))) if status == "complete" || status == "error" => {
                RequestClaim::Completed(result)
            }
            Some((status, _)) if status == "running" => RequestClaim::InProgress,
            Some(_) => RequestClaim::New,
            None => {
                bail!("failed to create request claim");
            }
        })
    }

    pub fn complete_request(&self, session_id: &str, key: &str, result_json: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute(
            "UPDATE requests SET status = 'complete', result_json = ?3, updated_at = ?4
             WHERE session_id = ?1 AND request_key = ?2",
            params![session_id, key, result_json, Self::now()],
        )?;
        Ok(())
    }

    pub fn fail_request(&self, session_id: &str, key: &str, result_json: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute(
            "UPDATE requests SET status = 'error', result_json = ?3, updated_at = ?4
             WHERE session_id = ?1 AND request_key = ?2",
            params![session_id, key, result_json, Self::now()],
        )?;
        Ok(())
    }

    pub fn count_sessions(&self) -> Result<usize> {
        let conn = self.conn.lock().expect("store mutex");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn save_workspace(&self, info: &bridge_core::workspace::WorkspaceInfo) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute("INSERT INTO workspaces(id,definition) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET definition=excluded.definition",
            params![info.id, serde_json::to_string(info)?])?;
        Ok(())
    }

    pub fn workspaces(&self) -> Result<Vec<bridge_core::workspace::WorkspaceInfo>> {
        let conn = self.conn.lock().expect("store mutex");
        let mut stmt = conn.prepare("SELECT definition FROM workspaces ORDER BY json_extract(definition,'$.last_used_at') DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn active_workspace_id(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("store mutex");
        Ok(conn
            .query_row(
                "SELECT value FROM bridge_state WHERE key='active_workspace'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_active_workspace(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute("INSERT INTO bridge_state(key,value) VALUES('active_workspace',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [id])?;
        Ok(())
    }

    pub fn forget_workspace(&self, id: &str) -> Result<()> {
        if self.active_workspace_id()?.as_deref() == Some(id) {
            bail!("cannot forget the active workspace; switch first");
        }
        let conn = self.conn.lock().expect("store mutex");
        conn.execute("DELETE FROM workspaces WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn rebind_sessions(&self, workspace_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute("UPDATE sessions SET workspace_id=?1", [workspace_id])?;
        Ok(())
    }
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        agent: row.get(1)?,
        provider: row.get(2)?,
        tab_id: row.get(3)?,
        window_id: row.get(4)?,
        url: row.get(5)?,
        conversation: row.get(6)?,
        extension_version: row.get(7)?,
        workspace_id: row.get(10)?,
        created_at: row.get(8)?,
        last_seen_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store() -> (tempfile::TempDir, Store) {
        let temp = tempdir().unwrap();
        let store = Store::open(
            &temp.path().join("db.sqlite3"),
            &temp.path().join("audit.jsonl"),
            &temp.path().join("locks"),
        )
        .unwrap();
        (temp, store)
    }

    fn input(seq: usize) -> AuditInput {
        AuditInput {
            agent: "CHATGPT".into(),
            session_id: "s".into(),
            action: "write".into(),
            path: None,
            decision: "allowed".into(),
            detail: format!("entry {seq}"),
        }
    }

    #[test]
    fn audit_chain_verifies() {
        let (_temp, store) = store();
        store.append_audit(input(1)).unwrap();
        store.append_audit(input(2)).unwrap();
        let verification = store.verify_audit().unwrap();
        assert!(verification.valid, "{verification:?}");
        assert_eq!(verification.database_entries, 2);
        assert_eq!(verification.file_entries, 2);
    }

    #[test]
    fn request_dedup_states() {
        let (_temp, store) = store();
        assert!(matches!(
            store.claim_request("s", "k", "read_file").unwrap(),
            RequestClaim::New
        ));
        assert!(matches!(
            store.claim_request("s", "k", "read_file").unwrap(),
            RequestClaim::InProgress
        ));
        store.complete_request("s", "k", "{\"ok\":true}").unwrap();
        match store.claim_request("s", "k", "read_file").unwrap() {
            RequestClaim::Completed(value) => assert!(value.contains("true")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn durable_response_budget_conflict_and_task_limit() {
        let (_temp, store) = store();
        let args = serde_json::json!({"path":"PROJECT"});
        let claim = |response: &str, task: &str, index, args: &serde_json::Value| {
            store
                .claim_response(ResponseRequest {
                    scope: "workspace/provider/conversation",
                    response,
                    task,
                    index,
                    tool: "list_files",
                    arguments: args,
                    response_limit: 2,
                    task_limit: 3,
                    timeout_secs: 60,
                })
                .unwrap()
        };
        assert!(matches!(claim("r", "t", 0, &args), RequestClaim::New));
        assert!(matches!(
            claim("r", "t", 0, &args),
            RequestClaim::InProgress
        ));
        store
            .finish_response(
                "workspace/provider/conversation",
                "r",
                0,
                "{\"status\":\"ok\"}",
            )
            .unwrap();
        assert!(matches!(
            claim("r", "t", 0, &args),
            RequestClaim::Completed(_)
        ));
        assert!(
            matches!(claim("r", "t", 0, &serde_json::json!({"path":"SHARED"})), RequestClaim::Rejected(s) if s.contains("request_conflict"))
        );
        assert!(matches!(claim("r", "t", 1, &args), RequestClaim::New));
        assert!(
            matches!(claim("r", "t", 2, &args), RequestClaim::Rejected(s) if s.contains("response_budget_exceeded"))
        );
        assert!(matches!(claim("r2", "t", 0, &args), RequestClaim::New));
        assert!(
            matches!(claim("r3", "t", 0, &args), RequestClaim::Rejected(s) if s.contains("task_budget_exceeded"))
        );
        assert!(matches!(
            claim("r4", "new-user-task", 0, &args),
            RequestClaim::New
        ));
    }

    #[test]
    fn expired_request_is_terminal_and_never_executes_again() {
        let (_temp, store) = store();
        let args = serde_json::json!({});
        let claim = || {
            store
                .claim_response(ResponseRequest {
                    scope: "s",
                    response: "r",
                    task: "t",
                    index: 0,
                    tool: "list_files",
                    arguments: &args,
                    response_limit: 1,
                    task_limit: 1,
                    timeout_secs: 0,
                })
                .unwrap()
        };
        assert!(matches!(claim(), RequestClaim::New));
        assert!(matches!(claim(), RequestClaim::Completed(s) if s.contains("request_timeout")));
        assert!(!store.finish_response("s", "r", 0, "{}").unwrap());
        assert!(matches!(claim(), RequestClaim::Completed(s) if s.contains("request_timeout")));
    }

    #[test]
    fn readonly_diagnostics_detect_divergent_valid_audit_file() {
        let (temp, store) = store();
        store.append_audit(input(1)).unwrap();
        let readonly = Store::open_readonly(
            &temp.path().join("db.sqlite3"),
            &temp.path().join("audit.jsonl"),
        )
        .unwrap();
        assert_eq!(readonly.integrity_check().unwrap(), "ok");
        assert!(readonly.verify_audit().unwrap().valid);
        let mut entry: AuditFileEntry =
            serde_json::from_str(&fs::read_to_string(temp.path().join("audit.jsonl")).unwrap())
                .unwrap();
        entry.body.detail = "different but correctly re-hashed event".into();
        entry.entry_hash = hex::encode(Sha256::digest(serde_json::to_vec(&entry.body).unwrap()));
        fs::write(
            temp.path().join("audit.jsonl"),
            serde_json::to_string(&entry).unwrap() + "\n",
        )
        .unwrap();
        assert!(
            !readonly.verify_audit().unwrap().valid,
            "valid chains must also agree with each other"
        );
    }
}
