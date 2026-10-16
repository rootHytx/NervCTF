# Instance challenges

A `type: instance` challenge provisions an ephemeral container or VM for each team. Players request an instance, receive a host and port, and destroy it when they are done.

Two components are required, both deployed by `nervctf setup`:

1. the `remote-monitor` service on the CTFd host;
2. the `nervctf_instance` CTFd plugin in CTFd.

---

## `challenge.yml` fields

### Required

```yaml
type: instance
instance:
  backend: docker          # docker | compose | lxc | vagrant
  internal_ports: [1337]   # port(s) exposed inside the container
  connection: nc           # nc | http | ssh
```

### Top-level fields

```yaml
name: "Challenge Name"
category: pwn
description: |
  Describe the challenge.
value: 0
type: instance
state: visible
version: "0.3"
topics: [topic1, topic2]    # optional freeform labels, separate from CTFd tags

# Dynamic scoring (optional)
extra:
  initial: 500
  decay: 50
  minimum: 100
  decay_function: linear    # linear (default) | logarithmic
```

### Full reference

```yaml
instance:
  # ── Backend ─────────────────────────────────────────────────────────────────
  backend: docker

  # Docker — a local path (".") or a registry reference
  image: .

  # Compose — relative to the challenge directory, uploaded to the monitor on deploy
  compose_file: docker-compose.yml
  compose_service: app        # service that exposes internal_port

  # LXC — image name or qcow2 path (the server needs LXD)
  lxc_image: ubuntu:22.04

  # Vagrant — path to a directory containing a Vagrantfile (stub)
  vagrantfile: ./vm

  # ── Ports ────────────────────────────────────────────────────────────────────

  # Single port (most challenges):
  internal_ports: [1337]

  # Multi-port: every listed port gets its own random host port.
  # The first entry is the primary port used in the connection string and the
  # `port` field. All mappings are returned in `extra_ports`.
  # Backward compat: the scalar `internal_port: 1337` is still accepted.
  # internal_ports: [80, 443]

  # Compose only — an alternative to internal_ports for challenges with several
  # independent services that each need their own random host port(s).
  # Maps each service name to the internal ports it exposes.
  # Mutually exclusive with internal_ports (a warning is raised if both are set).
  # service_ports:
  #   app:   [80]
  #   admin: [8080]

  connection: nc              # nc | http | ssh
  command: null               # override the container entrypoint or CMD
  timeout_minutes: 45
  max_renewals: 3

  # ── Flag ─────────────────────────────────────────────────────────────────────
  flag_mode: random           # static | random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  random_flag_length: 16

  # How the per-instance flag reaches the container:
  flag_delivery: env          # env (default) | file

  # env: FLAG is injected as an environment variable named FLAG.
  #   docker backend  — passed with `docker run -e FLAG=<value>`
  #   compose backend — set in the shell before `docker compose up`; use ${FLAG} in docker-compose.yml

  # file: the flag is written to a read-only bind-mounted file inside the container.
  #   docker backend  — written to /tmp/ctf-flags/<container_name>.flag on the runner/host,
  #                     then mounted at flag_file_path inside the container.
  #   compose backend — written to <project_name>.flag in the challenge directory on the
  #                     runner/host, then mounted at flag_file_path inside the target service.
  flag_file_path: /challenge/flag    # required for flag_delivery: file (both backends)
  flag_service: app                  # compose only: service that receives the file mount
                                     # defaults to compose_service; ignored for docker
```

### Static flags

Use `flag_mode: static` (or omit `flag_mode`) and define flags in the top-level `flags:` list. Every team shares one flag.

```yaml
type: instance
flags:
  - CTF{hardcoded_flag}
instance:
  backend: docker
  image: myimage:latest
  internal_ports: [4000]
  connection: nc
```

---

## Scoring

An instance challenge can use dynamic scoring like any other. Set `extra:` at the top level:

```yaml
type: instance
value: 0           # required but unused when extra.initial is set
extra:
  initial: 500
  decay: 50
  minimum: 100
instance:
  ...
```

