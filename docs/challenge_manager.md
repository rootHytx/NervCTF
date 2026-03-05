# `challenge_manager` Module Documentation

## Overview

The `challenge_manager` module provides the mid-level orchestration layer for
CTFd challenge operations. It wraps `CtfdClient` and the directory scanner to
offer a unified API for scanning local files, fetching remote state, and
performing CRUD operations.

The primary **deploy and sync workflow** lives in `src/main.rs`
(`deploy_challenges()`), which uses `CtfdClient` directly for maximum control
over the 4-phase deployment order. `ChallengeManager` remains available for
programmatic use and is exercised by the legacy `ChallengeSynchronizer`.

---

## Module Structure

```
src/challenge_manager/
├── mod.rs     — ChallengeManager struct and CRUD helpers
├── sync.rs    — ChallengeSynchronizer, SyncAction, needs_update()
└── utils.rs   — (legacy) basic field validation helper
```

---

## `ChallengeManager`

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `client` | `CtfdClient` | Async API client for CTFd |
| `base_path` | `PathBuf` | Root directory for local challenge files |
| `requirements_queue` | `RequirementsQueue` | Dependency tracking for sync order |

### Key Methods

#### `new(client: CtfdClient, base_path: &Path) -> Self`
Creates a new manager. Instantiate once per session.

#### `scan_local_challenges(&self) -> Result<Vec<Challenge>>`
Recursively walks `base_path` for `challenge.yml` files, parses each one, and
sets `source_path` to the containing directory. Parse failures are logged and
skipped (non-fatal).

#### `get_all_challenges(&self) -> Result<Option<Vec<Challenge>>>`
Fetches all challenges from the remote CTFd instance via `GET /api/v1/challenges`.

#### `create_challenge(&self, data: &Value) -> Result<Option<Challenge>>`
POSTs a challenge payload to CTFd. Returns the created challenge with its assigned
`id`. Only the core fields are included; flags, hints, tags, and files must be
added as separate requests.

#### `update_challenge(&self, id: u32, data: &Value) -> Result<Option<Challenge>>`
PATCHes an existing challenge. For full updates (replacing all sub-resources),
the higher-level `update_challenge_phase1()` in `main.rs` deletes existing
flags/tags/hints/files before recreating them.

