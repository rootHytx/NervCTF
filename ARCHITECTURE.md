# NervCTF — Architecture & Implementation Reference

> Machine-readable context document for AI assistants. Covers the full system as of 2026-03-20.
> Always prefer reading actual source files over trusting stale details here.
> Workspace root: `/home/hytx/Desktop/CYBERSEC/tese/NervCTF`

---

## 1. PROJECT PURPOSE

NervCTF is a two-binary Rust + Python toolchain for managing CTF competitions on top of CTFd:

1. **`nervctf`** (CLI) — reads `challenge.yml` files from a local directory tree, deploys/syncs
   them to a CTFd instance via the remote-monitor, and registers per-challenge instance configs.
2. **`remote-monitor`** (HTTP server) — runs on the CTFd host, writes all CTFd data directly
   via MariaDB SQL, manages ephemeral challenge containers/VMs per team.
3. **CTFd plugin** (`nervctf_instance`, Python) — installed inside CTFd; adds the `instance`
   challenge type and proxies player lifecycle requests to the remote-monitor.

---

## 2. REPOSITORY LAYOUT

```
NervCTF/
├── Cargo.toml                   # workspace manifest; members: src/nervctf, src/remote-monitor
├── Cargo.lock
├── flake.nix                    # sole dev environment (provides rustc, cargo, ansible, openssl, …)
├── flake.lock
├── ARCHITECTURE.md              # this file
├── .nervctf.yml                 # local config (gitignored in practice)
│
├── src/
│   ├── nervctf/                 # CLI crate
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs           # re-exports: challenge_manager, ctfd_api, directory_scanner, fix, setup, validator, utils
│   │   │   ├── main.rs          # clap CLI entry point + deploy/sync logic
│   │   │   ├── utils.rs         # Config struct + load_config / save_config / find_config_path
│   │   │   ├── setup.rs         # `nervctf setup` — interactive Ansible deployment
│   │   │   ├── fix.rs           # `nervctf fix` — YAML issue fixer + container→instance migrator
│   │   │   ├── validator.rs     # `nervctf validate` — schema/lint warnings for challenge.yml
│   │   │   ├── directory_scanner.rs  # recursive challenge.yml finder (max_depth=5, no symlinks)
│   │   │   ├── challenge_manager/
│   │   │   │   ├── mod.rs       # ChallengeManager: CRUD wrappers over CtfdClient
│   │   │   │   └── sync.rs      # ChallengeSynchronizer + needs_update() + SyncAction enum
│   │   │   └── ctfd_api/
│   │   │       ├── mod.rs       # pub use CtfdClient, RequirementsQueue, models
│   │   │       ├── client.rs    # CtfdClient (async reqwest), execute(), pagination helpers
│   │   │       └── models/
│   │   │           └── mod.rs   # all data types (Challenge, FlagContent, HintContent, …)
│   │   │       endpoints/
│   │   │           ├── challenges.rs
│   │   │           ├── flags.rs
│   │   │           ├── hints.rs
│   │   │           ├── tags.rs
│   │   │           └── files.rs
│   │   └── assets/
│   │       ├── nervctf_playbook.yml     # Ansible playbook for full server setup
│   │       ├── install_docker_on_remote.sh  # referenced by playbook (not embed in binary)
│   │       └── ctfd-plugin/             # Python CTFd plugin (deployed via Ansible rsync)
│   │           ├── __init__.py
│   │           ├── models/
│   │           │   ├── __init__.py
│   │           │   └── challenge.py     # InstanceChallenge SQLAlchemy model
│   │           └── assets/
│   │               ├── view.{html,js}
│   │               ├── create.{html,js}
│   │               └── update.{html,js}
│   │
│   └── remote-monitor/          # HTTP server crate
│       ├── Cargo.toml
│       ├── assets/
│       │   └── admin.html       # admin dashboard (embedded via include_str! at compile time)
│       └── src/
│           ├── main.rs          # axum 0.7 server, all routes, AppState, background expiry
│           ├── db.rs            # SQLite via rusqlite; Db = Arc<Mutex<Connection>>
│           └── instance/
│               ├── mod.rs       # provision(), cleanup_container(), generate_flag(), sanitize_name()
│               ├── docker.rs    # pick_free_port(), run_container(), remove_container(), build_image()
│               ├── compose.rs   # up(), down(), compose_cmd() — real implementation
│               ├── lxc.rs       # launch(), delete() — real implementation
│               └── vagrant.rs   # up() — stub (returns error)
│
└── templates/                   # challenge.yml templates for authors
    ├── standard/
    ├── docker/
    ├── compose/
    ├── lxc/
    └── vagrant/
```

---

## 3. DEV ENVIRONMENT

**Always use Nix flake for any build/run command:**
```
nix develop .# --command cargo build
nix develop .# --command cargo build --release -p remote-monitor
nix develop .# --command cargo test
```

The flake provides: `pkg-config`, `openssl`, `rustc`, `cargo`, `rustfmt`, `clippy`, `ansible`.
`PKG_CONFIG_PATH` is set for openssl. There is no `shell.nix` — only `flake.nix`.

