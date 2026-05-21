# Instance Challenges

`type: instance` challenges provision an ephemeral container or VM for each team. Players
request an instance, receive a host/port, and destroy it when done.

Requires:
1. **`remote-monitor`** service running on the CTFd host
2. **`nervctf_instance` CTFd plugin** installed in CTFd

Both are deployed by `nervctf setup`.

---

## `challenge.yml` Fields

### Required

```yaml
type: instance
instance:
  backend: docker          # docker | compose | lxc | vagrant
  internal_ports: [1337]   # port(s) exposed inside the container (see below)
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
topics: [topic1, topic2]    # optional; freeform topic labels (not CTFd tags)

# Dynamic scoring (optional — see Scoring section)
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

  # Docker — image is a local path (".") or a registry ref
  image: .

  # Compose — relative to challenge dir, uploaded to monitor on deploy
  compose_file: docker-compose.yml
  compose_service: app        # service that exposes internal_port

  # LXC — image name or qcow2 path (server must have LXD installed)
  lxc_image: ubuntu:22.04

  # Vagrant — path to directory containing Vagrantfile (stub, not yet functional)
  vagrantfile: ./vm

  # ── Common ──────────────────────────────────────────────────────────────────

  # Single port (most challenges):
  internal_ports: [1337]

  # Multi-port: all listed ports are exposed on separate randomly-assigned host ports.
  # The first entry is the primary port (used in `connection` string and `port` field).
  # All allocated host ports are returned in the `extra_ports` response field.
  # Backward compat: `internal_port: 1337` (scalar) is still accepted.
  # internal_ports: [80, 443]

  # Compose only — alternative to `internal_ports` for challenges with multiple independent
  # services that each need their own randomly-allocated host port(s).
  # Maps each service name to the internal ports it exposes.
  # Mutually exclusive with `internal_ports` (a warning is raised if both are set).
  # service_ports:
  #   app:   [80]
  #   admin: [8080]

  connection: nc              # nc | http | ssh
  command: null               # override container entrypoint/CMD (optional)
  timeout_minutes: 45
  max_renewals: 3

  # ── Flag ────────────────────────────────────────────────────────────────────
  flag_mode: random           # static | random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  random_flag_length: 16

  # How the per-instance flag reaches the container:
  flag_delivery: env          # env (default) | file

  # "env": FLAG is injected as an environment variable named FLAG.
  #   docker backend  — passed via `docker run -e FLAG=<value>`
  #   compose backend — set in the shell before `docker compose up`; use ${FLAG} in docker-compose.yml

  # "file": flag is written to a bind-mounted read-only file inside the container.
  #   docker backend  — written to /tmp/ctf-flags/<container_name>.flag on the runner/host,
  #                     then mounted at flag_file_path inside the container.
  #   compose backend — written to <project_name>.flag in the challenge dir on the runner/host,
  #                     then mounted at flag_file_path inside the target service.
  flag_file_path: /challenge/flag    # required for flag_delivery: file (both backends)
  flag_service: app                  # compose only: service that receives the file mount
                                     # (defaults to compose_service; ignored for docker backend)
```

### Static flags

Use `flag_mode: static` (or omit `flag_mode`) and define flags in the top-level `flags:` list:

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

Instance challenges can use dynamic scoring the same as any other challenge. Set `extra:` at
the top level:

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

Without `extra:`, the challenge is deployed as `standard` type with the given `value`.

---

## Deploy Flow

When `nervctf deploy` processes a `type: instance` challenge:

1. Create/update the challenge in CTFd (direct MariaDB write)
2. Build step — depends on backend and mode:

**Single-machine** (no `runner_ip` in `.nervctf.yml`):

| Backend | Action |
|---------|--------|
| `docker` (local path) | Pack challenge dir as tar.gz → `POST /api/v1/instance/build` → monitor runs `docker build` |
| `compose` (relative path) | Pack challenge dir as tar.gz → `POST /api/v1/instance/build-compose` → monitor runs `docker compose build` |
| `docker` (registry ref) | No build step — image is pulled at provision time |

**Split-machine** (`runner_ip` set in `.nervctf.yml`):

| Backend | Action |
|---------|--------|
| `docker` (local path) | rsync challenge dir to runner → `POST /api/v1/instance/build` |
| `compose` (relative path) | rsync challenge dir to runner → `POST /api/v1/instance/build-compose-remote` → monitor SSHes to runner and runs `docker compose build` |

3. `POST /api/v1/instance/register` — register `InstanceConfig` on the monitor

### Split-machine mode

When `runner_ip` is set in `.nervctf.yml`, challenge containers run on a separate worker node
instead of the CTFd host. The CLI rsyncs challenge files directly to the runner; the monitor
executes all Docker/Compose commands on the runner via SSH (`RUNNER_SSH_TARGET`).

