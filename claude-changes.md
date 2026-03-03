# Claude-Assisted Changes

This document records all changes made by Claude Code to the NervCTF codebase
in the session of 2026-03-03.

The goal was to fix all critical bugs that made the tool non-functional, close
spec compliance gaps against the ctfcli `challenge.yml` format, and rewrite the
dead `remote-monitor` skeleton into a working HTTP proxy.

---

## Critical Bug Fixes

### Bug 1 — `CtfdClient::execute()` ignored POST/PATCH responses
**File:** `nervctf/src/ctfd_api/client.rs`

`execute()` only called `parse_response()` for `GET` requests. Every other
method (POST, PATCH) returned `Ok(None)`, so `deploy_single_challenge()` received
no challenge ID and immediately panicked on `.unwrap()`.

```rust
// Before
if method == Method::GET {

// After
if method != Method::DELETE {
```

---

### Bug 2 — `FlagData` serialized `CaseInsensitive` as `"caseinsensitive"`
**File:** `nervctf/src/ctfd_api/models/mod.rs`

`rename_all = "lowercase"` collapsed two words into one. CTFd's API expects
`"case_insensitive"`.

```rust
// Before
#[serde(rename_all = "lowercase")]
pub enum FlagData { CaseSensitive, CaseInsensitive }

// After
#[serde(rename_all = "snake_case")]
pub enum FlagData { CaseSensitive, CaseInsensitive }
```

---

### Bug 3 — `verify_local_challenges()` panicked on missing `description`
**File:** `nervctf/src/main.rs`

`challenge.description.clone().expect("REASON")` panicked whenever a challenge
had no description field. Descriptions are optional in the ctfcli spec.

```rust
// Before
if challenge.description.clone().expect("REASON").trim().is_empty() {

// After
if challenge.description.as_deref().unwrap_or("").trim().is_empty() {
```

---

### Bug 4 — Hint creation sent wrong field name `"value"` instead of `"cost"`
**File:** `nervctf/src/challenge_manager/mod.rs`

CTFd's `/api/v1/hints` endpoint expects the point-cost field to be named `"cost"`.
Sending `"value"` silently created hints with zero cost.

```rust
// Before
"value": hint.cost,

// After
"cost": cost,
```

---

## Spec Compliance Gaps

### Gap 1 — `Hint` couldn't deserialize the simple string format
**File:** `nervctf/src/ctfd_api/models/mod.rs`

The ctfcli spec allows hints as plain strings (`hints: ["Try harder"]`) as well
as detailed objects. The old `Hint` struct required a `content:` key.

Added `HintContent` enum and changed `Challenge.hints`:

```rust
#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(untagged)]
pub enum HintContent {
    Simple(String),
    Detailed {
        content: String,
        cost: Option<u32>,
        title: Option<String>,
    },
}
```

`Challenge.hints` changed from `Option<Vec<Hint>>` to `Option<Vec<HintContent>>`.

The original `Hint` struct is retained for deserializing CTFd API responses
(which always return the detailed format).

---

### Gap 2 — `FlagContent::Detailed` required the `data` field
**File:** `nervctf/src/ctfd_api/models/mod.rs`

The spec allows `{type: "static", content: "flag{x}"}` with no `data` field.
Static flags default to case-sensitive when `data` is absent.

```rust
// Before
data: FlagData,

// After
data: Option<FlagData>,
```

All pattern-match sites updated to `data.as_ref().map(...).unwrap_or_default()`.

---

### Gap 3 — `requirements` couldn't express advanced format or integer IDs
**File:** `nervctf/src/ctfd_api/models/mod.rs`

The spec supports:
- Simple name list: `requirements: ["Warmup"]`
- Integer IDs: `requirements: [1, 2]`
- Advanced object: `requirements: {prerequisites: ["Warmup"], anonymize: true}`

Added `Requirements` enum:

```rust
#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(untagged)]
pub enum Requirements {
    Simple(Vec<serde_json::Value>),   // strings or ints
    Advanced {
        prerequisites: Vec<serde_json::Value>,
        #[serde(default)]
        anonymize: bool,
    },
}
```

`Challenge.requirements` changed from `Option<Vec<String>>` to
`Option<Requirements>`.

A `prerequisite_names() -> Vec<String>` helper method extracts names/IDs as
strings for topological sorting. `RequirementsQueue::resolve_dependencies` was
updated from `HashMap<&str, HashSet<&str>>` to `HashMap<String, HashSet<String>>`
to avoid lifetime issues with owned data returned by this method.

---

### Gap 4 — `deploy_single_challenge()` was incomplete
**File:** `nervctf/src/main.rs`

The old implementation only sent `name`, `category`, `description`, `value`,
`type`, `state`, `connection_info`, and `requirements` in the create payload,
and had stub file upload. Rewrote with 8 ordered steps:

