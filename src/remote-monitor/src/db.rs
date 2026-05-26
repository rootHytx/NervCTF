//! SQLite state store for instance configs and active instances.

use anyhow::{anyhow, Result};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub type Db = Arc<Mutex<Connection>>;

pub fn open(path: &str) -> Result<Db> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
    init_schema(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS instance_configs (
            challenge_name  TEXT PRIMARY KEY,
            ctfd_id         INTEGER NOT NULL,
            backend         TEXT NOT NULL,
            config_json     TEXT NOT NULL,
            image_tag       TEXT,
            updated_at      TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS instances (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            challenge_name  TEXT NOT NULL,
            team_id         INTEGER NOT NULL,
            container_id    TEXT,
            host            TEXT NOT NULL,
            port            INTEGER NOT NULL,
            connection_type TEXT NOT NULL,
            status          TEXT NOT NULL,
            flag            TEXT,
            ctfd_flag_id    INTEGER,
            renewals_used   INTEGER DEFAULT 0,
            created_at      TEXT DEFAULT (datetime('now')),
            expires_at      TEXT NOT NULL,
            UNIQUE(challenge_name, team_id)
        );

        CREATE TABLE IF NOT EXISTS flag_attempts (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            challenge_name  TEXT NOT NULL,
            team_id         INTEGER NOT NULL,
            user_id         INTEGER NOT NULL,
            submitted_flag  TEXT NOT NULL,
            is_correct      INTEGER NOT NULL DEFAULT 0,
            is_flag_sharing INTEGER NOT NULL DEFAULT 0,
            owner_team_id   INTEGER,
            timestamp       TEXT DEFAULT (datetime('now'))
        );

        -- Permanent record of every flag ever generated for a team on a challenge.
        -- Never deleted when an instance is stopped or expires so that flag sharing
        -- detection works even after the original instance is gone.
        CREATE TABLE IF NOT EXISTS team_flags (
            challenge_name  TEXT NOT NULL,
            team_id         INTEGER NOT NULL,
            flag            TEXT NOT NULL,
            created_at      TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (challenge_name, team_id, flag)
        );

        -- Read-only cache of correct solves synced from CTFd MariaDB submissions table.
        -- Written only by the background sync task; never modified by game logic.
        CREATE TABLE IF NOT EXISTS ctfd_solves (
            challenge_name  TEXT NOT NULL,
            team_id         INTEGER NOT NULL,
            user_id         INTEGER,
            solved_at       TEXT,
            PRIMARY KEY (challenge_name, team_id)
        );

        -- Cached CTFd teams and users (id→name) for display purposes.
        CREATE TABLE IF NOT EXISTS ctfd_teams (
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ctfd_users (
            id      INTEGER PRIMARY KEY,
            name    TEXT NOT NULL,
            team_id INTEGER
        );

        CREATE TABLE IF NOT EXISTS operator_tokens (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            label        TEXT NOT NULL,
            token_hash   TEXT NOT NULL UNIQUE,
            created_at   TEXT DEFAULT (datetime('now')),
            last_used_at TEXT
        );

        CREATE TABLE IF NOT EXISTS sessions (
            session_id  TEXT PRIMARY KEY,
            operator_id INTEGER NOT NULL REFERENCES operator_tokens(id) ON DELETE CASCADE,
            created_at  TEXT DEFAULT (datetime('now')),
            expires_at  TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_instances_status_expires ON instances(status, expires_at);
        CREATE INDEX IF NOT EXISTS idx_instances_team ON instances(team_id, status);
        CREATE INDEX IF NOT EXISTS idx_team_flags_lookup ON team_flags(challenge_name, flag);
        CREATE INDEX IF NOT EXISTS idx_flag_attempts_challenge ON flag_attempts(challenge_name, team_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

        -- Singleton row (id=1 enforced by CHECK) that stores the most recent CTFd
        -- schema fingerprint and capability status produced by run_probe().
        -- Created once at startup; overwritten on every probe run.
        CREATE TABLE IF NOT EXISTS ctfd_probe (
            id                    INTEGER PRIMARY KEY CHECK (id = 1),
            probed_at             TEXT NOT NULL DEFAULT (datetime('now')),
            ctfd_version_tag      TEXT,
            ctfd_version_source   TEXT,
            is_team_mode          INTEGER,
            challenges_cols       TEXT,
            has_dynamic_table     INTEGER NOT NULL DEFAULT 0,
            dynamic_cols          TEXT,
            has_next_id           INTEGER NOT NULL DEFAULT 0,
            has_attribution       INTEGER NOT NULL DEFAULT 0,
            has_logic             INTEGER NOT NULL DEFAULT 0,
            has_position          INTEGER NOT NULL DEFAULT 0,
            dynamic_inline        INTEGER NOT NULL DEFAULT 0,
            dynamic_partial       INTEGER NOT NULL DEFAULT 0,
            has_instance_table    INTEGER NOT NULL DEFAULT 0,
            cap_challenge_crud    TEXT NOT NULL DEFAULT 'unknown',
            cap_dynamic_scoring   TEXT NOT NULL DEFAULT 'unknown',
            cap_player_auth       TEXT NOT NULL DEFAULT 'unknown',
            cap_instance_flags    TEXT NOT NULL DEFAULT 'unknown',
            cap_redis_sync        TEXT NOT NULL DEFAULT 'degraded',
            probe_notes           TEXT NOT NULL DEFAULT '[]'
        );
        "#,
    )?;
    // Migrations for existing databases.
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN flag TEXT", []);
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN ctfd_flag_id INTEGER", []);
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN user_id INTEGER", []);
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN extra_ports TEXT", []);
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS ctfd_solves (
            challenge_name TEXT NOT NULL,
            team_id        INTEGER NOT NULL,
            user_id        INTEGER,
            solved_at      TEXT,
            PRIMARY KEY (challenge_name, team_id)
        )",
        [],
    );
    let _ = conn.execute("ALTER TABLE ctfd_solves ADD COLUMN user_id INTEGER", []);
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS ctfd_teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
        [],
    );
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS ctfd_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, team_id INTEGER)",
        [],
    );
    // Backfill team_flags from any instances that already have a flag recorded.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO team_flags (challenge_name, team_id, flag)
         SELECT challenge_name, team_id, flag FROM instances WHERE flag IS NOT NULL",
        [],
    );
    // Migrations for operator auth tables.
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS operator_tokens (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            label        TEXT NOT NULL,
            token_hash   TEXT NOT NULL UNIQUE,
            created_at   TEXT DEFAULT (datetime('now')),
            last_used_at TEXT
        );
        CREATE TABLE IF NOT EXISTS sessions (
            session_id  TEXT PRIMARY KEY,
            operator_id INTEGER NOT NULL REFERENCES operator_tokens(id) ON DELETE CASCADE,
            created_at  TEXT DEFAULT (datetime('now')),
            expires_at  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);"
    );
    // Migration: add ctfd_probe table for existing databases that predate this feature.
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ctfd_probe (
            id                    INTEGER PRIMARY KEY CHECK (id = 1),
            probed_at             TEXT NOT NULL DEFAULT (datetime('now')),
            ctfd_version_tag      TEXT,
            ctfd_version_source   TEXT,
            is_team_mode          INTEGER,
            challenges_cols       TEXT,
            has_dynamic_table     INTEGER NOT NULL DEFAULT 0,
            dynamic_cols          TEXT,
            has_next_id           INTEGER NOT NULL DEFAULT 0,
            has_attribution       INTEGER NOT NULL DEFAULT 0,
            has_logic             INTEGER NOT NULL DEFAULT 0,
            has_position          INTEGER NOT NULL DEFAULT 0,
            dynamic_inline        INTEGER NOT NULL DEFAULT 0,
            dynamic_partial       INTEGER NOT NULL DEFAULT 0,
            has_instance_table    INTEGER NOT NULL DEFAULT 0,
            cap_challenge_crud    TEXT NOT NULL DEFAULT 'unknown',
            cap_dynamic_scoring   TEXT NOT NULL DEFAULT 'unknown',
            cap_player_auth       TEXT NOT NULL DEFAULT 'unknown',
            cap_instance_flags    TEXT NOT NULL DEFAULT 'unknown',
            cap_redis_sync        TEXT NOT NULL DEFAULT 'degraded',
            probe_notes           TEXT NOT NULL DEFAULT '[]'
        );"
    );
    Ok(())
}

