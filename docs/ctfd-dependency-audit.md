# CTFd dependency audit

This audit covers every NervCTF operation that reaches CTFd.

---

## Architecture summary

NervCTF touches CTFd through three layers:

| Layer | Who | How | Bypasses CTFd logic? |
|---|---|---|---|
| Direct MariaDB SQL | `remote-monitor` | mysql_async | Yes — all challenge, flag, hint, tag, and topic CRUD goes straight to the database |
| Out-of-band filesystem | `remote-monitor` and Ansible | `std::fs::write`, rsync | Yes — uploads are written directly and plugin files are rsynced |
| CTFd Python plugin | `nervctf_instance` | In-process Flask routes, SQLAlchemy ORM | No — it runs inside CTFd |

The CLI never calls CTFd's HTTP API directly. Every CLI operation goes to the monitor's REST facade (`/api/v1/...`), which translates it into MariaDB SQL.

---

## 1. MariaDB table and operation matrix

### Tables always accessed

| Table | Operations | Columns | Notes |
|---|---|---|---|
| `challenges` | SELECT, INSERT, UPDATE, DELETE | `id, name, description, category, value, type, state, max_attempts, connection_info, requirements, next_id` | Main CRUD table |
| `flags` | SELECT, INSERT, DELETE | `id, challenge_id, type, content, data` | `type` is hardcoded `'static'` on insert |
| `hints` | SELECT, INSERT, DELETE | `id, challenge_id, content, cost, type` | `type` is hardcoded `'standard'` on insert |
| `tags` | SELECT, INSERT, DELETE | `id, challenge_id, value` | Full-replace on update |
| `files` | SELECT, INSERT, DELETE | `id, challenge_id, type, location` | `type` is hardcoded `'challenge'`; the file is written to disk separately |
| `topics` | SELECT, INSERT IGNORE | `id, value` | Canonical topic rows |
| `challenge_topics` | SELECT, INSERT IGNORE, DELETE | `challenge_id, topic_id` | Junction table |
| `users` | SELECT, INSERT (first admin only) | `token, banned, hidden, team_id, id, name, type` | Token auth, name cache, and the plugin's first-run admin creation |
| `teams` | SELECT | `id, name` | Team name cache |
| `submissions` | SELECT (read-only) | `team_id, user_id, challenge_id, date, type` | Solve sync; only `type='correct'` rows are read |
| `configs` / `config` | SELECT, INSERT, UPDATE | `key, value, type` | Event settings from `nervctf event`; `user_mode` and `ctf_version` are read by the probe. The monitor auto-detects which table name the build uses. |

### Tables accessed conditionally (schema-detected at runtime)

| Table or column | Detection | Used columns | Condition |
|---|---|---|---|
| `challenges.attribution` | `information_schema.COLUMNS` | `attribution` | UPDATE SET and SELECT; added in CTFd 3.7.0 |
| `challenges.logic` | `information_schema.COLUMNS` | `logic` | INSERT and UPDATE SET; added in CTFd 3.7.x |
| `challenges.position` | `information_schema.COLUMNS` | `position` | SELECT only; the value is discarded |
| `challenges.initial/minimum/decay/function` | `information_schema.COLUMNS` (all four) | inline scoring | Newer CTFd moved scoring inline |
| `dynamic_challenge` | `information_schema.COLUMNS` | `id` plus optional scoring columns | Older CTFd join table; stub-or-full insert |
| `nervctf_instance_challenge` | `information_schema.tables` | 19 columns | Plugin table; LEFT JOIN when absent |

Two schema probes run on every mutating call:

```sql
SELECT COLUMN_NAME FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'challenges';

SELECT COLUMN_NAME FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'dynamic_challenge';
```

---

## 2. Hardcoded polymorphic strings

| String | Column | Table | Purpose |
|---|---|---|---|
| `'static'` | `type` | `flags` | Every flag the monitor inserts is static |
| `'standard'` | `type` | `hints` | Every hint the monitor inserts is standard |
| `'challenge'` | `type` | `files` | Every file record the monitor inserts |
| `'correct'` | `type` | `submissions` | Solve-sync filter |
| `'standard'` | `type` | `challenges` | Default challenge type and fallback |
| `'dynamic'` | `type` | `challenges` | Gates the `dynamic_challenge` insert |
| `'instance'` | `type` | `challenges` | Gates the `nervctf_instance_challenge` upsert |
| `'linear'` | `function` | `dynamic_challenge` / `challenges` | Default decay function |

---

