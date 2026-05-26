//! Direct SQL access to CTFd's MariaDB database.
//! Replaces fragile CTFd REST API calls with stable SQL queries.

use anyhow::{anyhow, Result};
use mysql_async::{prelude::*, Pool};
use crate::db::Db;
use serde_json::{json, Value};
use tracing::{info, warn};

/// Build a connection pool for CTFd's MariaDB. `url` is a `mysql://` connection string.
pub fn create_pool(url: &str) -> Result<Pool> {
    let opts = mysql_async::Opts::from_url(url)
        .map_err(|e| anyhow!("Invalid CTFd DB URL '{}': {}", url, e))?;
    Ok(Pool::new(opts))
}

#[derive(Debug, Clone, PartialEq)]
pub enum CtfdMode {
    TeamMode,
    UserMode,
    Unknown,
}

/// Queries CTFd's `configs` table to detect team-mode vs user-mode.
/// Returns `Unknown` if the `configs` table is inaccessible or the key is absent.
pub async fn detect_ctfd_mode(pool: &mysql_async::Pool) -> CtfdMode {
    let mut conn = match pool.get_conn().await {
        Ok(c) => c,
        Err(_) => return CtfdMode::Unknown,
    };
    let row: Option<String> = conn
        .exec_first(
            "SELECT `value` FROM configs WHERE `key` = 'user_mode' LIMIT 1",
            (),
        )
        .await
        .unwrap_or(None);
    match row.as_deref() {
        Some("1") | Some("true") => CtfdMode::UserMode,
        Some("0") | Some("false") | Some("") | None => CtfdMode::TeamMode,
        _ => CtfdMode::Unknown,
    }
}

/// INSERT a static flag for a challenge. Returns the new flag id, or None on failure.
pub async fn create_flag(pool: &Pool, challenge_id: i64, content: &str) -> Option<i64> {
    let mut conn = match pool.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            warn!("ctfd_db: create_flag: connection error: {}", e);
            return None;
        }
    };
    let sql = "INSERT INTO flags (challenge_id, type, content, data) VALUES (?, 'static', ?, '')";
    match conn.exec_drop(sql, (challenge_id, content)).await {
        Ok(()) => {
            let id = conn.last_insert_id().unwrap_or(0) as i64;
            info!("ctfd_db: created flag {} for challenge {}", id, challenge_id);
            Some(id)
        }
        Err(e) => {
            warn!("ctfd_db: create_flag failed for challenge {}: {}", challenge_id, e);
            None
        }
    }
}

/// DELETE a flag by id. Returns an error if the operation fails.
pub async fn delete_flag(pool: &Pool, flag_id: i64) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_flag: connection error: {}", e))?;
    conn.exec_drop("DELETE FROM flags WHERE id = ?", (flag_id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_flag {}: {}", flag_id, e))?;
    info!("ctfd_db: deleted flag {}", flag_id);
    Ok(())
}

/// Validate a CTFd API token and return team_id, or None if invalid/banned/hidden/teamless.
pub async fn validate_token(pool: &Pool, token: &str) -> Option<i64> {
    let mut conn = match pool.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            warn!("ctfd_db: validate_token: connection error: {}", e);
            return None;
        }
    };
    let sql = "SELECT team_id FROM users WHERE token = ? AND banned = 0 AND hidden = 0 LIMIT 1";
    match conn.exec_first::<Option<i64>, _, _>(sql, (token,)).await {
        Ok(Some(team_id)) => team_id,
        Ok(None) => None,
        Err(e) => {
            warn!("ctfd_db: validate_token error: {}", e);
            None
        }
    }
}

/// Return all challenges as JSON values (for the diff endpoint).
pub async fn list_challenges(pool: &Pool) -> Result<Vec<Value>> {
    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| anyhow!("ctfd_db: list_challenges: connection error: {}", e))?;

    let rows: Vec<(i64, String, Option<String>, Option<String>, Option<i64>, String, String)> = conn
        .exec(
            "SELECT id, name, description, category, value, `type`, state FROM challenges",
            (),
        )
        .await
        .map_err(|e| anyhow!("ctfd_db: list_challenges query failed: {}", e))?;

    Ok(rows
        .into_iter()
        .map(|(id, name, description, category, value, r#type, state)| {
            json!({
                "id":          id,
                "name":        name,
                "description": description,
                "category":    category,
                "value":       value,
                "type":        r#type,
                "state":       state,
            })
        })
        .collect())
}

// ── Challenge CRUD (full SQL — replaces CTFd HTTP API) ───────────────────────

/// Check whether `nervctf_instance_challenge` exists (created by our CTFd plugin).
async fn has_instance_table(conn: &mut mysql_async::Conn) -> bool {
    conn.exec_first::<String, _, _>(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'nervctf_instance_challenge' LIMIT 1",
        (),
    ).await.ok().flatten().is_some()
}

/// Which optional columns exist in the `challenges` table (varies by CTFd version).
struct ChallengesSchema {
    has_attribution: bool,
    has_logic: bool,
    has_position: bool,
    /// true  = initial/minimum/decay/function are ALL inline in `challenges` (newer CTFd)
    /// false = they live in the separate `dynamic_challenge` join table (older CTFd),
    ///         OR the migration is partial (dynamic_partial=true).
    dynamic_in_challenges: bool,
    /// True when only *some* inline scoring columns are present — indicates a partial
    /// migration. Used by the probe system to report BROKEN dynamic scoring.
    dynamic_partial: bool,
    /// Whether `challenges.next_id` column exists (added in CTFd 3.5.x).
    has_next_id: bool,
    /// true = `dynamic_challenge` join table exists in the database.
    /// CTFd uses SQLAlchemy joined-table inheritance: the row must always be present.
    has_dynamic_table: bool,
    /// true = `dynamic_challenge` table has its own scoring columns (initial/minimum/decay/function).
    /// Older CTFd: scoring lives only in dynamic_challenge.
    /// Newer CTFd: scoring is inline in challenges; dynamic_challenge is a bare stub (id only).
    dynamic_table_has_scoring: bool,
}