Without `extra:`, the challenge deploys as `standard` with the given `value`.

---

## Deploy flow

When `nervctf deploy` processes a `type: instance` challenge:

1. Create or update the challenge in CTFd (direct MariaDB write).
2. Build, depending on backend and mode.

**Single-machine mode** (no `runner_ip`):

| Backend | Action |
|---|---|
| `docker` (local path) | Pack the challenge directory as tar.gz → `POST /api/v1/instance/build` → monitor runs `docker build` |
| `compose` (relative path) | Pack the challenge directory as tar.gz → `POST /api/v1/instance/build-compose` → monitor runs `docker compose build` |
| `docker` (registry ref) | No build; the image is pulled at provision time |

**Split-machine mode** (`runner_ip` set):

| Backend | Action |
|---|---|
| `docker` (local path) | rsync the challenge directory to the runner → `POST /api/v1/instance/build` |
| `compose` (relative path) | rsync the challenge directory to the runner → `POST /api/v1/instance/build-compose-remote` → monitor runs `docker compose build` over SSH |

3. `POST /api/v1/instance/register` to register the `InstanceConfig` on the monitor.

### Split-machine mode

With `runner_ip` set, challenge containers run on a separate worker node. The CLI rsyncs challenge files to the runner, and the monitor runs every Docker and Compose command on the runner over SSH (`RUNNER_SSH_TARGET`).

```yaml
# .nervctf.yml
runner_ip: 192.168.1.50
runner_user: docker                    # default: docker
runner_domain: challenges.example.com  # optional
```

`runner_domain` lets you show players a DNS name instead of the raw IP. The backend (SSH connections and `RUNNER_SSH_TARGET`) always uses `runner_ip`; only `PUBLIC_HOST` — the value in player connection strings — is replaced with the domain.

Bind-mount paths in `docker-compose.yml` must use the path as seen on the runner filesystem, not inside the monitor container. The runner stores challenge files at the same path used during rsync.

---

## Docker backend

The `docker` backend runs one container per team.

**`flag_delivery: env`** (default):

```
docker run -d \
  --name ctf-<challenge>-<random6> \
  -p <host_port1>:<internal_port1> [-p <host_port2>:<internal_port2> ...] \
  -e FLAG=<random_flag> \
  <image_tag> [command]
```

Read the flag inside the container from the `FLAG` environment variable.

**`flag_delivery: file`:**

```
docker run -d \
  --name ctf-<challenge>-<random6> \
  -p <host_port1>:<internal_port1> \
  -v /tmp/ctf-flags/<name>.flag:<flag_file_path>:ro \
  <image_tag> [command]
```

The flag is written to `/tmp/ctf-flags/<container_name>.flag` on the runner/host before the container starts, then bind-mounted read-only at `flag_file_path`. The file is removed when the instance stops or expires.

```yaml
instance:
  backend: docker
  flag_delivery: file
  flag_file_path: /challenge/flag
```

**Common behaviour:**

- Each internal port gets its own random host port in 40000–60000, allocated atomically, so ports within one instance never collide.
- The primary port (the first in `internal_ports`) is the `port` field in API responses; all mappings also appear in `extra_ports`.
- Container names are `ctf-<sanitized_challenge_name>-<6 random chars>`, unique per provision.
- The image is built once during `nervctf deploy` and reused for every team.

### `image` field

| Value | Behaviour |
|---|---|
| `"."` | Local build: CLI packs the challenge directory, monitor runs `docker build` |
| `"./subdir"` | Local build from a subdirectory |
| `"myimage:tag"` | Registry image, pulled directly on the monitor with no build step |

---

## Compose backend

The `compose` backend manages one `docker compose` project per team:

- Challenge files live on the monitor at `$CHALLENGES_BASE_DIR/<sanitized_name>/`. The default is `/opt/nervctf/challenges/<name>/`; Ansible sets it to `<ctfd_path>/remote-monitor/data/challenges/<name>/`.
- Project name: `ctf-<sanitized_challenge_name>-<6 random chars>`.
- A per-team override file (`<project_name>.override.yml`) is written next to the compose file.