## 3. HTTP endpoints (CLI to monitor facade)

The monitor's facade mirrors CTFd's API path structure:

| Endpoint | Method | Purpose |
|---|---|---|
| `/api/v1/challenges` | GET (paginated `?page=N`), POST | List or create challenges |
| `/api/v1/challenges/{id}` | PATCH, DELETE | Update or delete |
| `/api/v1/flags` | GET (`?challenge_id=N`), POST | List or create flags |
| `/api/v1/flags/{id}` | DELETE | Delete a flag |
| `/api/v1/hints` | GET, POST | List or create hints |
| `/api/v1/hints/{id}` | DELETE | Delete a hint |
| `/api/v1/tags` | GET, POST | List or create tags |
| `/api/v1/tags/{id}` | DELETE | Delete a tag |
| `/api/v1/files` | GET (`?challenge_id=N`), POST (multipart) | List or upload files |
| `/api/v1/files/{id}` | DELETE | Delete a file |
| `/api/v1/topics` | GET, POST | List or create topics |
| `/api/v1/topics/{id}` | DELETE | Delete a topic |
| `/api/v1/instance/register` | POST | Register an instance config to SQLite |
| `/api/v1/instance/request` | POST | Provision a container |
| `/api/v1/instance/info` | GET | Get running instance info |
| `/api/v1/instance/renew` | POST | Extend the instance TTL |
| `/api/v1/instance/stop` | POST | Stop a container |
| `/api/v1/plugin/*` | POST | Plugin-facing routes; same as `instance/*` but with the plugin token and a CTFd-validated `team_id` |

**Gap:** sub-resources (flags, hints, tags, files, topics) are not paginated. Their list calls use `?challenge_id=N` with no paging, which works only because CTFd's default page size is large enough for typical challenge data.

**Response parsing:** only the `data` key is extracted. The `errors` key is never parsed and the `success` boolean is never checked.

---

## 4. Authentication flows

### CLI to monitor

```
Authorization: Token <MONITOR_TOKEN (plaintext)>
→ SHA-256 hashed and compared against operator_tokens.token_hash (SQLite)
```

The header format is `Token xxx`, not `Bearer xxx`; the monitor rejects the Bearer scheme.

### CTFd plugin to monitor

```
Authorization: Token <NERVCTF_MONITOR_TOKEN (env var)>
Body: {challenge_name, team_id, user_id}
```

`team_id` and `user_id` come from CTFd's `get_current_team()` and `get_current_user()` and are trusted without re-validation. Only the monitor token is checked.

### Player browser to monitor (`/instance/:name`)

```
Authorization: Token <CTFd user API token (plaintext)>
→ SELECT team_id FROM users WHERE token = ? AND banned = 0 AND hidden = 0
```

CTFd's `users.token` column holds plaintext API tokens and the comparison is plaintext-to-plaintext with no expiry. This is the `users` table's `token` column, not CTFd's `tokens` application-token table.

---

## 5. Plugin internal API surface

| Import or call | Stability risk |
|---|---|
| `from CTFd.models import ChallengeFiles, Challenges, Fails, Flags, Hints, Solves, Tags, db` | Low — stable since CTFd 3.x |
| `from CTFd.plugins.challenges import CHALLENGE_CLASSES, BaseChallenge` | Medium-high — `BaseChallenge` method signatures changed between minor versions |
| `from CTFd.utils.user import get_current_team, get_current_user` | Low |
| `from CTFd.utils.decorators import authed_only` | Low |
| `BaseChallenge.attempt(challenge, request)` return type | High — `(bool, str)` in CTFd ≤3.3, `ChallengeResponse` object in ≥3.4. The dual-path code catches `TypeError`, but `ChallengeResponse` is subscriptable, so the `getattr(result, "success", False)` fallback may never fire and a correct submission can be misreported |
| `BaseChallenge.solve(user, team, challenge, request)` | Medium — 4-arg form assumed |
| `BaseChallenge.fail(user, team, challenge, request)` | Medium |
| `db.create_all()` in `load()` | Low-medium — additive only; column additions to `nervctf_instance_challenge` are skipped on upgrade |

---

## 6. Challenge lifecycle

### Deploy phases (in order)

