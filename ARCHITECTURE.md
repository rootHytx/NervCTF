# Architecture

NervCTF runs CTF competitions on CTFd. It has three parts:

- **`nervctf`** (CLI) reads `challenge.yml` files from a local directory tree, deploys and syncs them to CTFd through the remote monitor, and registers per-challenge instance configs.
- **`remote-monitor`** (HTTP server) runs on the CTFd host. It writes CTFd data through MariaDB directly, manages ephemeral challenge containers and VMs per team, and serves an admin dashboard.
- **`nervctf_instance`** (CTFd plugin, Python) is installed inside CTFd. It adds the `instance` challenge type and forwards player lifecycle requests to the monitor.

This document is the system reference. Where it disagrees with the source, trust the source.

---

## Repository layout

```
NervCTF/
├── Cargo.toml                   # workspace manifest; members: src/nervctf, src/remote-monitor
├── Cargo.lock
├── flake.nix                    # the only dev environment (rustc, cargo, ansible, openssl, …)
├── RELEASE.md, bump-version.sh  # release tooling
├── ARCHITECTURE.md              # this file
│
├── src/
│   ├── nervctf/                 # CLI crate
│   │   ├── src/
│   │   │   ├── lib.rs           # re-exports the modules below
│   │   │   ├── main.rs          # clap CLI, deploy/sync orchestration, probe
│   │   │   ├── utils.rs         # Config struct and load/ save/ find_config_path
│   │   │   ├── setup.rs         # `nervctf setup` and `setup --upgrade`
│   │   │   ├── fix.rs           # `nervctf fix`
│   │   │   ├── validator.rs     # `nervctf validate`
│   │   │   ├── directory_scanner.rs  # recursive challenge.yml finder (depth 5, no symlinks)
│   │   │   ├── challenge_manager/
│   │   │   │   ├── mod.rs       # ChallengeManager: CRUD wrappers over CtfdClient
│   │   │   │   └── sync.rs      # ChallengeSynchronizer, needs_update(), SyncAction
│   │   │   └── ctfd_api/
│   │   │       ├── client.rs    # CtfdClient (async reqwest), execute(), pagination
│   │   │       ├── models/mod.rs # data types (Challenge, FlagContent, HintContent, …)
│   │   │       └── endpoints/   # challenges.rs, flags.rs, hints.rs, tags.rs, files.rs
│   │   └── assets/
│   │       ├── nervctf_playbook.yml           # full server setup
│   │       ├── nervctf_upgrade_playbook.yml   # plugin/binary/image upgrade
│   │       ├── nervctf_preflight_playbook.yml # pre-provision checks
│   │       └── ctfd-plugin/                    # Python plugin, embedded into the binary
│   │
│   └── remote-monitor/          # HTTP server crate
│       ├── assets/admin.html    # admin dashboard, embedded with include_str!
│       └── src/
│           ├── main.rs          # axum 0.7 server, routes, AppState, background tasks
│           ├── db.rs            # SQLite via rusqlite; Db = Arc<Mutex<Connection>>
│           ├── ctfd_db.rs       # MariaDB SQL for all CTFd data
│           └── instance/
│               ├── mod.rs       # provision(), cleanup_container(), generate_flag(), sanitize_name()
│               ├── docker.rs    # pick_free_ports(), run_container(), remove_container(), build_image()
│               ├── compose.rs   # up(), down(), compose_cmd()
│               ├── lxc.rs       # launch(), delete()
│               └── vagrant.rs   # up() — stub
│
└── templates/                   # challenge.yml templates (standard, docker, compose, lxc, vagrant)
```

---

## Dev environment

Run every build and test command through the Nix flake:

```
nix develop .# --command cargo build
nix develop .# --command cargo build --release -p remote-monitor
nix develop .# --command cargo test
```

The flake provides `pkg-config`, `openssl`, `rustc`, `cargo`, `rustfmt`, `clippy`, and `ansible`, and sets `PKG_CONFIG_PATH` for OpenSSL. There is no `shell.nix`.

**Cross-compilation.** Each cross-stdenv sets `CC` globally. Pin each target with a `CC_<triple>` variable and reset `CC` to the native gcc in the shell hook, so the last cross-compiler cannot poison native builds. See `docs/dev-notes.md`.

---

## Configuration

Priority, highest first:

1. CLI flags: `--monitor-url`, `--monitor-token`
2. Environment variables: `MONITOR_URL`, `MONITOR_TOKEN`
3. `.nervctf.yml` or `.nervctf.yaml`, searched upward from the challenges directory

The `Config` struct (`utils.rs`):

