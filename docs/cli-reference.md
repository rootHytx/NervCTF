# CLI reference

## Configuration

`.nervctf.yml` (or `.nervctf.yaml`) is searched upward from the working directory. `nervctf setup` creates it with mode `0600`.

```yaml
# ── Monitor ───────────────────────────────────────────────────────────────────
monitor_ip: 10.0.0.10
monitor_port: 33133
monitor_token: <64-char hex>          # remote-monitor token, not a CTFd token

# ── Deployment (setup / setup --upgrade) ──────────────────────────────────────
monitor_user: root                    # SSH user on the CTFd host
monitor_ctfd_path: /home/root/CTFd    # default: /home/<monitor_user>/CTFd
ssh_key_path: ~/.ssh/id_rsa           # private key; its .pub is authorised remotely

# ── Local challenges ──────────────────────────────────────────────────────────
challenges_path: ./challenges

# ── Monitor tuning (applied at setup time) ────────────────────────────────────
max_concurrent_provisions: 4
max_instances_per_team: 3             # 0 = unlimited
ctfd_domain: ctfd.example.com         # dashboard links; default http://<monitor_ip>

# ── Split-machine mode (optional) ─────────────────────────────────────────────
runner_ip: 192.168.1.50
runner_user: docker
runner_domain: challenges.example.com  # player-facing name; default runner_ip

# ── Event settings (optional) ─────────────────────────────────────────────────
event:
  name: "My CTF"
  description: "Annual capture-the-flag event."
  start: "2026-10-01T09:00:00Z"
  end: "2026-10-02T09:00:00Z"
  user_mode: teams                    # teams | users
  challenge_visibility: public        # public | private
  registration_visibility: public     # public | private
  score_visibility: public            # public | private | hidden
  account_visibility: public          # public | private
  verify_emails: false

# ── First CTFd admin (prompted by setup) ──────────────────────────────────────
admin:
  user: admin
  email: admin@example.com
  password: change-me
```

Precedence: CLI flags > environment variables > `.nervctf.yml`.

| CLI flag | Environment variable | Description |
|---|---|---|
| `--monitor-url` | `MONITOR_URL` | Full monitor URL; overrides `monitor_ip` and `monitor_port` |
| `--monitor-token` | `MONITOR_TOKEN` | Monitor token |

`event:` and `admin:` are optional. `nervctf setup` prompts for the admin account and saves it. The plugin uses it only while CTFd's `setup` setting is absent or false, so it does not overwrite an account changed later in CTFd.

## Global flags

| Flag | Default | Description |
|---|---|---|
| `-c, --challenges-dir <path>` | `.` | Root directory for `challenge.yml` files |
| `-v, --verbose` | off | Verbose output |
| `--monitor-url <url>` | config/env | Override the monitor URL for this run |
| `--monitor-token <token>` | config/env | Override the monitor token for this run |

## `setup`

```sh
nervctf setup
nervctf setup --upgrade
```

`setup` checks the controller for `ansible-playbook`, `ssh`, and `rsync`, then prompts for the connection details and the first CTFd admin. It saves `.nervctf.yml`, runs a preflight against each host, and runs the provisioning playbook. The playbook installs Docker, clones CTFd, deploys the plugin, builds the monitor image, writes `docker-compose.override.yml`, and starts the stack. On first start the plugin creates the admin and writes `ctf_name`, `ctf_description`, `user_mode`, and `setup = "true"`.

`--upgrade` pushes the current plugin and monitor binary, rebuilds the monitor image, and recreates the whole Compose stack. Recreating applies the override environment and makes nginx re-resolve the `ctfd` container. It does not re-provision CTFd or regenerate the token. If an older config has no `admin:` block, it prompts for one.

| Flag | Description |
|---|---|
| `--upgrade` | Update an existing deployment instead of provisioning a fresh one |

The preflight checks passwordless sudo (`sudo -n true`), `python3`, `rsync`, and `git` on the relevant hosts. Failures stop before provisioning.

## `deploy`

```sh
nervctf deploy [--dry-run] [--recreate] [--prune]
```

Validates the local challenges, diffs them against CTFd, and applies the changes in four phases:

| Phase | What happens |
|---|---|
| 1 — cores | Create or update challenges; upload flags, tags, topics, and hints; note changed files |
| 2 — files | Upload attachment files |
| 3 — requirements | Resolve prerequisite names to CTFd IDs and patch `requirements` |
| 4 — next pointers | Resolve `next` names to IDs and patch `next_id` |

Phases 3 and 4 run after phase 1 so forward references resolve.