| Phase | Action | Why this order |
|---|---|---|
| 1 — cores | POST or PATCH the challenge row, then flags, tags, topics, hints | Challenges must exist before files or cross-references |
| 2 — file upload | POST multipart to `/files` | Needs the challenge ID; separated from phase 1 for error isolation |
| 3 — requirements | Re-fetch all IDs, PATCH `requirements.prerequisites` (array of integer IDs) | Prerequisite IDs are known only after phase 1 completes |
| 4 — next pointers | PATCH `next_id` | Same reason as phase 3 |
| 5 — prune (optional, `--prune`) | DELETE orphaned remote challenges | Done last, so it cannot break the requirements of live challenges |

### `needs_update` comparison fields

Compared: `category`, `value` (skipped for dynamic), `description`, `state`, `connection_info`, `attempts`, `extra` (JSON), `flags` (sorted `(content, type, data)` tuples), `tags` (sorted strings), `hints` (sorted `(content, cost)` tuples), `requirements` (sorted names).

Not compared, so they never trigger an update on their own: `name` (identity), `files`, `topics`, `next`, `author`, `image`, `protocol`, `host`, `healthcheck`, `version`.

**Gap:** a challenge where only `files` or `topics` changed is not detected as needing an update unless `--recreate` is used.

### Sub-resource sync strategy

| Sub-resource | Strategy | Comparison key |
|---|---|---|
| Flags | Full replace (delete all, insert all) | `content` in `replace_flags`; `(content, type, data)` in `needs_update`. Inconsistent: a flag type change passes `needs_update` but is dropped by `replace_flags` |
| Tags | Full replace | `value` string |
| Hints | Full replace | `content` only. A cost change passes `needs_update` but is dropped by `replace_hints` |
| Topics | True diff (delete extras, add missing) | `value` string |
| Files | Delete all, re-upload in phase 2 | Filename (last URL segment) |

### Type changes

A type change (for example `standard` to `dynamic`) triggers delete and recreate rather than PATCH. CTFd's PATCH endpoint updates `challenges.type` but does not create the required joined-table row (`dynamic_challenge` or `nervctf_instance_challenge`), which causes a 500 on the next access. The delete and recreate loses the challenge's numeric ID, breaking any hardcoded numeric prerequisite references.

---

## 7. Instance challenge lifecycle

### State machine

```
NONEXISTENT
    │
    ▼ POST /api/v1/plugin/request or /instance/request
PROVISIONING ← insert_provisioning_stub() (status='provisioning', port=0)
    │
    ▼ Background Tokio task (under the semaphore):
    │  1. Read config from SQLite instance_configs
    │  2. Allocate a port from get_used_ports()
    │  3. docker/compose/lxc up
    │  4. [if flag_mode=random] ctfd_db::create_flag() → INSERT INTO flags
    │  5. db::insert_instance() → UPDATE instances: status='running'
    │
RUNNING
    │
    ├─[renew]─────────────────────────── extends expires_at (no CTFd DB writes)
    │
    ├─[explicit stop / expiry]──────────┐
    │                                   ▼
    │                             cleanup_container()
    │                             ctfd_db::delete_flag() → DELETE FROM flags
    │                             db::delete_instance()
    │
    ├─[plugin solve]────────────────────┐
    │                                   ▼
    │                             mark_instance_solved() → status='solved'
    │                             ctfd_db::delete_flag() → DELETE FROM flags
    │                             cleanup_container() (background)
    │
SOLVED
    │
    ├─[solve deleted in CTFd]─────────── revert_unsolved_instances() → RUNNING
    │
[PROVISIONING stuck >30 min] → expiry task deletes it (same as stop)
[Orphan compose project] → compose::down()
[Container killed externally] → db::delete_instance() + delete_flag()
```

### CTFd tables touched during gameplay

| Table | Operation | Trigger |
|---|---|---|
| `flags` | INSERT `(challenge_id, 'static', content, '')` | Provision with `flag_mode=random` |
| `flags` | DELETE `WHERE id = ?` | Stop, expiry, solve, orphan, or health |
| `submissions` | SELECT (read-only) | Background sync every N seconds |
| `challenges` | SELECT (joined in sync) | Background sync |
| `users` | SELECT `team_id WHERE token = ?` | Every player request (auth) |
| `users` | SELECT `id, name, team_id` | Background sync (name cache) |
| `teams` | SELECT `id, name` | Background sync (name cache) |

### Background tasks

| Task | Interval | CTFd tables | Purpose |
|---|---|---|---|
| `sync_solves` | `CTFD_DB_SYNC_INTERVAL` (default 30 s) | `submissions` (R), `challenges` (R) | Full-replace the `ctfd_solves` cache; triggers revert and stale-attempt cleanup |
| `sync_users_and_teams` | Same | `teams` (R), `users` (R) | Full-replace the team and user name caches |
| Expiry, orphan, health | 30 s | `flags` (W: DELETE) | Expire instances, kill orphan compose projects, clean dead containers |