// ── Instance configs ──────────────────────────────────────────────────────────

pub fn upsert_config(db: &Db, challenge_name: &str, ctfd_id: u32, backend: &str, config_json: &str) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        r#"INSERT INTO instance_configs (challenge_name, ctfd_id, backend, config_json)
           VALUES (?1, ?2, ?3, ?4)
           ON CONFLICT(challenge_name) DO UPDATE SET
             ctfd_id=excluded.ctfd_id,
             backend=excluded.backend,
             config_json=excluded.config_json,
             updated_at=datetime('now')"#,
        params![challenge_name, ctfd_id, backend, config_json],
    )?;
    Ok(())
}

pub fn get_config(db: &Db, challenge_name: &str) -> Result<Option<String>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT config_json FROM instance_configs WHERE challenge_name = ?1",
    )?;
    let mut rows = stmt.query(params![challenge_name])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn get_ctfd_id(db: &Db, challenge_name: &str) -> Result<Option<i64>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let result = conn.query_row(
        "SELECT ctfd_id FROM instance_configs WHERE challenge_name = ?1",
        params![challenge_name],
        |row| row.get::<_, i64>(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn update_image_tag(db: &Db, challenge_name: &str, image_tag: &str) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "UPDATE instance_configs SET image_tag = ?1 WHERE challenge_name = ?2",
        params![image_tag, challenge_name],
    )?;
    Ok(())
}

