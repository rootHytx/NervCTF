# Architecture

NervCTF runs a CTFd competition from local YAML. Three components:

- **`nervctf`** (CLI) scans `challenge.yml` files, validates them, and deploys them through the monitor.
- **`remote-monitor`** (HTTP service) runs on the CTFd host, writes CTFd's MariaDB directly, manages per-team containers, and serves an admin dashboard.
- **`nervctf_instance`** (CTFd plugin) adds the `instance` challenge type and the player lifecycle controls.

Single-machine mode runs everything on the CTFd host. Split-machine mode runs containers on a runner reached over SSH.

Command and flag reference: [`docs/cli-reference.md`](docs/cli-reference.md). Per-module detail: [`docs/`](docs/).

## Repository layout

```
src/nervctf/                 CLI crate
  src/main.rs                commands, deploy orchestration, probe, event
  src/setup.rs               setup/upgrade and Ansible invocation
  src/utils.rs               Config, EventConfig, AdminConfig
  src/validator.rs           challenge checks
  src/fix.rs                 YAML patching
  src/directory_scanner.rs   challenge.yml discovery (depth 5, no symlinks)
  src/challenge_manager/     sync/diff logic
  src/ctfd_api/              monitor HTTP client and data types
  assets/                    playbooks + embedded CTFd plugin
src/remote-monitor/          monitor crate
  src/main.rs                axum routes and background tasks
  src/db.rs                  SQLite state store
  src/ctfd_db.rs             MariaDB access
  src/instance/              docker, compose, lxc, vagrant backends
templates/                   challenge.yml templates
```

Build with `nix develop .# --command cargo build --release`; cross-compilation notes are in [`docs/dev-notes.md`](docs/dev-notes.md).

## Configuration