1. **POST challenge core** — adds `attempts`, `extra` to the payload
2. **POST flags** — handles both `Simple` and `Detailed` variants; `data` is now optional
3. **POST tags** — handles both `Simple` and `Detailed` tag variants
4. **POST topics** — new; was never deployed before
5. **POST hints** — handles `HintContent::Simple` and `HintContent::Detailed`
6. **POST files via multipart** — fully implemented; previously a stub
7. **PATCH requirements** — resolves names/IDs → integer IDs, then PATCHes
8. **PATCH next** — new; resolves `next` challenge name → ID

---

### Gap 5 — `needs_update()` only compared 3 fields
**File:** `nervctf/src/challenge_manager/sync.rs`

Previously checked only `category`, `value`, `description`. Now also compares:

- `state`
- `connection_info`
- `attempts`
- `extra` (via JSON serialization)
- `flags` — sorted content strings
- `tags` — sorted value strings
- `hints` — sorted content strings

---

## New Feature: `.nervctf.yml` Config File

**File:** `nervctf/src/utils.rs`

Added `Config` struct and `load_config(start_dir)` that walks up the directory
tree to find `.nervctf.yml`. Env vars override the file; CLI flags override
env vars.

```yaml
# .nervctf.yml
ctfd_url: https://ctfd.example.com
ctfd_api_key: ctfd_...
monitor_url: http://server:33133
monitor_token: secret
base_dir: ./challenges
```

Priority (highest to lowest):
1. CLI flags (`--monitor-url`, `--monitor-token`)
2. Environment variables (`CTFD_URL`, `CTFD_API_KEY`, `MONITOR_URL`, `MONITOR_TOKEN`)
3. `.nervctf.yml` config file

---

## New Feature: `--monitor-url` / `--monitor-token` CLI Flags

**File:** `nervctf/src/main.rs`

When `monitor_url` and `monitor_token` are both resolved (from CLI, env, or
config file), `CtfdClient` is pointed at the monitor instead of CTFd directly.
No other code changes are needed — the proxy exposes identical `/api/v1/*` routes.

```sh
nervctf deploy --monitor-url http://server:33133 --monitor-token mysecret
```

---

## Remote Monitor Rewrite

**Files:** `remote-monitor/Cargo.toml`, `remote-monitor/src/main.rs`

### Before
Dead TCP skeleton — accepted connections, printed "New connection!", did nothing.

### After
Full HTTP reverse proxy built on **axum 0.7**:

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

**Routes:**

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/health` | No | Liveness check |
| ANY | `/api/v1/diff` | Yes | Local/remote diff summary |
| ANY | `/api/v1/*path` | Yes | Transparent CTFd proxy |

**Environment variables:**

| Variable | Default | Description |
|----------|---------|-------------|
| `CTFD_URL` | — | CTFd instance URL (required) |
| `CTFD_API_KEY` | — | CTFd admin token (required) |
| `MONITOR_TOKEN` | — | Token clients must present (required) |
| `MONITOR_PORT` | `33133` | Port to bind |
| `MONITOR_BIND` | `0.0.0.0` | Bind address |

**`POST /api/v1/diff` body:**
```json
{"challenges": [<same format as CTFd challenge list>]}
```
Returns `{to_create, to_update, up_to_date, remote_only}`.

**Implementation note:** axum 0.7 uses `http 1.x` while `reqwest 0.11` uses
`http 0.2.x`. `Method` and header types are different crate versions and cannot
be used interchangeably. The proxy bridges this by converting via `.as_str()` /
`.as_bytes()` at the boundary between the two stacks.

### New dependencies (`remote-monitor/Cargo.toml`)

```toml
tokio = { version = "1", features = ["full"] }
axum = "0.7"
reqwest = { version = "0.11", features = ["json", "multipart", "blocking", "stream"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
tower-http = { version = "0.4", features = ["trace"] }
bytes = "1.0"
```

---

## Files Modified

| File | Change |
|------|--------|
| `nervctf/src/ctfd_api/client.rs` | Bug 1: parse response for all non-DELETE methods |
| `nervctf/src/ctfd_api/models/mod.rs` | Bug 2, Gap 1-3: FlagData snake_case, HintContent enum, Requirements enum, FlagContent.data optional, PartialEq derives for State/FlagType/FlagData/ChallengeType, resolve_dependencies uses owned Strings |
| `nervctf/src/challenge_manager/mod.rs` | Bug 4, Gap 1: hint cost field, HintContent variants, Requirements.prerequisite_names(), FlagContent.data optional handling |
| `nervctf/src/challenge_manager/sync.rs` | Gap 5: extended needs_update comparison |
| `nervctf/src/main.rs` | Bug 3, Gap 4: verify fix, deploy rewrite, .nervctf.yml config, --monitor-url/--monitor-token flags |
| `nervctf/src/utils.rs` | New: Config struct + load_config() |
| `remote-monitor/Cargo.toml` | New: add axum, reqwest, serde, etc. |
| `remote-monitor/src/main.rs` | Complete rewrite as HTTP proxy |
