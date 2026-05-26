# Remote Monitor

The `remote-monitor` is an HTTP server that runs on the CTFd host. It:

1. **Manages** all CTFd data directly via MariaDB SQL — no HTTP proxy, no `CTFD_API_KEY`
2. **Manages** ephemeral challenge instances (containers/VMs) per team
3. **Serves** a player-facing HTML UI for instance lifecycle

Deployed automatically by `nervctf setup`.

---

## Architecture

```
CLI  ──Token<monitor>──▶  remote-monitor:33133  ──SQL──▶  CTFd MariaDB
                                │                    └──▶  CTFd uploads dir (files)
                     instance manager
               ┌───────────────┴────────────────┐
          single-machine               split-machine
         (local docker daemon)   (SSH to runner node)
```

The monitor runs as a Docker container inside the same Compose stack as CTFd.
`nervctf setup` writes a `docker-compose.override.yml` that wires it in:

```yaml
# docker-compose.override.yml — single-machine mode (no runner_ip)
services:
  remote-monitor:
    image: nervctf-monitor:latest
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock        # local Docker daemon
      - /usr/libexec/docker/cli-plugins:/usr/libexec/docker/cli-plugins:ro
      - <ctfd_path>/remote-monitor/data:<ctfd_path>/remote-monitor/data
      - <ctfd_path>/.data/CTFd/uploads:<ctfd_path>/.data/CTFd/uploads

# docker-compose.override.yml — split-machine mode (runner_ip set)
services:
  remote-monitor:
    image: nervctf-monitor:latest
    volumes:
      - <ctfd_path>/remote-monitor/monitor_ssh_key:/run/monitor_ssh_key:ro  # SSH key for runner
      - <ctfd_path>/remote-monitor/data:<ctfd_path>/remote-monitor/data
      - <ctfd_path>/.data/CTFd/uploads:<ctfd_path>/.data/CTFd/uploads
```

In split-machine mode, Docker commands run on a separate worker node via SSH
(`RUNNER_SSH_TARGET`). Challenge files are rsynced directly to the runner by the CLI.
The monitor's SSH private key is bind-mounted at `/run/monitor_ssh_key` and copied to
`/root/.ssh/id_rsa` by the container entrypoint on startup.

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CTFD_DB_URL` | required | MariaDB URL (`mysql://user:pass@host/db`) |
| `MONITOR_TOKEN` | required | Token required on all admin routes |
| `PUBLIC_HOST` | required | Hostname/domain returned to players in connection strings. Set from `runner_domain` (if set) → `runner_ip` → CTFd host IP by the Ansible playbook. |
| `CTFD_UPLOADS_DIR` | `""` | Absolute path to CTFd uploads dir (for file writes) |
| `CHALLENGES_BASE_DIR` | `/opt/nervctf/challenges` | Root for server-side challenge files |
| `RUNNER_SSH_TARGET` | `""` | SSH target for split-machine mode (e.g. `docker@192.168.1.50`) |
| `MONITOR_PORT` | `33133` | TCP port to bind |
| `MONITOR_BIND` | `0.0.0.0` | TCP bind address |
| `DB_PATH` | `./monitor.db` | SQLite file path |
| `MAX_CONCURRENT_PROVISIONS` | `4` | Semaphore limit for concurrent docker/compose ops |
| `MAX_INSTANCES_PER_TEAM` | `0` | Max active instances per team across all challenges (0 = unlimited) |
| `CTFD_DB_SYNC_INTERVAL` | `30` | Seconds between CTFd MariaDB → SQLite sync cycles |
| `CTFD_DOMAIN` | `""` | CTFd base URL shown in admin dashboard links (e.g. `http://ctfd.example.com`). Defaults to `http://{PUBLIC_HOST}` if unset. Set by Ansible from `ctfd_domain` in `.nervctf.yml`. |

---

## Schema Detection

### `ChallengesSchema` struct

`detect_challenges_schema()` is called inside every mutating challenge operation
(`create_challenge`, `update_challenge`, `list_challenges_full`, `get_challenge_full`).
It queries `information_schema.COLUMNS` to discover which optional columns exist in the
running CTFd database. No result is cached — each call re-probes MariaDB.