pub fn get_image_tag(db: &Db, challenge_name: &str) -> Result<Option<String>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT image_tag FROM instance_configs WHERE challenge_name = ?1",
    )?;
    let mut rows = stmt.query(params![challenge_name])?;
    if let Some(row) = rows.next()? {
        let tag: Option<String> = row.get(0)?;
        Ok(tag)
    } else {
        Ok(None)
    }
}

pub fn list_configs(db: &Db) -> Result<Vec<Value>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT challenge_name, ctfd_id, backend, image_tag, updated_at FROM instance_configs ORDER BY challenge_name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "challenge_name": row.get::<_, String>(0)?,
            "ctfd_id": row.get::<_, i64>(1)?,
            "backend": row.get::<_, String>(2)?,
            "image_tag": row.get::<_, Option<String>>(3)?,
            "updated_at": row.get::<_, String>(4)?,
        }))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

// ── Active instances ──────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct InstanceRow {
    pub id: i64,
    pub challenge_name: String,
    pub team_id: i64,
    pub user_id: Option<i64>,
    pub container_id: Option<String>,
    pub host: String,
    pub port: i64,
    pub connection_type: String,
    pub status: String,
    pub renewals_used: i64,
    pub expires_at: String,
    pub flag: Option<String>,
    /// JSON object `{"<internal>": <host>}` for each port mapping when >1 port exposed.
    pub extra_ports: Option<String>,
}

pub fn get_instance(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<InstanceRow>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT id, challenge_name, team_id, user_id, container_id, host, port, connection_type, status, renewals_used, expires_at, flag, extra_ports
         FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
    )?;
    let mut rows = stmt.query(params![challenge_name, team_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(InstanceRow {
            id: row.get(0)?,
            challenge_name: row.get(1)?,
            team_id: row.get(2)?,
            user_id: row.get(3)?,
            container_id: row.get(4)?,
            host: row.get(5)?,
            port: row.get(6)?,
            connection_type: row.get(7)?,
            status: row.get(8)?,
            renewals_used: row.get(9)?,
            expires_at: row.get(10)?,
            flag: row.get(11)?,
            extra_ports: row.get(12)?,
        }))
    } else {
        Ok(None)
    }
}

/// Count active (running + provisioning) instances for a team across all challenges.
pub fn count_active_instances_for_team(db: &Db, team_id: i64) -> Result<i64> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM instances WHERE team_id = ?1 AND status IN ('running', 'provisioning')",
        params![team_id],
        |row| row.get(0),
    )?;
    Ok(n)
}

/// Insert a placeholder row with status='provisioning' so the info endpoint can
/// return status immediately while compose runs in the background.
/// Uses INSERT OR IGNORE so a concurrent retry doesn't clobber an existing row.
///
/// `container_id` should be the pre-generated project/container name so that the
/// background orphan cleanup sees it as a tracked instance before compose::up returns.
pub fn insert_provisioning_stub(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    user_id: Option<i64>,
    host: &str,
    connection_type: &str,
    expires_at: &str,
    container_id: Option<&str>,
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "INSERT OR IGNORE INTO instances (challenge_name, team_id, user_id, container_id, host, port, connection_type, status, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, 'provisioning', ?7)",
        params![challenge_name, team_id, user_id, container_id, host, connection_type, expires_at],
    )?;
    Ok(())
}

pub fn insert_instance(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    user_id: Option<i64>,
    container_id: &str,
    host: &str,
    port: i64,
    connection_type: &str,
    expires_at: &str,
    flag: Option<&str>,
    ctfd_flag_id: Option<i64>,
    extra_ports: Option<&str>,
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        r#"INSERT INTO instances (challenge_name, team_id, user_id, container_id, host, port, connection_type, status, flag, ctfd_flag_id, extra_ports, expires_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9, ?10, ?11)
           ON CONFLICT(challenge_name, team_id) DO UPDATE SET
             user_id=excluded.user_id, container_id=excluded.container_id,
             host=excluded.host, port=excluded.port,
             connection_type=excluded.connection_type, status='running',
             flag=excluded.flag, ctfd_flag_id=excluded.ctfd_flag_id,
             extra_ports=excluded.extra_ports,
             expires_at=excluded.expires_at, renewals_used=0"#,
        params![challenge_name, team_id, user_id, container_id, host, port, connection_type, flag, ctfd_flag_id, extra_ports, expires_at],
    )?;
    // Persist the flag permanently so sharing detection works after the instance is gone.
    if let Some(f) = flag {
        conn.execute(
            "INSERT OR IGNORE INTO team_flags (challenge_name, team_id, flag) VALUES (?1, ?2, ?3)",
            params![challenge_name, team_id, f],
        )?;
    }
    Ok(())
}

