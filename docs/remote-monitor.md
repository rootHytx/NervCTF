# Remote Monitor

The `remote-monitor` is an HTTP server that runs on the CTFd host. It:

1. **Manages** all CTFd data directly via MariaDB SQL (no HTTP proxy)
2. **Manages** ephemeral challenge instances (containers/VMs) per team
3. **Serves** a player-facing HTML UI for instance lifecycle

Deployed automatically by `nervctf setup`.

---

## Architecture

```
CLI  ──Token<monitor>──▶  remote-monitor:33133  ──SQL──▶  CTFd MariaDB
                                │                    └──▶  CTFd uploads dir (files)
                     instance manager
                  ┌─────────────┴──────────────┐
               Docker          Compose         LXC
           (per-container)  (per-project)  (per-lxc-container)
```

The monitor runs as a Docker container inside the same Compose stack as CTFd:

```yaml
# docker-compose.override.yml (written by Ansible)
services:
  remote-monitor:
    image: nervctf-monitor:latest
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock   # Docker-outside-of-Docker
      - remote_monitor_data:/data                    # SQLite DB
      - /data/challenges:/data/challenges            # challenge files (bind mount)
```

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CTFD_DB_URL` | `""` | MariaDB connection URL (e.g. `mysql://user:pass@db/ctfd`) |
| `CTFD_UPLOADS_DIR` | `""` | Absolute path to CTFd uploads dir (for file writes) |
| `CTFD_URL` | `http://localhost:8000` | CTFd base URL (for player token validation only) |
| `MONITOR_TOKEN` | `""` | Token required on all admin routes |
| `PUBLIC_HOST` | `127.0.0.1` | Hostname returned to players in connection strings |
| `MONITOR_PORT` | `33133` | TCP port to bind |
| `DB_PATH` | `/data/monitor.db` | SQLite file path |
| `CHALLENGES_BASE_DIR` | `/opt/nervctf/challenges` | Root for server-side challenge files |

---

## Routes

### No auth

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | `{"ok": true}` |
| `GET` | `/instance/:name` | HTML player UI page |

### Admin auth (`Authorization: Token <MONITOR_TOKEN>`)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/instance/build` | Upload Docker build context (tar.gz); builds image |
| `POST` | `/api/v1/instance/build-compose` | Upload Compose context (tar.gz); builds images |
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
| `DELETE` | `/api/v1/files/{id}` | Delete file record + disk (SQL) |
| `POST` | `/api/v1/topics` | Upsert topic (SQL) |

### Plugin auth (admin token + explicit `team_id` — called by CTFd plugin)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/plugin/info` | Get instance info for a team |
| `POST` | `/api/v1/plugin/request` | Provision instance |
| `POST` | `/api/v1/plugin/renew` | Extend expiry |
| `DELETE` | `/api/v1/plugin/stop` | Destroy instance |
| `DELETE` | `/api/v1/plugin/stop_all` | Destroy all instances for a challenge |
| `GET` | `/api/v1/plugin/flag` | Get stored random flag for a team |

### Player auth (CTFd user token validated via `GET /api/v1/users/me`)

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

1. Extracts tar.gz to `CHALLENGES_BASE_DIR/<sanitized_name>/`
2. Wipes existing directory before extraction (prevents Docker placeholder dirs)
3. Reads `config_json` from DB to find the Dockerfile path
4. Runs `docker build -t <image_tag> .`
5. Stores `image_tag` in `instance_configs` table

### `POST /api/v1/instance/build-compose`

Same as above, but runs `docker compose -f <compose_file> build` instead of `docker build`.

### Placeholder directory problem

Docker creates an empty directory at a bind-mount source path when the file/dir doesn't
exist at container start time. If the monitor was started before any challenge was deployed,
Docker creates stub directories like `/data/challenges/my-challenge/certs/`. A subsequent
`tar -xzf` cannot overwrite a directory with a file of the same name.

**Fix**: the monitor wipes `CHALLENGES_BASE_DIR/<name>/` with `remove_dir_all()` before
extracting, on every build request.

---

## SQLite Schema