async fn detect_challenges_schema(conn: &mut mysql_async::Conn) -> ChallengesSchema {
    let cols: Vec<String> = conn
        .exec(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'challenges'",
            (),
        )
        .await
        .unwrap_or_default();
    let col_set: std::collections::HashSet<String> = cols.into_iter().collect();
    let dyn_cols: Vec<String> = conn
        .exec(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'dynamic_challenge'",
            (),
        )
        .await
        .unwrap_or_default();
    let has_dynamic_table = !dyn_cols.is_empty();
    let dyn_col_set: std::collections::HashSet<String> = dyn_cols.into_iter().collect();
    // All four inline scoring columns must be present together; a partial migration
    // (e.g. only `initial` added) would cause the generated SELECT to reference
    // non-existent columns, producing runtime SQL errors or silent NULLs.
    let has_initial  = col_set.contains("initial");
    let has_minimum  = col_set.contains("minimum");
    let has_decay    = col_set.contains("decay");
    let has_function = col_set.contains("function");
    ChallengesSchema {
        has_attribution:          col_set.contains("attribution"),
        has_logic:                col_set.contains("logic"),
        has_position:             col_set.contains("position"),
        dynamic_in_challenges:    has_initial && has_minimum && has_decay && has_function,
        dynamic_partial:          has_initial && !(has_minimum && has_decay && has_function),
        has_next_id:              col_set.contains("next_id"),
        has_dynamic_table,
        dynamic_table_has_scoring: dyn_col_set.contains("initial"),
    }
}

/// Build the full-challenge SELECT query.
/// Column count and order always match `row_to_value` (NULL placeholders keep indices stable).
fn build_full_query(has_instance: bool, schema: &ChallengesSchema) -> String {
    // Optional newer columns — NULL placeholder keeps row_to_value indices stable
    let attr_col    = if schema.has_attribution { "c.attribution" } else { "NULL" };
    let logic_col   = if schema.has_logic       { "c.logic" }       else { "NULL" };
    let pos_col     = if schema.has_position    { "c.position" }    else { "NULL" };
    // NULL placeholder keeps col 10 (next_id) stable for row_to_value on CTFd <3.5.x
    let next_id_col = if schema.has_next_id     { "c.next_id" }     else { "NULL" };

    // Dynamic scoring: inline in challenges (newer) or LEFT JOIN dynamic_challenge (older)
    let (dyn_cols, dyn_join) = if schema.dynamic_in_challenges {
        (
            "c.initial, c.minimum, c.decay, c.`function`".to_string(),
            String::new(),
        )
    } else {
        (
            "d.initial, d.minimum, d.decay, d.`function`".to_string(),
            "LEFT JOIN dynamic_challenge d ON d.id = c.id".to_string(),
        )
    };

    let icols = if has_instance {
        "i.backend, i.image, i.command, i.compose_file, i.compose_service, \
         i.lxc_image, i.vagrantfile, \
         i.internal_port, i.connection, i.timeout_minutes, i.max_renewals, \
         i.flag_mode, i.flag_prefix, i.flag_suffix, i.random_flag_length, \
         i.initial_value, i.minimum_value, i.decay_value, i.decay_function"
    } else {
        "NULL, NULL, NULL, NULL, NULL, \
         NULL, NULL, \
         NULL, NULL, NULL, NULL, \
         NULL, NULL, NULL, NULL, \
         NULL, NULL, NULL, NULL"
    };
    let ijoin = if has_instance { "LEFT JOIN nervctf_instance_challenge i ON i.id = c.id" } else { "" };
    format!(
        "SELECT c.id, c.name, c.description, c.category, c.value, c.`type`, c.state, \
                c.max_attempts, c.connection_info, c.requirements, {next_id_col}, \
                {attr_col}, {logic_col}, {pos_col}, \
                {dyn_cols}, \
                {icols} \
         FROM challenges c {dyn_join} {ijoin}"
    )
}

fn row_to_value(row: &mysql_async::Row) -> Value {
    macro_rules! col_str {
        ($i:expr) => {
            row.get::<Option<String>, _>($i).unwrap_or(None)
        };
    }
    macro_rules! col_i64 {
        ($i:expr) => {
            row.get::<Option<i64>, _>($i).unwrap_or(None)
        };
    }

    let id: i64 = row.get(0).unwrap_or(0);
    let name: String = col_str!(1).unwrap_or_default();
    let description = col_str!(2);
    let category = col_str!(3);
    let value = col_i64!(4);
    let type_: String = col_str!(5).unwrap_or_else(|| "standard".to_string());
    let state: String = col_str!(6).unwrap_or_else(|| "hidden".to_string());
    let max_attempts = col_i64!(7).unwrap_or(0);
    let connection_info = col_str!(8);
    let requirements_str = col_str!(9);
    let next_id = col_i64!(10);

    // cols 11-13: new challenges columns
    let _attribution = col_str!(11);
    let _logic       = col_str!(12);
    let _position    = col_i64!(13);

    // cols 14-17: scoring (now directly in challenges)
    let d_initial  = col_i64!(14);
    let d_minimum  = col_i64!(15);
    let d_decay    = col_i64!(16);
    let d_function = col_str!(17);

    // cols 18-36: nervctf_instance_challenge
    let i_backend            = col_str!(18);
    let i_image              = col_str!(19);
    let i_command            = col_str!(20);
    let i_compose_file       = col_str!(21);
    let i_compose_service    = col_str!(22);
    let i_lxc_image          = col_str!(23);
    let i_vagrantfile        = col_str!(24);
    let i_internal_port      = col_i64!(25);
    let i_connection         = col_str!(26);
    let i_timeout_minutes    = col_i64!(27);
    let i_max_renewals       = col_i64!(28);
    let i_flag_mode          = col_str!(29);
    let i_flag_prefix        = col_str!(30);
    let i_flag_suffix        = col_str!(31);
    let i_random_flag_length = col_i64!(32);
    let i_initial_value      = col_i64!(33);
    let i_minimum_value      = col_i64!(34);
    let i_decay_value        = col_i64!(35);
    let i_decay_function     = col_str!(36);

    let requirements: Value = requirements_str
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);

    let mut v = json!({
        "id": id,
        "name": name,
        "description": description,
        "category": category,
        "value": value,
        "type": type_,
        "state": state,
        "max_attempts": max_attempts,
        "connection_info": connection_info,
        "requirements": requirements,
        "next_id": next_id,
    });

    if let Some(init) = d_initial {
        v["initial"] = json!(init);
        v["minimum"] = json!(d_minimum);
        v["decay"] = json!(d_decay);
        v["function"] = json!(d_function);
        v["extra"] = json!({"initial": init, "minimum": d_minimum, "decay": d_decay});
    }
    if let Some(ref backend) = i_backend {
        v["backend"] = json!(backend);
        v["image"] = json!(i_image);
        v["command"] = json!(i_command);
        v["compose_file"] = json!(i_compose_file);
        v["compose_service"] = json!(i_compose_service);
        v["lxc_image"] = json!(i_lxc_image);
        v["vagrantfile"] = json!(i_vagrantfile);
        v["internal_port"] = json!(i_internal_port);
        v["connection"] = json!(i_connection);
        v["timeout_minutes"] = json!(i_timeout_minutes);
        v["max_renewals"] = json!(i_max_renewals);
        v["flag_mode"] = json!(i_flag_mode);
        v["flag_prefix"] = json!(i_flag_prefix);
        v["flag_suffix"] = json!(i_flag_suffix);
        v["random_flag_length"] = json!(i_random_flag_length);
        if i_initial_value.is_some() {
            v["initial_value"] = json!(i_initial_value);
            v["minimum_value"] = json!(i_minimum_value);
            v["decay_value"] = json!(i_decay_value);
            v["decay_function"] = json!(i_decay_function);
            v["extra"] = json!({"initial": i_initial_value, "minimum": i_minimum_value, "decay": i_decay_value});
        }
    }
    v
}

