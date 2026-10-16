# Developer notes

Operational knowledge, architectural constraints, and gotchas that are not obvious from reading the code. See `ARCHITECTURE.md` for the system overview and `docs/` for feature documentation.

---

## Build environment

Run every cargo and build command through the Nix flake devShell:

```bash
nix develop .# --command <cmd>
```

`flake.nix` provides rustc, cargo, rustfmt, clippy, pkg-config, openssl, and ansible. `shell.nix` was removed; `flake.nix` is the only dev-environment definition. The devShell sets `PKG_CONFIG_PATH` for OpenSSL.

Build and copy in one step:

```bash
# Both crates
nix develop .# --command cargo build --release \
    --target x86_64-unknown-linux-musl -p remote-monitor -p nervctf

cp target/x86_64-unknown-linux-musl/release/remote-monitor dist/remote-monitor-linux-x86_64-static
cp target/x86_64-unknown-linux-musl/release/nervctf dist/nervctf-linux-x86_64-static
```

Verify with the release musl build, not `cargo build` (dev/native). The musl binary is what gets deployed, so compiling and producing the artifact should be one step.

---

## Cross-compilation gotchas (`flake.nix`)

Adding `musl64/aarch64/mingw64.stdenv.cc` to `packages` triggers setup hooks that each set `CC=<cross-compiler>`. The last one in the list wins, so `CC` ends up as `x86_64-w64-mingw32-gcc`, which pollutes native builds.

The fix is to pin every target through a `CC_<triple>` environment variable in the devShell `shellHook` and to reset `CC` to native gcc. This entry is required:

```nix
CC_x86_64_unknown_linux_gnu = "${pkgs.stdenv.cc}/bin/cc";
```

Without it, ring and cc-rs compile COFF objects for ELF targets, and the link fails.

**Windows `libpthread.a`:** inject it through `NIX_LDFLAGS_x86_64_w64_mingw32`, the variable Nix's gcc-wrapper reads. `RUSTFLAGS -L` and `-C link-arg` do not reach the external cross-linker that cargo invokes.

---

## Key architecture files

| File | Purpose |
|---|---|
| `Cargo.toml` | Workspace manifest (members: `src/nervctf`, `src/remote-monitor`) |
| `src/nervctf/src/ctfd_api/client.rs` | `CtfdClient` with async reqwest |
| `src/nervctf/src/ctfd_api/models/mod.rs` | All data types; `deserialize_ports` visitor for `internal_ports` compatibility |
| `src/nervctf/src/challenge_manager/` | CRUD and sync logic |
| `src/nervctf/src/main.rs` | CLI (clap) and config loading |
| `src/nervctf/src/utils.rs` | `Config` struct and `load_config()` for `.nervctf.yml` |
| `src/nervctf/src/validator.rs` | Challenge validation; backend-specific checks |
| `src/remote-monitor/src/main.rs` | axum 0.7 server; all HTTP handlers |
| `src/remote-monitor/src/instance/mod.rs` | `provision()` dispatches to backends |
| `src/remote-monitor/src/instance/compose.rs` | Compose backend; `up()` and `down()` |
| `src/remote-monitor/src/instance/docker.rs` | Docker backend; `pick_free_ports()` |
| `src/remote-monitor/src/db.rs` | SQLite via rusqlite; `Db = Arc<Mutex<Connection>>` |
| `src/nervctf/assets/ctfd-plugin/__init__.py` | CTFd Flask plugin; `_to_connection()` |
| `src/nervctf/assets/ctfd-plugin/assets/view.js` | Player UI; `renderConnectionInfo()` |

---

## Known dependency conflict

axum 0.7 uses `http 1.x`; reqwest 0.11 uses `http 0.2.x`. They cannot share header or status types.

Convert between them with `.as_str()`, `.as_bytes()`, or a string round-trip. See `remote-monitor/src/main.rs`.

---

## Config loading priority

1. CLI flags (`--monitor-url`, `--monitor-token`)
2. Environment variables (`CTFD_URL`, `CTFD_API_KEY`, `MONITOR_URL`, `MONITOR_TOKEN`)
3. `.nervctf.yml` (walked up from `--challenges-dir`)

---

## CTFd deployment requirements (operational)

**CTFd version pinning.** The setup playbook pins CTFd to 3.7.3 at first install only. The upgrade playbook warns when the installed version differs from 3.7.3 but does not abort. After any CTFd upgrade, run `nervctf probe` to confirm that all capabilities still work.

**Challenge visibility** must be "Public", not "Private", before `nervctf deploy`. CTFd enforces visibility on `/api/v1/challenges`, and Private mode redirects even valid API tokens to `/login`. Set it in CTFd Admin → Config → Visibility.

**File upload permissions.** With Docker, the CTFd upload directory needs `chown -R 1001:1001 <CTFd>/.data/CTFd/uploads`. Otherwise `POST /api/v1/files` returns 500.

**File uploads** must send every file for a challenge in one multipart request (multiple `file` parts). One request per file returns 500. This matches ctfcli's `_create_all_files()`.