```sql
-- instance_configs: written by nervctf deploy
CREATE TABLE instance_configs (
    challenge_name  TEXT PRIMARY KEY,
    ctfd_id         INTEGER NOT NULL,
    backend         TEXT NOT NULL,       -- "docker"|"compose"|"lxc"|"vagrant"
    config_json     TEXT NOT NULL,       -- full InstanceConfig as JSON
    image_tag       TEXT,                -- resolved image name after build
    updated_at      TEXT DEFAULT (datetime('now'))
);

-- instances: active per-team containers
CREATE TABLE instances (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    container_id    TEXT,                -- docker ID, compose project name, or LXC name
    host            TEXT NOT NULL,
    port            INTEGER NOT NULL,
    connection_type TEXT NOT NULL,
    status          TEXT NOT NULL,       -- "running"|"stopped"|"error"
    flag            TEXT,                -- per-team random flag (null for static)
    renewals_used   INTEGER DEFAULT 0,
    created_at      TEXT DEFAULT (datetime('now')),
    expires_at      TEXT NOT NULL,       -- "YYYY-MM-DD HH:MM:SS" UTC
    UNIQUE(challenge_name, team_id)
);
```

`Db = Arc<Mutex<Connection>>`. WAL mode enabled. `flag` column is added via migration
on every startup (idempotent `ALTER TABLE … ADD COLUMN`).

---

## Background Expiry

A task spawned at startup runs every 30 seconds:
1. `get_expired_instances()` — query rows where `expires_at < datetime('now')`
2. For each: call `cleanup_container(container_id)` (tries compose down, lxc delete, docker rm)
3. `delete_instance()` — remove the DB row

---

## Instance Backends

### Container/Project naming

All instances follow the pattern: `ctf-<sanitized_challenge_name>-t<team_id>`

`sanitize_name(name)` lowercases and replaces non-alphanumeric/hyphen chars with hyphens.

### Port allocation

`pick_free_port(used_ports)` picks a random port in 40000–50000 not in the set of currently
used ports (queried from DB before each allocation).

### `cleanup_container(container_id)`

If the ID starts with `"ctf-"` and is < 80 chars: tries `compose::down()` and `lxc::delete()`.
Always tries `docker::remove_container()`.

---

## `/data/challenges` bind mount

Challenge files are stored at `CHALLENGES_BASE_DIR` (default `/data/challenges`).
This path must be identical on the host and inside the monitor container.

The named Docker volume `remote_monitor_data` is mounted at `/data` (holds the SQLite DB).
The bind mount `/data/challenges:/data/challenges` **shadows** that subdirectory with the
host filesystem, so:

- Challenge files written by the monitor are visible to the host Docker daemon at the same
  absolute path
- When a challenge's `docker-compose.yml` bind-mounts `/data/challenges/my-challenge/certs/`,
  Docker resolves that path on the host — not inside the monitor container

---

## CTFd Plugin (`nervctf_instance`)

Installed to `CTFd/plugins/nervctf_instance/`. Adds `type: instance` as a CTFd challenge type.

The plugin:
- Stores extra fields in an `InstanceChallenge` SQLAlchemy model (extends `Challenges`)
- Calls `POST /api/v1/instance/register` on the monitor when challenges are created/updated
- Proxies player actions to `/api/v1/plugin/*` using the admin token + explicit `team_id`
  (players never see the admin token)
- Overrides `attempt()` for `flag_mode: random`: fetches the correct flag from
  `GET /api/v1/plugin/flag` and compares directly

Blueprint routes within CTFd (all `@authed_only`):
- `GET /api/v1/containers/info/<challenge_id>`
- `POST /api/v1/containers/request`
- `POST /api/v1/containers/renew`
- `POST /api/v1/containers/stop`

Required env vars in CTFd's `.env` (written by Ansible):
- `NERVCTF_MONITOR_URL=http://remote-monitor:33133`
- `NERVCTF_MONITOR_TOKEN=<token>`

---

## Building the monitor binary

```sh
# Static musl binary (recommended for server deployment)
nix develop .# --command cargo build --release \
  --target x86_64-unknown-linux-musl -p remote-monitor

# Or standard Linux release
nix develop .# --command cargo build --release -p remote-monitor
```

After building, re-run `nervctf setup` to upload the new binary and rebuild the Docker image.