**Cross-compilation gotcha** (musl/mingw targets): Each cross-stdenv sets `CC` globally;
pin each target via `CC_<triple>` env vars + reset `CC` to native gcc in shellHook to prevent
the last cross-compiler from poisoning native builds.

---

## 4. CONFIG SYSTEM

Priority (highest wins):
1. CLI flags: `--monitor-url`, `--monitor-token`
2. Env vars: `MONITOR_URL`, `MONITOR_TOKEN`
3. `.nervctf.yml` / `.nervctf.yaml` (walked up from challenges_dir)

**`Config` struct** (`utils.rs`):
```rust
pub struct Config {
    pub monitor_url:              Option<String>,
    pub monitor_token:            Option<String>,
    pub challenges_dir:           Option<String>,
    pub target_ip:                Option<String>,   // setup only
    pub target_user:              Option<String>,   // setup only
    pub ssh_pubkey_path:          Option<String>,   // setup only
    pub ctfd_remote_path:         Option<String>,   // setup only
    pub monitor_port:             Option<String>,   // setup only
    pub max_concurrent_provisions: Option<u32>,     // monitor tuning
    pub max_instances_per_team:   Option<u32>,     // monitor tuning (0 = unlimited)
    pub runner_ip:                Option<String>,   // split-machine mode
    pub runner_user:              Option<String>,   // split-machine mode
}
```

`load_config(start_dir)` walks up directories looking for `.nervctf.yml`.
`save_config(config, path)` serialises to YAML (skips `None` fields).

**Split-machine mode**: when `runner_ip` + `runner_user` are set, `nervctf deploy` rsyncs
challenge files directly to `{runner_user}@{runner_ip}:/home/docker/challenges/` instead of
uploading via the monitor. After rsync, it calls `POST /api/v1/instance/build-compose-remote`
to trigger image builds on the runner via SSH.

---

## 5. DATA MODELS (`ctfd_api/models/mod.rs`)

### ChallengeType
```rust
enum ChallengeType { Standard, Dynamic, Instance }
// serde: "standard" | "dynamic" | "instance"
// "instance" challenges deploy to CTFd as "standard" or "dynamic" depending on
// whether extra.initial is set. CTFd itself never sees "instance" as the type.
```

### Challenge (top-level YAML struct)
Required fields: `name`, `category`, `value`, `type`
Key optional fields:
- `extra: Option<Extra>` — `{initial, decay, minimum}` for Dynamic scoring
- `instance: Option<InstanceConfig>` — only for `type: instance`
- `flags: Option<Vec<FlagContent>>`
- `hints: Option<Vec<HintContent>>`
- `tags: Option<Vec<Tag>>`
- `files: Option<Vec<String>>` — relative paths from challenge dir
- `requirements: Option<Requirements>`
- `state: Option<State>` — `"hidden"` | `"visible"`
- `connection_info: Option<String>`
- `attempts: Option<u32>`
- `source_path: String` — injected by scanner, not serialised; absolute path to challenge dir
- `unknown_yaml_keys: Vec<String>` — injected by scanner for lint warnings

### FlagContent (untagged enum)
```rust
enum FlagContent {
    Simple(String),
    Detailed { id, challenge_id, type_: FlagType, content: String, data: Option<FlagData> }
}
// FlagType: "static" | "regex"
// FlagData: "case_sensitive" | "case_insensitive"  (snake_case serde)
```

### HintContent (untagged enum)
```rust
enum HintContent {
    Simple(String),
    Detailed { content: String, cost: Option<u32>, title: Option<String> }
}
// .content_str() helper extracts content from either variant
```

### Tag (untagged enum)
```rust
enum Tag {
    Simple(String),
    Detailed { challenge_id, id, value: String }
}
// .value_str() helper
```

### Requirements (untagged enum)
```rust
enum Requirements {
    Simple(Vec<serde_json::Value>),          // list of names or integer IDs
    Advanced { prerequisites: Vec<Value>, anonymize: bool }
}
// .prerequisite_names() → Vec<String> (integers coerced to string)
```

### InstanceConfig
```rust
pub struct InstanceConfig {
    pub backend: InstanceBackend,  // docker | compose | lxc | vagrant
    pub image: Option<String>,
    pub compose_file: Option<String>,    // default: "docker-compose.yml"
    pub compose_service: Option<String>,
    pub lxc_image: Option<String>,
    pub vagrantfile: Option<String>,
    pub internal_port: u32,
    pub connection: String,              // "nc" | "http" | "ssh"
    pub timeout_minutes: Option<u32>,
    pub max_renewals: Option<u32>,
    pub command: Option<String>,
    pub flag_mode: Option<InstanceFlagMode>,    // "static" | "random"
    pub flag_prefix: Option<String>,
    pub flag_suffix: Option<String>,
    pub random_flag_length: Option<u32>,
    pub flag_delivery: Option<FlagDelivery>,   // "env" (default) | "file"
    pub flag_file_path: Option<String>,        // absolute path inside container (file mode)
    pub flag_service: Option<String>,          // compose service receiving flag file
}
```

### RequirementsQueue
Topological sorter for deploy ordering. Uses Kahn's algorithm.
`resolve_dependencies(actions: Vec<SyncAction>) → Vec<SyncAction>` — returns Create/Update
actions in dependency order, UpToDate/RemoteOnly appended after.