**CTFd pagination.** `GET /api/v1/challenges` is paginated (20 per page by default). `get_challenges()` loops through `meta.pagination.next` until it is exhausted. Without the loop, challenges past page 1 always appear in `to_create`, so every re-deploy duplicates them.

---

## Rust type notes

| Type | Note |
|---|---|
| `Challenge.hints` | `Option<Vec<HintContent>>` — not `Hint`, which is used for CTFd API responses |
| `Challenge.requirements` | `Option<Requirements>` enum (Simple or Advanced) |
| `FlagContent::Detailed.data` | `Option<FlagData>` |
| `FlagData` | `rename_all = "snake_case"` → `CaseInsensitive = "case_insensitive"` |
| `RequirementsQueue.resolve_dependencies` | `HashMap<String, HashSet<String>>` (owned strings) |
| `internal_ports` | `Vec<u32>` with `alias = "internal_port"` and a custom `deserialize_ports` visitor; accepts a scalar or an array in YAML |
| `extra_ports` | `Option<String>` in `InstanceRow`; JSON `{"<internal>": <host>}` for multi-port, `None` for single-port |
| `service_ports` | `Option<HashMap<String, Vec<u32>>>` in `InstanceConfig`; mutually exclusive with `internal_ports` for the compose backend |

---

## remote-monitor routes

| Auth | Method | Path | Description |
|---|---|---|---|
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

Player auth uses CTFd's `GET /api/v1/users/me` with a bearer token. `AppState` is wrapped in `Arc<AppState>`. The `sqlite_to_ms(s)` helper converts a SQLite datetime `"YYYY-MM-DD HH:MM:SS"` to Unix milliseconds and is used by the plugin.

---

## CTFd plugin notes