```yaml
# .nervctf.yml
runner_ip: 192.168.1.50
runner_user: docker           # default: docker
runner_domain: challenges.example.com  # optional
```

`runner_domain` lets you expose a DNS name to players instead of the raw IP. The backend
(SSH connections, `RUNNER_SSH_TARGET`) always uses `runner_ip`; only `PUBLIC_HOST` — the
value shown in player connection strings — is replaced with the domain.

Bind mount paths in `docker-compose.yml` must use the path as seen on the **runner** filesystem
(not the monitor container). The runner stores challenge files at the same path used during rsync.

---

## Docker Backend

The `docker` backend runs a single container per team.

**`flag_delivery: env`** (default):

```
docker run -d \
  --name ctf-<challenge>-<random6> \
  -p <host_port1>:<internal_port1> [-p <host_port2>:<internal_port2> ...] \
  -e FLAG=<random_flag> \
  <image_tag> [command]
```

Read the flag inside the container via the `FLAG` environment variable.

**`flag_delivery: file`**:

```
docker run -d \
  --name ctf-<challenge>-<random6> \
  -p <host_port1>:<internal_port1> \
  -v /tmp/ctf-flags/<name>.flag:<flag_file_path>:ro \
  <image_tag> [command]
```

The flag is written to `/tmp/ctf-flags/<container_name>.flag` on the runner/host before
the container starts, then bind-mounted read-only at `flag_file_path` inside the container.
The file is deleted when the instance is stopped or expires.

```yaml
instance:
  backend: docker
  flag_delivery: file
  flag_file_path: /challenge/flag
```

**Common:**

