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

/// DELETE a flag by id. Errors are logged and swallowed.
pub async fn delete_flag(pool: &Pool, flag_id: i64) {
    let mut conn = match pool.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            warn!("ctfd_db: delete_flag: connection error: {}", e);
            return;
        }
    };
    match conn.exec_drop("DELETE FROM flags WHERE id = ?", (flag_id,)).await {
        Ok(()) => info!("ctfd_db: deleted flag {}", flag_id),
        Err(e) => warn!("ctfd_db: delete_flag {} failed: {}", flag_id, e),
    }
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

/// Build the full-challenge SELECT query.
/// `initial/minimum/decay/function` live directly in `challenges` in this CTFd version.
/// Column count and order always match `row_to_value` (NULL placeholders keep indices stable).
fn build_full_query(has_instance: bool) -> String {
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
                c.max_attempts, c.connection_info, c.requirements, c.next_id, \
                c.attribution, c.logic, c.position, \
                c.initial, c.minimum, c.decay, c.`function`, \
                {icols} \
         FROM challenges c {ijoin}"
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
    let query = build_full_query(has_instance_table(&mut conn).await);
    let rows: Vec<mysql_async::Row> = conn.exec(&query, ()).await
        .map_err(|e| anyhow!("ctfd_db: list_challenges_full query: {}", e))?;
    Ok(rows.iter().map(row_to_value).collect())
}

pub async fn get_challenge_full(pool: &Pool, id: i64) -> Result<Option<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: get_challenge_full: {}", e))?;
    let query = format!("{} WHERE c.id = ?", build_full_query(has_instance_table(&mut conn).await));
    let rows: Vec<mysql_async::Row> = conn.exec(&query, (id,)).await
        .map_err(|e| anyhow!("ctfd_db: get_challenge_full query: {}", e))?;
    Ok(rows.first().map(row_to_value))
}

