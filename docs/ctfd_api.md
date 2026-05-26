# `ctfd_api` Module Documentation

The `ctfd_api` module is the HTTP client layer used by the `nervctf` CLI to communicate
with the **remote-monitor**. Despite the module name (a historical artifact), no request
in this module ever reaches CTFd's own REST API. All endpoints are implemented by the
monitor, which translates them into direct MariaDB SQL queries against CTFd's database.

---

## Module Structure

```
src/ctfd_api/
├── mod.rs            — re-exports CtfdClient
├── client.rs         — HTTP client, auth, request/response handling
├── models/
│   └── mod.rs        — all data types (Challenge, FlagContent, etc.)
└── endpoints/
    ├── challenges.rs  — challenge CRUD + sub-resource queries
    ├── flags.rs
    ├── files.rs
    ├── hints.rs
    └── tags.rs
```

---

## 1. `client.rs` — `CtfdClient`

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `client` | `reqwest::Client` | Async HTTP client with `Authorization: Token <monitor_token>` default header |
| `base_url` | `String` | Monitor URL (trailing slash stripped) |

### Construction

```rust
pub fn new(monitor_url: &str, monitor_token: &str) -> Result<Self>
```

- Sets `Authorization: Token <monitor_token>` as a default header on every request.
- 10-second default timeout.
- Redirect following disabled (`Policy::none()`).
- `BASE_PATH = "/api/v1"` is a constant appended per-request in `request()`.

### `request`

```rust
pub async fn request<T: Serialize + ?Sized>(
    &self, method, endpoint, body: Option<&T>,
) -> Result<Response>
```

Constructs the full URL (`base_url + /api/v1 + endpoint`), attaches JSON body when provided,
returns the raw `Response`. Returns `Err` for non-2xx status codes with the response body
included in the error message.

### `parse_response` (private)

Internal helper. Reads body bytes, errors on empty response, parses as JSON. Handles both
the monitor's wrapped `{"data": ...}` response format and bare object responses. Shows
the first 500 bytes on parse failure for diagnostics.

### `execute`

```rust
pub async fn execute<T: DeserializeOwned, B: Serialize + ?Sized>(
    &self, method, endpoint, body: Option<&B>,
) -> Result<Option<T>>
```

Combines `request` + `parse_response`. Returns `Ok(None)` only for `DELETE` requests;
parses and returns body for all other methods.

### `upload_file`

```rust
pub async fn upload_file(
    &self, endpoint: &str, form: reqwest::multipart::Form,
) -> Result<()>
```

Uploads files with a 120-second timeout. All files for a challenge must be batched into
a **single** multipart request — the monitor's file handler (and CTFd's underlying handler)
calls `request.files.getlist("file")` and returns 500 if called once per file.

```rust
let mut form = reqwest::multipart::Form::new()
    .text("challenge_id", id.to_string())
    .text("type", "challenge");
for part in file_parts {
    form = form.part("file", part);  // multiple "file" parts, same field name
}
client.upload_file("/files", form).await?;
```

### `request_without_body`

Convenience wrapper for requests where the response body is discarded (typically `DELETE`).

---

## 2. `endpoints/` — Resource Methods

All endpoint methods are `impl CtfdClient` extensions. They call paths under `/api/v1/`
which are **handled by the remote-monitor** via direct SQL — not forwarded to CTFd's own
REST API.

### `challenges.rs`

| Method | Monitor route | Description |
|--------|--------------|-------------|
| `get_challenges()` | `GET /challenges?page=N` | All challenges; **paginated** (loops via `meta.pagination.next`) |
| `create_challenge(data)` | `POST /challenges` | Returns full object with assigned `id` |
| `update_challenge(id, data)` | `PATCH /challenges/{id}` | Partial update |
| `delete_challenge(id)` | `DELETE /challenges/{id}` | Delete |
| `get_challenge_files_endpoint(id)` | `GET /files?challenge_id=N` | File list for challenge |
| `get_challenge_flags_endpoint(id)` | `GET /flags?challenge_id=N` | Flag list for challenge |
| `get_challenge_tags_endpoint(id)` | `GET /tags?challenge_id=N` | Tag list for challenge |
| `get_challenge_hints_endpoint(id)` | `GET /hints?challenge_id=N` | Hint list for challenge |

**Pagination note**: CTFd's MariaDB table (and the monitor's list handler) defaults to
20 challenges per page. Without looping, challenges beyond page 1 always appear in
`to_create`, causing duplicates on every re-deploy.

### `flags.rs`, `hints.rs`, `tags.rs`