Before phase 1, deploy reads the cached compatibility probe and aborts if dynamic scoring is `broken`. It warns if CTFd is in user mode, if capabilities are degraded, or if the CTFd version differs from 3.7.3.

A challenge counts as changed when any of these differ: `category`, `description`, `state`, `connection_info`, `attempts`, `extra`, flags `(content, type, data)`, hints `(content, cost)`, tags, topics, files, or `instance`.

A type change (for example `standard` to `dynamic`) deletes and recreates the challenge, because CTFd's PATCH does not create the matching join-table row.

| Flag | Description |
|---|---|
| `-d, --dry-run` | Show the diff without applying it |
| `--recreate` | Put every challenge in the update queue, even if up to date; re-syncs files and rebuilds images |
| `--prune` | Delete remote challenges that no longer exist locally (after phase 4) |

## `validate`

```sh
nervctf validate [--debug]
```

Checks challenge YAML without contacting the server. Exits 1 on errors. Duplicate names, missing required fields, broken file references, and self-referencing requirements or `next` pointers are errors. Unknown keys, missing descriptions, and unresolved prerequisite names are warnings. The full rule table is in [validator.md](validator.md).

| Flag | Description |
|---|---|
| `--debug` | Print the parsed field dictionary for each challenge |

## `probe`

```sh
nervctf probe [--refresh] [--json]
```

Asks the monitor for its CTFd compatibility report. The monitor caches the result at startup; `--refresh` re-runs it against MariaDB.

| Capability | Covered |
|---|---|
| `cap_challenge_crud` | Challenge INSERT/UPDATE/DELETE |
| `cap_dynamic_scoring` | Scoring columns and dynamic table state |
| `cap_player_auth` | Team-mode detection |
| `cap_instance_flags` | Plugin table presence |
| `cap_redis_sync` | Redis invalidation after direct writes (always degraded) |

Exit code 0 when every capability is `ok` or `degraded`; 1 when any critical capability is `broken`.

| Flag | Description |
|---|---|
| `--refresh` | Force a fresh probe instead of the cached result |
| `--json` | Print raw JSON |

## `event`

```sh
nervctf event [--show]
```

Maps the `event:` block to CTFd settings and writes them through the monitor. Managed keys: `ctf_name`, `ctf_description`, `start`, `end`, `user_mode`, `challenge_visibility`, `registration_visibility`, `score_visibility`, `account_visibility`, `verify_emails`. Unknown keys are rejected.

The command does not create an admin account. `setup` completes CTFd's first-run wizard when `admin:` is configured. CTFd caches some settings; run `setup --upgrade` if a value does not appear.

| Flag | Description |
|---|---|
| `--show` | Print the mapped settings without applying them |

## `list`, `scan`, `fix`

```sh
nervctf list [--detailed]
nervctf scan [--detailed]
nervctf fix [--dry-run]
```

`list` prints the challenges found locally. `scan` prints counts by type and category plus point totals. `fix` patches common missing fields (`state`, `version`, `description`, and similar).

| Command | Flag | Description |
|---|---|---|
| `list` | `-d, --detailed` | Print full field values |
| `scan` | `-d, --detailed` | Print a per-challenge breakdown |
| `fix` | `-d, --dry-run` | Show patches without writing files |

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| No challenges found | Files not named `challenge.yml`, or deeper than 5 levels | Check the path and depth |
| `state: Field may not be null` | CTFd needs `state` | `nervctf fix` |
| File upload 500 | Uploads directory ownership | `chown -R 1001:1001 <ctfd_path>/.data/CTFd/uploads` |
| Monitor 401 | Token mismatch | Match `monitor_token` with the server's `MONITOR_TOKEN` |
| `ansible-playbook: not found` | Ansible missing | Install Ansible or set `ANSIBLE_PLAYBOOK` |
| `Preflight failed ... passwordless-sudo=FAILED` | No passwordless sudo | Add `<user> ALL=(ALL) NOPASSWD:ALL` in `/etc/sudoers.d/` |
| `Preflight failed ... rsync=FAILED` / `git=FAILED` | Target lacks a tool | Install `rsync` / `git` on the target |
| `nervctf_instance_challenge doesn't exist` | Plugin not loaded | Run `setup --upgrade` and check the CTFd logs |
| Players get 403 on instance request | CTFd is in user mode | Switch CTFd to team mode |
| Dynamic challenges return 500 | Partial CTFd migration | Run `probe --refresh` |
| Instance shows the wrong host | `runner_domain` or `PUBLIC_HOST` wrong | Set `runner_domain` and run `setup --upgrade` |
