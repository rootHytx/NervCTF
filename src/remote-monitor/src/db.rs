//! SQLite state store for instance configs and active instances.

use anyhow::{anyhow, Result};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub type Db = Arc<Mutex<Connection>>;

pub fn open(path: &str) -> Result<Db> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
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
        "#,
    )?;
    // Migrations for existing databases.
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN flag TEXT", []);
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN ctfd_flag_id INTEGER", []);
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN user_id INTEGER", []);
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
    pub container_id: Option<String>,
    pub host: String,
    pub port: i64,
    pub connection_type: String,
    pub status: String,
    pub renewals_used: i64,
    pub expires_at: String,
    pub flag: Option<String>,
}

pub fn get_instance(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<InstanceRow>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT id, challenge_name, team_id, container_id, host, port, connection_type, status, renewals_used, expires_at, flag
         FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
    )?;
    let mut rows = stmt.query(params![challenge_name, team_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(InstanceRow {
            id: row.get(0)?,
            challenge_name: row.get(1)?,
            team_id: row.get(2)?,
            container_id: row.get(3)?,
            host: row.get(4)?,
            port: row.get(5)?,
            connection_type: row.get(6)?,
            status: row.get(7)?,
            renewals_used: row.get(8)?,
            expires_at: row.get(9)?,
            flag: row.get(10)?,
        }))
    } else {
        Ok(None)
    }
}

/// Insert a placeholder row with status='provisioning' so the info endpoint can
/// return status immediately while compose runs in the background.
/// Uses INSERT OR IGNORE so a concurrent retry doesn't clobber an existing row.
pub fn insert_provisioning_stub(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    user_id: Option<i64>,
    host: &str,
    connection_type: &str,
    expires_at: &str,
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        "INSERT OR IGNORE INTO instances (challenge_name, team_id, user_id, host, port, connection_type, status, expires_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, 'provisioning', ?6)",
        params![challenge_name, team_id, user_id, host, connection_type, expires_at],
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
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        r#"INSERT INTO instances (challenge_name, team_id, user_id, container_id, host, port, connection_type, status, flag, ctfd_flag_id, expires_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9, ?10)
           ON CONFLICT(challenge_name, team_id) DO UPDATE SET
             user_id=excluded.user_id, container_id=excluded.container_id,
             host=excluded.host, port=excluded.port,
             connection_type=excluded.connection_type, status='running',
             flag=excluded.flag, ctfd_flag_id=excluded.ctfd_flag_id,
             expires_at=excluded.expires_at, renewals_used=0"#,
        params![challenge_name, team_id, user_id, container_id, host, port, connection_type, flag, ctfd_flag_id, expires_at],
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
pub fn get_used_ports(db: &Db) -> Result<std::collections::HashSet<u16>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT port FROM instances WHERE status = 'running'",
    )?;
    let ports = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .filter_map(|r| r.ok())
        .filter_map(|p| u16::try_from(p).ok())
        .collect();
    Ok(ports)
}

/// Returns `(challenge_name, container_id, team_id, ctfd_flag_id)` for all expired running instances.
pub fn get_expired_instances(db: &Db) -> Result<Vec<(String, Option<String>, i64, Option<i64>)>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT challenge_name, container_id, team_id, ctfd_flag_id FROM instances WHERE expires_at < datetime('now') AND status = 'running'",
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
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT container_id, ctfd_flag_id FROM instances WHERE challenge_name = ?1",
    )?;
    let pairs: Vec<(Option<String>, Option<i64>)> = stmt
        .query_map(params![challenge_name], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    conn.execute(
        "DELETE FROM instances WHERE challenge_name = ?1",
        params![challenge_name],
    )?;
    // Purge incorrect, non-sharing attempts for all teams on this challenge.
    conn.execute(
        "DELETE FROM flag_attempts WHERE challenge_name = ?1 AND is_correct = 0 AND is_flag_sharing = 0",
        params![challenge_name],
    )?;
    Ok(pairs)
}