/// Updates ctfd_flag_id for an existing instance row after the CTFd flag has been
/// created. Called immediately after create_flag() succeeds so that a crash between
/// insert_instance (ctfd_flag_id=NULL) and this call leaves the row without an ID —
/// the cleanup task already skips delete_flag when ctfd_flag_id IS NULL.
pub fn set_ctfd_flag_id(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    ctfd_flag_id: i64,
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "UPDATE instances SET ctfd_flag_id = ?1 WHERE challenge_name = ?2 AND team_id = ?3",
        params![ctfd_flag_id, challenge_name, team_id],
    )?;
    Ok(())
}

pub fn update_expires_at(db: &Db, challenge_name: &str, team_id: i64, expires_at: &str) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "UPDATE instances SET expires_at = ?1, renewals_used = renewals_used + 1 WHERE challenge_name = ?2 AND team_id = ?3",
        params![expires_at, challenge_name, team_id],
    )?;
    Ok(())
}

/// Delete an instance row and return `(container_id, ctfd_flag_id)` for cleanup.
/// Returns `Ok(None)` if no matching row existed.
pub fn delete_instance(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<(Option<String>, Option<i64>)>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let row = conn.query_row(
        "SELECT container_id, ctfd_flag_id FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
        params![challenge_name, team_id],
        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?)),
    );
    match row {
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
        Ok((container_id, ctfd_flag_id)) => {
            conn.execute(
                "DELETE FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
                params![challenge_name, team_id],
            )?;
            // Purge incorrect, non-sharing attempts for this team+challenge.
            // Correct solves and sharing alerts are kept permanently.
            conn.execute(
                "DELETE FROM flag_attempts WHERE challenge_name = ?1 AND team_id = ?2 AND is_correct = 0 AND is_flag_sharing = 0",
                params![challenge_name, team_id],
            )?;
            Ok(Some((container_id, ctfd_flag_id)))
        }
    }
}

/// Mark an instance as solved (keeps the row visible in admin panel) and return
/// `(container_id, ctfd_flag_id)` for cleanup. Returns `Ok(None)` if no row existed.
pub fn mark_instance_solved(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<(Option<String>, Option<i64>)>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let row = conn.query_row(
        "SELECT container_id, ctfd_flag_id FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
        params![challenge_name, team_id],
        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?)),
    );
    match row {
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
        Ok((container_id, ctfd_flag_id)) => {
            conn.execute(
                "UPDATE instances SET status='solved' WHERE challenge_name = ?1 AND team_id = ?2",
                params![challenge_name, team_id],
            )?;
            // Purge noise attempts — keep correct solves and sharing alerts.
            conn.execute(
                "DELETE FROM flag_attempts WHERE challenge_name = ?1 AND team_id = ?2 AND is_correct = 0 AND is_flag_sharing = 0",
                params![challenge_name, team_id],
            )?;
            Ok(Some((container_id, ctfd_flag_id)))
        }
    }
}

/// Returns all host ports currently in use by running instances.
/// Includes both the primary port and any extra ports stored in the extra_ports JSON column.
pub fn get_used_ports(db: &Db) -> Result<std::collections::HashSet<u16>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT port, extra_ports FROM instances WHERE status = 'running' OR (status = 'provisioning' AND port > 0)",
    )?;
    let mut ports = std::collections::HashSet::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    for r in rows {
        let (port, extra_ports) = r?;
        if let Ok(p) = u16::try_from(port) { ports.insert(p); }
        if let Some(ep) = extra_ports {
            if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&ep) {
                for (_, v) in map {
                    if let Some(p) = v.as_u64().and_then(|p| u16::try_from(p).ok()) {
                        ports.insert(p);
                    }
                }
            }
        }
    }
    Ok(ports)
}

/// Returns `(challenge_name, container_id, team_id, ctfd_flag_id)` for all expired running instances.
pub fn get_expired_instances(db: &Db) -> Result<Vec<(String, Option<String>, i64, Option<i64>)>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT challenge_name, container_id, team_id, ctfd_flag_id FROM instances \
         WHERE (expires_at < datetime('now') AND status = 'running') \
            OR (status = 'provisioning' AND created_at < datetime('now', '-30 minutes'))",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Returns all `status='running'` instances for container health checking.
pub fn get_running_instances(db: &Db) -> Result<Vec<(String, i64, Option<String>, Option<i64>)>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT challenge_name, team_id, container_id, ctfd_flag_id FROM instances WHERE status = 'running'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut result = Vec::new();
    for r in rows { result.push(r?); }
    Ok(result)
}

