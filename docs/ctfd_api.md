# `ctfd_api` Module Documentation

The `ctfd_api` module is the abstraction layer for all interactions with the CTFd
REST API. It provides a type-safe, async, and extensible interface for all CTFd
resources — challenges, flags, files, hints, and tags.

---

## Module Structure

```
src/ctfd_api/
├── mod.rs            — re-exports CtfdClient
├── client.rs         — HTTP client, auth, request/response handling
├── models/
│   └── mod.rs        — all data types (Challenge, FlagContent, etc.)
└── endpoints/
    ├── mod.rs
    ├── challenges.rs  — challenge CRUD + sub-resource queries
    ├── flags.rs
    ├── files.rs
    ├── hints.rs
    └── tags.rs
```

---

## 1. `client.rs` — `CtfdClient`

### Purpose

`CtfdClient` manages authentication, HTTP client configuration, and provides
generic request/response primitives. All endpoint methods are implemented as
`impl CtfdClient` blocks in the `endpoints/` files.

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `client` | `reqwest::Client` | Async HTTP client (primary) |
| `blocking_client` | `reqwest::blocking::Client` | Retained for `post_file()` legacy callers |
| `base_url` | `String` | CTFd base URL (trailing slash stripped) |
| `api_key` | `String` | API key stored for future reference |

### Construction

```rust
pub fn new(base_url: &str, api_key: &str) -> Result<Self>
```

- Sets `Authorization: Token <api_key>` as the **only** default header.
  `Content-Type` is intentionally omitted at the client level:
  - JSON requests set it implicitly via `.json(body)`.
  - Multipart requests set it via `.multipart(form)`.
  Setting `Content-Type: application/json` as a default causes 500 errors on
  CTFd's file upload endpoint.
- Configures a 10-second default timeout on both clients.
- Disables automatic redirect following (`Policy::none()`). This ensures
  authentication failures (CTFd returning 302 → `/login`) surface as clear
  errors instead of silently landing on an HTML page.

---

### `request`

```rust
pub async fn request<T: Serialize + ?Sized>(
    &self,
    method: Method,
    endpoint: &str,
    body: Option<&T>,
) -> Result<Response>
```

Constructs the full URL (`base_url + /api/v1 + endpoint`), attaches a JSON body
when provided, sends the request, and returns the raw `Response`. Returns `Err`
for any non-2xx status, including the raw error body in the message.

---

### `parse_response`

```rust
pub async fn parse_response<T: DeserializeOwned>(response: Response) -> Result<T>
```

1. Reads the entire response body as bytes.
2. Returns a descriptive error if the body is empty (common when credentials
   are invalid or CTFd redirects to the login page).
3. Parses as JSON, showing the first 500 bytes of the raw body on parse failure
   so mismatches between expected and actual CTFd responses are immediately
   diagnosable.
4. Handles both CTFd's wrapped format `{"data": ...}` and bare object responses.

**Debugging tip:** if you see `JSON parse error from http://.../login`, your API
key is not being accepted by CTFd. Check `Challenge Visibility` (must not be
`Private`) and verify the key is still valid.

---

### `execute`

```rust
pub async fn execute<T: DeserializeOwned, B: Serialize + ?Sized>(
    &self,
    method: Method,
    endpoint: &str,
    body: Option<&B>,
) -> Result<Option<T>>
```

Combines `request` + `parse_response`. Returns `Ok(None)` only for `DELETE`
requests; parses and returns the body for all other methods (GET, POST, PATCH).

**Key fix vs. earlier versions:** the original implementation only parsed
responses for `GET`. This caused `create_challenge()`, `create_flag()`, etc. to
always return `None`, making the created resource's ID unavailable and causing
panics when the deploy code tried to unwrap it.

---

### `upload_file`

```rust
pub async fn upload_file(
    &self,
    endpoint: &str,
    form: reqwest::multipart::Form,
) -> Result<()>
```

Uploads files to CTFd using the **async** client with a 120-second timeout
(accommodating large files). The caller is responsible for building the form with
all files batched into a single request:

```rust
let mut form = reqwest::multipart::Form::new()
    .text("challenge_id", id.to_string())
    .text("type", "challenge");
for (_, part) in file_parts {
    form = form.part("file", part);  // multiple "file" parts, same field name
}
client.upload_file("/files", form).await?;
```

**Critical:** CTFd's `/api/v1/files` handler calls `request.files.getlist("file")`
— it expects all files for a challenge in a single multipart request. Sending one
file per request produces a 500 Internal Server Error.