---

## 6. CLI COMMANDS (`main.rs`)

```
nervctf [OPTIONS] <COMMAND>

Options:
  -c, --challenges-dir <PATH>   default "."
  -v, --verbose
  --monitor-url <URL>
  --monitor-token <TOKEN>

Commands:
  deploy [--dry-run] [--recreate]   create new challenges + update changed ones
  list [--detailed]                 list local challenges
  scan [--detailed]                 scan + print statistics
  validate [--debug]                lint challenge.yml files (full view with --debug)
  fix [--dry-run]                   interactive YAML fixer (state/author/version)
  setup [--upgrade]                 Ansible-based server deployment wizard
```

`--recreate` forces all challenges into the update queue (skips up-to-date check, skips
the `Proceed? (y/N)` prompt). Useful for re-syncing files to the runner or rebuilding images.

### Deploy flow (key logic in `deploy_challenges`):

All API calls go to `remote-monitor`, which executes them against CTFd MariaDB directly.

1. Scan local challenges tree → `Vec<Challenge>`
2. Run validator; abort if any errors
3. `GET /api/v1/challenges` (paginated) → remote list
4. For each local challenge:
   - Not on remote → `create_challenge_phase1()` (`POST /challenges` → monitor → SQL INSERT)
   - On remote + `needs_update()` or `--recreate` → `update_challenge_phase1()` (`PATCH /challenges/{id}` → monitor → SQL UPDATE)
5. After all base creates/updates: flags, tags, files, hints, requirements, state patches
6. For `type: instance`:
   - Deploy to CTFd as `standard` or `dynamic` (based on `extra.initial` presence)
   - **Split-machine mode** (`runner_ip` set):
     - rsync challenge dir to `{runner_user}@{runner_ip}:/home/docker/challenges/<name>/`
     - `POST /api/v1/instance/build-compose-remote` → monitor SSHes to runner and runs `docker compose build`
   - **Single-machine mode**:
     - `POST /api/v1/instance/build` (docker, local image path) or `POST /api/v1/instance/build-compose` (compose) — uploads tar.gz
   - `POST /api/v1/instance/register` — registers `InstanceConfig` on monitor

### `needs_update()` (`challenge_manager/sync.rs`)
Free function (also used as method via delegation). Compares:
`category`, `value`, `description`, `state`, `connection_info`, `attempts`,
`extra` (JSON comparison), `flags` (sorted content strings), `tags` (sorted values),
`hints` (sorted content strings), `requirements` (presence only, not deep comparison).
Note: CTFd list endpoint never returns flags/tags/hints, so those comparisons only fire
when both sides are `Some` (i.e. after a per-challenge detail fetch).

### File uploads
All files for a challenge must be sent in ONE multipart request (multiple `"file"` parts).
One request per file → CTFd returns 500. Matches ctfcli's `_create_all_files()` pattern.

### CTFd requirements
- Challenge Visibility must be "Public" (Admin → Config → Visibility). Private mode blocks
  even valid API tokens from `GET /api/v1/challenges`.
- Upload dir: `chown -R 1001:1001 <CTFd>/.data/CTFd/uploads` when using Docker.

---

## 7. CTFD API CLIENT (`ctfd_api/client.rs`)

```rust
pub struct CtfdClient {
    client: reqwest::Client,
    base_url: String,   // {monitor_url}/api/v1
    // auth: Authorization: Token <monitor_token>
}
```

All API calls go to `remote-monitor` (`{monitor_url}/api/v1/...`), which handles them via
direct MariaDB SQL. Response shape is CTFd-compatible (`{"success": true, "data": ...}`).
No `CTFD_URL` or `CTFD_API_KEY` — the monitor is the sole gateway.

- `execute(method, path, body) → Result<Option<Value>>`
- `get_challenges()` — paginated via `meta.pagination.next`; loops until no next page
- `get_challenge_id(name)` — scans all challenges for matching name
- `create_challenge`, `update_challenge`, `delete_challenge`
- `create_flag`, `delete_flag`, `get_challenge_flags_endpoint`
- `create_tag`, `delete_tag`, `get_challenge_tags_endpoint`
- `create_hint`, `delete_hint`, `get_challenge_hints_endpoint`
- `create_file` (multipart/form-data), `delete_file`, `get_challenge_files_endpoint`
- `parse_response` — private; unwraps `{"success": true, "data": ...}` envelope

**Dependency conflict**: axum 0.7 uses `http 1.x`; reqwest 0.11 uses `http 0.2.x`.
Bridge via string conversion (`.as_str()` / `.as_bytes()`). Do not mix HeaderMap types.

---

## 8. REMOTE-MONITOR SERVER (`remote-monitor/src/main.rs`)

### AppState
```rust
pub struct AppState {
    pub monitor_token: String,          // MONITOR_TOKEN
    pub public_host: String,            // PUBLIC_HOST
    pub db: Db,                         // Arc<Mutex<Connection>> (SQLite)
    pub ctfd_pool: mysql_async::Pool,   // CTFd MariaDB pool
    pub challenges_base_dir: String,    // CHALLENGES_BASE_DIR
    pub ctfd_uploads_dir: String,       // CTFD_UPLOADS_DIR
    pub runner_ssh_target: Option<String>,  // RUNNER_SSH_TARGET (split-machine mode)
    pub provision_sem: Arc<Semaphore>,  // MAX_CONCURRENT_PROVISIONS
    pub max_instances_per_team: u64,   // MAX_INSTANCES_PER_TEAM (0 = unlimited)
}
```

