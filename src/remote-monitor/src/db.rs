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
            renewals_used   INTEGER DEFAULT 0,
            created_at      TEXT DEFAULT (datetime('now')),
            expires_at      TEXT NOT NULL,
            UNIQUE(challenge_name, team_id)
        );
        "#,
    )?;
    // Migration: add flag column to existing databases that predate the column.
    let _ = conn.execute("ALTER TABLE instances ADD COLUMN flag TEXT", []);
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
}

pub fn get_instance(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<InstanceRow>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT id, challenge_name, team_id, container_id, host, port, connection_type, status, renewals_used, expires_at
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
        }))
    } else {
        Ok(None)
    }
}

pub fn insert_instance(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    container_id: &str,
    host: &str,
    port: i64,
    connection_type: &str,
    expires_at: &str,
    flag: Option<&str>,
) -> Result<()> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    conn.execute(
        r#"INSERT INTO instances (challenge_name, team_id, container_id, host, port, connection_type, status, flag, expires_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8)
           ON CONFLICT(challenge_name, team_id) DO UPDATE SET
             container_id=excluded.container_id, host=excluded.host, port=excluded.port,
             connection_type=excluded.connection_type, status='running',
             flag=excluded.flag, expires_at=excluded.expires_at, renewals_used=0"#,
        params![challenge_name, team_id, container_id, host, port, connection_type, flag, expires_at],
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

pub fn delete_instance(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<String>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let container_id: Option<String> = conn.query_row(
        "SELECT container_id FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
        params![challenge_name, team_id],
        |row| row.get(0),
    ).ok().flatten();
    conn.execute(
        "DELETE FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
        params![challenge_name, team_id],
    )?;
    Ok(container_id)
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

/// Returns (challenge_name, container_id, team_id) for all expired running instances.
pub fn get_expired_instances(db: &Db) -> Result<Vec<(String, Option<String>, i64)>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT challenge_name, container_id, team_id FROM instances WHERE expires_at < datetime('now') AND status = 'running'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r?);
    }
    Ok(result)
}

/// Get the random flag stored for a team's instance (returns None if not set or no instance).
pub fn get_instance_flag(db: &Db, challenge_name: &str, team_id: i64) -> Result<Option<String>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let result = conn.query_row(
        "SELECT flag FROM instances WHERE challenge_name = ?1 AND team_id = ?2",
        params![challenge_name, team_id],
        |row| row.get::<_, Option<String>>(0),
    );
    match result {
        Ok(flag) => Ok(flag),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Delete all instances for a challenge (used when challenge is deleted).
/// Returns list of container_ids to clean up.
pub fn delete_all_instances_for_challenge(db: &Db, challenge_name: &str) -> Result<Vec<String>> {
    let conn = db.lock().map_err(|_| anyhow!("db lock poisoned"))?;
    let mut stmt = conn.prepare(
        "SELECT container_id FROM instances WHERE challenge_name = ?1 AND container_id IS NOT NULL",
    )?;
    let cids: Vec<String> = stmt
        .query_map(params![challenge_name], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    conn.execute(
        "DELETE FROM instances WHERE challenge_name = ?1",
        params![challenge_name],
    )?;
    Ok(cids)
}

