# NervCTF — CTFd Dependency Audit

> Covers all NervCTF operations against CTFd.

---

## Architecture Summary

NervCTF touches CTFd through three distinct layers:

| Layer | Who | How | Bypasses CTFd logic? |
|---|---|---|---|
| Direct MariaDB SQL | `remote-monitor` | mysql_async | **Yes** — all challenge/flag/hint/tag/topic CRUD goes directly to the DB |
| Out-of-band filesystem | `remote-monitor` + Ansible | `std::fs::write`, rsync | **Yes** — uploads written directly; plugin files rsynced |
| CTFd Python plugin | `nervctf_instance` plugin | In-process Flask routes, SQLAlchemy ORM | No — runs inside CTFd |

The CLI (`nervctf`) never calls CTFd's HTTP API directly. All CLI operations go to the remote-monitor's REST facade (`/api/v1/...`), which translates them to direct MariaDB SQL.

---

## 1. Complete MariaDB Table / Operation Matrix

### Tables Always Accessed (no detection guard)

| Table | Operations | Key Columns | Notes |
|---|---|---|---|
| `challenges` | SELECT, INSERT, UPDATE, DELETE | `id, name, description, category, value, type, state, max_attempts, connection_info, requirements, next_id` | Primary CRUD table |
| `flags` | SELECT, INSERT, DELETE | `id, challenge_id, type, content, data` | `type` hardcoded `'static'` on insert |
| `hints` | SELECT, INSERT, DELETE | `id, challenge_id, content, cost, type` | `type` hardcoded `'standard'` on insert |
| `tags` | SELECT, INSERT, DELETE | `id, challenge_id, value` | Full-replace strategy on update |
| `files` | SELECT, INSERT, DELETE | `id, challenge_id, type, location` | `type` hardcoded `'challenge'`; file written to disk separately |
| `topics` | SELECT, INSERT IGNORE | `id, value` | Canonical topic row |
| `challenge_topics` | SELECT, INSERT IGNORE, DELETE | `challenge_id, topic_id` | Junction table |
| `users` | SELECT | `token, banned, hidden, team_id, id, name` | Token auth + user name cache |
| `teams` | SELECT | `id, name` | Team name cache |
| `submissions` | SELECT (read-only) | `team_id, user_id, challenge_id, date, type` | Solve sync; only `type='correct'` rows read |

### Tables Conditionally Accessed (schema-detected at runtime)

| Table | Detection query | Columns used | Condition |
|---|---|---|---|
| `challenges.attribution` | `information_schema.COLUMNS` | `attribution` | UPDATE SET, SELECT — added CTFd 3.7.0 |
| `challenges.logic` | `information_schema.COLUMNS` | `logic` | INSERT, UPDATE SET — added CTFd 3.7.x |
| `challenges.position` | `information_schema.COLUMNS` | `position` | SELECT only (value discarded in code) |
| `challenges.initial/minimum/decay/function` | `information_schema.COLUMNS` (presence of `initial`) | All four inline | Newer CTFd moved scoring inline |
| `dynamic_challenge` | `information_schema.COLUMNS` (table empty = absent) | `id` ± scoring cols | Older CTFd join table; stub-or-full insert |
| `nervctf_instance_challenge` | `information_schema.tables` | 19 columns | NervCTF plugin table; LEFT JOIN when absent |

### Two schema-detection probes run on every mutating call:

```sql
-- Probe 1: challenges columns
SELECT COLUMN_NAME FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'challenges'

-- Probe 2: dynamic_challenge columns
SELECT COLUMN_NAME FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'dynamic_challenge'
```

---

## 2. Hardcoded Discriminator / Polymorphic Strings