pub async fn list_challenges_full(pool: &Pool) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_challenges_full: {}", e))?;
    let schema = detect_challenges_schema(&mut conn).await;
    let has_inst = has_instance_table(&mut conn).await;
    let query = build_full_query(has_inst, &schema);
    let rows: Vec<mysql_async::Row> = conn.exec(&query, ()).await
        .map_err(|e| anyhow!("ctfd_db: list_challenges_full query: {}", e))?;
    Ok(rows.iter().map(row_to_value).collect())
}

pub async fn get_challenge_full(pool: &Pool, id: i64) -> Result<Option<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: get_challenge_full: {}", e))?;
    let schema = detect_challenges_schema(&mut conn).await;
    let has_inst = has_instance_table(&mut conn).await;
    let query = format!("{} WHERE c.id = ?", build_full_query(has_inst, &schema));
    let rows: Vec<mysql_async::Row> = conn.exec(&query, (id,)).await
        .map_err(|e| anyhow!("ctfd_db: get_challenge_full query: {}", e))?;
    Ok(rows.first().map(row_to_value))
}

pub async fn create_challenge(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_challenge: {}", e))?;
    let schema = detect_challenges_schema(&mut conn).await;

    let name = body["name"].as_str().unwrap_or("").to_string();
    let category = body["category"].as_str().unwrap_or("").to_string();
    let description = body["description"].as_str().map(|s| s.to_string());
    let value = body["value"].as_i64().unwrap_or(0);
    let type_ = body["type"].as_str().unwrap_or("standard").to_string();
    let state = body["state"].as_str().unwrap_or("hidden").to_string();
    let max_attempts = body["max_attempts"].as_i64()
        .or_else(|| body["attempts"].as_i64())
        .unwrap_or(0);
    let connection_info = body["connection_info"].as_str().map(|s| s.to_string());
    let requirements: Option<String> = if body["requirements"].is_object() {
        serde_json::to_string(&body["requirements"]).ok()
    } else {
        None
    };
    let next_id: Option<i64> = body["next_id"].as_i64();

    let is_scored = type_ == "dynamic" || type_ == "instance";
    let initial  = if is_scored { body["initial"].as_i64().or_else(|| body["initial_value"].as_i64()) } else { None };
    let minimum  = if is_scored { body["minimum"].as_i64().or_else(|| body["minimum_value"].as_i64()) } else { None };
    let decay    = if is_scored { body["decay"].as_i64().or_else(|| body["decay_value"].as_i64()) } else { None };
    let function = if is_scored { body["function"].as_str().or_else(|| body["decay_function"].as_str()).map(|s| s.to_string()) } else { None };

    // Build INSERT dynamically: only include columns that exist in this CTFd version
    let mut col_names: Vec<&str> = vec![
        "name", "category", "description", "value", "`type`", "state",
        "max_attempts", "connection_info", "requirements",
    ];
    if schema.has_next_id { col_names.push("next_id"); }
    if schema.has_logic { col_names.push("logic"); }
    if schema.dynamic_in_challenges && is_scored {
        col_names.extend(["initial", "minimum", "decay", "`function`"]);
    }
    let placeholders: Vec<&str> = col_names.iter().map(|_| "?").collect();
    let sql = format!(
        "INSERT INTO challenges ({}) VALUES ({})",
        col_names.join(", "),
        placeholders.join(", "),
    );

    use mysql_async::prelude::ToValue;
    let logic = body["logic"].as_str().unwrap_or("").to_string();
    let mut params: Vec<mysql_async::Value> = vec![
        name.clone().to_value(), category.clone().to_value(), description.clone().to_value(),
        value.to_value(), type_.clone().to_value(), state.clone().to_value(),
        max_attempts.to_value(), connection_info.clone().to_value(),
        requirements.clone().to_value(),
    ];
    if schema.has_next_id { params.push(next_id.to_value()); }
    if schema.has_logic { params.push(logic.to_value()); }
    if schema.dynamic_in_challenges && is_scored {
        params.push(initial.to_value());
        params.push(minimum.to_value());
        params.push(decay.to_value());
        params.push(function.clone().to_value());
    }
    conn.exec_drop(sql, mysql_async::Params::Positional(params)).await
        .map_err(|e| anyhow!("ctfd_db: insert challenge: {}", e))?;

    let new_id = conn.last_insert_id().unwrap_or(0) as i64;

    // Insert into dynamic_challenge whenever the table exists — CTFd's SQLAlchemy
    // joined-table inheritance requires this row regardless of whether challenges.initial
    // also exists. Newer CTFd: dynamic_challenge is a bare stub (id only); scoring
    // is already inline in challenges. Older CTFd: table has its own scoring columns.
    if type_ == "dynamic" && schema.has_dynamic_table {
        if schema.dynamic_table_has_scoring {
            let di = initial.unwrap_or(value);
            let dm = minimum.unwrap_or(1);
            let dd = decay.unwrap_or(50);
            let df = function.clone().unwrap_or_else(|| "linear".to_string());
            conn.exec_drop(
                "INSERT INTO dynamic_challenge (id, initial, minimum, decay, `function`) VALUES (?, ?, ?, ?, ?)",
                (new_id, di, dm, dd, df),
            ).await.map_err(|e| anyhow!("ctfd_db: insert dynamic_challenge: {}", e))?;
        } else {
            conn.exec_drop(
                "INSERT INTO dynamic_challenge (id) VALUES (?)",
                (new_id,),
            ).await.map_err(|e| anyhow!("ctfd_db: insert dynamic_challenge (stub): {}", e))?;
        }
    }

    if type_ == "instance" {
        upsert_instance_row(&mut conn, new_id, body).await?;
    }

    info!("ctfd_db: created challenge '{}' id={}", name, new_id);
    Ok(json!({
        "id": new_id, "name": name, "category": category,
        "description": description, "value": value, "type": type_,
        "state": state, "max_attempts": max_attempts,
        "connection_info": connection_info,
    }))
}

