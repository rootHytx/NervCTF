# Remote monitor

`remote-monitor` is an HTTP server that runs on the CTFd host. It:

1. manages CTFd data directly through MariaDB SQL, with no HTTP proxy and no `CTFD_API_KEY`;
2. manages ephemeral challenge instances (containers and VMs) per team;
3. serves a player-facing HTML UI for the instance lifecycle.

`nervctf setup` deploys it.

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

The monitor runs as a Docker container in the same Compose stack as CTFd. `nervctf setup` writes the `docker-compose.override.yml` that connects it:

```yaml
# single-machine mode (no runner_ip)
services:
  remote-monitor:
    image: nervctf-monitor:latest
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock        # local Docker daemon
      - /usr/libexec/docker/cli-plugins:/usr/libexec/docker/cli-plugins:ro
      - <ctfd_path>/remote-monitor/data:<ctfd_path>/remote-monitor/data
      - <ctfd_path>/.data/CTFd/uploads:<ctfd_path>/.data/CTFd/uploads

# split-machine mode (runner_ip set)
services:
  remote-monitor:
    image: nervctf-monitor:latest
    volumes:
      - <ctfd_path>/remote-monitor/monitor_ssh_key:/run/monitor_ssh_key:ro  # SSH key for runner
      - <ctfd_path>/remote-monitor/data:<ctfd_path>/remote-monitor/data
      - <ctfd_path>/.data/CTFd/uploads:<ctfd_path>/.data/CTFd/uploads
```

In split-machine mode, Docker commands run on a separate worker node over SSH (`RUNNER_SSH_TARGET`), and the CLI rsyncs challenge files to the runner. The monitor's SSH private key is bind-mounted at `/run/monitor_ssh_key` and copied to `/root/.ssh/id_rsa` by the container entrypoint at startup.

---

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `CTFD_DB_URL` | required | MariaDB URL (`mysql://user:pass@host/db`) |
| `MONITOR_TOKEN` | required | Token required on all admin routes |
| `PUBLIC_HOST` | required | Hostname or domain in player connection strings. The playbook sets it from `runner_domain`, then `runner_ip`, then the CTFd host IP. |
| `CTFD_UPLOADS_DIR` | `""` | Absolute path to the CTFd uploads directory |
| `CHALLENGES_BASE_DIR` | `/opt/nervctf/challenges` | Root for server-side challenge files |
| `RUNNER_SSH_TARGET` | `""` | SSH target for split-machine mode (for example `docker@192.168.1.50`) |
| `MONITOR_PORT` | `33133` | TCP port to bind |
| `MONITOR_BIND` | `0.0.0.0` | TCP bind address |
| `DB_PATH` | `./monitor.db` | SQLite file path |
| `MAX_CONCURRENT_PROVISIONS` | `4` | Semaphore limit for concurrent docker/compose operations |
| `MAX_INSTANCES_PER_TEAM` | `0` | Max active instances per team (0 = unlimited) |
| `CTFD_DB_SYNC_INTERVAL` | `30` | Seconds between CTFd-to-SQLite sync cycles |
| `CTFD_DOMAIN` | `""` | CTFd base URL shown in dashboard links (for example `http://ctfd.example.com`); defaults to `http://{PUBLIC_HOST}`. The playbook sets it from `ctfd_domain` in `.nervctf.yml`. |

---

## Schema detection

### `ChallengesSchema` and `detect_challenges_schema()`

`detect_challenges_schema()` runs inside every mutating challenge operation (`create_challenge`, `update_challenge`, `list_challenges_full`, `get_challenge_full`). It queries `information_schema.COLUMNS` to discover which optional columns exist. No result is cached; each call re-probes MariaDB.

| Field | Meaning |
|---|---|
| `has_attribution` | `challenges.attribution` exists (CTFd 3.7.0). Guarded in SELECT and the UPDATE SET. |
| `has_logic` | `challenges.logic` exists (CTFd 3.7.x). Guarded in INSERT and the UPDATE SET. |
| `has_position` | `challenges.position` exists. Read in SELECT; the value is discarded. |
| `dynamic_in_challenges` | All four of `initial`, `minimum`, `decay`, and `function` are inline in `challenges` (newer CTFd). When false, scoring comes from the `dynamic_challenge` join table. |
| `dynamic_partial` | Some but not all four inline scoring columns exist, indicating a partial migration. `dynamic_in_challenges` is false in this state, so the code falls back to the join-table path. |
| `has_next_id` | `challenges.next_id` exists (CTFd 3.5.x). Guarded in SELECT (NULL placeholder), INSERT, and the UPDATE SET. |
| `has_dynamic_table` | The `dynamic_challenge` join table exists. CTFd's joined-table inheritance always needs a row here for `type='dynamic'`. |
| `dynamic_table_has_scoring` | `dynamic_challenge` has its own `initial`/`minimum`/`decay`/`function` columns (older CTFd). Newer CTFd uses a bare `(id)` stub. |

