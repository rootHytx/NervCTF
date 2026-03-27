# NervCTF

**NervCTF** is a CLI tool and Rust library for managing CTFd challenges. It validates, deploys, and synchronizes challenges from local `challenge.yml` files to a CTFd instance, and manages ephemeral per-team challenge containers via the `remote-monitor` service.

---

## Features

- Scan local `challenge.yml` files and deploy/sync to CTFd via direct MariaDB writes
- Full ctfcli spec: simple and detailed flags, hints, requirements (name/ID/advanced)
- Pre-deploy validation with errors (blocking) and warnings (advisory)
- Dependency-ordered deployment (topological sort)
- Smart sync — only updates challenges where fields actually changed
- `type: instance` challenges with ephemeral per-team containers (Docker, Compose, LXC)
- Split-machine mode: challenge containers run on a separate worker node
- Flag-sharing detection: alerts when a team submits another team's instance flag
- `nervctf fix` — interactive YAML patcher for missing fields
- `nervctf setup` — one-command server provisioning via embedded Ansible playbook

---

## Installation

### With Nix (recommended)

```sh
nix develop .# --command cargo build --release
# or
make release-nix     # builds both binaries
make release-musl    # static Linux x86_64 binary
```

`nix develop .#` provides Rust, pkg-config, OpenSSL, and Ansible. Run `make help` for all targets.

### Without Nix

```sh
# Debian/Ubuntu
sudo apt install build-essential pkg-config libssl-dev
curl https://sh.rustup.rs -sSf | sh
cargo build --release
```

### Cross-compilation targets (from Linux via Nix)

| Target | Command |
|--------|---------|
| Linux x86_64 musl (static) | `make release-musl` |
| Linux aarch64 CLI | `make release-arm64` |
| Windows x86_64 CLI | `make release-windows` |

`remote-monitor` is Linux x86_64 only. macOS targets require a macOS machine.

---

## Configuration

Priority (highest wins): CLI flags → env vars → `.nervctf.yml`

| CLI flag | Env var | Description |
|----------|---------|-------------|
| `--monitor-url` | `MONITOR_URL` | Remote monitor URL |
| `--monitor-token` | `MONITOR_TOKEN` | Monitor authentication token |

### `.nervctf.yml`

Searched upward from `--challenges-dir`. Created by `nervctf setup`.

```yaml
monitor_url: http://server:33133
monitor_token: mysecret
challenges_dir: ./challenges

# Split-machine mode (containers run on a separate worker node)
runner_ip: 192.168.1.50
runner_user: docker
```

---

## Directory Structure

```
.
├── .nervctf.yml
└── challenges/
    ├── web/
    │   └── sqli/
    │       ├── challenge.yml
    │       └── dist/source.py
    └── pwn/
        └── overflow/
            └── challenge.yml
```

Challenge files are found recursively (max depth 5). Supported filenames: `challenge.yml`, `challenge.yaml`, or any `*challenge*.yml`.

---

## Usage

### Validate

```sh
nervctf validate
nervctf validate --debug    # full field-by-field view
```

Runs automatically before every `deploy`. Exits 1 on errors.

### Deploy

```sh
nervctf deploy              # create new + update changed challenges
nervctf deploy --dry-run    # show diff only
nervctf deploy --recreate   # force re-deploy all challenges (re-syncs files, rebuilds images)
```

Four phases: (1) cores + flags + tags + hints → (2) files → (3) requirements → (4) next pointers.

### List / Scan

```sh
nervctf list           # list local challenges
nervctf scan           # scan + print statistics
```

### Fix

```sh
nervctf fix            # patch missing state/author/version fields interactively
nervctf fix --dry-run
```

### Setup

```sh
nervctf setup          # provision server (Docker, CTFd, plugin, monitor) via Ansible
nervctf setup --upgrade  # push updated plugin + binary, rebuild, restart
```

Prompts for target IP, SSH user, CTFd remote path, and monitor token. Requires `ansible` (included in `nix develop .#`).

---

## Challenge Specification

### Minimal

```yaml
name: My Challenge
category: web
value: 100
type: standard
flags:
  - flag{example}
```

### Full example

```yaml
name: Advanced Challenge
author: author
category: web
description: Find the vulnerability.
value: 300
type: standard
state: visible
connection_info: "nc challenge.example.com 1337"
attempts: 5

flags:
  - flag{simple}
  - type: static
    content: "flag{alt}"
    data: case_insensitive

tags: [web, sql-injection]
hints:
  - "Free hint"
  - content: "Paid hint"
    cost: 50

files:
  - dist/source.py

requirements:
  - "Warmup"

next: "Follow-up Challenge"
```

### Dynamic scoring

```yaml
name: Hard Pwn
category: pwn
type: dynamic
value: 0
flags: [flag{x}]
extra:
  initial: 500
  decay: 50
  minimum: 100
```

---