pub async fn create_challenge(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_challenge: {}", e))?;

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
    let logic = body["logic"].as_str().unwrap_or("").to_string();

    // initial/minimum/decay/function live directly in challenges in this CTFd version
    let (initial, minimum, decay, function): (Option<i64>, Option<i64>, Option<i64>, Option<String>) =
        if type_ == "dynamic" || type_ == "instance" {
            let i = body["initial"].as_i64().or_else(|| body["initial_value"].as_i64());
            let mn = body["minimum"].as_i64().or_else(|| body["minimum_value"].as_i64());
            let d = body["decay"].as_i64().or_else(|| body["decay_value"].as_i64());
            let f = body["function"].as_str().or_else(|| body["decay_function"].as_str()).map(|s| s.to_string());
            (i, mn, d, f)
        } else {
            (None, None, None, None)
        };

    let sql = "INSERT INTO challenges \
        (name, category, description, value, `type`, state, max_attempts, \
         connection_info, requirements, next_id, logic, initial, minimum, decay, `function`) \
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    use mysql_async::prelude::ToValue;
    conn.exec_drop(sql, mysql_async::Params::Positional(vec![
        name.clone().to_value(), category.clone().to_value(), description.clone().to_value(),
        value.to_value(), type_.clone().to_value(), state.clone().to_value(),
        max_attempts.to_value(), connection_info.clone().to_value(),
        requirements.clone().to_value(), next_id.to_value(),
        logic.to_value(), initial.to_value(), minimum.to_value(), decay.to_value(),
        function.to_value(),
    ])).await.map_err(|e| anyhow!("ctfd_db: insert challenge: {}", e))?;

    let new_id = conn.last_insert_id().unwrap_or(0) as i64;

    if type_ == "dynamic" {
        let di = body["initial"].as_i64().unwrap_or(value);
        let dm = body["minimum"].as_i64().unwrap_or(1);
        let dd = body["decay"].as_i64().unwrap_or(50);
        let df = body["function"].as_str().unwrap_or("linear").to_string();
        let sql2 = "INSERT INTO dynamic_challenge \
            (id, dynamic_initial, dynamic_minimum, dynamic_decay, dynamic_function) \
            VALUES (?, ?, ?, ?, ?)";
        conn.exec_drop(sql2, (new_id, di, dm, dd, df)).await
            .map_err(|e| anyhow!("ctfd_db: insert dynamic_challenge: {}", e))?;
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

    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<mysql_async::Value> = Vec::new();

    let string_fields = [
        ("name", "name"), ("category", "category"), ("description", "description"),
        ("type", "`type`"), ("state", "state"), ("connection_info", "connection_info"),
        ("logic", "logic"), ("attribution", "attribution"),
        ("function", "`function`"),
    ];
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
    if let Some(n) = body["next_id"].as_i64() {
        sets.push("next_id = ?".to_string());
        params.push(mysql_async::Value::Int(n));
    }
    if body["requirements"].is_object() {
        if let Ok(s) = serde_json::to_string(&body["requirements"]) {
            sets.push("requirements = ?".to_string());
            params.push(mysql_async::Value::Bytes(s.into_bytes()));
        }
    }

    // Update initial/minimum/decay directly in challenges (they live there in this CTFd version)
    for (json_key, col) in &[("initial", "initial"), ("minimum", "minimum"), ("decay", "decay")] {
        if let Some(v) = body[*json_key].as_i64() {
            sets.push(format!("{} = ?", col));
            params.push(mysql_async::Value::Int(v));
        }
    }

    if !sets.is_empty() {
        let query = format!("UPDATE challenges SET {} WHERE id = ?", sets.join(", "));
        params.push(mysql_async::Value::Int(id));
        conn.exec_drop(query, mysql_async::Params::Positional(params)).await
            .map_err(|e| anyhow!("ctfd_db: update challenge {}: {}", id, e))?;
    }

    let type_ = body["type"].as_str().unwrap_or("");
    if type_ == "dynamic" || body["initial"].is_i64() || body["decay"].is_i64() {
        let di = body["initial"].as_i64().unwrap_or(0);
        let dm = body["minimum"].as_i64().unwrap_or(1);
        let dd = body["decay"].as_i64().unwrap_or(50);
        let df = body["function"].as_str().unwrap_or("linear").to_string();
        let sql3 = "INSERT INTO dynamic_challenge \
             (id, dynamic_initial, dynamic_minimum, dynamic_decay, dynamic_function) \
             VALUES (?, ?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE \
             dynamic_initial=VALUES(dynamic_initial), dynamic_minimum=VALUES(dynamic_minimum), \
             dynamic_decay=VALUES(dynamic_decay), dynamic_function=VALUES(dynamic_function)";
        conn.exec_drop(sql3, (id, di, dm, dd, df)).await
            .map_err(|e| anyhow!("ctfd_db: upsert dynamic_challenge: {}", e))?;
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

pub async fn delete_flag_by_id(pool: &Pool, id: i64) -> Result<()> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: delete_flag_by_id: {}", e))?;
    conn.exec_drop("DELETE FROM flags WHERE id = ?", (id,)).await
        .map_err(|e| anyhow!("ctfd_db: delete_flag_by_id {}: {}", id, e))
}

// ── Hints ─────────────────────────────────────────────────────────────────────

pub async fn list_hints(pool: &Pool, challenge_id: i64) -> Result<Vec<Value>> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: list_hints: {}", e))?;
    let rows: Vec<(i64, i64, String, i64, Option<String>)> = conn.exec(
        "SELECT id, challenge_id, content, cost, title FROM hints WHERE challenge_id = ?",
        (challenge_id,),
    ).await.map_err(|e| anyhow!("ctfd_db: list_hints: {}", e))?;
    Ok(rows.into_iter().map(|(id, cid, content, cost, title)| json!({
        "id": id, "challenge_id": cid, "content": content, "cost": cost, "title": title,
    })).collect())
}

pub async fn create_hint(pool: &Pool, body: &Value) -> Result<Value> {
    let mut conn = pool.get_conn().await
        .map_err(|e| anyhow!("ctfd_db: create_hint: {}", e))?;
    let challenge_id = body["challenge_id"].as_i64()
        .ok_or_else(|| anyhow!("missing challenge_id"))?;
    let content = body["content"].as_str().unwrap_or("").to_string();
    let cost = body["cost"].as_i64().unwrap_or(0);
    let title: Option<String> = body["title"].as_str().map(|s| s.to_string());
    let sql = "INSERT INTO hints (challenge_id, content, cost, `type`, title) VALUES (?, ?, ?, 'standard', ?)";
    conn.exec_drop(sql, (challenge_id, content.clone(), cost, title.clone())).await
        .map_err(|e| anyhow!("ctfd_db: create_hint: {}", e))?;
    let id = conn.last_insert_id().unwrap_or(0) as i64;
    Ok(json!({"id": id, "challenge_id": challenge_id, "content": content, "cost": cost, "title": title}))
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