### Environment variables consumed by remote-monitor
| Var | Default | Purpose |
|-----|---------|---------|
| `CTFD_DB_URL` | required | MariaDB URL (`mysql://user:pass@host/db`) |
| `MONITOR_TOKEN` | required | Admin auth token |
| `PUBLIC_HOST` | required | Hostname returned to players in connection strings |
| `CTFD_UPLOADS_DIR` | `""` | Absolute path to CTFd uploads dir (for file writes) |
| `CHALLENGES_BASE_DIR` | `/opt/nervctf/challenges` | Root for challenge files |
| `RUNNER_SSH_TARGET` | `""` | SSH target for split-machine mode (e.g. `docker@192.168.1.50`) |
| `MONITOR_PORT` | `33133` | TCP bind port |
| `MONITOR_BIND` | `0.0.0.0` | TCP bind address |
| `DB_PATH` | `./monitor.db` | SQLite file path |
| `MAX_CONCURRENT_PROVISIONS` | `4` | Semaphore limit for concurrent docker/compose ops |
| `MAX_INSTANCES_PER_TEAM` | `0` | Max active instances per team across all challenges (0 = unlimited) |
| `CTFD_DB_SYNC_INTERVAL` | `30` | Seconds between CTFd MariaDB → SQLite sync cycles |

### Admin dashboard

`GET /admin?token=<MONITOR_TOKEN>` — self-contained HTML (no CDN; air-gap safe).
Token via `?token=` query param or `Authorization: Token` header.

Three auto-refreshing tables:
- **Flag sharing alerts** (15 s) — flag submitted to wrong team's challenge instance
- **Active instances** (15 s) — all running containers with team/user/host:port/expiry
- **Recent flag attempts** (30 s) — last 200 attempts across all teams

### Routes

**No auth:**
- `GET /health` → `{"status": "ok"}`
- `GET /instance/:name` → HTML player UI page (inline, no CDN)

**Admin (`Authorization: Token <MONITOR_TOKEN>` or `?token=`):**
- `GET /admin` → serves admin dashboard HTML

**Admin (`Authorization: Token <MONITOR_TOKEN>`):**
- `POST /api/v1/instance/build` — multipart `challenge_name` + `context` (tar.gz); builds docker image
- `POST /api/v1/instance/build-compose` — multipart; wipes dir first, extracts tar.gz, builds compose images (single-machine)
- `POST /api/v1/instance/build-compose-remote` — JSON `{challenge_name, compose_file?, challenges_dir?}`; SSHes to runner and runs `docker compose build` (split-machine)
- `POST /api/v1/instance/register` — `{challenge_name, ctfd_id, backend, config_json}`
- `GET /api/v1/instance/list` → registered configs
- `GET /api/v1/admin/instances` → all active instances as JSON
- `GET /api/v1/admin/attempts[?alerts_only=true]` → flag attempt log (200 max) or sharing alerts
- `GET /api/v1/admin/solves` → correct solves per team (from ctfd_solves cache)
- CTFd CRUD (all via MariaDB SQL):
  - `GET/POST /api/v1/challenges`, `GET/PATCH/DELETE /api/v1/challenges/{id}`
  - `GET/POST /api/v1/flags`, `DELETE /api/v1/flags/{id}`
  - `GET/POST /api/v1/hints`, `DELETE /api/v1/hints/{id}`
  - `GET/POST /api/v1/tags`, `DELETE /api/v1/tags/{id}`
  - `GET/POST /api/v1/files`, `DELETE /api/v1/files/{id}` (disk + DB)
  - `POST /api/v1/topics`

**Plugin auth** (admin token + explicit `team_id` in body — called by CTFd plugin):
- `GET /api/v1/plugin/info?challenge_name=X&team_id=N`
- `POST /api/v1/plugin/request` → provisions instance
- `POST /api/v1/plugin/renew` → extends expiry
- `DELETE /api/v1/plugin/stop` → destroys one team's instance
- `DELETE /api/v1/plugin/stop_all` → destroys all instances for a challenge
- `POST /api/v1/plugin/solve` → marks solved + tears down instance
- `POST /api/v1/plugin/attempt` → records flag submission; detects sharing via `team_flags` table

**Player auth** (CTFd user token validated via direct MariaDB lookup):
- `POST /api/v1/instance/request` → provisions instance
- `GET /api/v1/instance/info?challenge_name=X`
- `POST /api/v1/instance/renew`
- `DELETE /api/v1/instance/stop`

### Background tasks

**Expiry task** — every 30 s:
- `get_expired_instances()` → for each: `cleanup_container(id, runner_ssh)` + `delete_instance()` + `delete_flag(ctfd_flag_id)`
- Orphan cleanup: `list_ctf_projects()` (compose) → stop any not in DB

