# NervCTF

**NervCTF** is a minimalistic, robust, and extensible CLI tool and Rust library for managing CTFd challenges. It is designed for CTF organizers and advanced users who need to efficiently synchronize, validate, and deploy challenges between a local file system and a remote [CTFd](https://ctfd.io/) instance.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Configuration](#configuration)
  - [Environment Variables](#environment-variables)
  - [Config File `.nervctf.yml`](#config-file-nervctfyml)
- [Directory Structure](#directory-structure)
- [Usage](#usage)
  - [Deploy Challenges](#deploy-challenges)
  - [List Challenges](#list-challenges)
  - [Scan Directory](#scan-directory)
  - [Auto (Sync)](#auto-sync)
  - [Remote Monitor](#remote-monitor)
- [Challenge Specification](#challenge-specification)
- [Remote Monitor](#remote-monitor-1)
- [Rules and Best Practices](#rules-and-best-practices)
- [Troubleshooting](#troubleshooting)
- [Development & Contribution](#development--contribution)
- [License](#license)

---

## Features

- **Automatic scanning** of local directories for challenge definitions (`challenge.yml`)
- **Full ctfcli spec compliance** — simple and detailed flags, hints, requirements (name/ID/advanced)
- **Validation** of challenge files and referenced resources
- **Dependency resolution** for challenge requirements (topological sort)
- **Atomic deployment** of challenges, flags, tags, topics, hints, and files to CTFd
- **Synchronization** between local and remote challenge states (diff, dry-run, and apply)
- **Config file support** — `.nervctf.yml` with env var and CLI flag overrides
- **Remote Monitor** — keep your CTFd admin key on the server; point the CLI at the monitor instead
- **Extensible**: Easily add new challenge fields, types, or API endpoints

---

## Installation

### Prerequisites

- Rust toolchain (1.70+ recommended)
- Access to a running CTFd instance (admin API key required)
- `pkg-config` and `openssl` headers (for TLS support)
- [Docker](https://www.docker.com/) (optional, for containerized usage)

### With Nix

A `shell.nix` is provided with all build dependencies:

```sh
nix-shell
cd nervctf && cargo build --release
```

### Build from Source

```sh
cd nervctf
cargo build --release
```

The binary will be at `nervctf/target/release/nervctf`.

### Docker

```sh
docker build -t nervctf .
docker run --rm -it -e CTFD_URL=... -e CTFD_API_KEY=... -v $(pwd):/workspace nervctf list
```

---

## Configuration

Configuration is resolved in this priority order (highest wins):

1. **CLI flags** (`--monitor-url`, `--monitor-token`)
2. **Environment variables**
3. **`.nervctf.yml`** config file (searched from `--base-dir` upward)

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `CTFD_URL` | Yes* | Base URL of your CTFd instance |
| `CTFD_API_KEY` | Yes* | CTFd admin API key |
| `MONITOR_URL` | No | Remote monitor URL (replaces `CTFD_URL` when set) |
| `MONITOR_TOKEN` | No | Monitor authentication token |

*Can also be set in `.nervctf.yml`.

```sh
export CTFD_URL="https://ctfd.example.com"
export CTFD_API_KEY="ctfd_..."
```

### Config File `.nervctf.yml`

Place a `.nervctf.yml` in your project root (or any ancestor directory). NervCTF
walks up from `--base-dir` until it finds one.

```yaml
# .nervctf.yml
ctfd_url: https://ctfd.example.com
ctfd_api_key: ctfd_...        # optional if set via env var
monitor_url: http://server:33133   # optional
monitor_token: mysecret            # optional
base_dir: ./challenges
```

When both `monitor_url` and `monitor_token` are resolved, all API traffic is
routed through the remote monitor instead of directly to CTFd. The monitor
exposes the same `/api/v1/*` API, so no other changes are needed.

---

## Directory Structure

```
.
├── .nervctf.yml          ← optional project config
├── challenges/
│   ├── web/
│   │   ├── my-challenge/
│   │   │   ├── challenge.yml
│   │   │   └── dist/
│   │   │       └── source.py
│   │   └── ...
│   ├── pwn/
│   │   └── ...
│   └── ...
└── remote-monitor/       ← proxy server (optional)
```

- Each challenge must have a `challenge.yml` in its own subdirectory.
- Referenced files (in `files:`) must exist relative to the challenge directory.

---

## Usage

Run `nervctf --help` for all options.

### Deploy Challenges

Deploy all local challenges to CTFd (or the monitor):

```sh
nervctf deploy
nervctf deploy --base-dir ./challenges
```

Handles flags, tags, topics, hints, file uploads, requirements, and `next` in
the correct order.

### List Challenges

```sh
nervctf list
nervctf list --detailed
```

### Scan Directory

Scan for challenge files and print statistics:

```sh
nervctf scan
nervctf scan --detailed
```

### Auto (Sync)

Verify local challenges then sync with CTFd:

```sh
nervctf auto
nervctf auto --dry-run     # preview diff without applying
nervctf auto --watch       # re-sync every 30 seconds
```

The sync compares all significant fields (flags, tags, hints, state,
connection\_info, attempts, extra) — not just name and value.

### Remote Monitor

Point the CLI at a running remote-monitor instead of CTFd directly:

```sh
# Via CLI flags
nervctf deploy --monitor-url http://server:33133 --monitor-token mysecret

# Via environment variables
export MONITOR_URL=http://server:33133
export MONITOR_TOKEN=mysecret
nervctf deploy

# Via .nervctf.yml
# monitor_url / monitor_token keys — see Configuration above
```

---

## Challenge Specification

Each challenge is described by a `challenge.yml`. NervCTF follows the
[ctfcli](https://github.com/CTFd/ctfcli) spec.

### Minimal example

```yaml
name: "My Challenge"
category: "web"
value: 100
type: standard
flags:
  - flag{example}
```

### Full example

```yaml
name: "Advanced Challenge"
author: "author"
category: "web"
description: "Find the vulnerability and capture the flag."
value: 300
type: standard
state: visible
connection_info: "nc challenge.example.com 1337"
attempts: 5

flags:
  # Simple (static, case-sensitive)
  - flag{simple}
  # Detailed with explicit type and case mode
  - type: static
    content: "flag{alt}"
    data: case_insensitive

tags:
  - web
  - sql-injection

topics:
  - OWASP Top 10

hints:
  # Simple free hint
  - "Check the source code"
  # Detailed hint with cost
  - content: "Look at the HTTP headers"
    cost: 50
  - content: "The flag is in the cookie"
    cost: 100
    title: "Big hint"

files:
  - dist/source.py
  - dist/Dockerfile

requirements:
  # Simple name list
  - "Warmup"
  # OR advanced format:
  # prerequisites: ["Warmup"]
  # anonymize: true

next: "Follow-up Challenge"

extra:
  initial: 500
  decay: 50
  minimum: 100

version: "0.1"
```

### `requirements` formats

```yaml
# Simple list of names
requirements:
  - "Warmup"
  - "Another Challenge"

# Simple list of integer IDs
requirements:
  - 1
  - 3

# Advanced object
requirements:
  prerequisites:
    - "Warmup"
  anonymize: true
```

---

## Remote Monitor

The `remote-monitor` is a standalone HTTP server that acts as a transparent
proxy between the NervCTF CLI and CTFd. It keeps your CTFd admin key on the
server while clients authenticate with a separate monitor token.

```
Developer Machine                     Server
┌──────────────┐  Token <MONITOR>    ┌──────────────────────┐
│  nervctf CLI │────────────────────▶│  remote-monitor:33133│
│              │                     │  strips monitor token │
│              │                     │  adds CTFd API key   │
└──────────────┘                     └──────────┬───────────┘
                                                │ Token <CTFD_KEY>
                                                ▼
                                     ┌──────────────────────┐
                                     │  CTFd :8000          │
                                     └──────────────────────┘
```

### Starting the monitor

```sh
cd remote-monitor
CTFD_URL=http://localhost:8000 \
CTFD_API_KEY=ctfd_admin_key \
MONITOR_TOKEN=mysecret \
cargo run --release
```

Optional environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `MONITOR_PORT` | `33133` | Port to bind |
| `MONITOR_BIND` | `0.0.0.0` | Bind address |

### Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/health` | No | Liveness check |
| ANY | `/api/v1/diff` | Yes | Diff local vs. remote challenges |
| ANY | `/api/v1/*path` | Yes | Transparent proxy to CTFd |

### Diff endpoint

```sh
curl -X POST http://localhost:33133/api/v1/diff \
  -H "Authorization: Token mysecret" \
  -H "Content-Type: application/json" \
  -d '{"challenges": [...]}'
```

Response:
```json
{
  "to_create": ["New Challenge"],
  "to_update": ["Modified Challenge"],
  "up_to_date": ["Unchanged Challenge"],
  "remote_only": ["Orphan Challenge"]
}
```

---

## Rules and Best Practices

1. **Challenge files must be valid YAML** and match the expected schema.
2. **All referenced files must exist** relative to the challenge directory.
3. **Challenge names must be unique and non-empty.**
4. **Flags must not be empty** and should follow CTFd's flag format.
5. **Requirements must reference existing challenge names** (or valid integer IDs).
6. **Avoid circular dependencies** in requirements.
7. **Use `--dry-run`** before applying changes to a production CTFd instance.
8. **Backup your CTFd database** before bulk updates or syncs.
9. **Keep `challenge.yml` files under version control.**
10. **Use the remote monitor** when the CTFd admin key should not leave the server.

---

## Troubleshooting

- **No challenges found** — check your directory structure; challenges must be nested under `challenges/<category>/<name>/challenge.yml`.
- **File not found errors** — ensure all paths in `files:` exist relative to the challenge directory.
- **API errors** — verify `CTFD_URL` and `CTFD_API_KEY`; check CTFd admin logs.
- **Monitor 401 Unauthorized** — verify `MONITOR_TOKEN` matches what the server was started with.
- **Monitor unreachable** — check `MONITOR_PORT`/`MONITOR_BIND` and any firewall rules.
- **Partial updates** — if a sync fails mid-way, rerun; already-deployed resources are idempotent.
- **Dependency errors** — the sync resolves requirements topologically; circular deps will stall.

---

## Development & Contribution

Build and test:

```sh
# In nix-shell (recommended)
nix-shell
cd nervctf && cargo build
cd ../remote-monitor && cargo build
```

Run tests:

```sh
cd nervctf && cargo test
```

See `claude-changes.md` for a detailed record of recent changes.

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