/// Returns all tracked container_ids (non-null) as a HashSet, for orphan detection.
pub fn get_all_container_ids(db: &Db) -> Result<std::collections::HashSet<String>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare("SELECT container_id FROM instances WHERE container_id IS NOT NULL")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut set = std::collections::HashSet::new();
    for r in rows { set.insert(r?); }
    Ok(set)
}

/// Returns all active instances as JSON values for the admin dashboard.
pub fn list_all_instances(db: &Db) -> Result<Vec<Value>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT i.challenge_name, i.team_id, ct.name, i.user_id, cu.name,
                i.host, i.port, i.connection_type, i.status, i.expires_at, i.created_at
         FROM instances i
         LEFT JOIN ctfd_teams ct ON ct.id = i.team_id
         LEFT JOIN ctfd_users cu ON cu.id = i.user_id
         ORDER BY i.created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "challenge_name": row.get::<_, String>(0)?,
            "team_id": row.get::<_, i64>(1)?,
            "team_name": row.get::<_, Option<String>>(2)?,
            "user_id": row.get::<_, Option<i64>>(3)?,
            "user_name": row.get::<_, Option<String>>(4)?,
            "host": row.get::<_, String>(5)?,
            "port": row.get::<_, i64>(6)?,
            "connection_type": row.get::<_, String>(7)?,
            "status": row.get::<_, String>(8)?,
            "expires_at": row.get::<_, String>(9)?,
            "created_at": row.get::<_, String>(10)?,
        }))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

// ── Flag attempts ─────────────────────────────────────────────────────────────

pub fn insert_flag_attempt(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    user_id: i64,
    submitted_flag: &str,
    is_correct: bool,
    is_flag_sharing: bool,
    owner_team_id: Option<i64>,
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        r#"INSERT INTO flag_attempts (challenge_name, team_id, user_id, submitted_flag, is_correct, is_flag_sharing, owner_team_id)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            challenge_name, team_id, user_id, submitted_flag,
            is_correct as i64, is_flag_sharing as i64, owner_team_id,
        ],
    )?;
    Ok(())
}

pub fn list_flag_attempts(db: &Db, limit: i64) -> Result<Vec<Value>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT fa.id, fa.challenge_name,
                fa.team_id, ct.name,
                fa.user_id, cu.name,
                fa.submitted_flag, fa.is_correct, fa.is_flag_sharing,
                fa.owner_team_id, ct2.name,
                fa.timestamp
         FROM flag_attempts fa
         LEFT JOIN ctfd_teams ct  ON ct.id  = fa.team_id
         LEFT JOIN ctfd_users cu  ON cu.id  = fa.user_id
         LEFT JOIN ctfd_teams ct2 ON ct2.id = fa.owner_team_id
         ORDER BY fa.timestamp DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, i64>(0)?,
            "challenge_name": row.get::<_, String>(1)?,
            "team_id": row.get::<_, i64>(2)?,
            "team_name": row.get::<_, Option<String>>(3)?,
            "user_id": row.get::<_, i64>(4)?,
            "user_name": row.get::<_, Option<String>>(5)?,
            "submitted_flag": row.get::<_, String>(6)?,
            "is_correct": row.get::<_, i64>(7)? != 0,
            "is_flag_sharing": row.get::<_, i64>(8)? != 0,
            "owner_team_id": row.get::<_, Option<i64>>(9)?,
            "owner_team_name": row.get::<_, Option<String>>(10)?,
            "timestamp": row.get::<_, String>(11)?,
        }))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

pub fn list_sharing_alerts(db: &Db) -> Result<Vec<Value>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT fa.id, fa.challenge_name,
                fa.team_id, ct.name,
                fa.user_id, cu.name,
                fa.submitted_flag, fa.is_correct,
                fa.owner_team_id, ct2.name,
                fa.timestamp
         FROM flag_attempts fa
         LEFT JOIN ctfd_teams ct  ON ct.id  = fa.team_id
         LEFT JOIN ctfd_users cu  ON cu.id  = fa.user_id
         LEFT JOIN ctfd_teams ct2 ON ct2.id = fa.owner_team_id
         WHERE fa.is_flag_sharing = 1
         ORDER BY fa.timestamp DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, i64>(0)?,
            "challenge_name": row.get::<_, String>(1)?,
            "team_id": row.get::<_, i64>(2)?,
            "team_name": row.get::<_, Option<String>>(3)?,
            "user_id": row.get::<_, i64>(4)?,
            "user_name": row.get::<_, Option<String>>(5)?,
            "submitted_flag": row.get::<_, String>(6)?,
            "is_correct": row.get::<_, i64>(7)? != 0,
            "owner_team_id": row.get::<_, Option<i64>>(8)?,
            "owner_team_name": row.get::<_, Option<String>>(9)?,
            "timestamp": row.get::<_, String>(10)?,
        }))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Reverts instances from status='solved' back to 'running' when the corresponding