The override always contains:

1. Port mappings per service, one `host_port:internal_port` entry per listed port, all pre-allocated atomically before `docker compose up`. With `internal_ports`, every port maps to `compose_service` (default `app`). With `service_ports`, each service gets its own list.
2. An `image:` line for every pre-built service, referencing the image built by `nervctf deploy`.
3. A volume mount for `flag_delivery: file`, when set.

### Image naming

Images are built with `-p <dir_name>`, where `<dir_name>` is the lowercased name of the challenge directory (the parent of `compose_file`). Docker Compose tags them `<dir_name>-<service>`. The per-instance override sets `image: <dir_name>-<service>` for each locally built service, so the per-instance project name never affects the image lookup.

Services that reference a public image (`postgres`, `redis`, and so on) are left alone; only locally built services are overridden.

For a challenge directory `sigma-notes/` with services `app` and `sigma_admin`, Compose tags the images `sigma-notes-app` and `sigma-notes-sigma_admin`, and the override adds those as `image:` entries.

### Flag delivery for compose

**`flag_delivery: env`** (default): the monitor sets `FLAG=<value>` in the environment when it calls `docker compose up`. Use `${FLAG}` in `docker-compose.yml`:

```yaml
services:
  app:
    environment:
      - FLAG=${FLAG}
```

**`flag_delivery: file`:** the flag is written to `<project_name>.flag` in the challenge directory on the monitor and bind-mounted read-only at `flag_file_path`:

```yaml
instance:
  flag_delivery: file
  flag_file_path: /challenge/flag
  # flag_service: other-service  # optional, defaults to compose_service
```

### Multi-service port randomization

Use `service_ports` when a challenge exposes ports on more than one independent service. Each service gets its own set of random host ports.

```yaml
instance:
  backend: compose
  compose_file: docker-compose.yml
  service_ports:
    app:   [80]
    admin: [8080]
  connection: http
```

- `service_ports` replaces both `compose_service` and `internal_ports`; do not set both.
- The primary service, whose port appears in the player-facing `port` field, is `compose_service` when that key is in the map, otherwise the first declared service.
- Every allocated host port appears in the `extra_ports` response as `{"<internal_port>": <host_port>}`.
- The validator warns when both `service_ports` and `internal_ports` are set.

### Do not set `container_name:`

Leave `container_name:` out of your `docker-compose.yml`. Compose prefixes container names with the project name by default, which keeps them unique across teams. A hardcoded `container_name:` makes every team fight over one name and fail to start.

### Bind-mount path constraint

The monitor stores challenge files at `$CHALLENGES_BASE_DIR/<name>/`. A challenge's `docker-compose.yml` that references absolute paths (cert files, for instance) must use the full path as seen on the runner/host filesystem, because the host Docker daemon resolves bind-mount paths from the host, not from inside the monitor container.

`CHALLENGES_BASE_DIR` depends on `ctfd_path` in `.nervctf.yml`. Check the `docker-compose.override.yml` on the CTFd host for the exact value.

---

## LXC backend

Launches one LXC/LXD container per team:

1. `lxc launch <lxc_image> <container_name>`
2. `lxc wait --state=Running`
3. `lxc config device add` — one `ctfport{i}` proxy device per entry in `internal_ports` (`ctfport0`, `ctfport1`, …), each mapping a random host port to the matching internal port
4. `lxc exec` — inject the flag into `/challenge/flag` when `flag_mode: random`

LXD must be installed and initialised on the monitor server. `nervctf setup` installs LXD via snap and runs `lxd init --auto`.

---

## Vagrant backend

A stub that returns an error. The setup playbook installs Vagrant and libvirt, but the provisioning logic is not implemented.

---

## Instance lifecycle