pub async fn update_challenge(pool: &Pool, id: i64, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: update_challenge: {}", e))?;
    let schema = detect_challenges_schema(&mut conn).await;

    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<mysql_async::Value> = Vec::new();

    // Always-present string columns
    let mut string_fields: Vec<(&str, &str)> = vec![
        ("name", "name"), ("category", "category"), ("description", "description"),
        ("type", "`type`"), ("state", "state"), ("connection_info", "connection_info"),
    ];
    // Version-gated string columns
    if schema.has_logic       { string_fields.push(("logic", "logic")); }
    if schema.has_attribution { string_fields.push(("attribution", "attribution")); }
    // `function` only meaningful in challenges when inline scoring is present
    if schema.dynamic_in_challenges { string_fields.push(("function", "`function`")); }

    for (json_key, col) in &string_fields {
        if let Some(s) = body[*json_key].as_str() {
            sets.push(format!("{} = ?", col));
            params.push(mysql_async::Value::Bytes(s.as_bytes().to_vec()));
        }
    }
    if let Some(v) = body["value"].as_i64() {
        sets.push("value = ?".to_string());
        params.push(mysql_async::Value::Int(v));
    }
    if let Some(a) = body["max_attempts"].as_i64().or_else(|| body["attempts"].as_i64()) {
        sets.push("max_attempts = ?".to_string());
        params.push(mysql_async::Value::Int(a));
    }
    if schema.has_next_id {
        if let Some(n) = body["next_id"].as_i64() {
            sets.push("next_id = ?".to_string());
            params.push(mysql_async::Value::Int(n));
        }
    }
    if body["requirements"].is_object() {
        if let Ok(s) = serde_json::to_string(&body["requirements"]) {
            sets.push("requirements = ?".to_string());
            params.push(mysql_async::Value::Bytes(s.into_bytes()));
        }
    }
    // Inline dynamic scoring (newer CTFd only)
    if schema.dynamic_in_challenges {
        for (json_key, col) in &[("initial", "initial"), ("minimum", "minimum"), ("decay", "decay")] {
            if let Some(v) = body[*json_key].as_i64() {
                sets.push(format!("{} = ?", col));
                params.push(mysql_async::Value::Int(v));
            }
        }
    }

    if !sets.is_empty() {
        let query = format!("UPDATE challenges SET {} WHERE id = ?", sets.join(", "));
        params.push(mysql_async::Value::Int(id));
        conn.exec_drop(query, mysql_async::Params::Positional(params)).await
            .map_err(|e| anyhow!("ctfd_db: update challenge {}: {}", id, e))?;
    }

    let type_ = body["type"].as_str().unwrap_or("");
    // Upsert dynamic_challenge join row whenever the table exists.
    // ON DUPLICATE KEY UPDATE repairs existing challenges whose join row was previously missing.
    if type_ == "dynamic" && schema.has_dynamic_table {
        if schema.dynamic_table_has_scoring {
            let di = body["initial"].as_i64().unwrap_or(0);
            let dm = body["minimum"].as_i64().unwrap_or(1);
            let dd = body["decay"].as_i64().unwrap_or(50);
            let df = body["function"].as_str().unwrap_or("linear").to_string();
            conn.exec_drop(
                "INSERT INTO dynamic_challenge (id, initial, minimum, decay, `function`) VALUES (?, ?, ?, ?, ?) \
                 ON DUPLICATE KEY UPDATE initial=VALUES(initial), minimum=VALUES(minimum), \
                 decay=VALUES(decay), `function`=VALUES(`function`)",
                (id, di, dm, dd, df),
            ).await.map_err(|e| anyhow!("ctfd_db: upsert dynamic_challenge: {}", e))?;
        } else {
            conn.exec_drop(
                "INSERT IGNORE INTO dynamic_challenge (id) VALUES (?)",
                (id,),
            ).await.map_err(|e| anyhow!("ctfd_db: upsert dynamic_challenge (stub): {}", e))?;
        }
    }

    if type_ == "instance" || body["backend"].is_string() {
        upsert_instance_row(&mut conn, id, body).await?;
    }

    info!("ctfd_db: updated challenge {}", id);
    get_challenge_full(pool, id).await?.ok_or_else(|| anyhow!("challenge {} not found after update", id))
}