/// CTFd submission no longer exists in the ctfd_solves cache (i.e. was deleted in CTFd).
pub fn revert_unsolved_instances(db: &Db) -> Result<usize> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n = conn.execute(
        "UPDATE instances SET status='running'
         WHERE status='solved'
         AND NOT EXISTS (
           SELECT 1 FROM ctfd_solves cs
           WHERE cs.challenge_name = instances.challenge_name
           AND cs.team_id = instances.team_id
         )",
        [],
    )?;
    Ok(n)
}

/// Remove `flag_attempts` records where `is_correct=1` but the solve no longer
/// exists in the `ctfd_solves` cache (i.e. the CTFd submission was deleted).
/// Returns the number of records removed.
pub fn delete_stale_correct_attempts(db: &Db) -> Result<usize> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n = conn.execute(
        "DELETE FROM flag_attempts
         WHERE is_correct = 1
         AND NOT EXISTS (
             SELECT 1 FROM ctfd_solves cs
             WHERE cs.challenge_name = flag_attempts.challenge_name
             AND cs.team_id = flag_attempts.team_id
         )",
        [],
    )?;
    Ok(n)
}

/// Full-replace the ctfd_solves cache with the current snapshot from MariaDB.
/// Any rows not in `rows` (i.e. deleted submissions) are removed.
pub fn replace_ctfd_solves(db: &Db, rows: &[(i64, Option<i64>, String, String)]) -> Result<()> {
    let mut conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM ctfd_solves", [])?;
    for (team_id, user_id, challenge_name, solved_at) in rows {
        if challenge_name.is_empty() { continue; }
        tx.execute(
            "INSERT INTO ctfd_solves (challenge_name, team_id, user_id, solved_at) VALUES (?1, ?2, ?3, ?4)",
            params![challenge_name, team_id, user_id, solved_at],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Full-replace the ctfd_teams and ctfd_users caches.
pub fn replace_ctfd_teams_and_users(
    db: &Db,
    teams: &[(i64, String)],
    users: &[(i64, String, Option<i64>)],
) -> Result<()> {
    let mut conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM ctfd_teams", [])?;
    for (id, name) in teams {
        tx.execute("INSERT INTO ctfd_teams (id, name) VALUES (?1, ?2)", params![id, name])?;
    }
    tx.execute("DELETE FROM ctfd_users", [])?;
    for (id, name, team_id) in users {
        tx.execute(
            "INSERT INTO ctfd_users (id, name, team_id) VALUES (?1, ?2, ?3)",
            params![id, name, team_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Returns true if the team has a correct solve for the challenge, checking both
/// the local flag_attempts log and the CTFd solve cache synced from MariaDB.
pub fn has_correct_solve(db: &Db, challenge_name: &str, team_id: i64) -> Result<bool> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let in_attempts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM flag_attempts WHERE challenge_name = ?1 AND team_id = ?2 AND is_correct = 1",
        params![challenge_name, team_id],
        |row| row.get(0),
    )?;
    if in_attempts > 0 {
        return Ok(true);
    }
    let in_ctfd: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ctfd_solves WHERE challenge_name = ?1 AND team_id = ?2",
        params![challenge_name, team_id],
        |row| row.get(0),
    )?;
    Ok(in_ctfd > 0)
}

/// Returns one row per team+challenge that has a correct solve, sourced from
/// the ctfd_solves sync cache (so CTFd submission deletions are reflected).
/// Filtered to only instance challenges tracked by the monitor.
pub fn list_correct_solves(db: &Db) -> Result<Vec<Value>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT s.challenge_name,
                s.team_id, ct.name,
                s.user_id, cu.name,
                s.solved_at
         FROM ctfd_solves s
         JOIN instance_configs ic ON ic.challenge_name = s.challenge_name
         LEFT JOIN ctfd_teams ct ON ct.id = s.team_id
         LEFT JOIN ctfd_users cu ON cu.id = s.user_id
         ORDER BY s.solved_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "challenge_name": row.get::<_, String>(0)?,
            "team_id": row.get::<_, i64>(1)?,
            "team_name": row.get::<_, Option<String>>(2)?,
            "user_id": row.get::<_, Option<i64>>(3)?,
            "user_name": row.get::<_, Option<String>>(4)?,
            "timestamp": row.get::<_, Option<String>>(5)?,
        }))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Finds if a flag value belongs to a different team's instance (flag sharing detection).
/// Returns Some(owner_team_id) if sharing is detected, None otherwise.
/// Returns Some(owner_team_id) if the submitted flag was generated for a different team,
/// even if that team's instance has already been stopped or expired.
/// Queries `team_flags` (permanent) rather than `instances` (ephemeral).
pub fn find_flag_owner(db: &Db, challenge_name: &str, submitted_flag: &str, submitting_team_id: i64) -> Result<Option<i64>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let result = conn.query_row(
        "SELECT team_id FROM team_flags WHERE challenge_name = ?1 AND flag = ?2 AND team_id != ?3",
        params![challenge_name, submitted_flag, submitting_team_id],
        |row| row.get::<_, i64>(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Delete all instances for a challenge and return `(container_id, ctfd_flag_id)` pairs for cleanup.
pub fn delete_all_instances_for_challenge(db: &Db, challenge_name: &str) -> Result<Vec<(Option<String>, Option<i64>)>> {
    let mut conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let pairs: Vec<(Option<String>, Option<i64>)> = {
        let mut stmt = conn.prepare(
            "SELECT container_id, ctfd_flag_id FROM instances WHERE challenge_name = ?1",
        )?;
        let rows: Vec<_> = stmt.query_map(params![challenge_name], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM instances WHERE challenge_name = ?1",
        params![challenge_name],
    )?;
    // Purge incorrect, non-sharing attempts for all teams on this challenge.
    tx.execute(
        "DELETE FROM flag_attempts WHERE challenge_name = ?1 AND is_correct = 0 AND is_flag_sharing = 0",
        params![challenge_name],
    )?;
    tx.commit()?;
    Ok(pairs)
}

// ── Operator tokens ───────────────────────────────────────────────────────────

pub fn count_operator_tokens(db: &Db) -> Result<i64> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM operator_tokens", [], |r| r.get(0))?;
    Ok(n)
}

pub fn insert_operator_token(db: &Db, label: &str, token_hash: &str) -> Result<i64> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "INSERT INTO operator_tokens (label, token_hash) VALUES (?1, ?2)",
        params![label, token_hash],
    )?;
    Ok(conn.last_insert_rowid())
}

/// INSERT OR IGNORE version for bootstrap: returns true if a new row was inserted.
pub fn insert_operator_token_ignore(db: &Db, label: &str, token_hash: &str) -> Result<bool> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO operator_tokens (label, token_hash) VALUES (?1, ?2)",
        params![label, token_hash],
    )?;
    Ok(n > 0)
}