### NULL placeholder pattern

Where a column is absent, `build_full_query()` substitutes a literal `NULL` at the same position in the SELECT list. This keeps every index stable, so `row_to_value()` always reads the same index (column 10 is always `next_id`, even when the column does not exist). `Option<i64>` and `Option<String>` fields deserialise `NULL` as `None`.

### `detect_ctfd_mode()`

Runs once at startup, right after the MariaDB pool is created. The monitor first resolves whether the installed CTFd names its settings table `configs` or `config`, then queries:

```sql
SELECT `value` FROM <settings_table> WHERE `key` = 'user_mode' LIMIT 1
```

| Result | `CtfdMode` | Logged as |
|---|---|---|
| `'1'` or `'true'` | `UserMode` | `WARN` — all player routes will return 403 |
| `'0'`, `'false'`, `''`, or key absent | `TeamMode` | `INFO` — authentication will work |
| Query error or inaccessible table | `Unknown` | `WARN` — verify manually |

In user-mode, `validate_token()` returns `None` for every user because `users.team_id IS NULL`, and all four player routes (`/instance/request`, `/instance/info`, `/instance/renew`, `/instance/stop`) return 403. The startup warning lets the operator catch this before the CTF goes live.

---

## Routes

### No auth

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | `{"status": "ok"}` |
| `GET` | `/` | Login page; redirects to `/admin` when the session cookie is valid |
| `GET` | `/instance/:name` | HTML player page |

### Session auth (cookie `nervctf_session`)

| Method | Path | Description |
|---|---|---|
| `POST` | `/auth/login` | Create a session from a `{token}` JSON body; sets the `nervctf_session` cookie |
| `POST` | `/auth/logout` | Delete the session cookie |

### Admin auth (`Authorization: Token <MONITOR_TOKEN>`, `?token=`, or session cookie)

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin` | Admin dashboard HTML |

### Admin auth (`Authorization: Token <MONITOR_TOKEN>`)

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/v1/instance/build` | Upload a Docker build context (tar.gz) |
| `POST` | `/api/v1/instance/build-compose` | Upload a Compose context and build (single-machine) |
| `POST` | `/api/v1/instance/build-compose-remote` | Trigger a compose build on the runner over SSH (split-machine) |
| `POST` | `/api/v1/instance/register` | Register a challenge config |
| `GET` | `/api/v1/instance/list` | List registered configs |
| `GET/POST` | `/api/v1/challenges` | List or create challenges (SQL) |
| `GET/PATCH/DELETE` | `/api/v1/challenges/{id}` | Get, update, or delete a challenge (SQL) |
| `GET/POST` | `/api/v1/flags` | List or create flags (SQL) |
| `DELETE` | `/api/v1/flags/{id}` | Delete a flag (SQL) |
| `GET/POST` | `/api/v1/hints` | List or create hints (SQL) |
| `DELETE` | `/api/v1/hints/{id}` | Delete a hint (SQL) |
| `GET/POST` | `/api/v1/tags` | List or create tags (SQL) |
| `DELETE` | `/api/v1/tags/{id}` | Delete a tag (SQL) |
| `GET/POST` | `/api/v1/files` | List or upload files (SQL and disk) |
| `DELETE` | `/api/v1/files/{id}` | Delete a file record and its disk file |
| `POST` | `/api/v1/topics` | Upsert a topic (SQL) |
| `GET` | `/api/v1/admin/instances` | JSON list of all active instances |
| `GET` | `/api/v1/admin/attempts` | Flag attempt log (`?alerts_only=true` for sharing only) |
| `GET` | `/api/v1/admin/solves` | Correct solves per team |
| `GET` | `/api/v1/admin/config` | Runtime config (public host, challenges base dir, runner mode, …) |
| `GET` | `/api/v1/admin/probe` | CTFd compatibility probe; `?refresh=true` runs a fresh probe |
| `GET/POST` | `/api/v1/admin/tokens` | List or create operator tokens |
| `DELETE` | `/api/v1/admin/tokens/:id` | Revoke an operator token |
| `GET/PUT` | `/api/v1/admin/event` | Read or upsert the managed CTFd event settings |
| `GET` | `/api/v1/admin/events` | Lifecycle event log (`type`, `challenge`, `team`, `since`, `limit`, `offset`) |