| Event | What happens |
|---|---|
| Player requests an instance | A row is inserted with `status='provisioning'`; the container starts; the status becomes `'running'` |
| Player renews | `expires_at` is extended by `timeout_minutes`; `renewals_used` increments |
| Player stops | The container is removed and the row deleted |
| Instance expires | The 30-second background task calls `cleanup_container()` and deletes the row |
| Provisioning stuck over 30 minutes | The background task treats the row like an expired one, using `created_at` rather than `expires_at` |
| Container killed externally | The health check (each 30 s tick) finds the container absent from both `docker ps` and `docker compose ls`, deletes the row, and cleans up the flag |
| Challenge deleted | All instances stop and the config is removed from `instance_configs` |

---

## API responses

Every instance info, request, and renew response includes:

| Field | Type | Description |
|---|---|---|
| `status` | string | `provisioning`, `running`, or `none` |
| `host` | string | Public hostname or IP |
| `port` | number | Primary host port (`internal_ports[0]`, or the primary service's first port) |
| `connection_type` | string | Connection template (`nc`, `http`, or `ssh`) |
| `expires_at` | string | SQLite datetime of expiry |
| `extra_ports` | object or null | `{"<internal>": <host>}` for every mapping when more than one port is exposed; `null` for single-port |
| `connections` | array or null | Per-service labelled entries; `null` without `service_ports` |

### `extra_ports` (multi-port, one service)

For `internal_ports: [80, 443]`, all ports belong to one service and need no labels:

```json
{
  "port": 42100,
  "extra_ports": {"80": 42100, "443": 53210}
}
```

The plugin renders every port from `extra_ports` as its own link or command. The `port` field equals `extra_ports["<internal_ports[0]>"]`.

### `connections` (multi-service, `service_ports`)

For `service_ports: {app: [80], admin: [8080]}`, each service has its own labelled entry:

```json
{
  "port": 42100,
  "extra_ports": {"80": 42100, "8080": 54321},
  "connections": [
    {"label": "app",   "type": "http", "host": "1.2.3.4", "port": 42100},
    {"label": "admin", "type": "http", "host": "1.2.3.4", "port": 54321}
  ]
}
```

The plugin renders each entry as `<service>: <connection>`, so players can see which port belongs to which service.

---

## Provision ordering

With `flag_mode = "random"`, the monitor creates three persistent side-effects during provisioning: it starts the container, inserts the SQLite `instances` row, and inserts the CTFd MariaDB `flags` row. The order matters for crash safety.

The code enforces this order:

1. Start the container (docker, compose, or lxc).
2. `db::insert_instance(... ctfd_flag_id: None ...)` writes the SQLite row with a null flag ID.
3. `ctfd_db::create_flag(...)` creates the CTFd flag row.
4. `db::set_ctfd_flag_id(...)` updates the SQLite row with the new flag ID.

Why this order:

- A crash between steps 1 and 2 leaves no SQLite row and no MariaDB flag, so there is nothing to clean up.
- A crash between steps 2 and 3 leaves a SQLite row with `ctfd_flag_id = NULL`. The expiry and stop handlers test `if let Some(flag_id) = ctfd_flag_id` before calling `ctfd_db::delete_flag`, so the null row is cleaned up without a phantom MariaDB delete.
- A crash between steps 3 and 4 leaves a MariaDB flag with no matching SQLite `ctfd_flag_id`. This is the remaining gap: the flag is not deleted on expiry. The window is narrower than the old ordering and can be closed by a future reconciliation task that cross-references MariaDB flags against active instances.

The cleanup path is null-safe: every call site in `main.rs` that reads `ctfd_flag_id` wraps the delete in `if let Some(flag_id) = ctfd_flag_id`.

---

## Player UI

The monitor serves a small HTML page at `GET /instance/<challenge_name>` where players enter their CTFd API token and use the Fetch, Extend, and Terminate buttons.

The `nervctf_instance` plugin also adds a panel inside the CTFd challenge page that calls `/api/v1/containers/*`, so players can manage the instance without leaving CTFd.