pub async fn delete_challenge(pool: &Pool, id: i64) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_challenge: {}", e))?;
    conn.exec_drop("DELETE FROM challenges WHERE id = ?", (id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_challenge {}: {}", id, e))?;
    info!("ctfd_db: deleted challenge {}", id);
    Ok(())
}

async fn upsert_instance_row(conn: &mut mysql_async::Conn, id: i64, body: &Value) -> Result<()> {
    let backend = body["backend"].as_str().unwrap_or("docker").to_string();
    let image = body["image"].as_str().unwrap_or("").to_string();
    let command = body["command"].as_str().unwrap_or("").to_string();
    let compose_file = body["compose_file"].as_str().unwrap_or("docker-compose.yml").to_string();
    let compose_service = body["compose_service"].as_str().unwrap_or("").to_string();
    let lxc_image = body["lxc_image"].as_str().unwrap_or("").to_string();
    let vagrantfile = body["vagrantfile"].as_str().unwrap_or("").to_string();
    let internal_port = body["internal_port"].as_i64().unwrap_or(1337);
    let connection = body["connection"].as_str().unwrap_or("nc").to_string();
    let timeout_minutes = body["timeout_minutes"].as_i64().unwrap_or(45);
    let max_renewals = body["max_renewals"].as_i64().unwrap_or(3);
    let flag_mode = body["flag_mode"].as_str().unwrap_or("static").to_string();
    let flag_prefix = body["flag_prefix"].as_str().unwrap_or("").to_string();
    let flag_suffix = body["flag_suffix"].as_str().unwrap_or("").to_string();
    let random_flag_length = body["random_flag_length"].as_i64().unwrap_or(16);
    let initial_value: Option<i64> = body["initial_value"].as_i64()
        .or_else(|| body["initial"].as_i64());
    let minimum_value: Option<i64> = body["minimum_value"].as_i64()
        .or_else(|| body["minimum"].as_i64());
    let decay_value: Option<i64> = body["decay_value"].as_i64()
        .or_else(|| body["decay"].as_i64());
    let decay_function: Option<String> = body["decay_function"].as_str()
        .or_else(|| body["function"].as_str())
        .map(|s| s.to_string());

    let sql4 = "INSERT INTO nervctf_instance_challenge
         (id, backend, image, command, compose_file, compose_service, lxc_image, vagrantfile,
          internal_port, connection, timeout_minutes, max_renewals,
          flag_mode, flag_prefix, flag_suffix, random_flag_length,
          initial_value, minimum_value, decay_value, decay_function)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE
          backend=VALUES(backend), image=VALUES(image), command=VALUES(command),
          compose_file=VALUES(compose_file), compose_service=VALUES(compose_service),
          lxc_image=VALUES(lxc_image), vagrantfile=VALUES(vagrantfile),
          internal_port=VALUES(internal_port), connection=VALUES(connection),
          timeout_minutes=VALUES(timeout_minutes), max_renewals=VALUES(max_renewals),
          flag_mode=VALUES(flag_mode), flag_prefix=VALUES(flag_prefix),
          flag_suffix=VALUES(flag_suffix), random_flag_length=VALUES(random_flag_length),
          initial_value=VALUES(initial_value), minimum_value=VALUES(minimum_value),
          decay_value=VALUES(decay_value), decay_function=VALUES(decay_function)";
    use mysql_async::prelude::ToValue;
    let params: Vec<mysql_async::Value> = vec![
        id.to_value(), backend.to_value(), image.to_value(), command.to_value(),
        compose_file.to_value(), compose_service.to_value(),
        lxc_image.to_value(), vagrantfile.to_value(),
        internal_port.to_value(), connection.to_value(),
        timeout_minutes.to_value(), max_renewals.to_value(),
        flag_mode.to_value(), flag_prefix.to_value(), flag_suffix.to_value(),
        random_flag_length.to_value(),
        initial_value.to_value(), minimum_value.to_value(),
        decay_value.to_value(), decay_function.to_value(),
    ];
    conn.exec_drop(sql4, params)
        .await.map_err(|e| anyhow!("ctfd_db: upsert instance row {}: {}", id, e))
}

// ── Flags (extended) ──────────────────────────────────────────────────────────

pub async fn list_flags(pool: &Pool, challenge_id: i64) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_flags: {}", e))?;
    let rows: Vec<(i64, i64, String, String, String)> = conn.exec(
        "SELECT id, challenge_id, `type`, content, data FROM flags WHERE challenge_id = ?",
        (challenge_id,),
    ).await.map_err(|e| anyhow!("ctfd_db: list_flags: {}", e))?;
    Ok(rows.into_iter().map(|(id, cid, type_, content, data)| json!({
        "id": id, "challenge_id": cid, "type": type_, "content": content, "data": data,
    })).collect())
}

pub async fn create_flag_full(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_flag_full: {}", e))?;
    let challenge_id = body["challenge_id"].as_i64()
        .ok_or_else(|| anyhow!("missing challenge_id"))?;
    let type_ = body["type"].as_str().unwrap_or("static").to_string();
    let content = body["content"].as_str().unwrap_or("").to_string();
    let data = if body["data"].is_string() {
        body["data"].as_str().unwrap_or("").to_string()
    } else {
        body["data"].as_object().map(|_| serde_json::to_string(&body["data"]).unwrap_or_default()).unwrap_or_default()
    };
    let sql = "INSERT INTO flags (challenge_id, `type`, content, data) VALUES (?, ?, ?, ?)";
    conn.exec_drop(sql, (challenge_id, type_.clone(), content.clone(), data.clone())).await
        .map_err(|e| anyhow!("ctfd_db: create_flag_full: {}", e))?;
    let id = conn.last_insert_id().unwrap_or(0) as i64;
    Ok(json!({"id": id, "challenge_id": challenge_id, "type": type_, "content": content, "data": data}))
}

/// Alias kept for call sites that use the `_by_id` name; delegates to `delete_flag`.
#[inline]
pub async fn delete_flag_by_id(pool: &Pool, id: i64) -> Result<()> {
    delete_flag(pool, id).await
}

// ── Hints ─────────────────────────────────────────────────────────────────────

pub async fn list_hints(pool: &Pool, challenge_id: i64) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_hints: {}", e))?;
    let rows: Vec<(i64, i64, String, i64)> = conn.exec(
        "SELECT id, challenge_id, content, cost FROM hints WHERE challenge_id = ?",
        (challenge_id,),
    ).await.map_err(|e| anyhow!("ctfd_db: list_hints: {}", e))?;
    Ok(rows.into_iter().map(|(id, cid, content, cost)| json!({
        "id": id, "challenge_id": cid, "content": content, "cost": cost,
    })).collect())
}