- Named `nervctf_instance`; installed to `CTFd/plugins/nervctf_instance/`.
- `__init__.py`: `InstanceChallengeType`, a Flask blueprint, and `load()`.
- `models/challenge.py`: `InstanceChallenge(Challenges)` with `polymorphic_identity="instance"`.
- `assets/view.{html,js}`: player UI with Fetch, Extend, and Terminate buttons, calling `/api/v1/containers/*`.
- `assets/create.{html,js}` and `assets/update.{html,js}`: admin forms.
- Environment variables: `NERVCTF_MONITOR_URL`, `NERVCTF_MONITOR_TOKEN` (written to CTFd's `.env` by Ansible).
- The plugin proxies to the monitor using the admin token and `team_id`; CTFd user tokens are never exposed to the monitor.
- `view.js` uses the DOM API only (no `innerHTML`), so it passes the XSS security hook.

**`_to_connection(inst)` contract:**

- always returns `{type, host, port}`;
- sets `conn["ports"] = inst["extra_ports"]` when `extra_ports` is present (multi-port display);
- the routes also pass a `connections` array when present (multi-service `service_ports` challenges).

**`renderConnectionInfo(connection, parent)`** in `view.js`:

- checks `connection.ports` (a dict) first and renders one entry per port;
- falls back to `connection.port` (a scalar) for single-port;
- also handles the `url_list`, `ssh`, `http`, `https`, and `nc`/`tcp` types.

**`_renderWithLabel(connection, parent, connections)`:**

- when the `connections` array is non-empty, renders each service with a label (`app: http://...`);
- otherwise renders an "Instance Connection" header plus a single `renderConnectionInfo`.

---

## Instance challenge type: key behaviours

- `type: instance` deploys to CTFd as `standard` (no `extra.initial`) or `dynamic` (with `extra.initial`).
- Container naming: `ctf-{sanitized_challenge}-{6 random chars}`. The random suffix prevents orphan-checker races.
- The background expiry task runs every 30s. An instance is alive if it appears in either the compose project names or the Docker container names.
- `pick_free_ports(n)` allocates N ports atomically using an in-memory `allocated` HashSet alongside DB used-ports, which prevents collisions within one instance.
- `extra_ports` column: `None` for single-port; JSON `{"<internal>": <host>}` with all port pairs when more than one.
- `get_used_ports` scans both the `port` column and the `extra_ports` JSON values.
- The `service_ports` path in `provision()` sums the port count across services, allocates all of them atomically, and picks the primary service as `compose_service` if it is in the map, otherwise the first key.
- `compose::up()` receives `service_mappings: &HashMap<String, Vec<(u16, u32)>>`, fully resolved before the call, so `compose.rs` does not know about the config format.

---

## Setup command

- `setup.rs` finds the plugin at `src/nervctf/assets/ctfd-plugin/`, next to the executable, or from the copy embedded in the binary.
- Generates `MONITOR_TOKEN` from `/dev/urandom` (32 bytes).
- Resolves CTFd's published HTTP port from Compose rather than assuming port 80.
- Finds the remote-monitor binary (musl targets preferred).
- Runs a preflight playbook before provisioning: passwordless sudo, `python3`, `rsync`, and `git` on the relevant hosts.
- The playbook deploys the plugin over rsync and writes `NERVCTF_MONITOR_*` to CTFd's `.env`.
- The playbook installs LXD (snap) and Vagrant (HashiCorp apt); both steps use `ignore_errors: true`.
- Writes the configuration to `.nervctf.yml` before running the playbook.

---

## Changelog

### 2026-05-19 — Multi-port instance support

**Problem:** `internal_port` was scalar, so a challenge could expose only one port per instance.

**Changes:**

- `models/mod.rs`: `internal_port: u32` becomes `internal_ports: Vec<u32>` with an `alias` and a `deserialize_ports` visitor (accepts `1337` and `[80, 443]`).
- `validator.rs`: checks updated for `internal_ports`.
- `docker.rs`: added `pick_free_ports(used, count)`, the atomic multi-port allocator; `pick_free_port` is now a wrapper.
- `docker.rs`: `run_container` signature changed from `host_port + internal_port` to `port_mappings: &[(u16, u32)]`.
- `compose.rs`: `up()` signature updated to `port_mappings: &[(u16, u32)]`.
- `lxc.rs`: `launch` signature updated; loops add `ctfport{i}` proxy devices.
- `vagrant.rs`: stub signature updated.
- `instance/mod.rs`: `provision()` reads the `internal_ports` array (falling back to the scalar), calls `pick_free_ports(n)`, builds `port_mappings`, stores the `extra_ports` JSON, and gained the `build_extra_ports_json()` helper.
- `db.rs`: migration `ALTER TABLE instances ADD COLUMN extra_ports TEXT`; `InstanceRow` gains `extra_ports: Option<String>`; `get_used_ports` scans both columns; `insert_instance` gains an `extra_ports: Option<&str>` parameter.
- `main.rs`: all instance responses include an `"extra_ports"` field.

### 2026-05-19 — Health-check false-positive deletions (bug fix)

**Problem:** Containers disappeared at random. `list_running_container_ids` fetched hex IDs while the DB stored names, so they never matched, and compose projects never matched via `docker compose ls` for docker-backend containers.

**Changes:**

- `docker.rs`: renamed `list_running_container_ids` to `list_running_container_names`; format changed from `{{.ID}}` to `{{.Names}}`.
- `main.rs` background expiry (~line 217): new dual-list logic. An instance is alive if it appears in either the compose project names or the Docker container names; it is dead only when both queries succeed and confirm absence.

### 2026-05-19 — Docs and templates updated for `internal_ports`

All user-facing docs and challenge templates updated for the `internal_ports` array.

**Files:** `docs/instance-challenges.md`, `README.md`, all 12 templates (`docker/`, `compose/`, `lxc/`, `vagrant/`).

### 2026-05-20 — `service_ports` multi-service compose port randomization

**Goal:** Let compose challenges with several independent services (for example `app` and `admin`) each get their own randomly allocated host ports.

**Key design:** `compose::up()` receives a fully resolved `HashMap<service, Vec<(host, internal)>>`. Resolution happens in `provision()`, so `compose.rs` stays unaware of the config format.

**New field:** `service_ports: Option<HashMap<String, Vec<u32>>>` on `InstanceConfig`, with serde `#[serde(default)]`. Mutually exclusive with `internal_ports`.

**Changes:**

- `models/mod.rs`: added the `service_ports` field.
- `sync.rs`: test helper `base_instance_config()` gains `service_ports: None`.
- `compose.rs`: `up()` signature changed from `port_mappings + service` to `service_mappings: &HashMap<String, Vec<(u16, u32)>>` plus `primary_service`; the `add_service!` macro looks up each service in the map.
- `instance/mod.rs` compose arm: two paths. Path A (`service_ports` present) allocates per service; Path B (absent) wraps `internal_ports` into a single-key map. Both call the same `compose::up()`.
- `validator.rs`: compose backend checks for mutual exclusion, empty ports, and port range on `service_ports`.
- `docs/instance-challenges.md`: added a `service_ports` entry to the full reference and a "Multi-service port randomization" section to the Compose backend.
- `templates/compose/`: all three templates gained a commented `service_ports` example.

### 2026-05-20 — Multi-connection display in the CTFd player UI

**Goal:** Show all allocated ports in the CTFd challenge view, in two modes:

- multi-port (`internal_ports: [80, 443]`): all ports under one "Instance Connection" header;
- multi-service (`service_ports: {app: [80], admin: [8080]}`): each service labelled separately.

**Data flow:** the `connections` array is built in Rust from the `service_ports` config and the `extra_ports` DB data, passed through Python unchanged, and rendered in JavaScript.

**Changes:**

- `main.rs`: added `load_config_val()` and `build_connections()`; all six running-instance responses include a `"connections"` field (null for non-`service_ports`); `plugin_renew_handler` no longer loads the config twice.
- `__init__.py`: `_to_connection()` maps `extra_ports` to `conn["ports"]`; all three player routes pass `connections` through.
- `view.js`: `_renderWithLabel(connection, parent, connections)` gained an optional third argument; a non-empty array renders labelled per-service rows. Both call sites updated.
- `docs/instance-challenges.md`: the API Responses section documents `extra_ports` and `connections` with examples.