| Field | Meaning |
|-------|---------|
| `has_attribution` | `challenges.attribution` column present (added CTFd 3.7.0). Guarded in SELECT and UPDATE SET. |
| `has_logic` | `challenges.logic` column present (added CTFd 3.7.x). Guarded in INSERT and UPDATE SET. |
| `has_position` | `challenges.position` column present. Read in SELECT; value is discarded. |
| `dynamic_in_challenges` | **All four** of `initial`, `minimum`, `decay`, `function` are present inline in `challenges` (newer CTFd). When false, scoring comes from the `dynamic_challenge` join table. |
| `dynamic_partial` | One or more — but not all four — inline scoring columns are present. Indicates a partial schema migration. `dynamic_in_challenges` is `false` in this state, causing safe fallback to the join-table path. |
| `has_next_id` | `challenges.next_id` column present (added CTFd 3.5.x). Guarded in SELECT (NULL placeholder), INSERT, and UPDATE SET. |
| `has_dynamic_table` | `dynamic_challenge` join table exists. CTFd SQLAlchemy joined-table inheritance always requires a row here for `type='dynamic'` challenges. |
| `dynamic_table_has_scoring` | `dynamic_challenge` has its own `initial/minimum/decay/function` columns (older CTFd). Newer CTFd uses a bare `(id)` stub. |

#### NULL placeholder pattern

Wherever a column is absent, `build_full_query()` substitutes a literal `NULL` at the same
position in the SELECT column list. This keeps all column indices stable so `row_to_value()`
can always dereference the same index (e.g., col 10 is always `next_id`, even when the
column does not physically exist in the schema). `Option<i64>` and `Option<String>` fields
deserialise `NULL` as `None`, which is correct behaviour.

### `detect_ctfd_mode()`

Runs **once at startup**, immediately after the MariaDB pool is created. Queries:

```sql
SELECT `value` FROM configs WHERE `key` = 'user_mode' LIMIT 1
```

| Result | `CtfdMode` | Logged as |
|--------|------------|-----------|
| `'1'` or `'true'` | `UserMode` | `WARN` — all player routes will return 403 |
| `'0'`, `'false'`, `''`, or key absent | `TeamMode` | `INFO` — authentication will work |
| Query error / table inaccessible | `Unknown` | `WARN` — operator should verify manually |

When CTFd runs in user-mode, `validate_token()` returns `None` for every user because
`users.team_id IS NULL`. All four player-facing instance routes (`/instance/request`,
`/instance/info`, `/instance/renew`, `/instance/stop`) return HTTP 403. The startup warning
lets the operator catch this misconfiguration before the CTF goes live.

---

## Routes

### No auth

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | `{"status": "ok"}` |
| `GET` | `/` | Login page (redirects to `/admin` if session cookie is valid) |
| `GET` | `/instance/:name` | HTML player UI page |

### Session auth (cookie `nervctf_session`)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/auth/login` | Create session from `{token}` JSON body; sets `nervctf_session` cookie |
| `POST` | `/auth/logout` | Delete session cookie |

### Admin auth (`Authorization: Token <MONITOR_TOKEN>` or `?token=` or session cookie)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/admin` | Admin dashboard HTML |

### Admin auth (`Authorization: Token <MONITOR_TOKEN>`)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/instance/build` | Upload Docker build context (tar.gz) |
| `POST` | `/api/v1/instance/build-compose` | Upload Compose context tar.gz + build (single-machine) |
| `POST` | `/api/v1/instance/build-compose-remote` | Trigger compose build on runner via SSH (split-machine) |
| `POST` | `/api/v1/instance/register` | Register challenge config |
| `GET` | `/api/v1/instance/list` | List registered challenge configs |
| `GET/POST` | `/api/v1/challenges` | List or create challenges (SQL) |
| `GET/PATCH/DELETE` | `/api/v1/challenges/{id}` | Get, update, or delete challenge (SQL) |
| `GET/POST` | `/api/v1/flags` | List or create flags (SQL) |
| `DELETE` | `/api/v1/flags/{id}` | Delete flag (SQL) |
| `GET/POST` | `/api/v1/hints` | List or create hints (SQL) |
| `DELETE` | `/api/v1/hints/{id}` | Delete hint (SQL) |
| `GET/POST` | `/api/v1/tags` | List or create tags (SQL) |
| `DELETE` | `/api/v1/tags/{id}` | Delete tag (SQL) |
| `GET/POST` | `/api/v1/files` | List or upload files (SQL + disk) |
| `DELETE` | `/api/v1/files/{id}` | Delete file record + disk |
| `POST` | `/api/v1/topics` | Upsert topic (SQL) |
| `GET` | `/api/v1/admin/instances` | JSON list of all active instances |
| `GET` | `/api/v1/admin/attempts` | Flag attempt log (`?alerts_only=true` for sharing only) |
| `GET` | `/api/v1/admin/solves` | Correct solves per team |
| `GET` | `/api/v1/admin/config` | Runtime config dump (public_host, challenges_base_dir, runner mode, etc.) |
| `GET` | `/api/v1/admin/probe` | CTFd compatibility probe result; `?refresh=true` forces a fresh probe |
| `GET/POST` | `/api/v1/admin/tokens` | List or create operator tokens |
| `DELETE` | `/api/v1/admin/tokens/:id` | Revoke an operator token |