pub async fn create_hint(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_hint: {}", e))?;
    let challenge_id = body["challenge_id"].as_i64()
        .ok_or_else(|| anyhow!("missing challenge_id"))?;
    let content = body["content"].as_str().unwrap_or("").to_string();
    let cost = body["cost"].as_i64().unwrap_or(0);
    let sql = "INSERT INTO hints (challenge_id, content, cost, `type`) VALUES (?, ?, ?, 'standard')";
    conn.exec_drop(sql, (challenge_id, content.clone(), cost)).await
        .map_err(|e| anyhow!("ctfd_db: create_hint: {}", e))?;
    let id = conn.last_insert_id().unwrap_or(0) as i64;
    Ok(json!({"id": id, "challenge_id": challenge_id, "content": content, "cost": cost}))
}

pub async fn delete_hint(pool: &Pool, id: i64) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_hint: {}", e))?;
    conn.exec_drop("DELETE FROM hints WHERE id = ?", (id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_hint {}: {}", id, e))
}

// ── Tags ──────────────────────────────────────────────────────────────────────

pub async fn list_tags(pool: &Pool, challenge_id: i64) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_tags: {}", e))?;
    let rows: Vec<(i64, i64, String)> = conn.exec(
        "SELECT id, challenge_id, value FROM tags WHERE challenge_id = ?",
        (challenge_id,),
    ).await.map_err(|e| anyhow!("ctfd_db: list_tags: {}", e))?;
    Ok(rows.into_iter().map(|(id, cid, value)| json!({
        "id": id, "challenge_id": cid, "value": value,
    })).collect())
}

pub async fn create_tag(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_tag: {}", e))?;
    let challenge_id = body["challenge_id"].as_i64()
        .ok_or_else(|| anyhow!("missing challenge_id"))?;
    let value = body["value"].as_str().unwrap_or("").to_string();
    conn.exec_drop(
        "INSERT INTO tags (challenge_id, value) VALUES (?, ?)",
        (challenge_id, value.clone()),
    ).await.map_err(|e| anyhow!("ctfd_db: create_tag: {}", e))?;
    let id = conn.last_insert_id().unwrap_or(0) as i64;
    Ok(json!({"id": id, "challenge_id": challenge_id, "value": value}))
}

pub async fn delete_tag(pool: &Pool, id: i64) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_tag: {}", e))?;
    conn.exec_drop("DELETE FROM tags WHERE id = ?", (id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_tag {}: {}", id, e))
}

// ── Files ─────────────────────────────────────────────────────────────────────

pub async fn list_files(pool: &Pool, challenge_id: i64) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_files: {}", e))?;
    let rows: Vec<(i64, i64, String, String)> = conn.exec(
        "SELECT id, challenge_id, `type`, location FROM files WHERE challenge_id = ?",
        (challenge_id,),
    ).await.map_err(|e| anyhow!("ctfd_db: list_files: {}", e))?;
    Ok(rows.into_iter().map(|(id, cid, type_, location)| json!({
        "id": id, "challenge_id": cid, "type": type_, "location": location,
    })).collect())
}

pub async fn create_file_record(pool: &Pool, challenge_id: i64, file_type: &str, location: &str) -> Result<i64> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_file_record: {}", e))?;
    conn.exec_drop(
        "INSERT INTO files (challenge_id, `type`, location) VALUES (?, ?, ?)",
        (challenge_id, file_type, location),
    ).await.map_err(|e| anyhow!("ctfd_db: create_file_record: {}", e))?;
    Ok(conn.last_insert_id().unwrap_or(0) as i64)
}

pub async fn delete_file_record(pool: &Pool, id: i64) -> Result<Option<String>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_file_record: {}", e))?;
    let location: Option<String> = conn.exec_first(
        "SELECT location FROM files WHERE id = ?", (id,),
    ).await.map_err(|e| anyhow!("ctfd_db: delete_file_record select: {}", e))?;
    conn.exec_drop("DELETE FROM files WHERE id = ?", (id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_file_record delete: {}", e))?;
    Ok(location)
}

// ── Topics ────────────────────────────────────────────────────────────────────

pub async fn create_topic(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_topic: {}", e))?;
    let challenge_id = body["challenge_id"].as_i64()
        .ok_or_else(|| anyhow!("missing challenge_id"))?;
    let value = body["value"].as_str().unwrap_or("").to_string();

    conn.exec_drop("INSERT IGNORE INTO topics (value) VALUES (?)", (value.clone(),)).await
        .map_err(|e| anyhow!("ctfd_db: create_topic insert topics: {}", e))?;

    let topic_id: i64 = conn.exec_first(
        "SELECT id FROM topics WHERE value = ? LIMIT 1", (value.clone(),),
    ).await.map_err(|e| anyhow!("ctfd_db: create_topic select: {}", e))?
    .ok_or_else(|| anyhow!("topic not found after insert"))?;

    conn.exec_drop(
        "INSERT IGNORE INTO challenge_topics (challenge_id, topic_id) VALUES (?, ?)",
        (challenge_id, topic_id),
    ).await.map_err(|e| anyhow!("ctfd_db: create_topic link: {}", e))?;

    Ok(json!({"id": topic_id, "challenge_id": challenge_id, "value": value}))
}

pub async fn list_topics(pool: &Pool, challenge_id: i64) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_topics: {}", e))?;
    let rows: Vec<(i64, String)> = conn.exec(
        "SELECT t.id, t.value FROM topics t \
         JOIN challenge_topics ct ON ct.topic_id = t.id \
         WHERE ct.challenge_id = ?",
        (challenge_id,),
    ).await.map_err(|e| anyhow!("ctfd_db: list_topics query: {}", e))?;
    Ok(rows.into_iter().map(|(id, value)| json!({"id": id, "challenge_id": challenge_id, "value": value})).collect())
}