```rust
pub struct Config {
    pub monitor_ip:                Option<String>,
    pub monitor_port:              Option<String>,
    pub monitor_token:             Option<String>,
    pub monitor_user:              Option<String>,   // setup: SSH user on the CTFd host
    pub monitor_ctfd_path:         Option<String>,   // setup: CTFd install path
    pub ssh_key_path:              Option<String>,   // setup/deploy: private key
    pub challenges_path:           Option<String>,   // local challenges directory
    pub max_concurrent_provisions: Option<u32>,      // monitor tuning
    pub max_instances_per_team:    Option<u32>,      // monitor tuning (0 = unlimited)
    pub ctfd_domain:               Option<String>,   // CTFd URL for dashboard links
    pub runner_ip:                 Option<String>,   // split-machine mode
    pub runner_user:               Option<String>,   // split-machine mode
    pub runner_domain:             Option<String>,   // split-machine mode
    pub event:                     Option<EventConfig>, // CTFd event settings
    pub admin:                     Option<AdminConfig>, // first CTFd admin
}
```

`EventConfig` holds `name`, `description`, `start`, `end`, `user_mode`, the four visibility settings, and `verify_emails`. `AdminConfig` holds `user`, `email`, and `password`.

`load_config(start_dir)` walks up looking for `.nervctf.yml`. `save_config(config, path)` serialises to YAML, skips `None` fields, and writes the file with mode `0600` because it can contain the admin password.

In split-machine mode (`runner_ip` and `runner_user` set), `nervctf deploy` rsyncs challenge files to `{runner_user}@{runner_ip}` and the monitor runs container commands on the runner over SSH.

---

## Data models (`ctfd_api/models/mod.rs`)

### ChallengeType

```rust
enum ChallengeType { Standard, Dynamic, Instance }
// serde: "standard" | "dynamic" | "instance"
```

`instance` challenges deploy to CTFd as `standard` or `dynamic`, depending on whether `extra.initial` is set. CTFd never receives `"instance"` as a challenge type; the `nervctf_instance` plugin registers that type separately.

### Challenge

Required fields: `name`, `category`, `value`, `type`. Fields carried through:

- `extra: Option<Extra>` — `{initial, decay, minimum}` for dynamic scoring
- `instance: Option<InstanceConfig>` — only for `type: instance`
- `flags: Option<Vec<FlagContent>>`, `hints: Option<Vec<HintContent>>`, `tags: Option<Vec<Tag>>`
- `files: Option<Vec<String>>` — paths relative to the challenge directory
- `requirements: Option<Requirements>`, `next: Option<String>`
- `state: Option<State>` — `hidden` or `visible`
- `connection_info: Option<String>`, `attempts: Option<u32>`
- `source_path: String` — set by the scanner, not serialised
- `unknown_yaml_keys: Vec<String>` — set by the scanner for lint warnings

### FlagContent, HintContent, Tag, Requirements

All four are untagged enums that accept a shorthand or a detailed form:

```rust
enum FlagContent {
    Simple(String),
    Detailed { id, challenge_id, type_: FlagType, content: String, data: Option<FlagData> }
}
// FlagType: "static" | "regex"
// FlagData: "case_sensitive" | "case_insensitive"  (snake_case serde)

enum HintContent {
    Simple(String),
    Detailed { content: String, cost: Option<u32>, title: Option<String> }
}

enum Tag {
    Simple(String),
    Detailed { challenge_id, id, value: String }
}

enum Requirements {
    Simple(Vec<serde_json::Value>),          // names or integer IDs
    Advanced { prerequisites: Vec<Value>, anonymize: bool }
}
// .prerequisite_names() extracts every value as a string, coercing integers
```

### InstanceConfig

```rust
pub struct InstanceConfig {
    pub backend: InstanceBackend,          // docker | compose | lxc | vagrant
    pub image: Option<String>,
    pub compose_file: Option<String>,      // default "docker-compose.yml"
    pub compose_service: Option<String>,
    pub lxc_image: Option<String>,
    pub vagrantfile: Option<String>,
    pub internal_ports: Vec<u32>,          // accepts scalar or array
    pub service_ports: Option<HashMap<String, Vec<u32>>>,  // compose multi-service
    pub connection: String,                // "nc" | "http" | "ssh"
    pub timeout_minutes: Option<u32>,
    pub max_renewals: Option<u32>,
    pub command: Option<String>,
    pub flag_mode: Option<InstanceFlagMode>,   // "static" | "random"
    pub flag_prefix: Option<String>,
    pub flag_suffix: Option<String>,
    pub random_flag_length: Option<u32>,
    pub flag_delivery: Option<FlagDelivery>,   // "env" (default) | "file"
    pub flag_file_path: Option<String>,        // absolute path inside the container
    pub flag_service: Option<String>,          // compose service receiving the flag file
}
```

### RequirementsQueue

A topological sorter for deploy order, built on Kahn's algorithm. `resolve_dependencies(actions)` returns `Create` and `Update` actions in dependency order and appends `UpToDate` and `RemoteOnly` afterwards.