CLI flags > `MONITOR_URL` / `MONITOR_TOKEN` > `.nervctf.yml`. The full file and precedence rules are in the [CLI reference](docs/cli-reference.md#configuration).

`Config` in `utils.rs` holds the connection fields, monitor tuning, split-machine fields, an optional `EventConfig`, and an optional `AdminConfig`. `save_config` writes the file with mode `0600`.

## Deploy flow

`main.rs::deploy_challenges`:

1. Scan the tree and validate. Errors abort.
2. Fetch remote challenges (`GET /api/v1/challenges`, paginated).
3. Phase 1: create or update each challenge and its flags, tags, topics, and hints.
4. Phase 2: upload files (all files in one multipart request).
5. Phase 3: patch `requirements` after every challenge ID exists.
6. Phase 4: patch `next_id`.
7. Optional `--prune`: delete remote challenges absent locally.
8. For `type: instance`, build and register the instance config with the monitor.

All HTTP calls go to the monitor, which runs them against MariaDB. The CLI never calls CTFd's API.

The comparison fields, type-change recreate rule, and sub-resource strategies are in [`docs/challenge_manager.md`](docs/challenge_manager.md). The command flags and deploy gate are in the [CLI reference](docs/cli-reference.md#deploy).

## Data models

`ctfd_api/models/mod.rs` defines the challenge types. `ChallengeType` is `standard`, `dynamic`, or `instance`; an instance challenge deploys to CTFd as `standard` or `dynamic` and is registered with the plugin separately.

`Challenge` carries the core fields plus `flags`, `hints`, `tags`, `files`, `requirements`, `next`, `extra`, and `instance`. `FlagContent`, `HintContent`, and `Tag` accept a shorthand or a detailed form. `InstanceConfig` controls the backend, ports, lifetime, and flag delivery; its fields are in [`docs/instance-challenges.md`](docs/instance-challenges.md).

## Remote monitor

Runtime configuration (environment variables, routes, schema detection, probe, event endpoints) is in [`docs/remote-monitor.md`](docs/remote-monitor.md).

Background tasks:

- **Expiry** (30 s): delete expired or stuck instances, remove their CTFd flags, tear down containers, stop untracked `ctf-*` compose projects, and drop rows whose containers disappeared from both `docker ps` and `docker compose ls`.
- **CTFd sync** (`CTFD_DB_SYNC_INTERVAL`, default 30 s): refresh the solve cache, revert instances whose solve was deleted, remove stale attempts, and refresh team/user names.

SQLite tables and columns are defined in `src/remote-monitor/src/db.rs`: `instance_configs`, `instances`, `flag_attempts`, `team_flags`, `ctfd_solves`, `ctfd_teams`, `ctfd_users`, `operator_tokens`, `sessions`, `ctfd_probe`, and `events`. `Db` is `Arc<Mutex<Connection>>`, WAL is enabled, and migrations run on `open()`.

## Instance backends

- **docker** — one container per team; `docker run`/`rm`, ports allocated atomically in 40000–60000.
- **compose** — one project per team; a generated `<project>.override.yml` adds image, port, and flag-volume entries.
- **lxc** — one LXD container per team with proxy devices per port.
- **vagrant** — stub.

Every Docker/Compose/LXC call takes `runner_ssh: Option<&str>`; when set, it runs on the runner over SSH. Compose receives fully resolved `service_mappings`, so it does not parse the config format.

## CTFd plugin

Installed at `CTFd/plugins/nervctf_instance/` and embedded in the CLI.

- `InstanceChallengeType` implements create, read, update, delete, solve, and attempt.
- `load(app)` creates `nervctf_instance_challenge` and runs `_maybe_complete_setup(app)`.
- The setup routine creates the first admin and writes `ctf_name`, `ctf_description`, `user_mode`, and `setup = "true"` when the env vars are present and `setup` is absent or false. It detects whether the build hashes passwords on assignment and uses passlib `bcrypt_sha256` otherwise.
- Player routes forward to the monitor with the admin token and an explicit `team_id`.

## Playbooks

- `nervctf_preflight_playbook.yml` checks passwordless sudo, `python3`, `rsync`, and `git` with `raw`, so it works without Python on the target.
- `nervctf_playbook.yml` installs Docker, clones and starts CTFd, installs LXD/Vagrant, deploys the plugin, builds the monitor image, writes the override, and starts the stack.
- `nervctf_upgrade_playbook.yml` syncs the plugin and binary, rebuilds the image, writes the override, and recreates the whole Compose stack. Recreating (not restarting) applies the environment and refreshes nginx's upstream.

The override joins `remote-monitor` to CTFd's `internal` network so it can reach MariaDB, and sets `CHALLENGES_BASE_DIR` to a path that is identical on the host and inside the monitor. Challenge compose files that bind-mount host files must use that host path.

## Setup and event flow

`nervctf setup` prompts for the connection details and the first admin, saves the config, runs the preflight, then the playbook. The playbook passes the event/admin settings to the CTFd container; the plugin completes CTFd's first-run setup.

`nervctf event` writes the `event:` block through the monitor. `GET /api/v1/admin/events` exposes the lifecycle log. Both are documented in [`docs/cli-reference.md`](docs/cli-reference.md#event) and [`docs/remote-monitor.md`](docs/remote-monitor.md#event-settings-and-event-log).

## Validation and repair

`validator.rs` runs per-challenge and cross-challenge checks. Errors block deploy; warnings do not. The rule table is in [`docs/validator.md`](docs/validator.md).

`fix.rs` patches missing `state`, `author`, and `version` fields. It checks for a top-level key at column 0 and inserts after a chosen sibling key.

## Known limitations

- The Vagrant backend is a stub.
- `MAX_INSTANCES_PER_TEAM` is global, not per challenge.
- `deploy` asks for confirmation, which makes scripting awkward.
- `GET /instance/:name` has no server-side auth; the token is entered in the browser.
- The requirements comparison checks presence, not identity.
- The event log has no automatic retention limit.
- Dynamic scoring needs CTFd's Dynamic plugin.