pub async fn delete_topic(pool: &Pool, topic_id: i64) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_topic: {}", e))?;
    conn.exec_drop("DELETE FROM challenge_topics WHERE topic_id = ?", (topic_id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_topic: {}", e))
}

// ── Compatibility probe ───────────────────────────────────────────────────────

/// Full schema fingerprint and capability status for the connected CTFd instance.
/// Produced by [`run_probe`] and persisted in the `ctfd_probe` SQLite table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProbeResult {
    /// UTC timestamp when the probe was run, formatted as `YYYY-MM-DD HH:MM:SS UTC`.
    pub probed_at: String,
    /// CTFd version string from the `configs` table, or `None` if absent/inaccessible.
    pub ctfd_version_tag: Option<String>,
    /// How the version was obtained: `"configs_table"` or `"inferred"`.
    pub ctfd_version_source: String,
    /// `Some(true)` = team-mode, `Some(false)` = user-mode, `None` = could not determine.
    pub is_team_mode: Option<bool>,
    /// All column names present in the `challenges` table, sorted alphabetically.
    pub challenges_cols: Vec<String>,
    /// Whether the `dynamic_challenge` join table exists.
    pub has_dynamic_table: bool,
    /// All column names present in `dynamic_challenge`, sorted alphabetically.
    /// Empty when the table does not exist.
    pub dynamic_cols: Vec<String>,
    /// Whether `challenges.next_id` exists (added CTFd 3.5.x).
    pub has_next_id: bool,
    /// Whether `challenges.attribution` exists (added CTFd 3.7.0).
    pub has_attribution: bool,
    /// Whether `challenges.logic` exists (added CTFd 3.7.x).
    pub has_logic: bool,
    /// Whether `challenges.position` exists.
    pub has_position: bool,
    /// Whether all four inline scoring columns (`initial/minimum/decay/function`) are
    /// present in `challenges` (newer CTFd; replaces the `dynamic_challenge` join table
    /// for scoring).
    pub dynamic_inline: bool,
    /// Whether only a *partial* set of inline scoring columns is present — indicates a
    /// broken schema migration.
    pub dynamic_partial: bool,
    /// Whether the NervCTF plugin table `nervctf_instance_challenge` exists.
    pub has_instance_table: bool,
    /// `"ok"` | `"degraded"` | `"broken"` — challenge CRUD capability.
    pub cap_challenge_crud: String,
    /// `"ok"` | `"degraded"` | `"broken"` — dynamic scoring capability.
    pub cap_dynamic_scoring: String,
    /// `"ok"` | `"degraded"` | `"broken"` — player token → team_id auth capability.
    pub cap_player_auth: String,
    /// `"ok"` | `"degraded"` | `"broken"` — per-instance flag capability.
    pub cap_instance_flags: String,
    /// Always `"degraded"` — direct MariaDB writes bypass CTFd's Redis cache.
    pub cap_redis_sync: String,
    /// Human-readable warnings for each degraded or broken capability.
    pub probe_notes: Vec<String>,
}

/// Format a Unix timestamp (seconds) as `"YYYY-MM-DD HH:MM:SS UTC"` without
/// pulling in the `chrono` crate.
fn format_utc_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Reuse the same Gregorian computation that instance::expires_at_string uses,
    // inlined here to avoid a cross-module dependency on a private function.
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mon = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mon <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, mon, d, h, m, s)
}

