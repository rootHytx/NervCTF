# NervCTF

**NervCTF** is a minimalistic, robust, and extensible CLI tool and Rust library for managing CTFd challenges. It is designed for CTF organizers and advanced users who need to efficiently synchronize, validate, and deploy challenges between a local file system and a remote [CTFd](https://ctfd.io/) instance.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Configuration](#configuration)
- [Directory Structure](#directory-structure)
- [Usage](#usage)
  - [Deploy Challenges](#deploy-challenges)
  - [List Challenges](#list-challenges)
  - [Scan Directory](#scan-directory)
  - [Auto-Manager (Sync)](#auto-manager-sync)
- [Challenge Specification](#challenge-specification)
- [Rules and Best Practices](#rules-and-best-practices)
- [Troubleshooting](#troubleshooting)
- [Development & Contribution](#development--contribution)
- [License](#license)

---

## Features

- **Automatic scanning** of local directories for challenge definitions (`challenge.yml`, etc.)
- **Validation** of challenge files and referenced resources
- **Dependency resolution** for challenge requirements
- **Atomic deployment and update** of challenges, flags, tags, hints, and files to CTFd
- **Synchronization** between local and remote challenge states (diff, dry-run, and apply)
- **Extensible**: Easily add new challenge fields, types, or API endpoints
- **Comprehensive error reporting** and statistics

---

## Installation

### Prerequisites

- Rust toolchain (1.70+ recommended)
- Access to a running CTFd instance (admin API key required)
- [Docker](https://www.docker.com/) (optional, for containerized usage)

### Build from Source

```sh
git clone https://github.com/your-org/nervctf.git
cd nervctf/NervCTF/nervctf
cargo build --release
```

The binary will be at `target/release/nervctf`.

### Docker

A Dockerfile is provided for reproducible builds and deployment.

```sh
docker build -t nervctf .
docker run --rm -it -e CTFD_URL=... -e CTFD_API_KEY=... -v $(pwd):/workspace nervctf list
```

---

## Configuration

Set the following environment variables before running any commands:

- `CTFD_URL`: Base URL of your CTFd instance (e.g., `https://ctfd.example.com`)
- `CTFD_API_KEY`: Admin API key for authentication

You can use a `.env` file or export variables in your shell:

```sh
export CTFD_URL="https://ctfd.example.com"
export CTFD_API_KEY="YOUR_API_KEY"
```

---

## Directory Structure

Organize your challenges as follows:

```
challenges/
  category1/
    challenge1/
      challenge.yml
      dist/
      static/
      ...
    challenge2/
      challenge.yml
      ...
  category2/
    ...
```

- Each challenge must have a `challenge.yml` (or `.yaml`/`.json`) file in its directory.
- Referenced files (in `files:`) must exist relative to the challenge directory.

---

## Usage

Run `nervctf --help` for all options.

### Deploy Challenges

Deploy all local challenges to CTFd:

```sh
nervctf deploy --base-dir ./challenges
```

- Validates and uploads all challenges found in the specified directory.
- Reports success/failure for each challenge.

### List Challenges

List all local challenges (optionally with details):

```sh
nervctf list --base-dir ./challenges
nervctf list --base-dir ./challenges --detailed
```

### Scan Directory

Scan for challenge files and print statistics:

```sh
nervctf scan --base-dir ./challenges
nervctf scan --base-dir ./challenges --detailed
```

### Auto-Manager (Sync)

Automatically verify and synchronize local challenges with CTFd:

```sh
nervctf auto-manager --base-dir ./challenges
```

- Shows a diff of local vs. remote challenges.
- Resolves dependencies and applies changes in correct order.
- Use `--dry-run` to preview changes without applying.
- Use `--watch` to monitor for changes (experimental).

---

## Challenge Specification

Each challenge must be described in a YAML (or JSON) file. Example:

```yaml
name: "Example Challenge"
author: "author"
category: "web"
description: "This is a sample challenge."
value: 100
type: standard
flags:
  - flag{example}
  - { type: "static", content: "flag{alt}", data: "case_insensitive" }
tags:
  - web
  - easy
files:
  - dist/source.py
hints:
  - "Check the source code"
requirements:
  - "Warmup"
state: visible
version: "0.1"
```

See `docs/challenge_manager.md` for a full schema and advanced options.

---

## Rules and Best Practices

1. **Challenge files must be valid YAML/JSON** and match the expected schema.
2. **All referenced files must exist** relative to the challenge directory.
3. **Challenge names and categories must be unique and non-empty.**
4. **Flags must not be empty** and should follow CTFd's flag format.
5. **Requirements (dependencies) must reference existing challenge names.**
6. **Avoid circular dependencies** in requirements.
7. **Use dry-run mode** before applying changes to production CTFd instances.
8. **Backup your CTFd database** before bulk updates or syncs.
9. **Test locally** using the `scan` and `list` commands before deploying.
10. **Keep your challenge specification files under version control.**

---

## Troubleshooting

- **No challenges found:** Check your directory structure and file patterns.
- **File not found errors:** Ensure all files listed in `files:` exist.
- **API errors:** Verify your `CTFD_URL` and `CTFD_API_KEY`. Check CTFd logs for more details.
- **Partial updates:** If a sync or deploy fails mid-way, rerun the command after fixing errors.
- **Dependency issues:** Use the diff output to identify missing or circular requirements.

For more details, see `docs/challenge_manager.md` and `docs/ctfd_api.md`.

---

## Development & Contribution

- See `docs/challenge_manager.md` and `docs/ctfd_api.md` for developer documentation.
- Contributions are welcome! Please open issues or pull requests on the repository.
- Run tests with `cargo test`.

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.

---
