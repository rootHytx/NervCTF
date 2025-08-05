# CTFd Configurator

A Dockerized Rust application for managing CTFd challenges using ctfcli. This tool allows you to add, modify, delete, list, sync, and validate challenges on a CTFd instance through a simple CLI interface.

## Features

- Add new challenges from YAML files
- Modify existing challenges
- Delete challenges by ID
- List all challenges
- Sync challenges with CTFd
- Validate challenge configurations
- Dockerized environment with all dependencies

## Prerequisites

- Docker
- CTFd instance URL and API key

## Installation

1. Clone the repository:
```bash
git clone https://github.com/your-org/nerv-ctf.git
cd nerv-ctf/remote-monitor
```

2. Build the Docker images:
```bash
chmod +x build.sh
./build.sh
```

## Configuration

Set the following environment variables:

- `CTFD_URL`: URL of your CTFd instance (e.g., `http://localhost:8000`)
- `CTFD_API_KEY`: Your CTFd admin API key

You can set these in your environment or in a `.env` file in the project root.

## Usage

```bash
./run.sh [command] [options]
```

### Commands

| Command                | Description                                  | Example                              |
|------------------------|----------------------------------------------|--------------------------------------|
| `add <challenge-path>` | Add a new challenge                          | `./run.sh add challenges/sample-challenge` |
| `delete <id>`          | Delete a challenge by ID                     | `./run.sh delete 1`                  |
| `modify <id>`          | Modify an existing challenge                 | `./run.sh modify 1`                  |
| `list`                 | List all challenges                          | `./run.sh list`                      |
| `sync`                 | Sync challenges with CTFd                    | `./run.sh sync`                      |
| `validate`             | Validate challenge configurations            | `./run.sh validate`                  |

### Example Workflow

1. Add a new challenge:
```bash
./run.sh add challenges/sample-challenge
```

2. List challenges to verify:
```bash
./run.sh list
```

3. Sync challenges with CTFd:
```bash
./run.sh sync
```

## Challenge Structure

Challenges should be organized in directories with the following structure:
```
challenges/
└── challenge-name/
    ├── challenge.yml    # Challenge configuration
    └── files/           # Optional challenge files
```

### Sample challenge.yml
```yaml
name: Sample Challenge
category: Web
description: |
  This is a sample challenge created by the CTFd Configurator.
  Try to find the flag in the provided file!

value: 100
type: standard

flags:
  - type: static
    content: CTFd{s4mpl3_fl4g_f0r_t3st1ng}
    data: case_insensitive

files:
  - sample-file.txt

tags:
  - sample
  - easy

hints:
  - content: Check the provided file carefully
    cost: 10
```

## Development

### Build the Rust application
```bash
cargo build --release
```

### Run locally
```bash
CTFD_URL=http://your-ctfd-instance CTFD_API_KEY=your-api-key ./target/release/remote-monitor list
```

## License
MIT