/// Queries the connected CTFd MariaDB to build a full schema fingerprint and
/// capability status report. Runs at monitor startup and on demand via the probe
/// endpoint. Never fails — falls back to `"unknown"` / empty fields on errors so
/// a single inaccessible table does not abort the probe.
pub async fn run_probe(pool: &mysql_async::Pool) -> ProbeResult {
    let mut conn = match pool.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            // Cannot connect at all — return a minimal broken result.
            warn!("ctfd_probe: cannot connect to MariaDB: {}", e);
            return ProbeResult {
                probed_at: format_utc_now(),
                ctfd_version_tag: None,
                ctfd_version_source: "inferred".to_string(),
                is_team_mode: None,
                challenges_cols: Vec::new(),
                has_dynamic_table: false,
                dynamic_cols: Vec::new(),
                has_next_id: false,
                has_attribution: false,
                has_logic: false,
                has_position: false,
                dynamic_inline: false,
                dynamic_partial: false,
                has_instance_table: false,
                cap_challenge_crud: "broken".to_string(),
                cap_dynamic_scoring: "broken".to_string(),
                cap_player_auth: "broken".to_string(),
                cap_instance_flags: "broken".to_string(),
                cap_redis_sync: "degraded".to_string(),
                probe_notes: vec![
                    format!("Cannot connect to CTFd MariaDB: {}", e),
                ],
            };
        }
    };

    // Step 1: detect schema via the existing helper.
    let schema = detect_challenges_schema(&mut conn).await;

    // Step 2: query the full column list for challenges (sorted) for the probe record.
    let mut challenges_cols: Vec<String> = conn
        .exec(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'challenges' \
             ORDER BY COLUMN_NAME",
            (),
        )
        .await
        .unwrap_or_default();
    challenges_cols.sort();

    // Step 3: full column list for dynamic_challenge (sorted).
    let mut dynamic_cols: Vec<String> = conn
        .exec(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'dynamic_challenge' \
             ORDER BY COLUMN_NAME",
            (),
        )
        .await
        .unwrap_or_default();
    dynamic_cols.sort();

    // Step 4: detect CTFd version from the configs table.
    let (ctfd_version_tag, ctfd_version_source) = {
        let row: Option<String> = conn
            .exec_first(
                "SELECT `value` FROM configs WHERE `key` = 'ctf_version' LIMIT 1",
                (),
            )
            .await
            .unwrap_or(None);
        match row {
            Some(v) if !v.is_empty() => (Some(v), "configs_table".to_string()),
            // Key exists but is empty — treat as absent.
            Some(_) => (None, "inferred".to_string()),
            None => (None, "inferred".to_string()),
        }
    };

    // Step 5: detect team/user mode via the existing helper.
    // Re-use the module-level function; it acquires its own connection from the pool.
    let ctfd_mode = detect_ctfd_mode(pool).await;
    let is_team_mode: Option<bool> = match ctfd_mode {
        CtfdMode::TeamMode => Some(true),
        CtfdMode::UserMode => Some(false),
        CtfdMode::Unknown => None,
    };

    // Step 6: check for the NervCTF plugin table.
    let has_inst = has_instance_table(&mut conn).await;

    // Step 7: compute capability statuses (deterministic rules).
    let cap_challenge_crud = if schema.has_next_id {
        "ok"
    } else {
        "degraded"
    }.to_string();

    let cap_dynamic_scoring = if schema.dynamic_partial {
        "broken"
    } else if schema.dynamic_in_challenges || (schema.has_dynamic_table && !schema.dynamic_partial) {
        "ok"
    } else {
        // No inline scoring and no dynamic table at all — unexpected but not broken per se.
        "degraded"
    }.to_string();

    let cap_player_auth = match is_team_mode {
        Some(true) => "ok",
        Some(false) => "broken",
        None => "degraded",
    }.to_string();

    let cap_instance_flags = if has_inst { "ok" } else { "degraded" }.to_string();

    // Redis sync is always degraded — direct MariaDB writes always bypass the cache.
    let cap_redis_sync = "degraded".to_string();

    // Step 8: collect human-readable probe_notes for each non-ok capability.
    let mut probe_notes: Vec<String> = Vec::new();
    if cap_challenge_crud == "degraded" {
        probe_notes.push(
            "challenges.next_id column absent — next_id field will be silently ignored (CTFd < 3.5.x)".to_string(),
        );
    }
    if cap_dynamic_scoring == "broken" {
        probe_notes.push(
            "Partial inline scoring migration detected — deploy of dynamic challenges will fail with SQL errors".to_string(),
        );
    }
    if cap_player_auth == "broken" {
        probe_notes.push(
            "CTFd is in user-mode — all player instance requests will return 403".to_string(),
        );
    }
    if cap_player_auth == "degraded" {
        probe_notes.push(
            "CTFd mode could not be determined — player auth may fail if user-mode is active".to_string(),
        );
    }
    if cap_instance_flags == "degraded" {
        probe_notes.push(
            "nervctf_instance_challenge table absent — plugin not installed; instance challenges will fail".to_string(),
        );
    }
    // Redis sync note is always present.
    probe_notes.push(
        "Direct MariaDB writes bypass CTFd Redis cache — stale data may be served until CTFd restart or TTL expiry".to_string(),
    );

    ProbeResult {
        probed_at: format_utc_now(),
        ctfd_version_tag,
        ctfd_version_source,
        is_team_mode,
        challenges_cols,
        has_dynamic_table: schema.has_dynamic_table,
        dynamic_cols,
        has_next_id: schema.has_next_id,
        has_attribution: schema.has_attribution,
        has_logic: schema.has_logic,
        has_position: schema.has_position,
        dynamic_inline: schema.dynamic_in_challenges,
        dynamic_partial: schema.dynamic_partial,
        has_instance_table: has_inst,
        cap_challenge_crud,
        cap_dynamic_scoring,
        cap_player_auth,
        cap_instance_flags,
        cap_redis_sync,
        probe_notes,
    }
}

// ── Read-only sync from CTFd submissions ──────────────────────────────────────

/// Sync correct solves from CTFd's `submissions` table into the monitor's local
/// `ctfd_solves` SQLite cache.  Performs a full replace so deleted submissions
/// are also removed from the cache.  Read-only against MariaDB.
///
/// Called by the background sync task every `CTFD_DB_SYNC_INTERVAL` seconds.
pub async fn sync_solves(pool: &Pool, db: &Db) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: sync_solves: get_conn: {}", e))?;

    let rows: Vec<(i64, Option<i64>, String, String)> = conn.exec(
        "SELECT s.team_id, s.user_id, c.name, DATE_FORMAT(s.date, '%Y-%m-%d %H:%i:%S') \
         FROM submissions s \
         JOIN challenges c ON c.id = s.challenge_id \
         WHERE s.type = 'correct' AND s.team_id IS NOT NULL",
        (),
    ).await.map_err(|e| anyhow!("ctfd_db: sync_solves: query: {}", e))?;

    let n = rows.len();
    crate::db::replace_ctfd_solves(db, &rows)
        .map_err(|e| anyhow!("ctfd_db: sync_solves: sqlite: {}", e))?;
    let reverted = crate::db::revert_unsolved_instances(db)
        .map_err(|e| anyhow!("ctfd_db: sync_solves: revert: {}", e))?;
    if reverted > 0 {
        tracing::info!("ctfd_db: sync_solves: reverted {} instance(s) to running (solve deleted in CTFd)", reverted);
    }
    let stale = crate::db::delete_stale_correct_attempts(db)
        .map_err(|e| anyhow!("ctfd_db: sync_solves: delete_stale_attempts: {}", e))?;
    if stale > 0 {
        tracing::info!("ctfd_db: sync_solves: removed {} stale correct flag attempt(s) (submission deleted in CTFd)", stale);
    }
    tracing::debug!("ctfd_db: sync_solves: replaced with {} rows", n);
    Ok(())
}

/// Sync teams and users from CTFd's MariaDB into the local name cache.
/// Performs a full replace so renames and deletions are picked up.
/// Read-only against MariaDB.
pub async fn sync_users_and_teams(pool: &Pool, db: &Db) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: sync_users_and_teams: get_conn: {}", e))?;

    let teams: Vec<(i64, String)> = conn.exec(
        "SELECT id, name FROM teams",
        (),
    ).await.map_err(|e| anyhow!("ctfd_db: sync_users_and_teams: teams query: {}", e))?;

    let users: Vec<(i64, String, Option<i64>)> = conn.exec(
        "SELECT id, name, team_id FROM users",
        (),
    ).await.map_err(|e| anyhow!("ctfd_db: sync_users_and_teams: users query: {}", e))?;

    let (nt, nu) = (teams.len(), users.len());
    crate::db::replace_ctfd_teams_and_users(db, &teams, &users)
        .map_err(|e| anyhow!("ctfd_db: sync_users_and_teams: sqlite: {}", e))?;
    tracing::debug!("ctfd_db: sync_users_and_teams: {} teams, {} users", nt, nu);
    Ok(())
}

