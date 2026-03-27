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
  backend: docker       # docker | compose | lxc | vagrant
  internal_port: 1337   # port exposed inside the container
  connection: nc        # nc | http | ssh
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
  internal_port: 1337
  connection: nc              # nc | http | ssh
  command: null               # override container entrypoint/CMD (optional)
  timeout_minutes: 45
  max_per_team: 1             # max concurrent instances per team (enforced by plugin)
  max_renewals: 3

  # ── Flag ────────────────────────────────────────────────────────────────────
  flag_mode: random           # static | random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  random_flag_length: 16

  # How the per-instance flag reaches the container:
  flag_delivery: env          # env (default) | file

  # "env": FLAG is set as an environment variable.
  # Challenge's docker-compose.yml can use ${FLAG} in any service environment block.

  # "file": flag is written to a bind-mounted file inside the container.
  flag_file_path: /challenge/flag    # required for flag_delivery: file
  flag_service: app                  # compose service receiving the file (defaults to compose_service)
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
  internal_port: 4000
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
runner_user: docker   # default: docker
```

Bind mount paths in `docker-compose.yml` must use the path as seen on the **runner** filesystem
(not the monitor container). The runner stores challenge files at the same path used during rsync.

---

## Docker Backend

The `docker` backend runs a single container per team:

```
docker run -d \
  --name ctf-<challenge>-t<team_id> \
  -p <host_port>:<internal_port> \
  [-e FLAG=<random_flag>] \
  [--entrypoint <command>] \
  <image_tag>
```

- Port is picked randomly in range 40000–50000 (avoiding ports already in use)
- Container name: `ctf-<sanitized_challenge_name>-t<team_id>`
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

- Challenge files are stored on the monitor at `/data/challenges/<sanitized_name>/`
- A per-team override file (`<project_name>.override.yml`) is written next to the compose file
- The override maps `host_port:internal_port` and optionally injects the flag
- Project name: `ctf-<sanitized_challenge_name>-t<team_id>`

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

### Important: `container_name:` must not be set

Do not use `container_name:` in your `docker-compose.yml`. Docker Compose uses the project
name as a prefix by default, making container names unique across teams. A hardcoded
`container_name:` causes all teams to fight over the same name and fail to start.

### Bind mount path constraint

The monitor stores challenge files at `/data/challenges/<name>/`. Challenge docker-compose.yml
files that reference absolute paths (e.g. cert files) **must use `/data/challenges/<name>/...`**
as the path, because the host Docker daemon resolves bind mount paths from the host filesystem,
not from inside the monitor container.

---

## LXC Backend

Launches an LXC/LXD container per team:

1. `lxc launch <lxc_image> <container_name>`
2. `lxc wait --state=Running`
3. `lxc config device add` — proxy port `host_port → internal_port`
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
| Player requests instance | `provision()` called; container started; row inserted into `instances` table |
| Player renews | `expires_at` extended by `timeout_minutes`; `renewals_used` incremented |
| Player stops | Container removed; row deleted |
| Instance expires | Background task (30s interval) calls `cleanup_container()` and deletes row |
| Challenge deleted | All instances stopped; challenge config removed from `instance_configs` |

---

## Player UI

The monitor serves a minimal HTML page at `GET /instance/<challenge_name>`. Players enter
their CTFd API token and use Fetch/Extend/Terminate buttons.

The CTFd plugin (`nervctf_instance`) also adds a view panel within the CTFd challenge page
that calls `/api/v1/containers/*` endpoints to manage the instance without leaving CTFd.

---