---

### `post_file` (legacy)

```rust
pub async fn post_file<T: DeserializeOwned>(
    &self,
    endpoint: &str,
    form: Option<multipart::Form>,  // reqwest::blocking::multipart::Form
) -> Result<Option<T>>
```

Retained for legacy callers. Uses the blocking client. **Do not call from within
an async tokio context** — the blocking client creates its own runtime and panics
if one already exists. Use `upload_file` for all new code.

---

### `execute_with_params`

```rust
pub async fn execute_with_params<T, B, P>(
    &self, method, endpoint, body: Option<&B>, params: &P,
) -> Result<T>
```

Sends a request with URL query parameters. Useful for filtered or paginated
endpoints (e.g., `GET /api/v1/challenges?category=web`).

---

### `request_without_body`

```rust
pub async fn request_without_body<T: Serialize + ?Sized>(
    &self, method, endpoint, body: Option<&T>,
) -> Result<()>
```

Convenience wrapper for requests where the response body can be discarded
(typically `DELETE`).

---

## 2. `endpoints/` — Resource Methods

All endpoint methods are `impl CtfdClient` extensions. They use the generic
client methods for consistent error handling.

### `challenges.rs`

| Method | HTTP | Description |
|--------|------|-------------|
| `get_challenges()` | GET /challenges | All challenges |
| `get_challenge(id)` | GET /challenges/{id} | By ID |
| `get_challenge_id(name)` | GET /challenges | ID lookup by name |
| `create_challenge(data)` | POST /challenges | Create challenge, returns full object with ID |
| `update_challenge(id, data)` | PATCH /challenges/{id} | Partial update |
| `delete_challenge(id)` | DELETE /challenges/{id} | Delete |
| `get_challenge_files_endpoint(id)` | GET /challenges/{id}/files | Sub-resource list |
| `get_challenge_flags_endpoint(id)` | GET /challenges/{id}/flags | Sub-resource list |
| `get_challenge_tags_endpoint(id)` | GET /challenges/{id}/tags | Sub-resource list |
| `get_challenge_hints_endpoint(id)` | GET /challenges/{id}/hints | Sub-resource list |

### `flags.rs`, `hints.rs`, `tags.rs`

Each module provides `get_*`, `create_*`, and `delete_*` methods. The `create_*`
methods accept `&Value` payloads so callers retain full control over field names.

### `files.rs`

File listing and deletion. Creation is handled directly via `upload_file()`.

---

## 3. `models/mod.rs` — Data Types

### `Challenge`

The central struct, used for both local YAML deserialization and CTFd API
responses. Key fields:

| Field | Type | Notes |
|-------|------|-------|
| `name` | `String` | Required |
| `category` | `String` | Required |
| `value` | `u32` | Required; use `extra.initial` for dynamic |
| `challenge_type` | `ChallengeType` | `standard` \| `dynamic` |
| `description` | `Option<String>` | |
| `state` | `Option<State>` | `visible` \| `hidden` |
| `flags` | `Option<Vec<FlagContent>>` | Simple strings or detailed objects |
| `hints` | `Option<Vec<HintContent>>` | Simple strings or detailed objects |
| `tags` | `Option<Vec<Tag>>` | Simple strings or detailed objects |
| `files` | `Option<Vec<String>>` | Paths relative to `source_path` |
| `requirements` | `Option<Requirements>` | Simple list, integer IDs, or advanced |
| `extra` | `Option<Extra>` | `initial`, `decay`, `minimum` for dynamic |
| `connection_info` | `Option<String>` | Netcat address, URL, etc. |
| `next` | `Option<String>` | Name of the next challenge in sequence |
| `source_path` | `String` | Set at scan time; `#[serde(skip)]` |
| `id` | `Option<u32>` | Set from CTFd API response |

### Enum Types

#### `FlagContent`
```rust
#[serde(untagged)]
pub enum FlagContent {
    Simple(String),
    Detailed { type_: FlagType, content: String, data: Option<FlagData>, .. }
}
```
`data: Option<FlagData>` — the `data` field is optional so `{type: static, content: "flag{x}"}` without `data` deserializes correctly.

#### `FlagData`
```rust
#[serde(rename_all = "snake_case")]
pub enum FlagData { CaseSensitive, CaseInsensitive }
```
`rename_all = "snake_case"` serializes `CaseInsensitive` as `"case_insensitive"` — what CTFd expects. (An earlier `"lowercase"` was a bug that produced `"caseinsensitive"`.)