### Plugin auth (admin token plus explicit `team_id`, called by the CTFd plugin)

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/plugin/info` | Instance info for a team |
| `POST` | `/api/v1/plugin/request` | Provision an instance |
| `POST` | `/api/v1/plugin/renew` | Extend expiry |
| `DELETE` | `/api/v1/plugin/stop` | Destroy an instance |
| `DELETE` | `/api/v1/plugin/stop_all` | Destroy all instances for a challenge |
| `POST` | `/api/v1/plugin/solve` | Mark solved and tear down the instance |
| `POST` | `/api/v1/plugin/attempt` | Record a flag submission and detect sharing |

### Player auth (CTFd user token validated against MariaDB)

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/v1/instance/request` | Provision an instance |
| `GET` | `/api/v1/instance/info` | Get the caller's instance |
| `POST` | `/api/v1/instance/renew` | Extend expiry |
| `DELETE` | `/api/v1/instance/stop` | Destroy the caller's instance |

---

## Event settings and event log

### `GET /api/v1/admin/event`

Returns the managed CTFd settings as a flat JSON object. The monitor resolves the settings table (`configs` or `config`) at runtime.

### `PUT /api/v1/admin/event`

Upserts a flat JSON object. Keys are restricted to an allowlist: `ctf_name`, `ctf_description`, `start`, `end`, `user_mode`, `challenge_visibility`, `registration_visibility`, `score_visibility`, `account_visibility`, and `verify_emails`. Values must be scalars. Returns `{"written": N}`.

Unknown keys are rejected with HTTP 400, so a stale config cannot blank unrelated settings.

### `GET /api/v1/admin/events`

Returns the lifecycle log, newest first.

| Parameter | Default | Description |
|---|---|---|
| `type` | none | `instance.requested`, `instance.running`, `instance.stopped`, `instance.solved`, `instance.expired` |
| `challenge` | none | Challenge name |
| `team` | none | Numeric CTFd team id |
| `since` | none | SQLite datetime (`YYYY-MM-DD HH:MM:SS`) |
| `limit` | 200 | Maximum rows, clamped to 1–5000 |
| `offset` | 0 | Pagination offset |

Each row includes `id`, `event_type`, `challenge_name`, `team_id`, `team_name`, `user_id`, `user_name`, `instance_id`, `detail`, and `created_at`.

The table has no automatic retention limit. Prune it with `DELETE FROM events WHERE created_at < ...` if it grows too large.

---

## Build endpoints

### `POST /api/v1/instance/build`

Multipart fields: `challenge_name` (text) and `context` (tar.gz file).

1. Extract to `CHALLENGES_BASE_DIR/<sanitized_name>/`, wiping any existing directory first.
2. Run `docker build -t <image_tag> .`.
3. Store `image_tag` in `instance_configs`.

### `POST /api/v1/instance/build-compose`

Same extraction as `build`, then:

```
docker compose -f <compose_file> -p <dir_name> build
```

`<dir_name>` is the lowercased name of the challenge directory (the parent of `compose_file`). Compose therefore tags images as `<dir_name>-<service>`, the same prefix the per-instance override references.

### `POST /api/v1/instance/build-compose-remote`

JSON body: `{challenge_name, compose_file?, challenges_dir?}`.

Used in split-machine mode after the CLI has rsynced files to the runner. The monitor SSHes to `RUNNER_SSH_TARGET` and runs:

```
docker compose -f <compose_file> -p <dir_name> build
```

The `-p <dir_name>` convention matches the single-machine build. No files are uploaded; the CLI transfers them.

### Placeholder directory problem

Docker creates empty directories at bind-mount source paths that do not exist. If the monitor starts before any challenge is deployed, stub directories such as `<CHALLENGES_BASE_DIR>/my-chall/certs/` appear, and a later `tar -x` cannot replace a directory with a file.

The `build-compose` handler wipes `CHALLENGES_BASE_DIR/<name>/` before extracting.

---

## Background tasks

### Expiry task (every 30 s)

Three checks run each tick.

