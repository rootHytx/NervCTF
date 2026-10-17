# The `ctfd_api` module

`ctfd_api` is the HTTP client layer the `nervctf` CLI uses to talk to the remote monitor. Despite the name, which is historical, no request in this module reaches CTFd's REST API. The monitor implements every endpoint and translates it into direct MariaDB SQL against CTFd's database.

---

## Module structure

```
src/ctfd_api/
├── mod.rs            — re-exports CtfdClient
├── client.rs         — HTTP client, auth, request/response handling
├── models/
│   └── mod.rs        — all data types (Challenge, FlagContent, and so on)
└── endpoints/
    ├── challenges.rs  — challenge CRUD and sub-resource queries
    ├── flags.rs
    ├── files.rs
    ├── hints.rs
    └── tags.rs
```

---

## 1. `client.rs` — `CtfdClient`

### Fields

| Field | Type | Description |
|---|---|---|
| `client` | `reqwest::Client` | Async HTTP client with the `Authorization: Token <monitor_token>` default header |
| `base_url` | `String` | Monitor URL, with the trailing slash stripped |

### Construction

```rust
pub fn new(monitor_url: &str, monitor_token: &str) -> Result<Self>
```

It sets `Authorization: Token <monitor_token>` as a default header on every request, uses a 10-second timeout, and disables redirect following (`Policy::none()`). `BASE_PATH = "/api/v1"` is a constant appended per request in `request()`.

### `request`

```rust
pub async fn request<T: Serialize + ?Sized>(
    &self, method, endpoint, body: Option<&T>,
) -> Result<Response>
```

Builds the full URL (`base_url + /api/v1 + endpoint`), attaches a JSON body when present, and returns the raw `Response`. Non-2xx statuses return an `Err` that includes the response body.

### `parse_response` (private)

Reads the body bytes, errors on an empty response, and parses JSON. It handles both the monitor's wrapped `{"data": ...}` format and bare object responses, and prints the first 500 bytes when parsing fails, for diagnostics.

### `execute`

```rust
pub async fn execute<T: DeserializeOwned, B: Serialize + ?Sized>(
    &self, method, endpoint, body: Option<&B>,
) -> Result<Option<T>>
```

Combines `request` and `parse_response`. It returns `Ok(None)` only for `DELETE`; every other method parses and returns the body.

### `upload_file`

```rust
pub async fn upload_file(
    &self, endpoint: &str, form: reqwest::multipart::Form,
) -> Result<()>
```

Uploads files with a 120-second timeout. All files for a challenge must go into one multipart request. The monitor's file handler, like CTFd's, calls `request.files.getlist("file")` and returns 500 if it is called once per file.

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

A wrapper for requests whose response body is discarded, typically `DELETE`.

---

## 2. `endpoints/` — resource methods

Every endpoint method is an `impl CtfdClient` extension. They call paths under `/api/v1/` that the remote monitor handles with direct SQL. Nothing is forwarded to CTFd's REST API.

### `challenges.rs`

| Method | Monitor route | Description |
|---|---|---|
| `get_challenges()` | `GET /challenges?page=N` | All challenges; paginated (loops via `meta.pagination.next`) |
| `create_challenge(data)` | `POST /challenges` | Returns the full object with its assigned `id` |
| `update_challenge(id, data)` | `PATCH /challenges/{id}` | Partial update |
| `delete_challenge(id)` | `DELETE /challenges/{id}` | Delete |
| `get_challenge_files_endpoint(id)` | `GET /files?challenge_id=N` | File list for a challenge |
| `get_challenge_flags_endpoint(id)` | `GET /flags?challenge_id=N` | Flag list for a challenge |
| `get_challenge_tags_endpoint(id)` | `GET /tags?challenge_id=N` | Tag list for a challenge |
| `get_challenge_hints_endpoint(id)` | `GET /hints?challenge_id=N` | Hint list for a challenge |

**Pagination.** CTFd's MariaDB table and the monitor's list handler return 20 challenges per page by default. Without looping, challenges past page 1 always appear in `to_create`, so every re-deploy duplicates them.

### `flags.rs`, `hints.rs`, `tags.rs`

Each provides `get_*`, `create_*`, and `delete_*` methods accepting `&Value` payloads.

