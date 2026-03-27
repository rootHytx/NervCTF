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

The monitor runs as a Docker container inside the same Compose stack as CTFd:

```yaml
# docker-compose.override.yml (written by Ansible)
services:
  remote-monitor:
    image: nervctf-monitor:latest
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - remote_monitor_data:/data
      - {{ ctfd_uploads_dir }}:{{ ctfd_uploads_dir }}  # same-path bind mount
```

In split-machine mode, Docker commands run on a separate worker node via SSH
(`RUNNER_SSH_TARGET`). Challenge files are rsynced directly to the runner by the CLI.

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CTFD_DB_URL` | required | MariaDB URL (`mysql://user:pass@host/db`) |
| `MONITOR_TOKEN` | required | Token required on all admin routes |
| `PUBLIC_HOST` | required | Hostname returned to players in connection strings |
| `CTFD_UPLOADS_DIR` | `""` | Absolute path to CTFd uploads dir (for file writes) |
| `CHALLENGES_BASE_DIR` | `/opt/nervctf/challenges` | Root for server-side challenge files |
| `RUNNER_SSH_TARGET` | `""` | SSH target for split-machine mode (e.g. `docker@192.168.1.50`) |
| `MONITOR_PORT` | `33133` | TCP port to bind |
| `MONITOR_BIND` | `0.0.0.0` | TCP bind address |
| `DB_PATH` | `./monitor.db` | SQLite file path |
| `MAX_CONCURRENT_PROVISIONS` | `4` | Semaphore limit for concurrent docker/compose ops |
| `CTFD_DB_SYNC_INTERVAL` | `30` | Seconds between CTFd MariaDB → SQLite sync cycles |

---

## Routes

### No auth

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | `{"status": "ok"}` |
| `GET` | `/instance/:name` | HTML player UI page |

### Admin auth (`Authorization: Token <MONITOR_TOKEN>` or `?token=`)

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

Same, but runs `docker compose -f <compose_file> build` instead.

### `POST /api/v1/instance/build-compose-remote`

JSON body: `{challenge_name, compose_file?, challenges_dir?}`

Used in **split-machine mode** after the CLI has rsynced files to the runner.
The monitor SSHes to `RUNNER_SSH_TARGET` and runs `docker compose build` there.
No file upload — the CLI handles file transfer directly via rsync.

### Placeholder directory problem

Docker creates empty dirs at bind-mount source paths when they don't exist. If the monitor
starts before any challenge is deployed, stub directories like `/data/challenges/my-chall/certs/`
are created. A subsequent `tar -x` cannot overwrite a directory with a file.

**Fix**: the `build-compose` handler wipes `CHALLENGES_BASE_DIR/<name>/` before extracting.

---

## Background Tasks

### Expiry task (every 30 s)

1. `get_expired_instances()` → for each expired running instance:
   - `cleanup_container(id, runner_ssh)` — tries compose down, lxc delete, docker remove
   - `ctfd_db::delete_flag(ctfd_flag_id)` — removes dynamic flag from CTFd
   - `db::delete_instance()`
2. Orphan cleanup: list running `ctf-*` compose projects → stop any not tracked in DB

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
```

---

## Flag Sharing Detection

When a player submits a flag via the CTFd plugin:

1. `POST /api/v1/plugin/attempt` is called with `{challenge_name, team_id, user_id, submitted_flag, is_correct}`
2. Monitor queries `team_flags` for the submitted value belonging to a **different** team
3. If found: records `is_flag_sharing=1, owner_team_id=<other_team>` in `flag_attempts`
4. Alert appears on admin dashboard under **Flag Sharing Alerts**

`team_flags` is never cleared when an instance stops, so sharing detection works even after
the original team's instance has expired.
