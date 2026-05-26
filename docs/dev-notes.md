# NervCTF — Developer Notes

Operational knowledge, architectural constraints, and gotchas not obvious from reading the code.
See `ARCHITECTURE.md` for the system overview and `docs/` for feature documentation.

---

## Build Environment

Always use the Nix flake devShell for all cargo/build commands:

```bash
nix develop .# --command <cmd>
```

`flake.nix` provides: rustc, cargo, rustfmt, clippy, pkg-config, openssl, ansible.
`shell.nix` has been removed — flake.nix is the sole dev environment definition.
`PKG_CONFIG_PATH` is set for openssl automatically by the devShell.

**Build + copy in one step** (per `CLAUDE.md`):

```bash
# Both crates
nix develop .# --command cargo build --release \
    --target x86_64-unknown-linux-musl -p remote-monitor -p nervctf

cp target/x86_64-unknown-linux-musl/release/remote-monitor dist/remote-monitor-linux-x86_64-static
cp target/x86_64-unknown-linux-musl/release/nervctf dist/nervctf-linux-x86_64-static
```

Use the release musl build for verification, not `cargo build` (dev/native) — the musl binary is what gets deployed, so checking compilation and producing the artifact should be one step.

---

## Cross-Compilation Gotchas (flake.nix)

Adding `musl64/aarch64/mingw64.stdenv.cc` to `packages` triggers setup hooks that each set `CC=<cross-compiler>`. Last in list wins → `CC=x86_64-w64-mingw32-gcc` pollutes native builds.

**Fix:** Pin every target via `CC_<triple>` env var in devShell shellHook AND reset `CC` to native gcc. Must include:
```nix
CC_x86_64_unknown_linux_gnu = "${pkgs.stdenv.cc}/bin/cc";
```
Otherwise ring/cc-rs compiles COFF objects for ELF targets → link failure.