**CTFd sync task** — every `CTFD_DB_SYNC_INTERVAL` s (default 30):
- `ctfd_db::sync_solves()` — full-replace `ctfd_solves` SQLite cache from MariaDB `submissions`; reverts solved instances where submission was deleted; removes stale `is_correct=1` flag_attempts
- `ctfd_db::sync_users_and_teams()` — full-replace `ctfd_teams` + `ctfd_users` caches

### `check_monitor_auth(headers, expected_token) → bool`
Checks `Authorization: Token <value>` header. Used for all admin and plugin routes.

### `validate_ctfd_token(pool, token) → Option<i64>`
Direct MariaDB query on `users` table (`SELECT team_id FROM users WHERE token = ? AND banned = 0 AND hidden = 0`).
Returns `None` if invalid/no team. Used for player routes.

---

## 9. SQLITE SCHEMA (`remote-monitor/src/db.rs`)

```sql
CREATE TABLE instance_configs (
    challenge_name  TEXT PRIMARY KEY,
    ctfd_id         INTEGER NOT NULL,
    backend         TEXT NOT NULL,    -- "docker"|"compose"|"lxc"|"vagrant"
    config_json     TEXT NOT NULL,    -- full InstanceConfig as JSON
    image_tag       TEXT,             -- resolved after build
    updated_at      TEXT DEFAULT (datetime('now'))
);

CREATE TABLE instances (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    user_id         INTEGER,          -- CTFd user who requested
    container_id    TEXT,             -- docker ID, compose project name, or LXC name
    host            TEXT NOT NULL,
    port            INTEGER NOT NULL,
    connection_type TEXT NOT NULL,
    status          TEXT NOT NULL,    -- "running"|"provisioning"|"solved"
    flag            TEXT,             -- per-team random flag (null for static)
    ctfd_flag_id    INTEGER,          -- CTFd flag ID for cleanup on stop/expire
    renewals_used   INTEGER DEFAULT 0,
    created_at      TEXT DEFAULT (datetime('now')),
    expires_at      TEXT NOT NULL,    -- "YYYY-MM-DD HH:MM:SS" UTC
    UNIQUE(challenge_name, team_id)
);

CREATE TABLE flag_attempts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    user_id         INTEGER NOT NULL,
    submitted_flag  TEXT NOT NULL,
    is_correct      INTEGER NOT NULL DEFAULT 0,    -- 0|1 boolean
    is_flag_sharing INTEGER NOT NULL DEFAULT 0,    -- 1 = flag belonged to a different team
    owner_team_id   INTEGER,           -- team whose instance generated this flag (if sharing)
    timestamp       TEXT DEFAULT (datetime('now'))
);

-- Permanent flag history for sharing detection (never deleted on instance stop)
CREATE TABLE team_flags (
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    flag            TEXT NOT NULL,
    created_at      TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (challenge_name, team_id, flag)
);

-- Read-only cache of correct solves synced from CTFd MariaDB submissions
CREATE TABLE ctfd_solves (
    challenge_name  TEXT NOT NULL,
    team_id         INTEGER NOT NULL,
    user_id         INTEGER,
    solved_at       TEXT,
    PRIMARY KEY (challenge_name, team_id)
);

-- Cached team/user names from CTFd for display
CREATE TABLE ctfd_teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE ctfd_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, team_id INTEGER);
```

`Db = Arc<Mutex<Connection>>`. WAL mode enabled. Migrations are run on every `open()`.

Key functions:
- `upsert_config / get_config / get_image_tag / update_image_tag / list_configs`
- `insert_provisioning_stub` — INSERT OR IGNORE placeholder row (`status='provisioning'`)
- `insert_instance` — UPSERT (resets `renewals_used=0`); also writes to `team_flags`
- `get_instance / delete_instance / mark_instance_solved`
- `delete_all_instances_for_challenge` — used on challenge delete
- `get_used_ports → HashSet<u16>` / `get_expired_instances` / `list_all_instances`
- `insert_flag_attempt / list_flag_attempts(limit) / list_sharing_alerts`
- `find_flag_owner` — queries `team_flags` (permanent); returns `Some(owner_team_id)` if flag sharing
- `replace_ctfd_solves` — full DELETE + INSERT cycle (so deleted submissions are reflected)
- `revert_unsolved_instances` — sets `status='running'` where `status='solved'` but no ctfd_solve entry
- `delete_stale_correct_attempts` — removes `is_correct=1` flag_attempts with no matching ctfd_solve
- `replace_ctfd_teams_and_users` — full-replace name caches
- `has_correct_solve / list_correct_solves`

---

## 10. INSTANCE BACKENDS (`remote-monitor/src/instance/`)

### `mod.rs` — central dispatch

`provision(db, challenge_name, team_id, user_id, config, public_host, ctfd_pool, runner_ssh) → (host, port, conn_type, expires_at)`

- `generate_flag(config)` → `Option<String>` — `None` unless `flag_mode = "random"`; generates `<prefix><N chars><suffix>`
- `sanitize_name(name)` — lowercase, non-alphanumeric/hyphen → hyphen, trim
- container naming: `ctf-<sanitized>-t<team_id>`
- `expires_at_string(minutes)` → `"YYYY-MM-DD HH:MM:SS"` UTC (no chrono dep)
- `cleanup_container(container_id, runner_ssh: Option<&str>)` — tries compose::down, lxc::delete, docker::remove_container