| String | Column | Table | Purpose |
|---|---|---|---|
| `'static'` | `type` | `flags` | All flags inserted by monitor are static |
| `'standard'` | `type` | `hints` | All hints inserted by monitor are standard |
| `'challenge'` | `type` | `files` | All file records inserted by monitor |
| `'correct'` | `type` | `submissions` | Solve sync filter |
| `'standard'` | `type` | `challenges` | Default challenge type / fallback |
| `'dynamic'` | `type` | `challenges` | Gates `dynamic_challenge` insert |
| `'instance'` | `type` | `challenges` | Gates `nervctf_instance_challenge` upsert |
| `'linear'` | `function` | `dynamic_challenge` / `challenges` | Default decay function |

---

## 3. HTTP API Endpoints (CLI → remote-monitor facade)

The CLI talks to the remote-monitor which translates to SQL. The remote-monitor's facade mirrors CTFd's API path structure:

| Endpoint | Method | Purpose |
|---|---|---|
| `/api/v1/challenges` | GET (paginated `?page=N`), POST | List / create challenges |
| `/api/v1/challenges/{id}` | PATCH, DELETE | Update / delete |
| `/api/v1/flags` | GET (`?challenge_id=N`), POST | List / create flags |
| `/api/v1/flags/{id}` | DELETE | Delete flag |
| `/api/v1/hints` | GET, POST | List / create hints |
| `/api/v1/hints/{id}` | DELETE | Delete hint |
| `/api/v1/tags` | GET, POST | List / create tags |
| `/api/v1/tags/{id}` | DELETE | Delete tag |
| `/api/v1/files` | GET (`?challenge_id=N`), POST (multipart) | List / upload files |
| `/api/v1/files/{id}` | DELETE | Delete file |
| `/api/v1/topics` | GET, POST | List / create topics |
| `/api/v1/topics/{id}` | DELETE | Delete topic |
| `/api/v1/instance/register` | POST | Register instance challenge config to SQLite |
| `/api/v1/instance/request` | POST | Provision a container |
| `/api/v1/instance/info` | GET | Get running instance info |
| `/api/v1/instance/renew` | POST | Extend instance TTL |
| `/api/v1/instance/stop` | POST | Stop container |
| `/api/v1/plugin/*` | POST | CTFd-plugin-facing routes (same as instance/* but uses plugin token + CTFd-validated team_id) |

**Key gap:** No pagination is implemented for sub-resources (flags, hints, tags, files, topics). All sub-resource list calls use `?challenge_id=N` without paging — this works only because CTFd's default page size is large enough for typical challenge data.

**Response parsing:** Only the `data` key is extracted from responses. The `errors` envelope key is never parsed. The `success` boolean field is never checked.

---

## 4. Authentication Flows

### 4a. CLI → remote-monitor
```
Authorization: Token <MONITOR_TOKEN (plaintext)>
→ SHA-256 hashed and compared against operator_tokens.token_hash (SQLite)
```
Header format: `Token xxx` (NOT `Bearer xxx` — monitor explicitly rejects Bearer scheme).

### 4b. CTFd plugin → remote-monitor
```
Authorization: Token <NERVCTF_MONITOR_TOKEN (env var)>
Body: {challenge_name, team_id, user_id}
```
The `team_id` and `user_id` in the body come from CTFd's `get_current_team()` / `get_current_user()` — they are trusted by the monitor without re-validation. Only the monitor token itself is checked.

### 4c. Player browser → remote-monitor (direct path, `/instance/:name`)
```
Authorization: Token <CTFd user API token (plaintext)>
→ SELECT team_id FROM users WHERE token = ? AND banned = 0 AND hidden = 0
```
The CTFd `users.token` column stores plaintext API tokens. Comparison is plaintext-to-plaintext. No expiry. Token belongs to the **users table's `token` column**, not CTFd's `tokens` application-token table.

---

## 5. Plugin Internal API Surface (CTFd Python)

| Import / Call | Stability Risk |
|---|---|
| `from CTFd.models import ChallengeFiles, Challenges, Fails, Flags, Hints, Solves, Tags, db` | Low — stable since CTFd 3.x |
| `from CTFd.plugins.challenges import CHALLENGE_CLASSES, BaseChallenge` | **Medium-High** — `BaseChallenge` method signatures have changed between minor versions |
| `from CTFd.utils.user import get_current_team, get_current_user` | Low |
| `from CTFd.utils.decorators import authed_only` | Low |
| `BaseChallenge.attempt(challenge, request)` return type | **HIGH** — returns `(bool, str)` tuple in CTFd ≤3.3, `ChallengeResponse` object in ≥3.4. Dual-path code catches `TypeError` but `ChallengeResponse` is subscriptable — the fallback `getattr(result, "success", False)` may never trigger, silently misreporting correct submissions |
| `BaseChallenge.solve(user, team, challenge, request)` | Medium — 4-arg form assumed |
| `BaseChallenge.fail(user, team, challenge, request)` | Medium |
| `db.create_all()` in `load()` | Low-Medium — additive only; column additions to `nervctf_instance_challenge` on upgrade skip automatically |

---

## 6. Challenge Lifecycle

### Deploy Phases (in order)

| Phase | Action | Why this order |
|---|---|---|
| 1 — cores | POST/PATCH challenge row + flags/tags/topics/hints | Challenges must exist before files or cross-references |
| 2 — file upload | POST multipart to `/files` | Challenge ID required; separated from phase 1 for error isolation |
| 3 — requirements | Re-fetch all IDs, PATCH `requirements.prerequisites` (array of integer IDs) | Prerequisite IDs only known after all phase 1 creates complete |
| 4 — next pointers | PATCH `next_id` | Same reason as phase 3 |
| 5 — prune (optional, `--prune`) | DELETE orphaned remote challenges | Done last to avoid breaking requirements of live challenges |

### `needs_update` Comparison Field List

Fields compared: `category`, `value` (skipped for dynamic), `description`, `state`, `connection_info`, `attempts`, `extra` (JSON value), `flags` (sorted `(content, type, data)` tuples), `tags` (sorted strings), `hints` (sorted `(content, cost)` tuples), `requirements` (sorted names).

Fields NOT compared (never trigger update): `name` (identity key), `files`, `topics`, `next`, `author`, `image`, `protocol`, `host`, `healthcheck`, `version`.

**Critical gap:** A challenge where only `files` or `topics` changed is never detected as needing an update unless `--recreate` is forced.

### Sub-Resource Sync Strategy

| Sub-resource | Strategy | Comparison key |
|---|---|---|
| Flags | Full replace (delete all → insert all) | `content` in `replace_flags`; `(content, type, data)` in `needs_update` — **inconsistency**: flag type changes pass `needs_update` but are silently dropped by `replace_flags` |
| Tags | Full replace | `value` string |
| Hints | Full replace | `content` string only — cost changes pass `needs_update` but are dropped by `replace_hints` |
| Topics | True diff (delete extras, add missing) | `value` string |
| Files | Delete all, re-upload in phase 2 | filename (last URL segment) |

### Type Change Handling

Type changes (e.g., `standard` → `dynamic`) trigger **delete + recreate** rather than PATCH. Reason: CTFd's PATCH endpoint updates `challenges.type` but does not create the required joined-table row (`dynamic_challenge`/`nervctf_instance_challenge`), causing CTFd 500 on next access. The delete+recreate loses the challenge's numeric ID, breaking any hard-coded numeric prerequisite references.

---

## 7. Instance Challenge Lifecycle

### State Machine

```
NONEXISTENT
    │
    ▼ POST /api/v1/plugin/request or /instance/request
PROVISIONING ← insert_provisioning_stub() (status='provisioning', port=0)
    │
    ▼ Background Tokio task (under semaphore):
    │  1. Read config from SQLite instance_configs
    │  2. Allocate port from get_used_ports()
    │  3. docker/compose/lxc up
    │  4. [if flag_mode=random] ctfd_db::create_flag() → INSERT INTO flags
    │  5. db::insert_instance() → UPDATE instances: status='running'
    │
RUNNING
    │
    ├─[renew]─────────────────────────── extends expires_at (no CTFd DB ops)
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
    ├─[solve deleted in CTFd]─────────── revert_unsolved_instances() → back to RUNNING
    │
[PROVISIONING stuck >30min] → expiry task deletes it (same as stop)
[Orphan compose project] → compose::down()
[Container externally killed] → db::delete_instance() + delete_flag()
```

### CTFd Tables Touched During Game Play (not CRUD)

| Table | Operation | Trigger |
|---|---|---|
| `flags` | INSERT `(challenge_id, 'static', content, '')` | Provision with `flag_mode=random` |
| `flags` | DELETE `WHERE id = ?` | Stop / expiry / solve / orphan / health |
| `submissions` | SELECT (read-only, sync) | Background sync every N seconds |
| `challenges` | SELECT (via join in sync) | Background sync |
| `users` | SELECT `team_id WHERE token = ?` | Every player request (auth) |
| `users` | SELECT `id, name, team_id` | Background sync (name cache) |
| `teams` | SELECT `id, name` | Background sync (name cache) |

### Background Tasks

| Task | Interval | CTFd tables | Purpose |
|---|---|---|---|
| `sync_solves` | `CTFD_DB_SYNC_INTERVAL` (default 30s) | `submissions` (R), `challenges` (R) | Full-replace ctfd_solves SQLite cache; triggers revert + stale-attempt cleanup |
| `sync_users_and_teams` | Same | `teams` (R), `users` (R) | Full-replace team/user name caches |
| Expiry + orphan + health | 30s hardcoded | `flags` (W: DELETE) | Expire instances, kill orphan compose projects, clean dead containers |

---

## 8. Out-of-Band CTFd Filesystem Writes

| Write | Mechanism | Path | Trigger |
|---|---|---|---|
| Plugin files | Ansible `synchronize` (rsync, `delete: yes`) | `<ctfd_path>/CTFd/plugins/nervctf_instance/` | `nervctf setup` / `nervctf upgrade` |
| Docker Compose override | Ansible `copy` (generated YAML) | `<ctfd_path>/docker-compose.override.yml` | `nervctf setup` |
| Remote-monitor binary | Ansible `copy` | `<ctfd_path>/remote-monitor/remote-monitor` | `nervctf setup` / `nervctf upgrade` |
| Challenge files (upload) | Monitor `std::fs::write` | `<CTFD_UPLOADS_DIR>/<random-32-hex>/<filename>` | `POST /api/v1/files` (CLI deploy) |
| Per-instance compose override | Monitor SSH `cat >` or `std::fs::write` | `<challenges_dir>/<challenge>/<project>.override.yml` | Instance provision |
| Per-instance flag file | Monitor SSH `cat >` or `std::fs::write` | `<challenges_dir>/<challenge>/<project>.flag` | Instance provision (file delivery mode) |

**CTFd upload bypass:** When `CTFD_UPLOADS_DIR` is set, the monitor writes challenge files directly to disk and inserts the DB record manually. CTFd's nginx serves files directly from this directory. No cache invalidation is performed after writes. If the DB insert fails after a successful disk write, the file is orphaned on disk.

**No Redis interaction:** NervCTF never touches CTFd's Redis cache. All direct MariaDB writes bypass cache invalidation, meaning stale data may be served until the Redis TTL expires or CTFd restarts.

---

## 9. Configuration Surface

### Key `.nervctf.yml` Fields and How They Flow

| Field | Used by | Wired to |
|---|---|---|
| `monitor_ip` | CLI | Base URL for all API calls |
| `monitor_port` | CLI | Base URL |
| `monitor_token` | CLI | `Authorization: Token` header |
| `ssh_key_path` | CLI | `-i <key>` in SSH mkdir + rsync commands to runner |
| `runner_ip` / `runner_user` | CLI | `RunnerTarget.ssh_target` for SSH/rsync |
| `ctfd_path` | Ansible | Plugin install path, `.env` location, compose override |
| `monitor_ctfd_path` | Ansible | Bind-mount for uploads dir |

### Env Vars Read by Remote-Monitor at Startup

| Env var | Required? | Default |
|---|---|---|
| `CTFD_DB_URL` | **Required** | none |
| `MONITOR_TOKEN` | Optional | none |
| `PUBLIC_HOST` | **Required** | none |
| `MONITOR_PORT` | Optional | `33133` |
| `DB_PATH` | Optional | `./monitor.db` |
| `CHALLENGES_BASE_DIR` | Optional | `/opt/nervctf/challenges` |
| `CTFD_UPLOADS_DIR` | Optional | `""` (disabled) |
| `RUNNER_SSH_TARGET` | Optional (split mode) | none |
| `MAX_CONCURRENT_PROVISIONS` | Optional | `4` |
| `MAX_INSTANCES_PER_TEAM` | Optional | `0` (unlimited) |
| `CTFD_DB_SYNC_INTERVAL` | Optional | `30` |

**MariaDB credential sharing:** The Ansible playbook reads CTFd's `.env` file, strips the `+pymysql` driver qualifier from `DATABASE_URL`, and injects it as `CTFD_DB_URL` for the monitor. The monitor uses the same database user/password as CTFd with full read/write access.

---

## 10. Version Compatibility Matrix

| Feature | Detection? | Risk level | Notes |
|---|---|---|---|
| CTFd version pin | Static git tag only | **High** | `3.7.3` pin only on first setup; upgrade playbook has no version guard |
| `challenges.attribution` | Yes — info_schema | Low | Added 3.7.0; gracefully absent |
| `challenges.logic` | Yes — info_schema | Low | Default insert is `""` not NULL |
| `challenges.position` | Yes — info_schema | Low | Read but discarded |
| `challenges.initial/minimum/decay/function` (inline) | Yes — single-column proxy for `initial` | **High** | Other 3 columns not verified; partial migration causes silent NULL reads |
| `challenges.next_id` | **No** | Medium | Assumed always present; absent on CTFd <3.5.x breaks all CRUD |
| `dynamic_challenge` table | Yes — column count | Medium | Correct for MariaDB |
| `dynamic_challenge.function` column name | No | Medium | Hardcoded `function`; a rename would cause runtime SQL error |
| `nervctf_instance_challenge` table | Yes — info_schema | Low | LEFT JOIN fallback |
| `BaseChallenge.attempt()` return type | Partial (TypeError catch) | **High** | Dual-path has logic gap for subscriptable ChallengeResponse |
| `BaseChallenge.solve/fail` signatures | No | Medium | 4-arg form assumed |
| Team-mode vs user-mode | **No** | **Critical** | User-mode CTFd: all players get 403, no operator warning |
| CTFd uploads path `.data/CTFd/uploads` | No | Medium | Hardcoded Docker volume convention |
| Plugin path `CTFd/plugins/nervctf_instance/` | No | Medium | Assumes CTFd source layout |
| Redis cache invalidation | N/A | Medium | Never invalidated after direct DB writes |

---

## 11. Critical Risks (Ranked)

### Rank 1 — CRITICAL: User-mode CTFd silently denies all instance access
`validate_token()` treats `users.team_id IS NULL` as unauthorized and returns `None`. All player-facing instance routes return 403 "Not in a team". The operator gets no warning during `nervctf setup`. **Affects:** `ctfd_db.rs:59`, `__init__.py:63-64`, all four player routes.
**Status: Fixed in v2.3.1** — `detect_ctfd_mode()` queries `configs WHERE key='user_mode'` immediately after pool creation and logs a `WARN` if user-mode is detected, giving the operator an early signal before the CTF goes live.

### Rank 2 — HIGH: `BaseChallenge.attempt()` return-type dual-path has a logic gap
The plugin catches `TypeError` (non-subscriptable) and falls back to `getattr(result, "success", False)`. If CTFd returns a `ChallengeResponse` namedtuple (which IS subscriptable), the fallback never fires. A correct flag submission could be reported to the monitor as incorrect, silently breaking flag-sharing detection and solve-triggered instance teardown. **Affects:** `__init__.py:267-272`.

### Rank 3 — HIGH: Single-column proxy for inline scoring migration
`detect_challenges_schema()` uses presence of `challenges.initial` to conclude that all four scoring columns are inline. A partial migration (only `initial` added) causes the SELECT to emit non-existent column references, producing runtime SQL errors or silent NULLs. **Affects:** `ctfd_db.rs:153, 168-176`.
**Status: Fixed in v2.3.1** — `dynamic_in_challenges` now requires all four columns (`initial`, `minimum`, `decay`, `function`) via a four-way AND. A new `dynamic_partial` field is set when only some columns are present, enabling safe fallback to the join-table path without generating invalid SQL.

### Rank 4 — HIGH: CTFd version only pinned at initial setup
The `version: "3.7.3"` git tag is only applied when CTFd is not yet cloned. The upgrade playbook has no version guard. A manual `git pull` on the server silently drifts all schema assumptions. **Affects:** `nervctf_playbook.yml:108-115`.
**Partial mitigation in v2.4.0:** `nervctf probe` reports the live CTFd version detected via the `configs` table (`ctfd_version_tag` / `ctfd_version_source`). `nervctf deploy` compares this tag against the `TESTED_CTFD_VERSION` constant (`3.7.3` in `src/nervctf/src/main.rs`) and emits a warning if a mismatch is detected. Run `nervctf probe --refresh` after any CTFd upgrade to update the cached report. The deploy does not block on version drift alone — the probe's capability fields (`cap_dynamic_scoring`, etc.) provide the authoritative compatibility signal.

### Rank 5 — MEDIUM: `challenges.next_id` assumed always present
All INSERT/UPDATE/SELECT statements include `next_id` with no schema detection. Absent on CTFd <3.5.x; every challenge operation fails with "Unknown column". **Affects:** `ctfd_db.rs` throughout.
**Status: Fixed in v2.3.1** — `has_next_id` field added to `ChallengesSchema`; `build_full_query()` substitutes a `NULL` placeholder at column index 10 when the column is absent (preserving all downstream indices in `row_to_value()`), and `create_challenge()`/`update_challenge()` omit the column and its binding entirely when `has_next_id` is false.

### Rank 6 — MEDIUM: CTFd flag orphaned if monitor crashes mid-provision
`ctfd_db::create_flag()` runs before `db::insert_instance()`. A crash between these two leaves a `flags` row in MariaDB with no `ctfd_flag_id` reference in SQLite — it will never be deleted and remains a valid submission target indefinitely.

### Rank 7 — MEDIUM: No Redis cache invalidation after direct DB writes
All challenge/flag/hint/tag/file/topic writes bypass CTFd's HTTP API and go directly to MariaDB. CTFd's Redis cache is never invalidated, so stale data may be served to players until the TTL expires or CTFd is restarted.

### Rank 8 — LOW: `needs_update` misses `files` and `topics` changes
A challenge where only files or topics changed is never detected as needing an update. The operator must run `--recreate` to force re-sync of these sub-resources.

### Rank 9 — LOW: `replace_flags` / `replace_hints` compare only `content`
`needs_update` triggers on `(content, type, data)` tuple changes, but the replacement function only compares content strings. A flag type change (`static` → `regex`) correctly triggers `needs_update` but is then silently dropped by `replace_flags`. The flag type remains unchanged on CTFd after deploy.

---

## 12. Known Working CTFd Version

**CTFd 3.7.3** — only version officially tested and deployed. The schema detection guards give partial forward/backward compatibility but none are validated at startup. Running against any other version is untested.

---

*Generated: 2026-05-26 | 8 parallel audit agents | NervCTF v2.3.1*