### `files.rs`

File listing and deletion. Creation goes through `upload_file()`.

---

## 3. `models/mod.rs` — data types

### `ChallengeType`

```rust
enum ChallengeType { Standard, Dynamic, Instance }
// serde: "standard" | "dynamic" | "instance"
```

`Instance` challenges deploy to CTFd as `standard` or `dynamic`, depending on `extra.initial`. CTFd never receives `"instance"` as the type; the `nervctf_instance` plugin registers it separately.

### `Challenge` — key fields

| Field | Type | Notes |
|---|---|---|
| `name` | `String` | Required |
| `category` | `String` | Required |
| `value` | `u32` | Required; use `extra.initial` for dynamic |
| `challenge_type` | `ChallengeType` | `standard`, `dynamic`, or `instance` |
| `description` | `Option<String>` | |
| `state` | `Option<State>` | `visible` or `hidden` |
| `flags` | `Option<Vec<FlagContent>>` | Simple strings or detailed objects |
| `hints` | `Option<Vec<HintContent>>` | Simple strings or detailed objects |
| `tags` | `Option<Vec<Tag>>` | Simple strings or detailed objects |
| `files` | `Option<Vec<String>>` | Paths relative to `source_path` |
| `requirements` | `Option<Requirements>` | Simple list, integer IDs, or advanced |
| `extra` | `Option<Extra>` | `initial`, `decay`, and `minimum` for dynamic scoring |
| `instance` | `Option<InstanceConfig>` | Only for `type: instance` |
| `connection_info` | `Option<String>` | Netcat address, URL, and so on |
| `next` | `Option<String>` | Name of the next challenge in a sequence |
| `source_path` | `String` | Set at scan time; `#[serde(skip)]` |
| `unknown_yaml_keys` | `Vec<String>` | Unknown top-level keys; `#[serde(skip)]` |

### Enum types

#### `FlagContent`

```rust
#[serde(untagged)]
enum FlagContent {
    Simple(String),
    Detailed { type_: FlagType, content: String, data: Option<FlagData>, .. }
}
```

`data` is `Option<FlagData>`, so `{type: static, content: "flag{x}"}` with no `data` deserializes correctly.

#### `FlagData`

```rust
#[serde(rename_all = "snake_case")]
enum FlagData { CaseSensitive, CaseInsensitive }
```

`snake_case` produces `"case_insensitive"`, which is what CTFd expects.

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

`.prerequisite_names()` extracts every value as a string, skipping nulls.

#### `InstanceConfig`

See `docs/instance-challenges.md` for the full field reference.

### `RequirementsQueue`

A Kahn topological sort over named dependency nodes. `resolve_dependencies()` uses it to order `SyncAction` entries so prerequisites are created before their dependents.

---

## 4. Error handling

| Error message | Likely cause |
|---|---|
| `Empty response body from .../challenges` | Monitor not reachable, or `CTFD_DB_URL` misconfigured |
| `API error 401` | Monitor token mismatch between CLI and monitor |
| `File upload failed` | `CTFD_UPLOADS_DIR` not set, or not writable by the monitor process |
| `API error (POST /challenges): ...` | Malformed payload or a MariaDB constraint violation |

---

## 5. Relationship to the remote monitor

The monitor implements every `/api/v1/*` route that `CtfdClient` calls. The implementation is in `src/remote-monitor/src/ctfd_db.rs` (MariaDB SQL) and the axum handlers in `src/remote-monitor/src/main.rs`. The CLI never makes an HTTP request directly to CTFd.

The monitor handles only the routes in section 2. There is no catch-all proxy; unrecognised paths return 404.

---

## 6. Extending the API

1. Add a monitor route handler in `src/remote-monitor/src/main.rs`.
2. Add a client method to the appropriate `endpoints/*.rs` as an `impl CtfdClient`.
3. Use `execute::<ReturnType, _>(Method::POST, "/endpoint", Some(&payload))` for JSON requests.
4. Use `request_without_body(Method::DELETE, "/endpoint/id", None::<&()>)` for DELETE.
5. Add new fields to `Challenge`, or a new struct in `models/mod.rs`, with the right serde attributes.

---

## 7. Version compatibility