#### `delete_challenge(&self, id: u32) -> Result<()>`
Deletes a challenge and (on CTFd's side) all its associated flags, hints, tags,
and files.

#### `generate_requirements_list(&mut self, challenges: Vec<Challenge>)`
Populates `requirements_queue` with dependency information extracted from each
challenge's `requirements` field. Used by `ChallengeSynchronizer` to determine
the creation order.

#### `synchronizer(&self) -> sync::ChallengeSynchronizer`
Returns a new `ChallengeSynchronizer` bound to this manager.

---

## `sync` Submodule

### `needs_update(remote: &Challenge, local: &Challenge) -> bool`

Standalone public function (also callable from `main.rs`) that returns `true` if
any significant field differs between the remote CTFd state and the local YAML.

**Fields compared:**

| Field | Notes |
|-------|-------|
| `category` | exact match |
| `value` | exact match |
| `description` | exact match |
| `state` | exact match |
| `connection_info` | exact match |
| `attempts` | exact match |
| `extra` | JSON-serialized comparison |
| `flags` | sorted list of `content` strings |
| `tags` | sorted list of tag values |
| `hints` | sorted list of `content` strings |

This function is the gatekeeper for the diff computation in `deploy_challenges()`:
only challenges where `needs_update()` returns `true` enter the UPDATE path.

### `ChallengeSynchronizer`

```rust
pub struct ChallengeSynchronizer {
    challenge_manager: ChallengeManager,
}
```

The synchronizer provides a `sync(&mut self, show_diff: bool)` method that
orchestrates the full compare-and-apply cycle using `SyncAction`:

```rust
pub enum SyncAction<'a> {
    Create   { name: String, challenge: &'a Challenge },
    Update   { name: String, local: &'a Challenge, remote: &'a Challenge },
    UpToDate { name: String, challenge: &'a Challenge },
    RemoteOnly { name: String, challenge: &'a Challenge },
}
```

Actions are topologically sorted by `RequirementsQueue::resolve_dependencies()`
(Kahn's algorithm) to ensure challenges without dependencies are created first.

**Note:** The 4-phase deploy in `main.rs` supersedes the synchronizer for the
primary workflow. The synchronizer remains useful when you need the full
`SyncAction` enum (e.g., to handle `RemoteOnly` challenges or build tooling on
top of the diff).

### Dependency Resolution (`RequirementsQueue`)

`RequirementsQueue` implements Kahn's topological sort over `SyncAction` entries:

1. Build a `HashMap<name, HashSet<prereq_names>>` from all `Create`/`Update`
   actions.
2. Seed a ready-queue with nodes that have no unresolved dependencies.
3. Process the ready-queue: for each resolved node, remove it from every other
   node's dependency set, then enqueue newly unblocked nodes.
4. Append `UpToDate` and `RemoteOnly` actions at the end (order-insensitive).

Circular dependencies cause the sort to silently drop the involved challenges.
Use `nervctf validate` to catch self-referencing requirements before they reach
this stage.

---

## `utils` Submodule

### `validate_challenge_config(config: &Challenge) -> Result<()>`

A lightweight legacy check confirming that `name`, `category`, and `value` are
non-empty/non-zero and that at least one flag is present. This predates the full
`validator` module; for comprehensive validation use `validator::validate_challenges()`.

---

## Error Handling

- All methods use `anyhow` for rich error context.
- `Result<Option<T>>` distinguishes "not found" (`Ok(None)`) from actual API/IO
  errors (`Err`).
- Scan failures in individual challenge files are printed to stderr and skipped,
  so a single broken YAML does not abort the full scan.

---

## Deploy Workflow (current)

The primary deployment path is in `src/main.rs`. Here is the end-to-end flow:

```
1. scan_directory()           — find all challenge.yml files
2. validate_challenges()      — pre-flight checks (errors block deploy)
3. get_challenges()           — fetch remote state
4. needs_update()             — compute diff (CREATE / UPDATE / UP-TO-DATE)
5. show diff, confirm
─── Phase 1 ──────────────────────────────────────────────────────────────
6. POST /challenges           — create/update core fields
7. POST /flags                — create flags
8. POST /tags                 — create tags
9. POST /topics               — create topics
10. POST /hints               — create hints
─── Phase 2 ──────────────────────────────────────────────────────────────
11. POST /files               — upload all files per challenge in one
                                batched multipart request
─── Phase 3 ──────────────────────────────────────────────────────────────
12. PATCH /challenges/{id}    — set requirements (names → IDs resolved)
─── Phase 4 ──────────────────────────────────────────────────────────────
13. PATCH /challenges/{id}    — set next_id (names → IDs resolved)
```

Phases 3 and 4 run after all challenges exist so forward-references never fail.

---

## File Uploads

Files are uploaded via `CtfdClient::upload_file()`, which uses the **async**
`reqwest::Client` with a 120-second timeout. All files belonging to a challenge
are batched into a **single** multipart `POST /api/v1/files` request:

```
POST /api/v1/files
Content-Type: multipart/form-data; boundary=...

--boundary
Content-Disposition: form-data; name="challenge_id"

339
--boundary
Content-Disposition: form-data; name="type"

challenge
--boundary
Content-Disposition: form-data; name="file"; filename="exploit.py"

<binary>
--boundary
Content-Disposition: form-data; name="file"; filename="Dockerfile"

<binary>
--boundary--
```

This matches ctfcli's `_create_all_files()` pattern. Sending one file per
request produces a 500 from CTFd.

**CTFd upload pre-requisite:** the uploads directory must be writable by CTFd's
process user (UID 1001 in the default Docker image):

```sh
sudo chown -R 1001:1001 /path/to/CTFd/.data/CTFd/uploads
```

---

## Debugging Notes

- **Partial updates**: if a Phase 1 operation fails, the challenge may exist
  on CTFd with no flags/hints/files. Rerunning deploy is safe — `needs_update()`
  will detect the discrepancy and reapply.
- **Circular requirements**: `RequirementsQueue::resolve_dependencies()` silently
  drops challenges caught in a cycle. Run `nervctf validate` first.
- **Schema changes**: if the CTFd API changes, update both the model structs in
  `models/mod.rs` and any payload construction in `main.rs`.

---

## Extending the Module

**New challenge fields:**
1. Add the field to `Challenge` in `models/mod.rs`.
2. Include it in the payload JSON in `create_challenge_phase1()` / `update_challenge_phase1()` in `main.rs`.
3. Add a comparison in `needs_update()` in `sync.rs`.
4. Optionally add a validation check in `validator.rs`.

**New challenge types:**
1. Extend `ChallengeType` enum in `models/mod.rs`.
2. Handle any type-specific payload differences in `create_challenge_phase1()`.

---

## References

- `src/nervctf/src/challenge_manager/mod.rs`
- `src/nervctf/src/challenge_manager/sync.rs`
- `src/nervctf/src/main.rs` — primary deploy orchestration
- [CTFd API Documentation](https://docs.ctfd.io/docs/api/redoc/)
- [Kahn's algorithm](https://en.wikipedia.org/wiki/Topological_sorting#Kahn's_algorithm)