**Windows `libpthread.a`:** Inject via `NIX_LDFLAGS_x86_64_w64_mingw32` (the env var Nix's gcc-wrapper reads). `RUSTFLAGS -L` / `-C link-arg` do NOT reach the external cross-linker that cargo invokes.

---

## Key Architecture Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace manifest (members: src/nervctf, src/remote-monitor) |
| `src/nervctf/src/ctfd_api/client.rs` | `CtfdClient` with async reqwest |
| `src/nervctf/src/ctfd_api/models/mod.rs` | All data types; `deserialize_ports` visitor for `internal_ports` compat |
| `src/nervctf/src/challenge_manager/` | CRUD + sync logic |
| `src/nervctf/src/main.rs` | CLI (clap), config loading |
| `src/nervctf/src/utils.rs` | `Config` struct + `load_config()` for `.nervctf.yml` |
| `src/nervctf/src/validator.rs` | Challenge validation; backend-specific checks |
| `src/remote-monitor/src/main.rs` | axum 0.7 server; all HTTP handlers |
| `src/remote-monitor/src/instance/mod.rs` | `provision()` — dispatches to backends |
| `src/remote-monitor/src/instance/compose.rs` | Compose backend; `up()` / `down()` |
| `src/remote-monitor/src/instance/docker.rs` | Docker backend; `pick_free_ports()` |
| `src/remote-monitor/src/db.rs` | SQLite via rusqlite; `Db = Arc<Mutex<Connection>>` |
| `src/nervctf/assets/ctfd-plugin/__init__.py` | CTFd Flask plugin; `_to_connection()` |
| `src/nervctf/assets/ctfd-plugin/assets/view.js` | Player UI; `renderConnectionInfo()` |

---

## Known Dependency Conflict

axum 0.7 uses `http 1.x`; reqwest 0.11 uses `http 0.2.x`. They cannot share header/status types directly.

**Bridge:** convert via `.as_str()` / `.as_bytes()` / string round-trip. See `remote-monitor/src/main.rs`.

---

## Config Loading Priority

1. CLI flags (`--monitor-url`, `--monitor-token`)
2. Env vars (`CTFD_URL`, `CTFD_API_KEY`, `MONITOR_URL`, `MONITOR_TOKEN`)
3. `.nervctf.yml` (walks up from `--challenges-dir`)

---

## CTFd Deployment Requirements (Operational)

- **CTFd version pinning:** The setup playbook pins CTFd to 3.7.3 at initial install only. The upgrade playbook warns if the installed version differs from 3.7.3 but does not abort. After any CTFd upgrade, run `nervctf probe` to verify that all NervCTF capabilities are still functional.

- **Challenge Visibility** must be "Public" (not "Private") before `nervctf deploy`.
  CTFd enforces visibility on `/api/v1/challenges` — Private mode redirects even valid API tokens to `/login`.
  Set via CTFd Admin → Config → Visibility.

- **File upload permissions:** CTFd upload dir needs `chown -R 1001:1001 <CTFd>/.data/CTFd/uploads`
  when using Docker. Otherwise `POST /api/v1/files` returns 500.

- **File uploads:** all files for a challenge must be in ONE multipart request (multiple `file` parts).
  One request per file → 500. Mirrors ctfcli's `_create_all_files()` pattern.

- **CTFd Pagination:** `GET /api/v1/challenges` is paginated (default 20/page).
  `get_challenges()` loops via `meta.pagination.next` until exhausted. Without this, challenges
  beyond page 1 always appear in `to_create` → duplicates on every re-deploy.

---

## Important Rust Type Notes

| Type | Note |
|------|------|
| `Challenge.hints` | `Option<Vec<HintContent>>` — not `Hint` (that's for CTFd API responses) |
| `Challenge.requirements` | `Option<Requirements>` enum (Simple or Advanced) |
| `FlagContent::Detailed.data` | `Option<FlagData>` (optional) |
| `FlagData` | `rename_all = "snake_case"` → `CaseInsensitive = "case_insensitive"` |
| `RequirementsQueue.resolve_dependencies` | `HashMap<String, HashSet<String>>` (owned strings) |
| `internal_ports` | `Vec<u32>` with `alias = "internal_port"` + custom `deserialize_ports` visitor — accepts both scalar and array in YAML |
| `extra_ports` | `Option<String>` in `InstanceRow`; JSON `{"<internal>": <host>}` for multi-port; `None` for single-port |
| `service_ports` | `Option<HashMap<String, Vec<u32>>>` in `InstanceConfig`; mutually exclusive with `internal_ports` for compose backend |

---

## remote-monitor Routes (current)

| Auth | Method | Path | Description |
|------|--------|------|-------------|
| None | GET | `/health` | Health check |
| None | GET | `/instance/:name` | HTML player page |
| Monitor token | POST | `/api/v1/instance/build` | Build image |
| Monitor token | POST | `/api/v1/instance/register` | Register challenge config |
| Monitor token | GET | `/api/v1/instance/list` | List instances |
| Monitor token + team_id | GET/POST/DELETE | `/api/v1/plugin/{info,request,renew,stop,stop_all,flag}` | Plugin routes |
| CTFd user token | POST | `/api/v1/instance/request` | Request instance (player) |
| CTFd user token | GET | `/api/v1/instance/info` | Get instance info (player) |
| CTFd user token | POST | `/api/v1/instance/renew` | Renew instance (player) |
| CTFd user token | DELETE | `/api/v1/instance/stop` | Stop instance (player) |
| Monitor token | ANY | `/api/v1/diff` | Challenge diff |
| Monitor token | ANY | `/api/v1/*path` | Transparent CTFd proxy |

Player auth: CTFd `GET /api/v1/users/me` with bearer token. `AppState` wrapped in `Arc<AppState>`.
`sqlite_to_ms(s)` helper converts SQLite datetime `"YYYY-MM-DD HH:MM:SS"` → Unix ms (used in plugin).

---

## CTFd Plugin Notes

- Named `nervctf_instance`, installed to `CTFd/plugins/nervctf_instance/`
- `__init__.py`: `InstanceChallengeType` + Flask blueprint + `load()`
- `models/challenge.py`: `InstanceChallenge(Challenges)` with `polymorphic_identity="instance"`
- `assets/view.{html,js}`: player UI — Fetch/Extend/Terminate buttons, calls `/api/v1/containers/*`
- `assets/create.{html,js}` + `update.{html,js}`: admin forms
- Env vars: `NERVCTF_MONITOR_URL`, `NERVCTF_MONITOR_TOKEN` (written to CTFd `.env` by Ansible)
- Plugin proxies to monitor using admin token + `team_id` — no CTFd user tokens exposed to monitor
- `view.js` uses DOM API exclusively (no `innerHTML`) — passes XSS security hook

**`_to_connection(inst)` contract:**
- Returns `{type, host, port}` always
- Also sets `conn["ports"] = inst["extra_ports"]` when `extra_ports` present (multi-port display)
- Routes also pass `connections` array when present (multi-service `service_ports` challenges)

**`renderConnectionInfo(connection, parent)`** in `view.js`:
- Checks `connection.ports` (dict) first — renders one entry per port
- Falls back to `connection.port` (scalar) for single-port
- Also handles `url_list`, `ssh`, `http`, `https`, `nc`/`tcp` types

**`_renderWithLabel(connection, parent, connections)`:**
- When `connections` array is non-empty: renders each service with label (`app: http://...`)
- Otherwise: renders "Instance Connection" header + single `renderConnectionInfo`

---

## Instance Challenge Type — Key Behaviors

- `type: instance` deploys to CTFd as `standard` (no `extra.initial`) or `dynamic` (with `extra.initial`)
- Container naming: `ctf-{sanitized_challenge}-{6 random chars}` (random suffix prevents orphan checker races)
- Background expiry task runs every 30s; health check: alive if in EITHER compose project names OR docker container names
- `pick_free_ports(n)`: atomically allocates N ports using an in-memory `allocated` HashSet alongside DB used-ports — prevents intra-instance collisions
- `extra_ports` column: `None` for single-port; `{"<internal>": <host>}` JSON for all port pairs when >1 total
- `get_used_ports` scans both `port` column and `extra_ports` JSON values
- `service_ports` path in `provision()`: total port count = sum across all services; all allocated atomically; primary service = `compose_service` if in map, else first key
- `compose::up()` receives `service_mappings: &HashMap<String, Vec<(u16, u32)>>` — fully resolved before calling, so compose.rs is unaware of config format

---

## Setup Command

- `setup.rs` finds plugin at `src/nervctf/assets/ctfd-plugin/` or next to exe
- Generates `MONITOR_TOKEN` via `/dev/urandom` (32 hex bytes)
- Detects CTFd running via TCP check on port 80
- Finds remote-monitor binary (prefers musl targets)
- Fallback for `ansible-playbook`: uses `nix develop {flake_dir} --command ...`
- Playbook: deploys plugin via rsync, writes `NERVCTF_MONITOR_*` to CTFd `.env`
- Playbook: installs LXD (snap) and Vagrant (hashicorp apt) — both `ignore_errors: true`
- Writes `monitor_url` and `monitor_token` to `.nervctf.yml` after success

---

## Changelog

### 2026-05-19 — Multi-port instance support

**Problem:** `internal_port` was scalar; challenges could only expose one port per instance.

**Changes:**
- `models/mod.rs`: `internal_port: u32` → `internal_ports: Vec<u32>` with `alias` + `deserialize_ports` visitor (accepts both `1337` and `[80, 443]`)
- `validator.rs`: updated checks for `internal_ports`
- `docker.rs`: added `pick_free_ports(used, count)` — atomic multi-port allocator; `pick_free_port` is now a wrapper
- `docker.rs`: `run_container` signature: `host_port + internal_port` → `port_mappings: &[(u16, u32)]`
- `compose.rs`: `up()` signature updated to `port_mappings: &[(u16, u32)]`
- `lxc.rs`: `launch` signature updated; loops adding `ctfport{i}` proxy devices
- `vagrant.rs`: stub signature updated
- `instance/mod.rs`: `provision()` reads `internal_ports` array (falls back to scalar); calls `pick_free_ports(n)`; builds `port_mappings`; stores `extra_ports` JSON; added `build_extra_ports_json()` helper
- `db.rs`: migration `ALTER TABLE instances ADD COLUMN extra_ports TEXT`; `InstanceRow` gains `extra_ports: Option<String>`; `get_used_ports` rewritten to scan both columns; `insert_instance` gains `extra_ports: Option<&str>` param
- `main.rs`: all instance responses include `"extra_ports"` field

---

### 2026-05-19 — Health check false-positive deletions (bug fix)

**Problem:** Containers were randomly disappearing. `list_running_container_ids` fetched hex IDs but DB stores names → mismatch; compose projects never matched via `docker compose ls` for docker-backend containers.

**Changes:**
- `docker.rs`: renamed `list_running_container_ids` → `list_running_container_names`; format `{{.ID}}` → `{{.Names}}`
- `main.rs` background expiry (~line 217): new dual-list logic — alive if found in EITHER compose project names OR docker container names; dead only when BOTH queries succeed and confirm absence

---

### 2026-05-19 — Docs and templates updated for `internal_ports`

All user-facing docs and challenge templates updated to reflect the `internal_ports` array field.

**Files:** `docs/instance-challenges.md`, `README.md`, all 12 templates (`docker/`, `compose/`, `lxc/`, `vagrant/`)

---

### 2026-05-20 — `service_ports` multi-service compose port randomization

**Goal:** Allow compose challenges with multiple independent services (e.g. `app` + `admin`) to each get their own randomly-allocated host ports.

**Key design:** `compose::up()` receives a fully-resolved `HashMap<service, Vec<(host, internal)>>` — resolution happens in `provision()`, keeping compose.rs unaware of config format.

**New field:** `service_ports: Option<HashMap<String, Vec<u32>>>` on `InstanceConfig` (serde `#[serde(default)]`). Mutually exclusive with `internal_ports`.

**Changes:**
- `models/mod.rs`: added `service_ports` field
- `sync.rs`: test helper `base_instance_config()` gains `service_ports: None`
- `compose.rs`: `up()` signature: `port_mappings + service` → `service_mappings: &HashMap<String, Vec<(u16, u32)>>` + `primary_service`; `add_service!` macro looks up each service in map
- `instance/mod.rs` compose arm: two paths — Path A (`service_ports` present): allocates per-service; Path B (absent): wraps `internal_ports` into single-key map; both converge to same `compose::up()` call
- `validator.rs`: compose backend — mutual-exclusion warning, empty-ports error, port-range errors for `service_ports`
- `docs/instance-challenges.md`: new `service_ports` entry in full reference; new "Multi-service port randomization" section in Compose Backend
- `templates/compose/`: all 3 templates gain commented `service_ports` example

---

### 2026-05-20 — Multi-connection display in CTFd player UI

**Goal:** Show all allocated ports in the CTFd challenge view. Two display modes:
- Multi-port (`internal_ports: [80, 443]`): all ports under one "Instance Connection" header
- Multi-service (`service_ports: {app: [80], admin: [8080]}`): each service labeled separately

**Data flow:** `connections` array built in Rust from `service_ports` config + `extra_ports` DB data → passed through Python unchanged → rendered in JS.

**Changes:**
- `main.rs`: added `load_config_val()` and `build_connections()` helpers; all 6 running-instance responses include `"connections"` field (null for non-service_ports); `plugin_renew_handler` consolidated double config load
- `__init__.py`: `_to_connection()` wires `extra_ports` → `conn["ports"]`; all 3 player routes pass `connections` through
- `view.js`: `_renderWithLabel(connection, parent, connections)` — new optional third arg; when non-empty array, renders labeled per-service rows; both call sites updated
- `docs/instance-challenges.md`: API Responses section documents both `extra_ports` and `connections` with examples