| Column or feature | Present since | Guard | Notes |
|---|---|---|---|
| `challenges.next_id` | CTFd 3.5.x | `has_next_id` — NULL placeholder in SELECT, omitted from INSERT/UPDATE when absent | Absent on older versions; before the guard, every challenge operation failed with "Unknown column 'next_id'" |
| `challenges.attribution` | CTFd 3.7.0 | `has_attribution` — guarded in SELECT and the UPDATE SET | Read-only; the monitor never sets it |
| `challenges.logic` | CTFd 3.7.x | `has_logic` — guarded in INSERT and the UPDATE SET | Default insert value is `""`, not NULL |
| `challenges.initial/minimum/decay/function` (inline) | Newer CTFd | `dynamic_in_challenges` requires all four columns | The single-column proxy (`initial` alone) was wrong; a partial migration sets `dynamic_partial=true` and falls back to the join-table path |
| User-mode CTFd | N/A (runtime config) | `detect_ctfd_mode()` at startup warns when user-mode is detected | User-mode sets `users.team_id = NULL` for all players; `validate_token()` returns `None`, so every player instance route returns 403 |

---

## 8. `nervctf probe`

The `probe` subcommand calls `GET /api/v1/admin/probe` on the remote monitor and prints a capability matrix showing CTFd schema compatibility and operational status.

### Usage

```
nervctf probe [--refresh] [--json]
```

| Flag | Description |
|---|---|
| `--refresh` | Force the monitor to re-run its schema queries instead of returning the cached result. Adds a small MariaDB round-trip. |
| `--json` | Print the raw probe data as pretty-printed JSON instead of the formatted table. Useful for scripting. |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Every capability is `ok` or at most `degraded`. Deploy is expected to succeed with possible caveats. |
| `1` | One or more critical capabilities (`cap_challenge_crud`, `cap_dynamic_scoring`, `cap_player_auth`, `cap_instance_flags`) are `broken`. Deploy is likely to fail. |

`cap_redis_sync` being `broken` or `degraded` does not affect the exit code. It is an operational warning only and does not block challenge CRUD.

### Capability fields

Every capability is one of `"ok"`, `"degraded"`, `"broken"`, or `"unknown"`.

| Field | What it covers |
|---|---|
| `cap_challenge_crud` | INSERT/UPDATE/DELETE on the `challenges` table |
| `cap_dynamic_scoring` | `dynamic_challenge` table state and inline scoring columns |
| `cap_player_auth` | Team-mode detection; user-mode CTFd blocks every player instance route |
| `cap_instance_flags` | `nervctf_instance_challenge` table presence |
| `cap_redis_sync` | Redis cache invalidation after direct MariaDB writes |

### Deploy-time probe gate

Before any remote writes, `nervctf deploy` fetches the cached probe result (no `--refresh`). The rules:

- **`cap_dynamic_scoring == "broken"`** — deploy aborts with exit code 1. The `dynamic_challenge` table is in a partial migration state, and deploying dynamic challenges would produce runtime SQL errors.
- **`cap_player_auth == "broken"`** — a warning is printed and deploy continues. Instance challenges will not be reachable by players until CTFd is switched to team mode.
- **Degraded capabilities** — a summary line lists which capabilities are degraded. Run `nervctf probe` for the full report.
- **Version mismatch** — when the probe reports a `ctfd_version_tag` different from the `TESTED_CTFD_VERSION` constant (`3.7.3`), a warning is printed and deploy continues with caution.
- **Probe unavailable** — when the monitor does not implement the probe endpoint (an older monitor binary), a warning is printed and deploy continues. This keeps compatibility with pre-probe monitor versions.

---

## 9. Event settings

`nervctf event` maps the `event:` block to CTFd setting keys and calls `execute::<Value, _>(Method::PUT, "/admin/event", Some(&payload))`. The monitor enforces the key allowlist and resolves whether the installed CTFd stores settings in `configs` or `config`. See `docs/remote-monitor.md` for the endpoint contract.

---

## References

- `src/nervctf/src/ctfd_api/client.rs`
- `src/nervctf/src/ctfd_api/models/mod.rs`
- `src/remote-monitor/src/ctfd_db.rs` — SQL implementation of the monitor API
- `src/remote-monitor/src/main.rs` — axum route definitions
