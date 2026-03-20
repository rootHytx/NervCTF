# NervCTF

**NervCTF** is a CLI tool and Rust library for managing CTFd challenges. It validates, deploys, and synchronizes challenges from local `challenge.yml` files to a CTFd instance, and manages ephemeral per-team challenge containers via the `remote-monitor` service.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Configuration](#configuration)
- [Directory Structure](#directory-structure)
- [Usage](#usage)
- [Challenge Specification](#challenge-specification)
- [Instance Challenges](#instance-challenges)
- [Remote Monitor](#remote-monitor)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

---

## Features

- Scan local `challenge.yml` files and deploy/sync to CTFd
- Full ctfcli spec: simple and detailed flags, hints, requirements (name/ID/advanced)
- Pre-deploy validation with errors (blocking) and warnings (advisory)
- Dependency-ordered deployment (topological sort via Kahn's algorithm)
- Smart sync — only updates challenges where fields actually changed
- `type: instance` challenges with ephemeral per-team containers (Docker, Compose, LXC)
- `nervctf fix` — interactive YAML patcher for missing fields
- `nervctf setup` — one-command server provisioning via embedded Ansible playbook

---

## Installation

### With Nix (recommended)

Uses `flake.nix` — all deps (Rust, pkg-config, OpenSSL, Ansible) are provided:

```sh
nix develop .# --command cargo build --release
```

Or use `make`:

```sh
nix develop .# --command make release-nix   # builds both binaries
```

### Without Nix

```sh
# Debian/Ubuntu
sudo apt install curl build-essential pkg-config libssl-dev
curl https://sh.rustup.rs -sSf | sh
cargo build --release

# macOS
cargo build --release  # Xcode tools + rustup required
```

### Cross-compilation targets

Run `make help` for all targets. Notable:

| Target | Command |
|--------|---------|
| Linux x86_64 musl (static) | `make release-musl` |
| Linux aarch64 | `make release-arm64` |
| Windows x86_64 | `make release-windows` |

macOS targets require a macOS machine (Apple SDK is non-redistributable).

---

## Configuration

Priority (highest wins): CLI flags → env vars → `.nervctf.yml`

### Environment Variables

| Variable | Description |
|----------|-------------|
| `CTFD_URL` | CTFd base URL |
| `CTFD_API_KEY` | CTFd admin API key |
| `MONITOR_URL` | Remote monitor URL (replaces direct CTFd access) |
| `MONITOR_TOKEN` | Monitor authentication token |

### `.nervctf.yml`

Searched upward from `--base-dir`. Created by `nervctf setup`.

```yaml
ctfd_url: https://ctfd.example.com
ctfd_api_key: ctfd_...
monitor_url: http://server:33133
monitor_token: mysecret
base_dir: ./challenges
```

See `docs/` for full config reference including setup/deployment fields.

---

## Directory Structure

```
.
├── .nervctf.yml
└── challenges/
    ├── web/
    │   └── my-challenge/
    │       ├── challenge.yml
    │       └── dist/source.py
    └── pwn/
        └── ...
```

Each challenge must have a `challenge.yml` in its own subdirectory under a category directory.

---

## Usage

### Validate

```sh
nervctf validate
nervctf validate --base-dir ./challenges
```

Runs automatically before every `deploy`. Exits 1 on errors. See `docs/validator.md` for all checks.

### Deploy

```sh
nervctf deploy
nervctf deploy --dry-run
```

Creates new challenges and updates changed ones. Phases: cores+flags+tags+hints → files → requirements → next pointers.

### Sync

```sh
nervctf sync           # show diff + confirm before applying
nervctf sync --diff    # show diff only
```

### List / Scan

```sh
nervctf list
```

### Fix

```sh
nervctf fix              # patch missing state/author/version fields interactively
nervctf fix --dry-run
```

### Setup

```sh
nervctf setup
```

Provisions a remote server with Docker, CTFd, the NervCTF CTFd plugin, and the remote-monitor service via Ansible. Prompts for target IP, SSH user, CTFd path, and monitor token. Requires `ansible` (included in `nix develop`).

### Export

```sh
nervctf export --output ./exported
```

Dumps all CTFd challenges to local YAML files.

---

## Challenge Specification

### Minimal

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
```

### With all optional fields

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
version: "0.3"
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

`type: instance` challenges provision an ephemeral container or VM for each team. Requires the remote-monitor service and the CTFd plugin.

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
  backend: docker       # docker | compose | lxc | vagrant
  image: .              # local path or registry image
  internal_port: 1337
  connection: nc
  flag_mode: random
  flag_prefix: "CTF{"
  flag_suffix: "}"
  timeout_minutes: 45
  max_renewals: 3
```

---

## Remote Monitor

The `remote-monitor` runs on the CTFd host. It keeps the CTFd admin key server-side, proxies API calls from the CLI, and manages instance lifecycle.

```
CLI  ──Token<monitor>──▶  remote-monitor:33133  ──Token<ctfd>──▶  CTFd:8000
                                │
                          instance manager
                       (docker/compose/lxc/vagrant)
```

Deployed automatically by `nervctf setup`. For manual details see `docs/remote-monitor.md`.

### Admin dashboard

Open in a browser — no extra tools needed:

```
http://<monitor-host>:33133/admin?token=<MONITOR_TOKEN>
```

The token is the `monitor_token` value in your `.nervctf.yml` (written there by `nervctf setup`).

The dashboard shows three auto-refreshing tables:
- **Flag Sharing Alerts** — submissions where the flag belonged to a *different* team's instance (refreshes every 15 s)
- **Active Instances** — all running containers with host/port and expiry (refreshes every 15 s)
- **Recent Flag Attempts** — last 200 submissions across all teams (refreshes every 30 s)

### Key routes

| Auth | Path | Description |
|------|------|-------------|
| None | `GET /health` | Liveness check |
| `?token=` or header | `GET /admin` | Admin dashboard HTML |
| None | `GET /instance/:name` | Player instance UI |
| Admin | `POST /api/v1/instance/build` | Upload Docker build context |
| Admin | `POST /api/v1/instance/build-compose` | Upload Compose challenge context |
| Admin | `POST /api/v1/instance/register` | Register challenge config |
| Admin | `GET /api/v1/admin/instances` | JSON list of all active instances |
| Admin | `GET /api/v1/admin/attempts` | JSON flag attempt log (`?alerts_only=true` for sharing only) |
| Plugin | `POST /api/v1/plugin/attempt` | Record a flag submission + detect sharing |
| Player | `POST /api/v1/instance/request` | Provision instance |
| Player | `POST /api/v1/instance/renew` | Extend expiry |
| Player | `DELETE /api/v1/instance/stop` | Destroy instance |
| Proxy | `ANY /api/v1/*` | Transparent CTFd proxy |

---

## Troubleshooting

- **No challenges found** — challenges must be at `challenges/<category>/<name>/challenge.yml`
- **302 redirect to /login** — invalid API key, or CTFd Visibility is set to *Private* (Admin → Config → Visibility → set to *Public*)
- **File upload 500** — fix with `chown -R 1001:1001 /path/to/CTFd/.data/CTFd/uploads`
- **`state: Field may not be null`** — run `nervctf fix` to add missing `state` fields
- **Monitor 401** — `MONITOR_TOKEN` mismatch between CLI and server
- **Compose file not found on monitor** — run `nervctf deploy` to upload the challenge context; see `docs/remote-monitor.md`
- **`ansible-playbook` not found** — run inside `nix develop .#` which provides Ansible

---

## Development

```sh
# Build
nix develop .# --command cargo build
nix develop .# --command cargo build --release -p remote-monitor

# Test
nix develop .# --command cargo test -p nervctf

# Format / lint
nix develop .# --command cargo fmt
nix develop .# --command cargo clippy
```

See `ARCHITECTURE.md` for a full machine-readable reference of the entire system.
See `docs/` for per-module documentation.
See `docs/claude-changes.md` for a record of all architectural changes.

---

## License

MIT License. See [LICENSE](LICENSE) for details.