---

## CLI commands

```
nervctf [OPTIONS] <COMMAND>

Options:
  -c, --challenges-dir <PATH>   default "."
  -v, --verbose
  --monitor-url <URL>
  --monitor-token <TOKEN>

Commands:
  setup [--upgrade]                 Ansible-based server deployment wizard
  deploy [--dry-run] [--recreate] [--prune]
  list [--detailed]
  scan [--detailed]
  validate [--debug]
  probe [--refresh] [--json]
  event [--show]                    Apply the `event:` block to CTFd settings
  fix [--dry-run]
```

`--recreate` forces every challenge into the update queue, skipping the up-to-date check and the confirmation prompt. Use it to re-sync files to the runner or rebuild images.

### Event settings and event log

`nervctf event` maps the `event:` block to CTFd settings and `PUT`s them to `/api/v1/admin/event`. The monitor accepts only an allowlist: `ctf_name`, `ctf_description`, `start`, `end`, `user_mode`, `challenge_visibility`, `registration_visibility`, `score_visibility`, `account_visibility`, and `verify_emails`. It resolves the settings table name at runtime (`configs` or `config`).

The monitor also appends lifecycle rows to its SQLite `events` table: `instance.requested`, `instance.running`, `instance.stopped`, `instance.solved`, and `instance.expired`. `GET /api/v1/admin/events` returns them with `type`, `challenge`, `team`, `since`, `limit`, and `offset` filters. Flag submissions are not duplicated there; they live in `flag_attempts`.

### Deploy flow (`deploy_challenges` in `main.rs`)

All API calls go to the remote monitor, which runs them against CTFd's MariaDB.

1. Scan the local tree into `Vec<Challenge>`.
2. Run the validator; abort on errors.
3. `GET /api/v1/challenges` (paginated) for the remote list.
4. For each local challenge:
   - not on the remote → `create_challenge_phase1()` (`POST /challenges`)
   - on the remote and `needs_update()` or `--recreate` → `update_challenge_phase1()` (`PATCH /challenges/{id}`)
5. After base creates and updates: flags, tags, topics, hints, files, requirements, and state patches.
6. For `type: instance`:
   - deploy to CTFd as `standard` or `dynamic`, based on `extra.initial`;
   - split-machine mode: rsync the challenge directory to the runner, then `POST /api/v1/instance/build-compose-remote`, which makes the monitor run `docker compose build` over SSH;
   - single-machine mode: `POST /api/v1/instance/build` (docker) or `/instance/build-compose` (compose), uploading a tar.gz;
   - `POST /api/v1/instance/register` to register the `InstanceConfig` on the monitor.

### `needs_update()` (`challenge_manager/sync.rs`)

A free function, also exposed as a method by delegation. It compares `category`, `value`, `description`, `state`, `connection_info`, `attempts`, `extra` (JSON), `flags` (sorted `(content, type, data)` tuples), `tags` (sorted values), `hints` (sorted `(content, cost)` tuples), and `requirements`. CTFd's list endpoint never returns flags, tags, or hints, so those comparisons fire only when both sides are `Some`.

### File uploads

Send all of a challenge's files in one multipart request (multiple `"file"` parts). One request per file returns 500. This matches ctfcli's `_create_all_files()`.

### CTFd requirements

- Challenge visibility must be "Public" (Admin → Config → Visibility). Private mode blocks even valid API tokens from `GET /api/v1/challenges`.
- The uploads directory needs `chown -R 1001:1001 <CTFd>/.data/CTFd/uploads` when using Docker.

---

## CTFd API client (`ctfd_api/client.rs`)

```rust
pub struct CtfdClient {
    client: reqwest::Client,
    base_url: String,   // {monitor_url}/api/v1
    // auth: Authorization: Token <monitor_token>
}
```

Every call goes to the remote monitor at `{monitor_url}/api/v1/...`, which handles it with direct MariaDB SQL. Responses are CTFd-compatible (`{"success": true, "data": ...}`). There is no `CTFD_URL` or `CTFD_API_KEY`; the monitor is the only gateway.

Methods include `execute`, `get_challenges` (paginated through `meta.pagination.next`), `get_challenge_id`, `create_challenge`/`update_challenge`/`delete_challenge`, the flag/tag/hint create/delete/list helpers, and `create_file` (multipart). `parse_response` unwraps the `{"success": true, "data": ...}` envelope.

**Dependency conflict.** axum 0.7 uses `http 1.x`; reqwest 0.11 uses `http 0.2.x`. Bridge the two with string conversion (`.as_str()`, `.as_bytes()`) and do not mix `HeaderMap` types.

---

## Remote monitor

### AppState