---

## 8. Out-of-band filesystem writes

| Write | Mechanism | Path | Trigger |
|---|---|---|---|
| Plugin files | Ansible `synchronize` (rsync, `delete: yes`) | `<ctfd_path>/CTFd/plugins/nervctf_instance/` | `nervctf setup` / `setup --upgrade` |
| Docker Compose override | Ansible `copy` | `<ctfd_path>/docker-compose.override.yml` | `nervctf setup` |
| Remote-monitor binary | Ansible `copy` | `<ctfd_path>/remote-monitor/remote-monitor` | `nervctf setup` / `setup --upgrade` |
| Challenge files | Monitor `std::fs::write` | `<CTFD_UPLOADS_DIR>/<random-32-hex>/<filename>` | `POST /api/v1/files` |
| Per-instance compose override | Monitor SSH `cat >` or `std::fs::write` | `<challenges_dir>/<challenge>/<project>.override.yml` | Instance provision |
| Per-instance flag file | Monitor SSH `cat >` or `std::fs::write` | `<challenges_dir>/<challenge>/<project>.flag` | Instance provision (file delivery) |

**Upload bypass.** With `CTFD_UPLOADS_DIR` set, the monitor writes challenge files to disk and inserts the DB record by hand. CTFd's nginx serves them from that directory. No cache invalidation happens after a write, and a failed DB insert after a successful disk write leaves an orphaned file.

**No Redis interaction.** NervCTF never touches CTFd's Redis cache, so direct MariaDB writes can serve stale data until the TTL expires or CTFd restarts.

---

## 9. Configuration surface

### `.nervctf.yml` fields and how they flow

| Field | Used by | Wired to |
|---|---|---|
| `monitor_ip` | CLI | Base URL for API calls |
| `monitor_port` | CLI | Base URL |
| `monitor_token` | CLI | `Authorization: Token` header |
| `ssh_key_path` | CLI | `-i <key>` in SSH mkdir and rsync commands to the runner |
| `runner_ip` / `runner_user` | CLI | `RunnerTarget.ssh_target` for SSH and rsync |
| `monitor_ctfd_path` | Ansible | Plugin install path, `.env` location, compose override |
| `ctfd_domain` / `runner_domain` | Monitor | Dashboard links and player-facing host |

### Environment variables read by the monitor at startup

| Variable | Required | Default |
|---|---|---|
| `CTFD_DB_URL` | Yes | none |
| `MONITOR_TOKEN` | Optional | none |
| `PUBLIC_HOST` | Yes | none |
| `MONITOR_PORT` | Optional | `33133` |
| `DB_PATH` | Optional | `./monitor.db` |
| `CHALLENGES_BASE_DIR` | Optional | `/opt/nervctf/challenges` |
| `CTFD_UPLOADS_DIR` | Optional | `""` (disabled) |
| `RUNNER_SSH_TARGET` | Optional (split mode) | none |
| `MAX_CONCURRENT_PROVISIONS` | Optional | `4` |
| `MAX_INSTANCES_PER_TEAM` | Optional | `0` (unlimited) |
| `CTFD_DB_SYNC_INTERVAL` | Optional | `30` |

**MariaDB credential sharing.** The playbook reads CTFd's `.env`, strips the `+pymysql` qualifier from `DATABASE_URL`, and injects it as `CTFD_DB_URL`. The monitor uses the same database user and password as CTFd, with full read and write access.

---

## 10. Version compatibility matrix