1. **Expiry cleanup.** `get_expired_instances()` returns two sets:
   - expired running instances: `status = 'running'` and `expires_at < now`;
   - stuck provisioning instances: `status = 'provisioning'` and `created_at < now - 30 min`, measured from `created_at` so short-timeout challenges do not trigger early.

   For each row: `db::delete_instance()`, then `ctfd_db::delete_flag(ctfd_flag_id)`, then `cleanup_container(id, runner_ssh)`, which tries compose down, lxc delete, and docker remove.

2. **Orphan cleanup.** List every `ctf-*` compose project with `docker compose ls --all` and stop any the DB does not track.

3. **Health check.** Query running Docker container names (`docker ps`) and running compose project names (`docker compose ls`). A `status='running'` row whose `container_id` appears in neither list was killed externally, so the row is deleted, the flag removed, and `cleanup_container()` called (best effort).

   Both queries return `None` on failure (SSH or Docker error). A row is marked dead only when at least one query succeeds and confirms the container is absent; when both fail, the row is kept, to avoid false deletes.

### CTFd sync task (every `CTFD_DB_SYNC_INTERVAL` s, default 30)

Reads from CTFd MariaDB (read-only) and updates the local SQLite caches.

1. `sync_solves()` full-replaces `ctfd_solves` from `submissions WHERE type='correct'`; `revert_unsolved_instances()` sets `status='running'` for instances whose solve was deleted; `delete_stale_correct_attempts()` removes `is_correct=1` flag attempts with no matching solve.
2. `sync_users_and_teams()` full-replaces the `ctfd_teams` and `ctfd_users` name caches.

---

## SQLite schema

The schema (`db.rs`) is reproduced in `ARCHITECTURE.md`. In short, the monitor stores instance configs, instances, flag attempts, a permanent `team_flags` history, read-only caches of CTFd solves and team/user names, operator tokens, browser sessions, the singleton `ctfd_probe` row, and the `events` lifecycle log.

`Db = Arc<Mutex<Connection>>`, WAL mode is on, and migrations run on every `open()`.

---

## Compatibility probe

At startup, right after the CTFd mode check, the monitor runs a one-shot compatibility probe against MariaDB. It fingerprints the schema and computes a capability status for each feature, persists the result to SQLite, and exposes it over REST so operators can inspect it without restarting.

### `ctfd_probe` table

A singleton row (`id = 1`), overwritten on every probe. Its columns are listed in `ARCHITECTURE.md`.

### `GET /api/v1/admin/probe`

Auth: monitor token, like every other `/api/v1/admin/` route.

| Parameter | Default | Description |
|---|---|---|
| `refresh` | `false` | Set to `true` to force a fresh probe and overwrite the cache |

- Without `?refresh=true`, it returns the cached result. If no result exists yet (first boot before the startup probe finishes), it runs a fresh probe.
- With `?refresh=true`, it always probes MariaDB, persists the result, and returns it. It returns HTTP 503 when the MariaDB connection is entirely unavailable.

The response uses the usual envelope:

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

### Capability values

| Value | Meaning |
|---|---|
| `"ok"` | The feature works against this CTFd instance |
| `"degraded"` | The feature works with reduced functionality or a known limitation |
| `"broken"` | The feature will fail at runtime; operator action is required |
| `"unknown"` | The probe has not run, or the relevant tables were inaccessible |

### Capability rules

| Capability | `ok` when | `degraded` when | `broken` when |
|---|---|---|---|
| `cap_challenge_crud` | `has_next_id` | `!has_next_id` (CTFd < 3.5.x) | — |
| `cap_dynamic_scoring` | inline scoring, or a clean join-table path | — | `dynamic_partial` (broken migration) |
| `cap_player_auth` | `is_team_mode == true` | mode unknown | `is_team_mode == false` (user-mode) |
| `cap_instance_flags` | `has_instance_table` | plugin table absent | — |
| `cap_redis_sync` | — | always (MariaDB writes bypass Redis) | — |

### `probe_notes`

Warning strings included whenever a capability is not `"ok"`. One entry is always present, the `cap_redis_sync` note. `nervctf probe` reads this field to show warnings.

---

## Flag-sharing detection

When a player submits a flag through the CTFd plugin:

1. `POST /api/v1/plugin/attempt` arrives with `{challenge_name, team_id, user_id, submitted_flag, is_correct}`.
2. The monitor looks in `team_flags` for the submitted value under a different team.
3. When it finds one, it records `is_flag_sharing=1, owner_team_id=<other_team>` in `flag_attempts`.
4. The alert appears on the dashboard under "Flag Sharing Alerts".

`team_flags` is never cleared when an instance stops, so sharing detection keeps working after the owner's instance has expired.