/// Returns the operator row id if the hash matches an existing token, else None.
pub fn validate_token_hash(db: &Db, token_hash: &str) -> Result<Option<i64>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let result = conn.query_row(
        "SELECT id FROM operator_tokens WHERE token_hash = ?1",
        params![token_hash],
        |row| row.get::<_, i64>(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn list_operator_tokens(db: &Db) -> Result<Vec<Value>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT id, label, created_at, last_used_at FROM operator_tokens ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, i64>(0)?,
            "label": row.get::<_, String>(1)?,
            "created_at": row.get::<_, String>(2)?,
            "last_used_at": row.get::<_, Option<String>>(3)?,
        }))
    })?;
    let mut result = Vec::new();
    for r in rows { result.push(r?); }
    Ok(result)
}

/// Delete the token and all its sessions (cascades via FK). Returns true if a row was deleted.
pub fn revoke_operator_token(db: &Db, id: i64) -> Result<bool> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n = conn.execute("DELETE FROM operator_tokens WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

// ── Sessions ──────────────────────────────────────────────────────────────────

pub fn create_session(db: &Db, session_id: &str, operator_id: i64, expires_at: &str) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "INSERT INTO sessions (session_id, operator_id, expires_at) VALUES (?1, ?2, ?3)",
        params![session_id, operator_id, expires_at],
    )?;
    Ok(())
}

/// Returns the operator_id if the session exists and has not expired, else None.
pub fn validate_session(db: &Db, session_id: &str) -> Result<Option<i64>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let result = conn.query_row(
        "SELECT operator_id FROM sessions WHERE session_id = ?1 AND expires_at > datetime('now')",
        params![session_id],
        |row| row.get::<_, i64>(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn delete_session(db: &Db, session_id: &str) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute("DELETE FROM sessions WHERE session_id = ?1", params![session_id])?;
    Ok(())
}

#[allow(dead_code)]
pub fn purge_expired_sessions(db: &Db) -> Result<usize> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let n = conn.execute("DELETE FROM sessions WHERE expires_at <= datetime('now')", [])?;
    Ok(n)
}

// ── CTFd probe ────────────────────────────────────────────────────────────────