All docker/compose/lxc functions accept `runner_ssh: Option<&str>`. When set, commands are
executed on the runner via `ssh -o BatchMode=yes <target> <cmd>`.

### `docker.rs`
- `pick_free_port(used_ports) → u16` — random in 40000–60000
- `run_container(..., runner_ssh) → String` — `docker run -d -p host:internal ...`
- `remove_container(id, runner_ssh)` — `docker rm -f`
- `build_image(context_tar_path, image_tag, runner_ssh)` — `docker build -t <tag>`

### `compose.rs`
- `up(..., runner_ssh) → (u16, String)`
  - Writes `<project_name>.override.yml` (port mapping + optional flag file bind-mount)
  - In split-machine mode: writes files to runner via `ssh mkdir -p && cat > file`
  - `flag_delivery = "env"`: injects `FLAG=<value>` env var; challenge compose uses `${FLAG}`
  - `flag_delivery = "file"`: writes `<project_name>.flag` + bind-mount override for `flag_service`
  - Runs: `DOCKER_BUILDKIT=1 docker compose -f <compose_file> -f <override> -p <project_name> up -d --force-recreate`
- `down(project_name, runner_ssh?)` — `docker compose -p <name> down -v`
- `build(compose_file, runner_ssh?)` — `docker compose -f <file> build`
- `list_ctf_projects()` — lists running `ctf-*` compose projects (for orphan cleanup)

### `lxc.rs`
- `launch(lxc_image, name, host_port, internal_port, flag)` — `lxc launch` → wait → proxy port → flag injection
- `delete(name)` — `lxc stop --force` + `lxc delete --force`

### `vagrant.rs`
- `up(...)` — stub, returns error

---

## 11. CTFD PLUGIN (`src/nervctf/assets/ctfd-plugin/`)

Plugin name: `nervctf_instance`. Installed to `CTFd/plugins/nervctf_instance/`.

### `__init__.py` — `InstanceChallengeType(BaseChallenge)`
Registered as `CHALLENGE_CLASSES["instance"]`. CTFd routes it via polymorphic `type = "instance"`.

Key methods:
- `create(request)` — creates `InstanceChallenge` row, calls `_register_with_monitor()`
- `read(challenge)` — returns challenge data + backend/connection/timeout/flag_mode
- `update(challenge, request)` — updates fields, re-registers with monitor
- `delete(challenge)` — calls `_stop_all_instances()` then cascades DB deletes
- `solve(user, team, challenge, request)` — delegates to `BaseChallenge.solve()`, then calls
  `POST /api/v1/plugin/solve` on monitor to tear down the team's instance
- `attempt(challenge, request)` — delegates to `BaseChallenge.attempt()` for verdict
  (returns a `ChallengeResponse` object with `.success` bool attribute, not a tuple);
  then fire-and-forgets `POST /api/v1/plugin/attempt` to monitor (timeout=0.5s, swallowed)
  with `{challenge_name, team_id, user_id, submitted_flag, is_correct}`.
  Never blocks the CTFd flag submission response.

### `_register_with_monitor(challenge)`
POSTs `{challenge_name, ctfd_id, backend, config_json}` to monitor's
`POST /api/v1/instance/register`. Called on create and update.

### Blueprint routes (all `@authed_only`):
All use `get_current_team()` to get `team_id`. Forward to monitor's `/api/v1/plugin/*`
routes using admin token + explicit `team_id` in body/params (players never get admin token).
- `GET /api/v1/containers/info/<challenge_id>`
- `POST /api/v1/containers/request` — also sends `user_id` from `get_current_user()`
- `POST /api/v1/containers/renew`
- `POST /api/v1/containers/stop`

### `models/challenge.py` — `InstanceChallenge(Challenges)`
SQLAlchemy polymorphic model (`__mapper_args__ = {"polymorphic_identity": "instance"}`).
Extra columns: `backend`, `image`, `command`, `compose_file`, `compose_service`,
`lxc_image`, `vagrantfile`, `internal_port`, `connection`, `timeout_minutes`,
`max_renewals`, `flag_mode`, `flag_prefix`, `flag_suffix`, `random_flag_length`,
`initial_value`, `minimum_value`, `decay_value`, `decay_function`.

### `assets/view.js`
DOM API only (no innerHTML). Fetch/Extend/Terminate buttons call `/api/v1/containers/*`.
Displays connection string and countdown timer when instance is running.
`expires_at` from monitor is `"YYYY-MM-DD HH:MM:SS"` UTC; plugin converts to ms in
`_sqlite_to_ms()` before sending to JS (which treats it as Unix ms).

---

## 12. ANSIBLE PLAYBOOK (`assets/nervctf_playbook.yml`)

Idempotent. Target group `ctfd`. Key extra vars: `ssh_key`, `ctfd_path`, `monitor_token`,
`monitor_port`; optional: `monitor_binary`, `plugin_src`, `ctfd_api_key`.