## Instance Challenges

`type: instance` challenges provision an ephemeral container for each team. Requires the remote-monitor service and the CTFd plugin (both deployed by `nervctf setup`).

See `docs/instance-challenges.md` for the full reference.

```yaml
name: Exploit Me
category: pwn
type: instance
value: 0
description: Connect and exploit.
extra:
  initial: 500
  decay: 50
  minimum: 100

instance:
  backend: docker         # docker | compose | lxc
  image: .                # local path (".") or registry image
  internal_port: 1337
  connection: nc          # nc | http | ssh
  flag_mode: random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  timeout_minutes: 45
  max_renewals: 3
```

ctfcli-compatible format (fields under `extra:`) is also accepted:

```yaml
type: instance
extra:
  backend: docker
  image: myimage:latest
  internal_port: 1337
  connection: nc
```

---

## Remote Monitor

The `remote-monitor` runs on the CTFd host. It is the **sole point of contact with CTFd** — writing directly to the MariaDB database — and manages instance lifecycle.

```
CLI  ──Token<monitor>──▶  remote-monitor:33133  ──SQL──▶  CTFd MariaDB
                                │                    └──▶  CTFd uploads dir
                          instance manager
                    ┌──────────┴──────────┐
                 local              split-machine
            (docker daemon)   (SSH to runner node)
```

Deployed automatically by `nervctf setup`. For manual details see `docs/remote-monitor.md`.

### Split-machine mode

When `runner_ip` is set in `.nervctf.yml`, the CLI rsyncs challenge files directly to the runner node. The monitor executes all Docker/Compose commands on the runner via SSH (`RUNNER_SSH_TARGET` env var).

### Admin dashboard

```
http://<monitor-host>:33133/admin?token=<MONITOR_TOKEN>
```

Three auto-refreshing tables: **Flag Sharing Alerts** · **Active Instances** · **Recent Flag Attempts**

### Key routes

| Auth | Path | Description |
|------|------|-------------|
| None | `GET /health` | Liveness check |
| `?token=` or header | `GET /admin` | Admin dashboard |
| None | `GET /instance/:name` | Player instance UI |
| Admin | `POST /api/v1/instance/build` | Upload Docker build context |
| Admin | `POST /api/v1/instance/build-compose` | Upload Compose context (single-machine) |
| Admin | `POST /api/v1/instance/build-compose-remote` | Trigger remote build (split-machine) |
| Admin | `POST /api/v1/instance/register` | Register challenge config |
| Admin | `GET/POST /api/v1/challenges[/{id}]` | Challenge CRUD (SQL) |
| Admin | `GET/POST/DELETE /api/v1/flags[/{id}]` | Flag CRUD (SQL) |
| Admin | `GET/POST/DELETE /api/v1/hints[/{id}]` | Hint CRUD (SQL) |
| Admin | `GET/POST/DELETE /api/v1/tags[/{id}]` | Tag CRUD (SQL) |
| Admin | `GET/POST/DELETE /api/v1/files[/{id}]` | File CRUD (SQL + disk) |
| Admin | `POST /api/v1/topics` | Topic upsert (SQL) |
| Admin | `GET /api/v1/admin/instances` | JSON list of active instances |
| Admin | `GET /api/v1/admin/attempts` | Flag attempt log (`?alerts_only=true`) |
| Admin | `GET /api/v1/admin/solves` | Correct solves per team |
| Plugin | `POST /api/v1/plugin/attempt` | Record flag submission + detect sharing |
| Plugin | `POST /api/v1/plugin/solve` | Mark solved + tear down instance |
| Player | `POST /api/v1/instance/request` | Provision instance |
| Player | `POST /api/v1/instance/renew` | Extend expiry |
| Player | `DELETE /api/v1/instance/stop` | Destroy instance |

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| No challenges found | Files must be at `<category>/<name>/challenge.yml` |
| File upload 500 | `chown -R 1001:1001 /path/to/CTFd/.data/CTFd/uploads` |
| `state: Field may not be null` | Run `nervctf fix` |
| Monitor 401 | `MONITOR_TOKEN` mismatch between CLI and server |
| Compose file not found on runner | Run `nervctf deploy` to sync challenge files |
| `ansible-playbook` not found | Run inside `nix develop .#` |
| `docker build` fails on runner | Ensure Docker + BuildKit (`docker-buildx-plugin`) are installed on runner |

---

## Development

```sh
make check          # cargo check all crates
make test           # nervctf unit tests
make fmt            # rustfmt
make release-nix    # native release build (both binaries)
```

Or directly:

```sh
nix develop .# --command cargo build
nix develop .# --command cargo build --release -p remote-monitor
nix develop .# --command cargo test -p nervctf
```

See `ARCHITECTURE.md` for a full system reference and `docs/` for per-module documentation.

---

## License

MIT License. See [LICENSE](LICENSE) for details.