```rust
pub struct AppState {
    pub monitor_token: String,             // MONITOR_TOKEN
    pub public_host: String,               // PUBLIC_HOST
    pub db: Db,                            // Arc<Mutex<Connection>> (SQLite)
    pub ctfd_pool: mysql_async::Pool,      // CTFd MariaDB pool
    pub challenges_base_dir: String,       // CHALLENGES_BASE_DIR
    pub ctfd_uploads_dir: String,          // CTFD_UPLOADS_DIR
    pub runner_ssh_target: Option<String>, // RUNNER_SSH_TARGET
    pub provision_sem: Arc<Semaphore>,     // MAX_CONCURRENT_PROVISIONS
    pub max_instances_per_team: u64,       // MAX_INSTANCES_PER_TEAM (0 = unlimited)
}
```

### Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `CTFD_DB_URL` | required | MariaDB URL (`mysql://user:pass@host/db`) |
| `MONITOR_TOKEN` | required | Admin auth token |
| `PUBLIC_HOST` | required | Hostname returned to players in connection strings |
| `CTFD_UPLOADS_DIR` | `""` | Absolute path to the CTFd uploads directory |
| `CHALLENGES_BASE_DIR` | `/opt/nervctf/challenges` | Root for challenge files |
| `RUNNER_SSH_TARGET` | `""` | SSH target for split-machine mode |
| `MONITOR_PORT` | `33133` | TCP bind port |
| `MONITOR_BIND` | `0.0.0.0` | TCP bind address |
| `DB_PATH` | `./monitor.db` | SQLite file path |
| `MAX_CONCURRENT_PROVISIONS` | `4` | Semaphore limit for concurrent docker/compose operations |
| `MAX_INSTANCES_PER_TEAM` | `0` | Max active instances per team (0 = unlimited) |
| `CTFD_DB_SYNC_INTERVAL` | `30` | Seconds between CTFd-to-SQLite sync cycles |
| `CTFD_DOMAIN` | `""` | CTFd URL shown in dashboard links |

### Admin dashboard

`GET /admin` serves a self-contained HTML page (no CDN, so it works air-gapped). The token comes from the `?token=` query parameter, an `Authorization: Token` header, or a session cookie. Three tables refresh themselves: flag-sharing alerts (15 s), active instances (15 s), and recent flag attempts (30 s).

### Routes

**No auth:**

- `GET /health` → `{"status": "ok"}`
- `GET /` → login page; redirects to `/admin` when the session cookie is valid
- `GET /instance/:name` → HTML player page

**Session auth (cookie `nervctf_session`):**

- `POST /auth/login`, `POST /auth/logout`

**Admin auth (`Authorization: Token <MONITOR_TOKEN>`, `?token=`, or session cookie):**

- `GET /admin`

**Admin auth (`Authorization: Token <MONITOR_TOKEN>`):**

- `POST /api/v1/instance/build`, `/instance/build-compose`, `/instance/build-compose-remote`
- `POST /api/v1/instance/register`, `GET /api/v1/instance/list`
- CTFd CRUD over SQL: `/api/v1/challenges`, `/flags`, `/hints`, `/tags`, `/files`, `/topics`
- `GET /api/v1/admin/instances`, `/admin/attempts`, `/admin/solves`, `/admin/config`, `/admin/probe`
- `GET/POST /api/v1/admin/tokens`, `DELETE /api/v1/admin/tokens/:id`

**Plugin auth (admin token plus explicit `team_id`, called by the CTFd plugin):**

- `/api/v1/plugin/{info, request, renew, stop, stop_all, solve, attempt}`

**Player auth (CTFd user token validated against MariaDB):**

- `/api/v1/instance/{request, info, renew, stop}`

### Background tasks

**Expiry task** (every 30 s): `get_expired_instances()` returns expired running instances and stuck provisioning instances (older than 30 minutes, measured from `created_at`). For each, delete the SQLite row, delete the CTFd flag, and clean up the container. The task also stops orphaned `ctf-*` compose projects and marks instances dead when the container is gone from both `docker ps` and `docker compose ls`.

**CTFd sync task** (every `CTFD_DB_SYNC_INTERVAL` s): `sync_solves()` full-replaces the `ctfd_solves` cache from MariaDB `submissions`, reverts solved instances whose submission was deleted, and removes stale correct attempts. `sync_users_and_teams()` full-replaces the name caches.

### Auth helpers

- `check_monitor_auth(headers, expected_token) -> bool` reads `Authorization: Token <value>` and gates every admin and plugin route.
- `validate_ctfd_token(pool, token) -> Option<i64>` runs `SELECT team_id FROM users WHERE token = ? AND banned = 0 AND hidden = 0` and returns `None` when invalid or teamless. Player routes use it.

---

## SQLite schema (`remote-monitor/src/db.rs`)

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