Tasks in order:
1. Install Docker (if `docker --version` fails)
2. Create `docker` group + user, authorize SSH pubkey
3. Clone CTFd (if not present), start via `docker compose up --build -d --no-recreate`
4. Install LXD via snap + `lxd init --auto`
5. Install Vagrant via HashiCorp apt repo
6. Add `docker` user to `lxd` and `libvirt` groups
7. Deploy plugin: rsync `plugin_src/` to `ctfd_path/CTFd/plugins/nervctf_instance/`
8. Create `/data/challenges` on host (owned by docker user)
9. Copy remote-monitor binary + write Dockerfile + `docker build -t nervctf-monitor:latest`
10. Write `docker-compose.override.yml` injecting `remote-monitor` service + CTFd env vars:
    - `NERVCTF_MONITOR_URL=http://remote-monitor:<port>`
    - `NERVCTF_MONITOR_TOKEN=<token>`
11. `docker compose up -d --force-recreate`

### docker-compose.override.yml (written by playbook)
```yaml
services:
  ctfd:
    environment:
      - NERVCTF_MONITOR_URL=http://remote-monitor:33133
      - NERVCTF_MONITOR_TOKEN=<token>
  remote-monitor:
    image: nervctf-monitor:latest
    restart: unless-stopped
    networks:
      - default
      - internal        # joins CTFd's internal network to reach MariaDB
    environment:
      - CTFD_DB_URL=mysql://ctfd:<db_password>@db/ctfd
      - CTFD_UPLOADS_DIR=<ctfd_path>/.data/CTFd/uploads
      - CTFD_URL=http://ctfd:8000
      - MONITOR_TOKEN=<token>
      - PUBLIC_HOST=<ansible_host>
      - MONITOR_PORT=33133
      - DB_PATH=/data/monitor.db
      - CHALLENGES_BASE_DIR=/data/challenges
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - /usr/libexec/docker/cli-plugins:/usr/libexec/docker/cli-plugins:ro
      - remote_monitor_data:/data
      - /data/challenges:/data/challenges
      - <ctfd_path>/.data/CTFd/uploads:<ctfd_path>/.data/CTFd/uploads
    ports:
      - "33133:33133"
networks:
  internal: {}
volumes:
  remote_monitor_data:
```

**Network note**: CTFd's `db` service is on an `internal: true` network. The override
adds `remote-monitor` to that network so it can reach MariaDB at hostname `db`.

**Critical path constraint**: `/data/challenges` must use identical absolute paths on the host
and inside the monitor container. Challenge docker-compose.yml files reference files like
`/data/challenges/my-challenge/certs/server.pem`. The monitor container accesses the same
path via the bind mount, and the host Docker daemon resolves those paths when the monitor
instructs it to launch challenge containers (Docker-outside-of-Docker via socket mount).

---

## 13. SETUP COMMAND (`setup.rs`)

Interactive wizard:
1. Prompts: base_dir, target_ip, target_user, ctfd_remote_path (default `/home/docker/CTFd`),
   monitor_port (default `33133`)
2. Generates or reuses `monitor_token` (32 hex bytes from `/dev/urandom`)
3. Lists `~/.ssh/*.pub` keys; offers to generate new keypair
4. Saves config to `.nervctf.yml` before running playbook
5. TCP-checks port 80 on target; if already up, confirms redeploy
6. Locates `remote-monitor` binary (prefers musl targets in target/ tree or next to exe)
7. Locates `ctfd-plugin` dir (next to exe, or in workspace `src/nervctf/assets/ctfd-plugin/`)
8. Writes playbook + inventory to tmpdir, runs `ansible-playbook`
9. If `ansible-playbook` not in PATH: falls back to `nix develop <flake_dir> --command ansible-playbook`

---

## 14. VALIDATOR (`validator.rs`)

`validate_challenges(base_dir, fix_mode) → Result<()>`

Runs `DirectoryScanner` + lint checks. Reports errors (block deploy) and warnings (advisory).

Error conditions:
- Missing required fields: `name`, `category`, `value`, `type`
- `type: instance` without `instance:` block
- `type: instance` without `instance.internal_port`
- `type: instance` without `instance.connection`
- `type: dynamic` without `extra.initial`

Warning conditions:
- Unknown YAML keys (captured in `unknown_yaml_keys` by scanner)
- `type: instance` with `flag_mode: random` but also has static `flags:` list
- `type: instance, backend: compose` with no `compose_service` set
- `type: instance, flag_delivery: file` without `flag_file_path`
- `value: 0` on non-dynamic challenges
- Missing `state` / `author` / `version` fields
- Empty `flags` list
- Duplicate challenge names

`RENDERED` constant: set of field paths that are expected (suppresses unknown-key warnings for known fields like `instance.flag_delivery`, `instance.flag_file_path`, etc.).

---

## 15. FIX COMMAND (`fix.rs`)

### `run_fix(base_dir, dry_run)`
Scans all `challenge.yml` for missing `state`, `author`, `version` fields.
Uses `has_field()` (top-level key detection, column-0 check) and `inject_field()`
(inject after specific sibling key, with fallback).

---

## 16. DIRECTORY SCANNER (`directory_scanner.rs`)

`DirectoryScanner::new(base_path)` — scans `base_path/challenges/<category>/<challenge>/challenge.yml`.

