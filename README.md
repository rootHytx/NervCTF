# NervCTF

**NervCTF** is a minimalistic, robust, and extensible CLI tool and Rust library for managing CTFd challenges. It is designed for CTF organizers and advanced users who need to efficiently validate, deploy, and synchronize challenges between a local file system and a remote [CTFd](https://ctfd.io/) instance.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Configuration](#configuration)
  - [Environment Variables](#environment-variables)
  - [Config File `.nervctf.yml`](#config-file-nervctfyml)
- [Directory Structure](#directory-structure)
- [Usage](#usage)
  - [Validate Challenge Files](#validate-challenge-files)
  - [Deploy Challenges](#deploy-challenges)
  - [List Challenges](#list-challenges)
  - [Scan Directory](#scan-directory)
  - [Fix Challenge Files](#fix-challenge-files)
  - [Setup Remote Environment](#setup-remote-environment)
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
- **Pre-deploy validation** — catches errors and warnings in challenge YAML files before touching CTFd
- **Dependency resolution** for challenge requirements (topological sort via Kahn's algorithm)
- **4-phase atomic deployment** — cores+flags+tags+hints → file uploads → requirements → next pointers
- **Smart sync** — compares all significant fields (flags, hints, tags, state, connection_info, attempts, extra) and only touches what changed
- **Config file support** — `.nervctf.yml` with env var and CLI flag overrides
- **Remote Monitor** — keep your CTFd admin key on the server; point the CLI at the monitor instead
- **`nervctf fix`** — interactive YAML linter/patcher; adds missing `state`, `author`, `version` fields
- **`nervctf setup`** — fully automated remote provisioning (Docker, CTFd, plugins) via embedded Ansible playbook
- **Extensible**: easily add new challenge fields, types, or API endpoints

---

## Installation

### Prerequisites

- Rust toolchain (1.70+ recommended)
- Access to a running CTFd instance (admin API key required)
- TLS is handled by `rustls` (pure Rust) — **no system OpenSSL required** for building

### With Nix (recommended)

A `shell.nix` is provided with all build dependencies (Rust, pkg-config, OpenSSL, Ansible):

```sh
nix-shell
make release          # builds both binaries and copies them to project root
```

Or build individually:

```sh
nix-shell --run "cargo build --release -p nervctf"
nix-shell --run "cargo build --release -p remote-monitor"
```

### Debian / Ubuntu

```sh
sudo apt install curl build-essential
curl https://sh.rustup.rs -sSf | sh
make release-linux
```

### Fedora / RHEL / CentOS

```sh
sudo dnf install curl gcc
curl https://sh.rustup.rs -sSf | sh
make release-linux
```

### Arch Linux

```sh
sudo pacman -S rust
make release-linux
```

### Alpine Linux (static binary)

```sh
apk add rust cargo
make release-musl    # produces a fully static binary with no libc dependency
```

### macOS

```sh
# Intel Mac
make release-macos

# Apple Silicon
make release-macos-arm
```

### All available `make` targets

Run `make help` to see all targets:

```
  release                [Nix] Release build inside nix-shell — all deps bundled
  release-nix            Alias for release
  release-linux          [Debian/Ubuntu/Fedora/Arch/RHEL] x86_64 Linux GNU binary
  release-musl           [Alpine/containers/any Linux] Static x86_64 binary
  release-arm64          [Raspberry Pi 4/5, AWS Graviton, Oracle ARM] aarch64 Linux GNU
  release-macos          [macOS Intel] x86_64 Apple Darwin binary
  release-macos-arm      [macOS Apple Silicon] aarch64 Apple Darwin binary
  release-windows        [Windows] x86_64 GNU Windows binary
  all-linux              Build all Linux release targets
  all-platforms          Build all platforms
  install                Install both binaries to /usr/local/bin (requires sudo)
  install-user           Install both binaries to ~/.local/bin (no sudo required)
  fmt                    Format all crates with rustfmt
  check                  Run cargo check on all workspace crates
  test                   Run nervctf unit tests
  clean                  Remove build artifacts, dist/, and root-level binaries
```

Cross-compilation targets (`release-musl`, `release-arm64`, `release-windows`) use
[`cross`](https://github.com/cross-rs/cross) when available, otherwise fall back to
`cargo` (requires the rustup target to be installed manually).

```sh
cargo install cross      # Docker-based cross-compiler
rustup target add x86_64-unknown-linux-musl   # or whichever target you need
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

### Validate Challenge Files

Check all `challenge.yml` files for errors and warnings before deploying:

```sh
nervctf validate
nervctf validate --base-dir ./challenges
```

Validation runs automatically as a pre-flight check before every `deploy`. It
blocks deployment on errors but only warns on non-critical issues.

**Errors** (block deployment):
- Missing or empty `name` / `category`
- `value == 0` for standard challenges
- Missing `extra` fields for dynamic challenges (`initial`, `decay`)
- No flags defined
- Empty flag content
- Referenced files missing on disk
- Duplicate challenge names
- Challenge lists itself as a prerequisite

**Warnings** (shown but do not block):
- Missing or empty `description`
- `extra.minimum` not set for dynamic challenges
- Prerequisite or `next` target not found in local challenge set

Exit code is 1 when errors are present (useful in CI pipelines).

### Deploy Challenges

Deploy all local challenges to CTFd (creates new, updates changed, skips up-to-date):

```sh
nervctf deploy
nervctf deploy --dry-run          # preview diff without applying changes
nervctf deploy --base-dir ./challenges
```

Deploy runs in four ordered phases:

1. **Phase 1 — Cores, flags, tags, topics, hints**: Create or update each
   challenge's core fields and all sub-resources.
2. **Phase 2 — File uploads**: Upload all files for each challenge in a single
   batched multipart request (matching CTFd's expected format).
3. **Phase 3 — Requirements**: Resolve prerequisite names to CTFd IDs and PATCH
   each challenge's requirements.
4. **Phase 4 — Next pointers**: Resolve `next:` names to IDs and PATCH.

Phases 3 and 4 run after all challenges exist so forward-references always resolve.

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

### Fix Challenge Files

Scan all `challenge.yml` files for missing required fields and interactively
patch them:

```sh
nervctf fix                          # interactive, applies changes
nervctf fix --dry-run                # preview what would be changed
nervctf fix --base-dir ./challenges  # target a specific directory
```

The fixer detects and offers to patch:

| Field | Default offered | Inserted after |
|-------|----------------|----------------|
| `state` | `visible` or `hidden` (user chooses) | `type:` line |
| `author` | user-entered string | `name:` line |
| `version` | `'0.3'` | before `flags:` |

Each category is reported separately and can be skipped independently.

### Setup Remote Environment

Provision a remote server with Docker, CTFd, and the required plugins entirely
from within the binary — no separate scripts or files needed:

```sh
nervctf setup
```

Prompts for target IP, SSH user, CTFd installation path, and SSH key selection
(or generation). Persists answers to `.env` for subsequent runs. Runs the
embedded Ansible playbook to:

1. Install rootless Docker
2. Create a `docker` user and configure SSH access
3. Clone CTFd (or use an existing installation)
4. Install the solve-webhook plugin and containers plugin
5. Deploy the CTFd Docker Compose stack

Requires `ansible` to be installed (included in `shell.nix`).

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
  - "Warmup"

next: "Follow-up Challenge"

version: "0.1"
```

### Dynamic scoring example

```yaml
name: "Hard Pwn"
category: "pwn"
type: dynamic
state: visible
description: "..."
flags:
  - flag{dynamic}
extra:
  initial: 500
  decay: 50
  minimum: 100
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

1. **Run `nervctf validate` before deploying.** Fix all errors; review warnings.
2. **Challenge files must be valid YAML** and match the expected schema.
3. **All referenced files must exist** relative to the challenge directory.
4. **Challenge names must be unique and non-empty.**
5. **Flags must not be empty** and should follow CTFd's flag format.
6. **Requirements must reference existing challenge names** (or valid integer IDs).
7. **Avoid circular dependencies** in requirements.
8. **Use `--dry-run`** before applying changes to a production CTFd instance.
9. **Backup your CTFd database** before bulk updates.
10. **Keep `challenge.yml` files under version control.**
11. **Use the remote monitor** when the CTFd admin key should not leave the server.

---

## Troubleshooting

- **No challenges found** — check directory structure; challenges must be nested as
  `challenges/<category>/<name>/challenge.yml`.

- **File not found errors** — ensure all paths in `files:` exist relative to the
  challenge directory.

- **API 302 redirect to /login** — the CTFd API key is invalid or expired, OR
  **Challenge Visibility** in CTFd admin is set to *Private*. Set it to *Public*
  (CTFd Admin → Config → Visibility) before running deploy.

- **File upload 500 errors** — the CTFd uploads directory lacks write permissions.
  Fix with:
  ```sh
  sudo chown -R 1001:1001 /path/to/CTFd/.data/CTFd/uploads
  ```

- **`state: Field may not be null`** — `challenge.yml` is missing the `state` field.
  Run `nervctf fix` to patch all affected files, or add `state: visible` manually.

- **Monitor 401 Unauthorized** — verify `MONITOR_TOKEN` matches what the server
  was started with.

- **Monitor unreachable** — check `MONITOR_PORT`/`MONITOR_BIND` and firewall rules.

- **Partial updates** — if a sync fails mid-way, rerun; already-deployed resources
  are idempotent.

- **Dependency errors** — the deploy resolves requirements topologically; circular
  deps will stall. Run `nervctf validate` to catch self-referencing requirements.

- **`nervctf setup` fails: ansible not found** — run inside `nix-shell` which
  provides ansible, or install ansible manually.

---

## Development & Contribution

Build and test:

```sh
# In nix-shell (recommended) — workspace root
nix-shell
cargo build             # both crates
cargo build -p nervctf
cargo build -p remote-monitor
make release            # release build + copy binaries to project root
```

Run tests:

```sh
cargo test -p nervctf
```

Format and lint:

```sh
make fmt
make check
```

See `docs/` for detailed module documentation and `docs/claude-changes.md` for a
record of all architectural changes.

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
