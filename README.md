# NervCTF

Run a CTFd competition from YAML. Deploy challenges with one command, provision per-team container instances, and detect flag sharing. The CLI talks to a monitor service that writes CTFd's MariaDB directly, so no CTFd API key is needed.

> Tested against CTFd 3.7.3. After a CTFd upgrade, run `nervctf probe --refresh` before the next deploy.

## How it works

- **`nervctf`** — CLI on your machine. Reads `challenge.yml`, validates, and deploys.
- **`remote-monitor`** — service on the CTFd host. MariaDB access, container instances, admin dashboard.
- **`nervctf_instance`** — CTFd plugin. Adds the `instance` challenge type and the player controls.

Single-machine mode runs everything on the CTFd host. Split-machine mode runs containers on a separate runner reached over SSH.

## Install

Download the binaries from [GitHub Releases](https://github.com/rootHytx/NervCTF/releases), rename them to `nervctf` and `remote-monitor`, and put them in `PATH`. The CTFd plugin is embedded in `nervctf`.

`nervctf setup` needs `ansible-playbook`, `ssh`, and `rsync` on the machine you run it from. Ansible's control node must be Linux or macOS. To use an Ansible binary outside `PATH`, set `ANSIBLE_PLAYBOOK=/path/to/ansible-playbook`.

From source:

```sh
nix develop .# --command cargo build --release
```

## Quick start

```sh
nervctf setup
nervctf validate
nervctf probe --refresh   # optional: inspect (and refresh) CTFd compatibility
nervctf deploy
```

`deploy` reads the monitor's cached probe before its first write and aborts if dynamic scoring is broken. Run `probe --refresh` after a CTFd upgrade so that cache reflects the new schema; a plain `probe` shows the cached result.

`setup` prompts for the host, SSH user, CTFd path, monitor port, token, SSH key, and the first CTFd admin account. It writes `.nervctf.yml` with mode `0600`, then completes CTFd's first-run setup, so the CTFd wizard does not appear.

## Configuration

Minimal `.nervctf.yml`:

```yaml
monitor_ip: 10.0.0.10
monitor_token: <64-char hex>
monitor_user: root
monitor_ctfd_path: /home/root/CTFd
ssh_key_path: ~/.ssh/id_rsa
challenges_path: ./challenges

event:
  name: "My CTF"
  user_mode: teams

admin:
  user: admin
  email: admin@example.com
  password: change-me
```

Precedence: CLI flags > environment variables (`MONITOR_URL`, `MONITOR_TOKEN`) > `.nervctf.yml`. The full field list, `event:` and `admin:` options, and split-machine settings are in the [CLI reference](docs/cli-reference.md#configuration).

## Challenge format

```yaml
name: Example
category: web
value: 100
type: standard
flags:
  - flag{example}
```

`type: instance` adds an `instance:` block that controls the container image, ports, lifetime, and flag delivery. See [instance challenges](docs/instance-challenges.md).

## Commands

| Command | Purpose |
|---|---|
| `setup [--upgrade]` | Provision or update the server |
| `deploy [--dry-run] [--recreate] [--prune]` | Sync challenges to CTFd |
| `validate [--debug]` | Check challenge files |
| `probe [--refresh] [--json]` | Check CTFd compatibility |
| `event [--show]` | Apply the `event:` block to CTFd settings |
| `list`, `scan`, `fix` | Inspect and repair local files |

Every flag, deploy phase, exit code, and command behavior is in the [CLI reference](docs/cli-reference.md).

## Documentation

| Document | Contents |
|---|---|
| [CLI reference](docs/cli-reference.md) | Commands, flags, configuration, behavior |
| [Instance challenges](docs/instance-challenges.md) | Instance YAML, backends, multi-port |
| [Remote monitor](docs/remote-monitor.md) | Routes, environment, probe, event log |
| [Challenge manager](docs/challenge_manager.md) | Sync and diff logic |
| [Validator](docs/validator.md) | Validation rules |
| [ctfd_api](docs/ctfd_api.md) | CLI client and monitor API |
| [CTFd dependency audit](docs/ctfd-dependency-audit.md) | Schema risks and compatibility |
| [Dev notes](docs/dev-notes.md) | Build, gotchas, implementation notes |
| [Architecture](ARCHITECTURE.md) | System reference |
| [Benchmark results](docs/benchmark-results.md) | Load-test data |

## License

MIT. See [LICENSE](LICENSE).