- Each internal port gets its own randomly-picked host port in range 40000–60000 (all allocated atomically — no collisions between ports of the same instance)
- Primary port (first in `internal_ports`) is the `port` field in all API responses; all port mappings are also available in `extra_ports` (see [API Responses](#api-responses))
- Container name: `ctf-<sanitized_challenge_name>-<6 random chars>` (unique per provision)
- Image is built once during `nervctf deploy` and reused for all teams

### `image` field

| Value | Behaviour |
|-------|-----------|
| `"."` | Local build: CLI packs challenge dir as tar.gz, monitor runs `docker build` |
| `"./subdir"` | Local build from a subdirectory |
| `"myimage:tag"` | Registry image: pulled directly on the monitor, no build step |

---

## Compose Backend

The `compose` backend manages a `docker compose` project per team:

- Challenge files are stored on the monitor at `$CHALLENGES_BASE_DIR/<sanitized_name>/` (default: `/opt/nervctf/challenges/<name>/`; Ansible sets this to `<ctfd_path>/remote-monitor/data/challenges/<name>/`)
- Project name: `ctf-<sanitized_challenge_name>-<6 random chars>`
- A per-team override file (`<project_name>.override.yml`) is written next to the compose file

The override always contains:

1. **Port mappings** per service: one `host_port:internal_port` entry per listed port — all pre-allocated atomically before `docker compose up`.
   - With `internal_ports`: all ports mapped to `compose_service` (default: `app`).
   - With `service_ports`: each service gets its own port list (see [Multi-service port randomization](#multi-service-port-randomization) below).
2. **`image:` key** for every pre-built service, referencing the image built by `nervctf deploy`
3. **Volume mount** for `flag_delivery: file` (if applicable)

### Image naming

Images are built with `-p <dir_name>` where `<dir_name>` is the **lowercased name of the
challenge directory** (the parent of `compose_file`). Docker Compose tags images as
`<dir_name>-<service>`. The per-instance override sets `image: <dir_name>-<service>` for
each such service so that the per-instance project name (`ctf-...-<random6>`) never bleeds
into the image lookup.

Public-image services (`postgres`, `redis`, etc.) that have their own `image:` in the base
compose file are not overridden — only services whose image was built locally are touched.

> **Example**: challenge directory `sigma-notes/` with services `app` and `sigma_admin`
> → images tagged `sigma-notes-app` and `sigma-notes-sigma_admin`
> → override adds `image: sigma-notes-app` / `image: sigma-notes-sigma_admin`

### Flag delivery for compose

**`flag_delivery: env`** (default):

The monitor sets `FLAG=<value>` as an environment variable when calling `docker compose up`.
Authors use `${FLAG}` in their `docker-compose.yml`:

```yaml
services:
  app:
    environment:
      - FLAG=${FLAG}
```

**`flag_delivery: file`**:

The flag is written to `<project_name>.flag` in the challenge dir on the monitor, and
bind-mounted read-only into the container at `flag_file_path`:

```yaml
instance:
  flag_delivery: file
  flag_file_path: /challenge/flag
  # flag_service: other-service  # optional, defaults to compose_service
```

### Multi-service port randomization

Use `service_ports` when a challenge exposes ports on **more than one independent service**.
Each service gets its own set of randomly-allocated host ports:

```yaml
instance:
  backend: compose
  compose_file: docker-compose.yml
  service_ports:
    app:   [80]
    admin: [8080]
  connection: http
```

- `service_ports` replaces both `compose_service` and `internal_ports` — do not set both.
- The primary service (whose port appears in the player-facing `port` field) is `compose_service`
  if that key is present in the map; otherwise the first declared service is used.
- All allocated host ports (across all services) appear in the `extra_ports` API response field
  as `{"<internal_port>": <host_port>}` entries.
- Validator raises a warning if `service_ports` and `internal_ports` are both set.

### Important: `container_name:` must not be set

Do not use `container_name:` in your `docker-compose.yml`. Docker Compose uses the project
name as a prefix by default, making container names unique across teams. A hardcoded
`container_name:` causes all teams to fight over the same name and fail to start.

### Bind mount path constraint

The monitor stores challenge files at `$CHALLENGES_BASE_DIR/<name>/`. Challenge docker-compose.yml
files that reference absolute paths (e.g. cert files) **must use the full path as seen on the runner/host filesystem** (i.e. `$CHALLENGES_BASE_DIR/<name>/...`), because the host Docker daemon resolves bind mount paths from the host filesystem, not from inside the monitor container.

The actual value of `CHALLENGES_BASE_DIR` depends on `ctfd_path` in `.nervctf.yml` — check the
`docker-compose.override.yml` on the CTFd host to see the exact path.

---

## LXC Backend

Launches an LXC/LXD container per team:

1. `lxc launch <lxc_image> <container_name>`
2. `lxc wait --state=Running`
3. `lxc config device add` — one `ctfport{i}` proxy device per entry in `internal_ports` (e.g. `ctfport0`, `ctfport1`, …), each mapping a random host port to the corresponding internal port
4. `lxc exec` — inject flag into `/challenge/flag` (if `flag_mode: random`)

Requires LXD to be installed and initialised on the monitor server. `nervctf setup`
installs LXD via snap and runs `lxd init --auto`.

---

## Vagrant Backend

Currently a stub — returns an error. Vagrant and libvirt are installed by the setup
playbook but the provisioning logic is not yet implemented.

---

## Instance Lifecycle

| Event | What happens |
|-------|-------------|
| Player requests instance | Row inserted with `status='provisioning'`; container started; status updated to `'running'` |
| Player renews | `expires_at` extended by `timeout_minutes`; `renewals_used` incremented |
| Player stops | Container removed; row deleted |
| Instance expires | Background task (30s interval) calls `cleanup_container()` and deletes row |
| Provisioning stuck >30 min | Background task treats the row the same as expired (`created_at` is the reference, not `expires_at`) |
| Container externally killed | Health check (runs every 30s tick) detects the container absent from both `docker ps` and `docker compose ls`; deletes row and cleans up flag |
| Challenge deleted | All instances stopped; challenge config removed from `instance_configs` |

---

## API Responses

All instance info/request/renew responses include the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `provisioning` \| `running` \| `none` |
| `host` | string | Public hostname/IP |
| `port` | number | Primary host port (maps to `internal_ports[0]`, or primary service's first port) |
| `connection_type` | string | Connection template (`nc` / `http` / `ssh`) |
| `expires_at` | string | SQLite datetime of expiry |
| `extra_ports` | object \| null | `{"<internal>": <host>}` for every port mapping when >1 port is exposed across all services; `null` for single-port challenges |
| `connections` | array \| null | Per-service labeled entries (see below); `null` for challenges without `service_ports` |

### `extra_ports` (multi-port, same service)

For `internal_ports: [80, 443]` — all ports go to one service, no labels needed:

```json
{
  "port": 42100,
  "extra_ports": {"80": 42100, "443": 53210}
}
```

The CTFd plugin renders all ports from `extra_ports` as separate links/commands. The `port` field equals `extra_ports["<internal_ports[0]>"]`.

### `connections` (multi-service, `service_ports`)

For `service_ports: {app: [80], admin: [8080]}` — each service has its own labeled entry:

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

The CTFd plugin renders each entry as `<service>: <connection>` so players see clearly which port belongs to which service.

---

## Player UI

The monitor serves a minimal HTML page at `GET /instance/<challenge_name>`. Players enter
their CTFd API token and use Fetch/Extend/Terminate buttons.

The CTFd plugin (`nervctf_instance`) also adds a view panel within the CTFd challenge page
that calls `/api/v1/containers/*` endpoints to manage the instance without leaving CTFd.

---