/// Persists the probe result as the singleton row (`id = 1`) in `ctfd_probe`.
/// Replaces any existing row — there can only ever be one.
pub fn save_probe_result(db: &Db, result: &crate::ctfd_db::ProbeResult) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    // Vec<String> → comma-joined TEXT for storage; reconstructed on load.
    let challenges_cols = result.challenges_cols.join(",");
    let dynamic_cols = result.dynamic_cols.join(",");
    let probe_notes = serde_json::to_string(&result.probe_notes)
        .unwrap_or_else(|_| "[]".to_string());
    // Option<bool> → NULL / 1 / 0
    let is_team_mode: Option<i64> = result.is_team_mode.map(|b| if b { 1 } else { 0 });
    conn.execute(
        r#"INSERT OR REPLACE INTO ctfd_probe (
            id, probed_at, ctfd_version_tag, ctfd_version_source,
            is_team_mode, challenges_cols, has_dynamic_table, dynamic_cols,
            has_next_id, has_attribution, has_logic, has_position,
            dynamic_inline, dynamic_partial, has_instance_table,
            cap_challenge_crud, cap_dynamic_scoring, cap_player_auth,
            cap_instance_flags, cap_redis_sync, probe_notes
        ) VALUES (
            1, ?1, ?2, ?3,
            ?4, ?5, ?6, ?7,
            ?8, ?9, ?10, ?11,
            ?12, ?13, ?14,
            ?15, ?16, ?17,
            ?18, ?19, ?20
        )"#,
        params![
            result.probed_at,
            result.ctfd_version_tag,
            result.ctfd_version_source,
            is_team_mode,
            challenges_cols,
            result.has_dynamic_table as i64,
            dynamic_cols,
            result.has_next_id as i64,
            result.has_attribution as i64,
            result.has_logic as i64,
            result.has_position as i64,
            result.dynamic_inline as i64,
            result.dynamic_partial as i64,
            result.has_instance_table as i64,
            result.cap_challenge_crud,
            result.cap_dynamic_scoring,
            result.cap_player_auth,
            result.cap_instance_flags,
            result.cap_redis_sync,
            probe_notes,
        ],
    )?;
    Ok(())
}

/// Loads the singleton probe result from SQLite.
/// Returns `None` if no probe has been run yet (row is absent).
pub fn load_probe_result(db: &Db) -> Result<Option<crate::ctfd_db::ProbeResult>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let result = conn.query_row(
        "SELECT probed_at, ctfd_version_tag, ctfd_version_source,
                is_team_mode, challenges_cols, has_dynamic_table, dynamic_cols,
                has_next_id, has_attribution, has_logic, has_position,
                dynamic_inline, dynamic_partial, has_instance_table,
                cap_challenge_crud, cap_dynamic_scoring, cap_player_auth,
                cap_instance_flags, cap_redis_sync, probe_notes
         FROM ctfd_probe WHERE id = 1",
        [],
        |row| {
            // Reconstruct Vec<String> from comma-joined strings; empty string → empty Vec.
            let challenges_cols_raw: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
            let challenges_cols: Vec<String> = if challenges_cols_raw.is_empty() {
                Vec::new()
            } else {
                challenges_cols_raw.split(',').map(|s| s.to_string()).collect()
            };
            let dynamic_cols_raw: String = row.get::<_, Option<String>>(6)?.unwrap_or_default();
            let dynamic_cols: Vec<String> = if dynamic_cols_raw.is_empty() {
                Vec::new()
            } else {
                dynamic_cols_raw.split(',').map(|s| s.to_string()).collect()
            };
            // Reconstruct probe_notes from JSON; fall back to empty Vec on parse error.
            let probe_notes_raw: String = row.get::<_, String>(19)?;
            let probe_notes: Vec<String> = serde_json::from_str(&probe_notes_raw)
                .unwrap_or_default();
            // NULL / 1 / 0 → Option<bool>
            let is_team_mode: Option<bool> = row.get::<_, Option<i64>>(3)?
                .map(|v| v != 0);
            Ok(crate::ctfd_db::ProbeResult {
                probed_at: row.get(0)?,
                ctfd_version_tag: row.get(1)?,
                ctfd_version_source: row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "inferred".to_string()),
                is_team_mode,
                challenges_cols,
                has_dynamic_table: row.get::<_, i64>(5)? != 0,
                dynamic_cols,
                has_next_id: row.get::<_, i64>(7)? != 0,
                has_attribution: row.get::<_, i64>(8)? != 0,
                has_logic: row.get::<_, i64>(9)? != 0,
                has_position: row.get::<_, i64>(10)? != 0,
                dynamic_inline: row.get::<_, i64>(11)? != 0,
                dynamic_partial: row.get::<_, i64>(12)? != 0,
                has_instance_table: row.get::<_, i64>(13)? != 0,
                cap_challenge_crud: row.get::<_, Option<String>>(14)?.unwrap_or_else(|| "unknown".to_string()),
                cap_dynamic_scoring: row.get::<_, Option<String>>(15)?.unwrap_or_else(|| "unknown".to_string()),
                cap_player_auth: row.get::<_, Option<String>>(16)?.unwrap_or_else(|| "unknown".to_string()),
                cap_instance_flags: row.get::<_, Option<String>>(17)?.unwrap_or_else(|| "unknown".to_string()),
                cap_redis_sync: row.get::<_, Option<String>>(18)?.unwrap_or_else(|| "degraded".to_string()),
                probe_notes,
            })
        },
    );
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