### Plugin auth (admin token + explicit `team_id` — called by CTFd plugin)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/plugin/info` | Get instance info for a team |
| `POST` | `/api/v1/plugin/request` | Provision instance |
| `POST` | `/api/v1/plugin/renew` | Extend expiry |
| `DELETE` | `/api/v1/plugin/stop` | Destroy instance |
| `DELETE` | `/api/v1/plugin/stop_all` | Destroy all instances for a challenge |
| `POST` | `/api/v1/plugin/solve` | Mark solved + tear down instance |
| `POST` | `/api/v1/plugin/attempt` | Record flag submission + detect flag sharing |

### Player auth (CTFd user token validated via direct MariaDB lookup)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/instance/request` | Provision instance |
| `GET` | `/api/v1/instance/info` | Get own instance |
| `POST` | `/api/v1/instance/renew` | Extend expiry |
| `DELETE` | `/api/v1/instance/stop` | Destroy own instance |

---

## Build Endpoints

### `POST /api/v1/instance/build`

Multipart fields: `challenge_name` (text) + `context` (tar.gz file).

1. Extracts to `CHALLENGES_BASE_DIR/<sanitized_name>/` (wipes existing first)
2. Runs `docker build -t <image_tag> .`
3. Stores `image_tag` in `instance_configs` table

### `POST /api/v1/instance/build-compose`

Same extraction as `build`, but runs:

```
docker compose -f <compose_file> -p <dir_name> build
```

`<dir_name>` is the lowercased name of the challenge directory (parent of `compose_file`).
This causes Docker Compose to tag images as `<dir_name>-<service>` — the same prefix that
the per-instance override file (written by `compose::up`) references in its `image:` entries.

### `POST /api/v1/instance/build-compose-remote`

JSON body: `{challenge_name, compose_file?, challenges_dir?}`

Used in **split-machine mode** after the CLI has rsynced files to the runner.
The monitor SSHes to `RUNNER_SSH_TARGET` and runs:

```
docker compose -f <compose_file> -p <dir_name> build
```

Same `-p <dir_name>` convention as the single-machine build.
No file upload — the CLI handles file transfer directly via rsync.

### Placeholder directory problem

Docker creates empty dirs at bind-mount source paths when they don't exist. If the monitor
starts before any challenge is deployed, stub directories like `<CHALLENGES_BASE_DIR>/my-chall/certs/`
are created. A subsequent `tar -x` cannot overwrite a directory with a file.

**Fix**: the `build-compose` handler wipes `CHALLENGES_BASE_DIR/<name>/` before extracting.

---

## Background Tasks

### Expiry task (every 30 s)

Three checks run on every tick:

1. **Expiry cleanup**: `get_expired_instances()` returns two sets:
   - **Expired running instances**: `status = 'running'` and `expires_at < now`
   - **Stuck provisioning instances**: `status = 'provisioning'` and `created_at < now - 30 min`
     (uses `created_at`, not `expires_at`, so short-timeout challenges don't trigger this early)

   For each matched row:
   - `db::delete_instance()`
   - `ctfd_db::delete_flag(ctfd_flag_id)` — removes dynamic flag from CTFd
   - `cleanup_container(id, runner_ssh)` — tries compose down, lxc delete, docker remove

2. **Orphan cleanup**: list all `ctf-*` compose projects (via `docker compose ls --all`) → stop any not tracked in DB.

3. **Health check**: query running docker container **names** (`docker ps`) and running compose project names (`docker compose ls`). For each `status='running'` DB row whose `container_id` appears in neither list, the container was externally killed:
   - `db::delete_instance()`
   - `ctfd_db::delete_flag(ctfd_flag_id)`
   - `cleanup_container()` (best-effort — container is already gone)

   Both queries return `None` on failure (SSH/docker error). The row is only marked dead when at least one query succeeds and confirms the container is absent; if both queries fail, the row is preserved to avoid false deletes.

### CTFd sync task (every `CTFD_DB_SYNC_INTERVAL` s, default 30)

Reads from CTFd MariaDB (read-only) and updates local SQLite caches:

1. `sync_solves()`:
   - Full-replace `ctfd_solves` table from `submissions WHERE type='correct'`
   - `revert_unsolved_instances()` — sets `status='running'` for instances whose solve was deleted
   - `delete_stale_correct_attempts()` — removes `is_correct=1` flag_attempts with no matching ctfd_solve
2. `sync_users_and_teams()` — full-replace `ctfd_teams` + `ctfd_users` name caches

---

## SQLite Schema

```sql
CREATE TABLE instance_configs (
    challenge_name  TEXT PRIMARY KEY,
    ctfd_id         INTEGER NOT NULL,
    backend         TEXT NOT NULL,       -- "docker"|"compose"|"lxc"|"vagrant"
    config_json     TEXT NOT NULL,       -- full InstanceConfig as JSON
    image_tag       TEXT,
    updated_at      TEXT DEFAULT (datetime('now'))
);

CREATE TABLE instances (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    user_id         INTEGER,
    container_id    TEXT,
    host            TEXT NOT NULL,
    port            INTEGER NOT NULL,
    connection_type TEXT NOT NULL,
    status          TEXT NOT NULL,       -- "running"|"provisioning"|"solved"
    flag            TEXT,
    ctfd_flag_id    INTEGER,
    renewals_used   INTEGER DEFAULT 0,
    created_at      TEXT DEFAULT (datetime('now')),
    expires_at      TEXT NOT NULL,
    UNIQUE(challenge_name, team_id)
);

CREATE TABLE flag_attempts (
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

-- Permanent per-team flag history (never deleted; used for sharing detection after instance stops)
CREATE TABLE team_flags (
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    flag            TEXT NOT NULL,
    created_at      TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (challenge_name, team_id, flag)
);

-- Read-only cache of correct CTFd submissions (synced from MariaDB)
CREATE TABLE ctfd_solves (
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    user_id         INTEGER,
    solved_at       TEXT,
    PRIMARY KEY (challenge_name, team_id)
);

-- Cached team/user names (synced from MariaDB)
CREATE TABLE ctfd_teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE ctfd_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, team_id INTEGER);

-- Operator tokens for admin/API access (hashed; multiple tokens supported)
CREATE TABLE operator_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    label        TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,   -- SHA-256 hex of the raw token
    created_at   TEXT DEFAULT (datetime('now')),
    last_used_at TEXT
);

-- Browser sessions (created by POST /auth/login, scoped to an operator_token)
CREATE TABLE sessions (
    session_id  TEXT PRIMARY KEY,
    operator_id INTEGER NOT NULL REFERENCES operator_tokens(id) ON DELETE CASCADE,
    created_at  TEXT DEFAULT (datetime('now')),
    expires_at  TEXT NOT NULL
);
```

---

## Compatibility Probe

### Overview

At monitor startup — immediately after the CTFd mode check — the monitor runs a one-shot
**compatibility probe** against the connected CTFd MariaDB. The probe fingerprints the
database schema and computes a capability status for each NervCTF feature. The result is
persisted to SQLite and exposed via a REST endpoint so operators can inspect it without
restarting the monitor.

### `ctfd_probe` SQLite table

Singleton row (`id = 1`, enforced by `CHECK`). Overwritten on every probe run.

```sql
CREATE TABLE IF NOT EXISTS ctfd_probe (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    probed_at             TEXT NOT NULL DEFAULT (datetime('now')),
    ctfd_version_tag      TEXT,                          -- from configs.ctf_version, or NULL
    ctfd_version_source   TEXT,                          -- "configs_table" | "inferred"
    is_team_mode          INTEGER,                       -- 1=team, 0=user, NULL=unknown
    challenges_cols       TEXT,                          -- comma-joined sorted column list
    has_dynamic_table     INTEGER NOT NULL DEFAULT 0,
    dynamic_cols          TEXT,                          -- comma-joined sorted column list
    has_next_id           INTEGER NOT NULL DEFAULT 0,
    has_attribution       INTEGER NOT NULL DEFAULT 0,
    has_logic             INTEGER NOT NULL DEFAULT 0,
    has_position          INTEGER NOT NULL DEFAULT 0,
    dynamic_inline        INTEGER NOT NULL DEFAULT 0,    -- all four inline scoring cols present
    dynamic_partial       INTEGER NOT NULL DEFAULT 0,    -- partial inline migration (broken)
    has_instance_table    INTEGER NOT NULL DEFAULT 0,
    cap_challenge_crud    TEXT NOT NULL DEFAULT 'unknown',
    cap_dynamic_scoring   TEXT NOT NULL DEFAULT 'unknown',
    cap_player_auth       TEXT NOT NULL DEFAULT 'unknown',
    cap_instance_flags    TEXT NOT NULL DEFAULT 'unknown',
    cap_redis_sync        TEXT NOT NULL DEFAULT 'degraded',
    probe_notes           TEXT NOT NULL DEFAULT '[]'    -- JSON array of warning strings
);
```

### `GET /api/v1/admin/probe`

Auth: monitor token (same as all other `/api/v1/admin/` routes).

Query parameters:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `refresh` | `false` | Set to `true` to force a fresh probe against MariaDB and overwrite the cache |

Behaviour:
- Without `?refresh=true`: returns the most recently cached result from SQLite. If no
  cached result exists (first boot before the startup probe completes), runs a fresh probe.
- With `?refresh=true`: always runs a fresh probe against MariaDB, persists the result, then
  returns it. Returns HTTP 503 if the MariaDB connection is completely unavailable.

Response envelope (matches all other CTFd-API-style endpoints):

```json
{
  "success": true,
  "data": {
    "probed_at": "2026-05-26 14:32:11 UTC",
    "ctfd_version_tag": "3.7.3",
    "ctfd_version_source": "configs_table",
    "is_team_mode": true,
    "challenges_cols": ["attribution", "category", "connection_info", "decay", "description",
                        "function", "id", "initial", "logic", "max_attempts", "minimum",
                        "name", "next_id", "position", "requirements", "state", "type", "value"],
    "has_dynamic_table": true,
    "dynamic_cols": ["id"],
    "has_next_id": true,
    "has_attribution": true,
    "has_logic": true,
    "has_position": true,
    "dynamic_inline": true,
    "dynamic_partial": false,
    "has_instance_table": true,
    "cap_challenge_crud": "ok",
    "cap_dynamic_scoring": "ok",
    "cap_player_auth": "ok",
    "cap_instance_flags": "ok",
    "cap_redis_sync": "degraded",
    "probe_notes": [
      "Direct MariaDB writes bypass CTFd Redis cache — stale data may be served until CTFd restart or TTL expiry"
    ]
  }
}
```

### Capability status values

| Value | Meaning |
|-------|---------|
| `"ok"` | Feature works correctly with this CTFd instance |
| `"degraded"` | Feature works with reduced functionality or with a known limitation |
| `"broken"` | Feature will fail at runtime; operator action is required |
| `"unknown"` | Probe has not yet run or the relevant tables were inaccessible |

### Capability rules

| Capability | `ok` condition | `degraded` condition | `broken` condition |
|---|---|---|---|
| `cap_challenge_crud` | `has_next_id` | `!has_next_id` (CTFd < 3.5.x) | — |
| `cap_dynamic_scoring` | inline scoring or clean join-table path | — | `dynamic_partial` (broken migration) |
| `cap_player_auth` | `is_team_mode == true` | mode unknown | `is_team_mode == false` (user-mode) |
| `cap_instance_flags` | `has_instance_table` | plugin table absent | — |
| `cap_redis_sync` | — | always (MariaDB writes bypass Redis) | — |

### `probe_notes` field

Machine-readable warning strings included when a capability is not `"ok"`. One entry is
always present (`cap_redis_sync` note). The CLI (`nervctf probe` command, Wave 2B) reads
this field to display warnings to the operator.

---

## Flag Sharing Detection

When a player submits a flag via the CTFd plugin:

1. `POST /api/v1/plugin/attempt` is called with `{challenge_name, team_id, user_id, submitted_flag, is_correct}`
2. Monitor queries `team_flags` for the submitted value belonging to a **different** team
3. If found: records `is_flag_sharing=1, owner_team_id=<other_team>` in `flag_attempts`
4. Alert appears on admin dashboard under **Flag Sharing Alerts**

`team_flags` is never cleared when an instance stops, so sharing detection works even after
the original team's instance has expired.