| Feature | Detected? | Risk | Notes |
|---|---|---|---|
| CTFd version pin | Static git tag only | High | `3.7.3` pinned at first setup only; the upgrade playbook has no version guard |
| `challenges.attribution` | Yes, info_schema | Low | Added 3.7.0; absent gracefully |
| `challenges.logic` | Yes, info_schema | Low | Default insert is `""`, not NULL |
| `challenges.position` | Yes, info_schema | Low | Read but discarded |
| `challenges.initial/minimum/decay/function` (inline) | Yes, all four columns | Medium | Requires all four; a partial migration is detected and falls back |
| `challenges.next_id` | Yes, `has_next_id` | Medium | Absent on CTFd <3.5.x; the NULL placeholder keeps indices stable |
| `dynamic_challenge` table | Yes | Medium | Correct for MariaDB |
| `dynamic_challenge.function` column name | No | Medium | Hardcoded `function`; a rename causes a runtime SQL error |
| `nervctf_instance_challenge` table | Yes, info_schema | Low | LEFT JOIN fallback |
| `configs` / `config` settings table | Yes, info_schema | Low | `config_table()` picks whichever exists; the plugin uses the ORM |
| Password hashing (`Users.password`) | Probed at first admin creation | Medium | Some builds use passlib `bcrypt_sha256` and hash on assignment; the plugin detects both behaviors |
| `BaseChallenge.attempt()` return type | Partial (`TypeError` catch) | High | The dual path has a gap for a subscriptable `ChallengeResponse` |
| `BaseChallenge.solve/fail` signatures | No | Medium | 4-arg form assumed |
| Team versus user mode | Yes, at startup | Medium | User-mode warns at startup and returns 403 on player routes |
| CTFd uploads path `.data/CTFd/uploads` | No | Medium | Hardcoded Docker volume convention |
| Plugin path `CTFd/plugins/nervctf_instance/` | No | Medium | Assumes CTFd's source layout |
| Redis cache invalidation | N/A | Medium | Never invalidated after direct DB writes |

---

## 11. Risks, ranked

### 1. Critical — user-mode CTFd denies all instance access

`validate_token()` treats `users.team_id IS NULL` as unauthorised and returns `None`, so every player instance route returns 403 "Not in a team". The operator gets no warning. **Affects:** `ctfd_db.rs:59`, `__init__.py:63-64`, all four player routes.
**Fixed in v2.3.1:** `detect_ctfd_mode()` queries the settings table (`configs` or `config`) for `user_mode` right after pool creation and logs a `WARN` when user-mode is detected.

### 2. High — `BaseChallenge.attempt()` dual-path gap

The plugin catches `TypeError` (non-subscriptable) and falls back to `getattr(result, "success", False)`. When CTFd returns a `ChallengeResponse` namedtuple, which is subscriptable, the fallback never fires. A correct submission can be reported to the monitor as incorrect, which breaks flag-sharing detection and solve-triggered teardown. **Affects:** `__init__.py:267-272`.

### 3. High — inline scoring was detected from one column (fixed)

`detect_challenges_schema()` inferred inline scoring from `challenges.initial` alone. A partial migration then produced SELECTs against columns that did not exist. **Affects:** `ctfd_db.rs:153, 168-176`.
**Fixed in v2.3.1:** `dynamic_in_challenges` now requires all four columns, and `dynamic_partial` marks the partial state so the code falls back to the join table.

### 4. High — CTFd version pinned only at first setup

The `version: "3.7.3"` git tag applies only when CTFd is not cloned yet, and the upgrade playbook has no version guard. A manual `git pull` on the server drifts every schema assumption. **Affects:** `nervctf_playbook.yml`.
**Partly addressed in v2.4.0:** `nervctf probe` reports the live version from the settings table (`configs` or `config`), and `nervctf deploy` compares it against `TESTED_CTFD_VERSION` (`3.7.3`), warning on a mismatch. Deploy does not block on version drift alone; the probe's capability fields are the compatibility signal.

### 5. Medium — `challenges.next_id` assumed present (fixed)

Statements included `next_id` with no detection, so CTFd <3.5.x failed every operation with "Unknown column". **Fixed in v2.3.1:** `has_next_id` gates a NULL placeholder in SELECT and omits the column in INSERT and UPDATE.

### 6. Medium — CTFd flag orphaned if the monitor crashes mid-provision

`ctfd_db::create_flag()` runs before `db::insert_instance()`. A crash between them leaves a MariaDB flag with no `ctfd_flag_id` in SQLite, so it is never deleted. The provision-ordering change in `instance-challenges.md` narrows this window.

### 7. Medium — no Redis cache invalidation

All challenge, flag, hint, tag, file, and topic writes go straight to MariaDB. CTFd's Redis cache is never invalidated, so stale data can reach players until the TTL expires or CTFd restarts.

### 8. Low — `needs_update` misses `files` and `topics` changes

A challenge where only files or topics changed is never detected. Run `--recreate` to force a re-sync.

### 9. Low — `replace_flags` and `replace_hints` compare only `content`

`needs_update` triggers on a `(content, type, data)` change, but the replacement function compares content strings only, so a flag type change is silently dropped and the type stays unchanged on CTFd.

---

## 12. Tested CTFd version

**CTFd 3.7.3** is the only version officially tested and deployed. The schema detection guards give partial forward and backward compatibility, but none of it is validated at startup. Other versions are untested.