#### `HintContent`
```rust
#[serde(untagged)]
pub enum HintContent {
    Simple(String),
    Detailed { content: String, cost: Option<u32>, title: Option<String> }
}
```
Allows `hints: ["free hint"]` alongside `hints: [{content: "...", cost: 50}]`.

#### `Requirements`
```rust
#[serde(untagged)]
pub enum Requirements {
    Simple(Vec<serde_json::Value>),   // strings or integer IDs
    Advanced { prerequisites: Vec<serde_json::Value>, anonymize: bool }
}
```
`prerequisite_names()` extracts all values as strings for dependency resolution,
skipping `null` entries.

#### `Tag`
```rust
#[serde(untagged)]
pub enum Tag {
    Simple(String),
    Detailed { value: String, .. }
}
```

### `RequirementsQueue`

Implements Kahn's topological sort over a set of named nodes with dependency
edges. Used in `ChallengeSynchronizer::resolve_dependencies()` to order
`SyncAction` entries so prerequisites are created before dependents.

---

## 4. Error Handling

- `request()` returns the HTTP status, method, endpoint, and full error body on
  failure.
- `parse_response()` returns the URL, HTTP status, parse error, and first 500
  characters of the response body on JSON parse failure.
- All errors propagate via `anyhow::Result`, preserving the full context chain.

**Common errors and causes:**

| Error message | Likely cause |
|---------------|--------------|
| `Empty response body from .../challenges` | CTFd not set up, or Challenge Visibility = Private |
| `JSON parse error ... Body: <!DOCTYPE html>` | Redirect to login page; invalid or expired API key |
| `File upload failed (500)` | CTFd uploads directory not writable by UID 1001 |
| `API error (POST /challenges): ...` | Malformed payload or unsupported challenge type |

---

## 5. Extending the API

1. Add a method to the appropriate `endpoints/*.rs` as `impl CtfdClient`.
2. Use `execute::<ReturnType, _>(Method::POST, "/endpoint", Some(&payload))` for
   JSON-body requests.
3. Use `request_without_body(Method::DELETE, "/endpoint/id", None::<&()>)` for
   DELETE.
4. If the response format differs from `{"data": ...}`, extend `parse_response()`
   or use `request()` directly and parse the bytes manually.
5. Add any new fields to `Challenge` or a new struct in `models/mod.rs` with
   appropriate `serde` attributes.

---

## 6. Example Usage

```rust
let client = CtfdClient::new("https://ctfd.example.com", "ctfd_...")?;

// Fetch all challenges
let challenges = client.get_challenges().await?.unwrap_or_default();

// Create a challenge
let payload = serde_json::json!({
    "name": "My Challenge",
    "category": "web",
    "value": 100,
    "type": "standard",
    "state": "visible",
    "description": "Find the flag.",
});
let created: Challenge = client
    .execute::<Challenge, _>(Method::POST, "/challenges", Some(&payload))
    .await?
    .ok_or_else(|| anyhow!("No response"))?;
let id = created.id.unwrap();

// Upload files (all at once)
let mut form = reqwest::multipart::Form::new()
    .text("challenge_id", id.to_string())
    .text("type", "challenge");
let bytes = tokio::fs::read("./dist/exploit.py").await?;
let part = reqwest::multipart::Part::bytes(bytes).file_name("exploit.py");
form = form.part("file", part);
client.upload_file("/files", form).await?;
```

---

## 7. Summary Table

| File | Responsibility | Key items |
|------|----------------|-----------|
| `client.rs` | Auth, request, response, upload | `CtfdClient`, `execute`, `upload_file`, `parse_response` |
| `endpoints/challenges.rs` | Challenge CRUD | `get_challenges`, `create_challenge`, `update_challenge` |
| `endpoints/flags.rs` | Flag CRUD | `create_flag`, `delete_flag` |
| `endpoints/files.rs` | File listing/deletion | `delete_file` |
| `endpoints/hints.rs` | Hint CRUD | `create_hint`, `delete_hint` |
| `endpoints/tags.rs` | Tag CRUD | `create_tag`, `delete_tag` |
| `models/mod.rs` | All data types | `Challenge`, `FlagContent`, `HintContent`, `Requirements` |

---

## References

- `src/nervctf/src/ctfd_api/client.rs`
- `src/nervctf/src/ctfd_api/models/mod.rs`
- [CTFd API Reference](https://docs.ctfd.io/docs/api/redoc/)
- [reqwest documentation](https://docs.rs/reqwest/)
- [serde documentation](https://serde.rs/)