Constants:
```rust
const CHALLENGE_PATTERNS: &[&str] = &["challenge.yml", "challenge.yaml", "challenge.json"];
const CHALLENGE_EXTENSIONS: &[&str] = &["yml", "yaml", "json"];
```

Max depth: 5. Does not follow symlinks.

Uses `serde_yaml::Value` for initial parse to extract unknown keys, then re-parses as `Challenge`.
`source_path` and `unknown_yaml_keys` are injected after parse (both marked `#[serde(skip)]`).

---

## 17. CHALLENGE YAML FORMAT

Minimal example:
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

Instance challenge example:
```yaml
name: My Container Challenge
category: pwn
value: 0         # 0 for dynamic scoring
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
  image: .           # "." = local build from challenge dir; or registry image
  internal_port: 1337
  connection: nc
  flag_mode: random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  random_flag_length: 16
  timeout_minutes: 45
  max_renewals: 3
```

Compose backend example:
```yaml
instance:
  backend: compose
  compose_file: docker-compose.yml  # relative to challenge dir
  compose_service: app              # service name that exposes the port
  internal_port: 8080
  connection: http
  flag_mode: random
  flag_delivery: env                # default: FLAG env var for ${FLAG} substitution
  # flag_delivery: file             # alternative: write flag to file in container
  # flag_file_path: /challenge/flag # required for file delivery
  # flag_service: flag-receiver     # optional: service receiving flag file (defaults to compose_service)
  timeout_minutes: 60
```

---

## 18. KEY BUGS FIXED (for historical context)

1. **Hints `value` vs `cost`**: CTFd API uses `cost`; old code sent `value` → hints had 0 cost
2. **Flag `data` field optional**: CTFd returns omitted `data`; required field caused parse failures
3. **Requirements untagged enum**: supports simple list `["name"]`, int list `[1,2]`, and
   advanced object `{prerequisites: [...], anonymize: bool}`
4. **FlagData serialization**: must be `snake_case` ("case_insensitive"), not "caseinsensitive"
5. **CTFd pagination**: `GET /api/v1/challenges` returns 20/page max; must loop via `meta.pagination.next`
6. **File uploads**: all files must be in one multipart request
7. **Compose files not reaching server**: CLI must upload tar.gz via `build-compose` endpoint;
   monitor wipes existing dir before extraction to prevent Docker placeholder directories
   (Docker creates empty dirs at bind-mount source paths when they don't exist at startup)
8. **Docker placeholder dirs blocking tar**: if Docker ran before files existed, certs/etc.
   become root-owned dirs; fixed by `remove_dir_all` before `create_dir_all` in build handler
9. **Hardcoded `container_name:` in compose files**: prevents multi-team instances; challenge
   authors must not set `container_name:` in their docker-compose.yml

---

## 19. CARGO DEPENDENCIES (notable)

### nervctf
- `clap 4` (derive) — CLI
- `reqwest 0.11` (blocking, multipart, json, rustls-tls) — HTTP client
- `tokio 1` (full) — async runtime
- `serde 1`, `serde_json`, `serde_yaml` — serialisation
- `anyhow 1` — error handling
- `walkdir 2` — directory traversal
- `dialoguer` — interactive prompts (setup/fix)
- `tempfile 3` — temp dir for Ansible assets

### remote-monitor
- `axum 0.7` (multipart) — HTTP server
- `reqwest 0.11` — outbound HTTP (CTFd proxy + token validation)
- `tokio 1` (full) — async runtime
- `serde 1`, `serde_json` — serialisation
- `rusqlite 0.31` (bundled) — SQLite
- `rand 0.8` — random flag generation + port selection
- `anyhow 1` — error handling
- `tower-http 0.4` (trace) — request tracing middleware
- `tracing 0.1`, `tracing-subscriber 0.3` (env-filter) — structured logging
- `shlex 1` — shell argument quoting
- `tempfile 3` — temp files for docker build

---

## 20. BUILD & RELEASE

```bash
# Dev build (both crates)
nix develop .# --command cargo build

# Release build for Linux musl (deployable to server without NixOS interpreter)
nix develop .# --command cargo build --release --target x86_64-unknown-linux-musl -p remote-monitor

# Run tests
nix develop .# --command cargo test

# Run tests for one crate
nix develop .# --command cargo test -p nervctf
```

Deployment of remote-monitor:
1. Build musl release binary
2. Run `nervctf setup` — playbook copies binary, builds Docker image, starts service
3. Or manually: `scp target/x86_64-unknown-linux-musl/release/remote-monitor user@host:~`
   then trigger playbook with `monitor_binary` extra-var

---

## 21. KNOWN LIMITATIONS / FUTURE WORK

- Vagrant backend is a stub (returns error); `lxc::destroy` not called on cleanup (only `lxc::delete`)
- Per-team instance cap is global (`MAX_INSTANCES_PER_TEAM` env var), not per-challenge
- `sync` command asks for confirmation interactively; not scriptable
- No authentication on player HTML page (`GET /instance/:name`) — token entered client-side
- Challenge requirements comparison is shallow (presence only, not prerequisite identity)
- CTFd API key for dynamic scoring challenges: `extra.initial` triggers Dynamic type but
  CTFd's Dynamic plugin must be installed; base CTFd does not include it by default