Each provides `get_*`, `create_*`, and `delete_*` methods accepting `&Value` payloads.

### `files.rs`

File listing and deletion. Creation is handled via `upload_file()`.

---

## 3. `models/mod.rs` — Data Types

### `ChallengeType`

```rust
enum ChallengeType { Standard, Dynamic, Instance }
// serde: "standard" | "dynamic" | "instance"
```

`Instance` challenges are deployed to CTFd as `standard` or `dynamic` (depending on
`extra.initial`). CTFd itself never receives `"instance"` as the type — the
`nervctf_instance` plugin registers the type separately.

### `Challenge` — key fields

| Field | Type | Notes |
|-------|------|-------|
| `name` | `String` | Required |
| `category` | `String` | Required |
| `value` | `u32` | Required; use `extra.initial` for dynamic |
| `challenge_type` | `ChallengeType` | `standard` \| `dynamic` \| `instance` |
| `description` | `Option<String>` | |
| `state` | `Option<State>` | `visible` \| `hidden` |
| `flags` | `Option<Vec<FlagContent>>` | Simple strings or detailed objects |
| `hints` | `Option<Vec<HintContent>>` | Simple strings or detailed objects |
| `tags` | `Option<Vec<Tag>>` | Simple strings or detailed objects |
| `files` | `Option<Vec<String>>` | Paths relative to `source_path` |
| `requirements` | `Option<Requirements>` | Simple list, integer IDs, or advanced |
| `extra` | `Option<Extra>` | `initial`, `decay`, `minimum` for dynamic scoring |
| `instance` | `Option<InstanceConfig>` | Only for `type: instance` |
| `connection_info` | `Option<String>` | Netcat address, URL, etc. |
| `next` | `Option<String>` | Name of next challenge in sequence |
| `source_path` | `String` | Set at scan time; `#[serde(skip)]` |
| `unknown_yaml_keys` | `Vec<String>` | Unknown top-level keys; `#[serde(skip)]` |

### Enum Types

#### `FlagContent`
```rust
#[serde(untagged)]
enum FlagContent {
    Simple(String),
    Detailed { type_: FlagType, content: String, data: Option<FlagData>, .. }
}
```
`data` is `Option<FlagData>` — `{type: static, content: "flag{x}"}` with no `data`
deserializes correctly.

#### `FlagData`
```rust
#[serde(rename_all = "snake_case")]
enum FlagData { CaseSensitive, CaseInsensitive }
```
`snake_case` produces `"case_insensitive"` — what CTFd expects.

#### `HintContent`
```rust
#[serde(untagged)]
enum HintContent {
    Simple(String),
    Detailed { content: String, cost: Option<u32>, title: Option<String> }
}
```

#### `Requirements`
```rust
#[serde(untagged)]
enum Requirements {
    Simple(Vec<serde_json::Value>),  // strings or integer IDs
    Advanced { prerequisites: Vec<serde_json::Value>, anonymize: bool }
}
```
`.prerequisite_names()` extracts all values as strings, skipping nulls.

#### `InstanceConfig`
See `docs/instance-challenges.md` for the full field reference.

### `RequirementsQueue`

Kahn's topological sort over named dependency nodes. Used in `resolve_dependencies()` to
order `SyncAction` entries so prerequisites are created before dependents.

---

## 4. Error Handling

| Error message | Likely cause |
|---------------|--------------|
| `Empty response body from .../challenges` | Monitor not reachable or `CTFD_DB_URL` misconfigured |
| `API error 401` | Monitor token mismatch between CLI and monitor |
| `File upload failed` | `CTFD_UPLOADS_DIR` not set or not writable by the monitor process |
| `API error (POST /challenges): ...` | Malformed payload or MariaDB constraint violation |

---

## 5. Relationship to the Remote-Monitor

The monitor implements every `/api/v1/*` route that `CtfdClient` calls. The implementation
lives in `src/remote-monitor/src/ctfd_db.rs` (MariaDB SQL) and the axum handlers in
`src/remote-monitor/src/main.rs`. There is no code path in the `nervctf` CLI that makes
an HTTP request directly to CTFd.

The monitor handles only the specific routes listed in Section 2 above. There is no
catch-all proxy — unrecognised paths return 404. All CLI functionality is covered by
the explicit handlers in `src/remote-monitor/src/main.rs`.

---

## 6. Extending the API