CREATE TABLE team_flags (   -- permanent; used for sharing detection after stop
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    flag            TEXT NOT NULL,
    created_at      TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (challenge_name, team_id, flag)
);

CREATE TABLE ctfd_solves (  -- read-only cache of correct CTFd submissions
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    user_id         INTEGER,
    solved_at       TEXT,
    PRIMARY KEY (challenge_name, team_id)
);

CREATE TABLE ctfd_teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE ctfd_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, team_id INTEGER);

CREATE TABLE operator_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    label        TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,   -- SHA-256 hex of the raw token
    created_at   TEXT DEFAULT (datetime('now')),
    last_used_at TEXT
);

CREATE TABLE sessions (
    session_id  TEXT PRIMARY KEY,
    operator_id INTEGER NOT NULL REFERENCES operator_tokens(id) ON DELETE CASCADE,
    created_at  TEXT DEFAULT (datetime('now')),
    expires_at  TEXT NOT NULL
);

CREATE TABLE ctfd_probe (   -- singleton row (id = 1)
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

CREATE TABLE events (   -- durable lifecycle log
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type     TEXT NOT NULL,
    challenge_name TEXT,
    team_id        INTEGER,
    user_id        INTEGER,
    instance_id    INTEGER,
    detail         TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
```

`Db = Arc<Mutex<Connection>>`. WAL mode is enabled and migrations run on every `open()`.

Functions include `upsert_config`, `get_config`, `get_image_tag`, `update_image_tag`, `list_configs`, `insert_provisioning_stub`, `insert_instance` (UPSERT that also writes `team_flags`), `get_instance`, `delete_instance`, `mark_instance_solved`, `delete_all_instances_for_challenge`, `get_used_ports`, `get_expired_instances`, `list_all_instances`, `insert_flag_attempt`, `list_flag_attempts`, `list_sharing_alerts`, `find_flag_owner`, `replace_ctfd_solves`, `revert_unsolved_instances`, `delete_stale_correct_attempts`, `replace_ctfd_teams_and_users`, `has_correct_solve`, and `list_correct_solves`.

---

## Instance backends (`remote-monitor/src/instance/`)

### `mod.rs`

`provision(db, challenge_name, team_id, user_id, config, public_host, ctfd_pool, runner_ssh) -> (host, port, conn_type, expires_at)`.

- `generate_flag(config) -> Option<String>` returns `None` unless `flag_mode = "random"`, otherwise `<prefix><N chars><suffix>`.
- `sanitize_name(name)` lowercases, turns non-alphanumeric characters into hyphens, and trims.
- Container naming is `ctf-<sanitized>-t<team_id>`.
- `expires_at_string(minutes)` returns `"YYYY-MM-DD HH:MM:SS"` UTC without a chrono dependency.
- `cleanup_container(container_id, runner_ssh)` tries `compose::down`, `lxc::delete`, and `docker::remove_container`.

Every docker, compose, and lxc function takes `runner_ssh: Option<&str>`. When set, commands run on the runner through `ssh -o BatchMode=yes <target> <cmd>`.

### `docker.rs`

- `pick_free_ports(used, count) -> Vec<u16>` allocates atomically in the 40000–60000 range.
- `run_container(..., runner_ssh)` runs `docker run -d -p host:internal ...`.
- `remove_container(id, runner_ssh)` runs `docker rm -f`.
- `build_image(context_tar_path, image_tag, runner_ssh)` runs `docker build -t <tag>`.

### `compose.rs`

- `up(..., runner_ssh) -> (u16, String)` writes `<project_name>.override.yml` with port mappings and an optional flag-file bind mount. In split-machine mode it writes the files to the runner over SSH. `flag_delivery: env` injects `FLAG=<value>`; `flag_delivery: file` writes `<project_name>.flag` and a bind mount for `flag_service`. It then runs `DOCKER_BUILDKIT=1 docker compose -f <compose_file> -f <override> -p <project_name> up -d --force-recreate`.
- `down(project_name, runner_ssh?)` runs `docker compose -p <name> down -v`.
- `build(compose_file, runner_ssh?)` runs `docker compose -f <file> build`.
- `list_ctf_projects()` lists running `ctf-*` projects for orphan cleanup.

### `lxc.rs`

- `launch(lxc_image, name, host_port, internal_port, flag)` runs `lxc launch`, waits for `Running`, adds a port proxy, and injects the flag.
- `delete(name)` runs `lxc stop --force` and `lxc delete --force`.

### `vagrant.rs`

`up(...)` is a stub that returns an error.

---

## CTFd plugin (`src/nervctf/assets/ctfd-plugin/`)

Plugin name `nervctf_instance`, installed to `CTFd/plugins/nervctf_instance/`. The CLI embeds it and extracts it on demand.

### `__init__.py` — `InstanceChallengeType(BaseChallenge)`

Registered as `CHALLENGE_CLASSES["instance"]`. CTFd routes `type = "instance"` challenges to it through polymorphic dispatch.

- `create(request)` creates the `InstanceChallenge` row and calls `_register_with_monitor()`.
- `read(challenge)` returns challenge data plus backend, connection, timeout, and flag mode.
- `update(challenge, request)` updates fields and re-registers with the monitor.
- `delete(challenge)` calls `_stop_all_instances()` and cascades DB deletes.
- `solve(user, team, challenge, request)` delegates to `BaseChallenge.solve()` and then calls `POST /api/v1/plugin/solve` on the monitor to tear down the team's instance.
- `attempt(challenge, request)` delegates to `BaseChallenge.attempt()` for the verdict and then fires `POST /api/v1/plugin/attempt` with `{challenge_name, team_id, user_id, submitted_flag, is_correct}`. The call uses a 0.5 s timeout and is swallowed, so it never blocks the CTFd flag submission response.

`_register_with_monitor(challenge)` posts `{challenge_name, ctfd_id, backend, config_json}` to `POST /api/v1/instance/register`.

### Blueprint routes (all `@authed_only`)

Each uses `get_current_team()` for `team_id` and forwards to the monitor's `/api/v1/plugin/*` routes with the admin token plus an explicit `team_id`; players never see the admin token.

- `GET /api/v1/containers/info/<challenge_id>`
- `POST /api/v1/containers/request` — also sends `user_id` from `get_current_user()`
- `POST /api/v1/containers/renew`
- `POST /api/v1/containers/stop`

### `models/challenge.py` — `InstanceChallenge(Challenges)`

A SQLAlchemy polymorphic model with `__mapper_args__ = {"polymorphic_identity": "instance"}`. Extra columns: `backend`, `image`, `command`, `compose_file`, `compose_service`, `lxc_image`, `vagrantfile`, `internal_port`, `connection`, `timeout_minutes`, `max_renewals`, `flag_mode`, `flag_prefix`, `flag_suffix`, `random_flag_length`, `initial_value`, `minimum_value`, `decay_value`, and `decay_function`.

`load(app)` creates the table with `db.create_all()` so `nervctf_instance_challenge` exists.

`_maybe_complete_setup(app)` also runs from `load()`. When CTFd's `setup` setting is absent or false and the `NERVCTF_ADMIN_*`/`NERVCTF_EVENT_*` environment variables are present, it creates the first admin and writes `ctf_name`, `ctf_description`, `user_mode`, and `setup = "true"`. It is a no-op after the first run, so the operator never sees CTFd's setup wizard.

### `assets/view.js`

Uses the DOM API only (no `innerHTML`). Fetch, Extend, and Terminate buttons call `/api/v1/containers/*`. It shows the connection string and a countdown when an instance is running. The monitor returns `expires_at` as `"YYYY-MM-DD HH:MM:SS"` UTC; the plugin converts it to milliseconds in `_sqlite_to_ms()` before passing it to JavaScript.

---

## Ansible playbooks

### `nervctf_preflight_playbook.yml`

Runs before provisioning with `gather_facts: false` and `become: false`, using the `raw` module so it works without Python on the target. It checks passwordless sudo (`sudo -n true`), `python3`, `rsync`, and `git` (CTFd host only) and stops with a message naming any missing item.

### `nervctf_playbook.yml`

Idempotent, target group `ctfd` (and optionally `runner`). Extra vars include `ssh_pubkey`, `ctfd_path`, `monitor_token`, `monitor_port`, and optionally `monitor_binary`, `plugin_src`, `ctfd_domain`, `runner_*`. Tasks in order:

1. Install Docker when `docker --version` fails.
2. Create the docker group and user and authorise the SSH public key.
3. Clone CTFd when it is absent and start it with `docker compose up --build -d --no-recreate`.
4. Install LXD via snap and run `lxd init --auto` (errors ignored).
5. Install Vagrant from the HashiCorp apt repo (errors ignored).
6. Add the docker user to the `lxd` and `libvirt` groups (errors ignored).
7. Deploy the plugin: rsync `plugin_src/` to `<ctfd_path>/CTFd/plugins/nervctf_instance/`.
8. Create the monitor data directory on the host.
9. Copy the remote-monitor binary, write its Dockerfile, and run `docker build -t nervctf-monitor:latest`.
10. Write `docker-compose.override.yml` with the `remote-monitor` service and CTFd environment (`NERVCTF_MONITOR_URL=http://remote-monitor:<port>`, `NERVCTF_MONITOR_TOKEN=<token>`).
11. Resolve CTFd's published HTTP port, then `docker compose up -d --force-recreate` and wait for the CTFd and monitor health endpoints.

### `docker-compose.override.yml` (written by the playbook)

```yaml
services:
  ctfd:
    environment:
      NERVCTF_MONITOR_URL: "http://remote-monitor:33133"
      NERVCTF_MONITOR_TOKEN: "<token>"
      # Emitted only when the config has event:/admin: blocks:
      NERVCTF_EVENT_NAME: "<event name>"
      NERVCTF_EVENT_DESCRIPTION: "<description>"
      NERVCTF_USER_MODE: "teams"
      NERVCTF_ADMIN_USER: "<admin user>"
      NERVCTF_ADMIN_EMAIL: "<admin email>"
      NERVCTF_ADMIN_PASSWORD: "<admin password>"
  remote-monitor:
    image: nervctf-monitor:latest
    restart: unless-stopped
    networks:
      - default
      - internal        # joins CTFd's internal network to reach MariaDB
    environment:
      - CTFD_DB_URL=mysql://ctfd:<db_password>@db/ctfd
      - CTFD_UPLOADS_DIR=<ctfd_path>/.data/CTFd/uploads
      - MONITOR_TOKEN=<token>
      - PUBLIC_HOST=<runner_domain | runner_ip | ansible_host>
      - MONITOR_PORT=33133
      - DB_PATH=<ctfd_path>/remote-monitor/data/monitor.db
      - CHALLENGES_BASE_DIR=<ctfd_path>/remote-monitor/data/challenges
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock      # single-machine mode only
      - /usr/libexec/docker/cli-plugins:/usr/libexec/docker/cli-plugins:ro
      - <ctfd_path>/remote-monitor/data:<ctfd_path>/remote-monitor/data
      - <ctfd_path>/.data/CTFd/uploads:<ctfd_path>/.data/CTFd/uploads
    ports:
      - "33133:33133"
networks:
  internal: {}
```

**Networks.** CTFd's `db` service sits on an `internal: true` network. The override puts `remote-monitor` on that network so it can reach MariaDB at the hostname `db`.

**Path constraint.** `CHALLENGES_BASE_DIR` must be the same absolute path on the host and inside the monitor container. Challenge compose files reference files like `<ctfd_path>/<challenge>/certs/server.pem`; the monitor reads the same path through the bind mount, and the host Docker daemon resolves those paths when the monitor tells it to launch challenge containers (Docker outside of Docker over the socket mount).

### `nervctf_upgrade_playbook.yml`

Syncs the plugin, copies the new monitor binary, rebuilds the image, writes the override, then recreates the whole Compose stack with `docker compose up -d --force-recreate`. Recreating the stack both applies the override environment and makes nginx re-resolve the recreated `ctfd` container; a `restart` leaves the old upstream IP and produces 502s. The playbook does not re-provision CTFd or regenerate the token. It warns when the installed CTFd version differs from 3.7.3.

---

## Setup command (`setup.rs`)

`nervctf setup` runs an interactive wizard:

1. Check the controller for `ansible-playbook` (respecting `ANSIBLE_PLAYBOOK`), `ssh`, and `rsync`.
2. Prompt for the challenges directory, monitor IP, SSH user, runner IP and user, CTFd path, monitor port, token, SSH key, and the first CTFd admin username, email, and password.
3. Reuse an existing `monitor_token` or generate one (32 bytes from `/dev/urandom`).
4. Store the private key path in `ssh_key_path` and pass the matching public key to the playbook.
5. Save `.nervctf.yml` with mode `0600`.
6. Locate the remote-monitor binary (musl targets preferred) and the CTFd plugin (on disk or the embedded copy).
7. Run the preflight playbook, then the main playbook. The playbook writes the event/admin settings into the CTFd container environment.
8. On first start, the plugin creates the admin and sets `ctf_name`, `ctf_description`, `user_mode`, and `setup = "true"`, so CTFd's first-run wizard does not appear.

`nervctf setup --upgrade` reads everything from `.nervctf.yml`, prompts only for the admin block when an older config lacks it, confirms, runs the preflight, and runs the upgrade playbook.

---

## Validator (`validator.rs`)

`validate_challenges(challenges) -> ValidationReport` runs per-challenge and cross-challenge checks and returns a report with errors (which block deploy) and warnings.

Error conditions include missing `name`, `category`, `value`, or `type`; `type: instance` without an `instance` block, `internal_ports`, or `connection`; `type: dynamic` without `extra.initial`; duplicate names; files that do not exist; and self-referencing requirements or `next` pointers.

Warning conditions include unknown YAML keys; `flag_mode: random` alongside a static `flags` list; a compose backend with no `compose_service`; `flag_delivery: file` without `flag_file_path`; `value: 0` on non-dynamic challenges; and missing `state`, `author`, or `version`.

The `RENDERED` set lists every known field path so unknown-key warnings fire only for genuinely unrecognised keys.

---

## Fix command (`fix.rs`)

`run_fix(base_dir, dry_run)` scans every `challenge.yml` for missing `state`, `author`, and `version` fields. `has_field()` checks for a top-level key at column 0; `inject_field()` inserts after a chosen sibling key, with a fallback.

---

## Directory scanner (`directory_scanner.rs`)

`DirectoryScanner::new(base_path)` scans `base_path/challenges/<category>/<challenge>/challenge.yml`.

```rust
const CHALLENGE_PATTERNS: &[&str] = &["challenge.yml", "challenge.yaml", "challenge.json"];
const CHALLENGE_EXTENSIONS: &[&str] = &["yml", "yaml", "json"];
```

Maximum depth is 5, and symlinks are not followed. It parses with `serde_yaml::Value` first to capture unknown keys, then re-parses as `Challenge`, injecting `source_path` and `unknown_yaml_keys` (both `#[serde(skip)]`).

---

## Challenge YAML

Minimal standard challenge:

```yaml
name: My Challenge
category: web
value: 100
type: standard
version: '0.3'
author: Author Name
state: visible
flags:
  - flag{example}
description: |
  Find the flag hidden in the page source.
```

Instance challenge with random flags:

```yaml
name: My Container Challenge
category: pwn
value: 0
type: instance
version: '0.3'
author: Author Name
state: visible
description: Connect to the service and exploit it.
extra:
  initial: 500
  decay: 50
  minimum: 100
instance:
  backend: docker
  image: .           # "." builds from the challenge directory; or a registry image
  internal_ports: [1337]
  connection: nc
  flag_mode: random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  random_flag_length: 16
  timeout_minutes: 45
  max_renewals: 3
```

Compose backend:

```yaml
instance:
  backend: compose
  compose_file: docker-compose.yml  # relative to the challenge directory
  compose_service: app
  internal_ports: [8080]
  connection: http
  flag_mode: random
  flag_delivery: env                # default: FLAG env var for ${FLAG} substitution
  # flag_delivery: file             # alternative: write the flag to a file in the container
  # flag_file_path: /challenge/flag # required for file delivery
  # flag_service: flag-receiver     # optional; defaults to compose_service
  timeout_minutes: 60
```

---

## Past bugs, kept for context

1. Hints used `value` instead of `cost`, so hints had zero cost.
2. The flag `data` field was treated as required, but CTFd omits it, which broke parsing.
3. The requirements untagged enum now accepts a simple list, an integer list, and the advanced object.
4. `FlagData` must serialise as `snake_case` (`case_insensitive`).
5. `GET /api/v1/challenges` returns at most 20 per page; the client loops through `meta.pagination.next`.
6. All files for a challenge must go in one multipart request.
7. Compose files reach the server as a tar.gz through `build-compose`; the monitor wipes the directory before extracting so Docker placeholder directories do not block extraction.
8. Those placeholder directories (created when Docker ran before the files existed) needed `remove_dir_all` before `create_dir_all` in the build handler.
9. A hardcoded `container_name:` in a compose file prevents multi-team instances; challenge authors must not set it.

---

## Cargo dependencies

**nervctf:** `clap 4` (derive), `reqwest 0.11` (multipart, json, rustls-tls), `tokio 1`, `serde`/`serde_json`/`serde_yaml`, `anyhow 1`, `walkdir 2`, `dialoguer`, `tempfile 3`.

**remote-monitor:** `axum 0.7` (multipart), `reqwest 0.11`, `tokio 1`, `serde`/`serde_json`, `rusqlite 0.31` (bundled), `rand 0.8`, `anyhow 1`, `tower-http 0.4` (trace), `tracing`/`tracing-subscriber`, `shlex 1`, `tempfile 3`.

---

## Build and release

```bash
nix develop .# --command cargo build
nix develop .# --command cargo build --release --target x86_64-unknown-linux-musl -p remote-monitor
nix develop .# --command cargo test
nix develop .# --command cargo test -p nervctf
```

To deploy the monitor: build the musl release binary, then run `nervctf setup`. The playbook copies the binary, builds the image, and starts the service. Alternatively, copy the binary manually with `scp` and trigger the playbook with the `monitor_binary` extra var.

---

## Known limitations

- The Vagrant backend is a stub; `lxc::destroy` is not called during cleanup, only `lxc::delete`.
- The per-team instance cap is global (`MAX_INSTANCES_PER_TEAM`), not per challenge.
- `nervctf deploy` asks for confirmation interactively, which makes it awkward to script.
- The player HTML page at `GET /instance/:name` has no server-side auth; the token is entered in the browser.
- The requirements comparison is shallow: it checks presence, not prerequisite identity.
- Dynamic scoring needs CTFd's Dynamic plugin, which stock CTFd does not include.