1. Add a monitor route handler in `src/remote-monitor/src/main.rs`.
2. Add a client method to the appropriate `endpoints/*.rs` as `impl CtfdClient`.
3. Use `execute::<ReturnType, _>(Method::POST, "/endpoint", Some(&payload))` for JSON requests.
4. Use `request_without_body(Method::DELETE, "/endpoint/id", None::<&()>)` for DELETE.
5. Add new fields to `Challenge` or a new struct in `models/mod.rs` with appropriate serde attributes.

---

## 7. Known Version Compatibility

| Column / Feature | Present since | Guard | Notes |
|-----------------|--------------|-------|-------|
| `challenges.next_id` | CTFd 3.5.x | `has_next_id` — NULL placeholder in SELECT; omitted from INSERT/UPDATE when absent | Absent on older versions; every challenge operation failed with "Unknown column 'next_id'" before this guard |
| `challenges.attribution` | CTFd 3.7.0 | `has_attribution` — already guarded in SELECT and UPDATE SET | Read-only path; never set by the monitor |
| `challenges.logic` | CTFd 3.7.x | `has_logic` — already guarded in INSERT and UPDATE SET | Default insert value is `""` not NULL |
| `challenges.initial/minimum/decay/function` (inline) | Newer CTFd | `dynamic_in_challenges` requires **all four** columns present | Single-column proxy (`initial` only) was wrong; a partial migration sets `dynamic_partial=true` and falls back safely to the join-table path |
| User-mode CTFd | N/A (runtime config) | `detect_ctfd_mode()` at startup warns if user-mode is detected | User-mode sets `users.team_id = NULL` for all players; `validate_token()` returns `None`, causing 403 on all player instance routes |

---

## 8. `nervctf probe` Command

The `nervctf probe` subcommand calls `GET /api/v1/admin/probe` on the remote monitor and
displays a capability matrix showing CTFd schema compatibility and operational status.

### Usage

```
nervctf probe [--refresh] [--json]
```

| Flag | Description |
|------|-------------|
| `--refresh` | Force the monitor to re-run its schema introspection queries instead of returning a cached result. Adds a small MariaDB round-trip overhead. |
| `--json` | Print the raw probe data as pretty-printed JSON instead of the formatted table. Suitable for scripting. |

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All capabilities are `ok` or at most `degraded`. Deployment is expected to succeed with possible caveats. |
| `1` | One or more critical capabilities (`cap_challenge_crud`, `cap_dynamic_scoring`, `cap_player_auth`, `cap_instance_flags`) are `broken`. Deployment is likely to fail. |

Note that `cap_redis_sync` being `broken` or `degraded` does **not** affect the exit code —
it is an operational warning only and does not prevent challenge CRUD operations.

### Capability Fields

All capability fields return one of: `"ok"`, `"degraded"`, `"broken"`, `"unknown"`.

| Field | What it covers |
|-------|----------------|
| `cap_challenge_crud` | INSERT/UPDATE/DELETE on the `challenges` table |
| `cap_dynamic_scoring` | `dynamic_challenge` table state and inline scoring columns |
| `cap_player_auth` | Team-mode detection; user-mode CTFd blocks all player instance routes |
| `cap_instance_flags` | `nervctf_instance_challenge` table presence |
| `cap_redis_sync` | Redis cache invalidation after direct MariaDB writes |

### Deploy-time Probe Gate

`nervctf deploy` automatically fetches the cached probe result (no `--refresh`) before
executing any remote writes. The following rules apply:

- **`cap_dynamic_scoring == "broken"`** — deploy is aborted with exit code 1. The
  `dynamic_challenge` table is in a partial migration state and deploying dynamic
  challenges would produce runtime SQL errors.
- **`cap_player_auth == "broken"`** — a warning is printed but deploy continues.
  Instance challenges will not be accessible to players until CTFd is switched to
  team mode.
- **Degraded capabilities** — a summary line is printed listing which capabilities
  are degraded. Run `nervctf probe` for the full report.
- **Version mismatch** — if the probe reports a `ctfd_version_tag` different from the
  `TESTED_CTFD_VERSION` constant (`3.7.3`), a warning is printed and deploy continues
  with caution.
- **Probe unavailable** — if the monitor does not yet implement the probe endpoint
  (older monitor binary), a warning is printed and deploy continues without blocking.
  This preserves backward compatibility with pre-probe monitor versions.

---

## References

- `src/nervctf/src/ctfd_api/client.rs`
- `src/nervctf/src/ctfd_api/models/mod.rs`
- `src/remote-monitor/src/ctfd_db.rs` — SQL implementation of all monitor API endpoints
- `src/remote-monitor/src/main.rs` — axum route definitions
