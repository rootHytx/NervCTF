# `challenge_manager` Module Documentation

## Overview

The `challenge_manager` module provides the mid-level orchestration layer for CTFd challenge
operations. It wraps `CtfdClient` to offer a unified API for scanning local files, fetching
remote state, and performing CRUD operations.

The primary deploy workflow lives in `src/main.rs` (`deploy_challenges()`), which uses
`CtfdClient` directly for maximum control over the 4-phase deployment order.
`ChallengeManager` is used for programmatic access and by the `ChallengeSynchronizer`.

---

## Module Structure

```
src/challenge_manager/
├── mod.rs     — ChallengeManager struct, CRUD helpers, inline utils submodule
└── sync.rs    — ChallengeSynchronizer, SyncAction, needs_update()
```

---

## `ChallengeManager`

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `client` | `CtfdClient` | Async API client (targets remote-monitor, which proxies to CTFd MariaDB) |
| `base_path` | `PathBuf` | Root directory for local challenge files |
| `requirements_queue` | `RequirementsQueue` | Dependency tracking for sync order |

### Key Methods

#### `new(client: CtfdClient, base_path: &Path) -> Self`

#### `scan_local_challenges(&self) -> Result<Vec<Challenge>>`
Recursively walks `base_path/challenges/` for `challenge.yml` files, parses each, and sets
`source_path`. Parse failures are printed to stderr and skipped (non-fatal).

#### `get_all_challenges(&self) -> Result<Option<Vec<Challenge>>>`
Fetches all challenges from CTFd via `GET /api/v1/challenges` (paginated).

#### `create_challenge(&self, data: &Challenge) -> Result<Option<Challenge>>`
POSTs core fields to CTFd. Returns the created challenge with its `id`. Flags, hints, tags,
and files are added separately.

#### `update_challenge(&self, id: u32, data: &Challenge) -> Result<Option<Challenge>>`
Full update: deletes all existing flags/tags/hints/files first, then recreates from config.
Also patches requirements and state.

#### `delete_challenge(&self, id: u32) -> Result<()>`

#### `generate_requirements_list(&mut self, challenges: Vec<Challenge>)`
Populates `requirements_queue` with dependency info for topological ordering.

#### `synchronizer(&self) -> sync::ChallengeSynchronizer`

---

## `sync` Submodule

### `needs_update(remote: &Challenge, local: &Challenge) -> bool`

Public free function. Returns `true` if any significant field differs.

**Fields compared:**

| Field | Comparison key | Notes |
|-------|----------------|-------|
| `category`, `value`, `description`, `state` | exact | |
| `connection_info`, `attempts` | exact | |
| `extra` | JSON-serialized | |
| `flags` | sorted `(content, type, data)` tuples | type/data changes are detected |
| `tags` | sorted tag value strings | |
| `hints` | sorted `(content, cost)` tuples | cost changes are detected |
| `requirements` | sorted prerequisite name/id list | |
| `instance` | JSON-serialized | |
| `files` | sorted basenames | triggers if local has files; remote never has this field populated |
| `topics` | sorted topic strings | triggers if local has topics; remote never has this field populated |

**Note on files and topics:** CTFd's `/challenges` list endpoint never includes `files` or
`topics` on the response objects, so `remote.files` and `remote.topics` are always `None`.
When local has files or topics defined, `needs_update` returns `true` unconditionally so that
`update_challenge_phase1` runs — `sync_files()` and `replace_topics()` then perform the real
idempotent diff against the live CTFd API and skip the update if nothing changed.

**Note on flags/hints/tags:** CTFd's list endpoint also never returns these sub-resource
fields, so flag/hint/tag comparisons only fire when both sides are `Some` (which happens
in tests and when the deploy loop explicitly populates the remote struct from a detail call).

The `ChallengeSynchronizer::needs_update()` method delegates directly to this free function
to avoid code duplication. Both the `sync` command and the test suite call the free function.

---

## Sub-Resource Sync Strategy

Sub-resources (flags, hints, tags, topics, files) are synced by `update_challenge_phase1()`
in `main.rs` after the core challenge PATCH. Each sub-resource has its own replace helper.

### Comparison keys per sub-resource

| Sub-resource | Comparison key | Where compared |
|--------------|----------------|----------------|
| flags | sorted `(content, type, data)` tuples | `replace_flags()` + `needs_update()` |
| hints | sorted `(content, cost)` tuples | `replace_hints()` + `needs_update()` |
| tags | sorted value strings | `replace_tags()` + `needs_update()` |
| topics | sorted value strings | `replace_topics()` (idempotent diff) + `needs_update()` |
| files | sorted basenames | `sync_files()` (idempotent diff) + `needs_update()` |

### Which sub-resources trigger `needs_update`

All five sub-resources are checked in `needs_update()`. For flags, hints, tags, and
requirements the check is symmetric (`if let (Some, Some)`). For files and topics the check
is asymmetric: any non-empty local list returns `true` because the remote `Challenge` struct
never has these fields populated from the list endpoint.

### Replace function flow

`replace_flags`, `replace_hints`, and `replace_tags` each follow the same pattern:
1. Fetch the current remote sub-resources via a dedicated CTFd endpoint.
2. Build sorted comparison vectors from both sides using the full comparison key.
3. If the vectors are equal, return early (no-op).
4. Otherwise, delete all existing remote sub-resources for this challenge.
5. POST the full local set as replacements.

`replace_topics` uses a differential approach (delete extras, add missing) because CTFd's
topics endpoint does not support bulk delete. `sync_files` deletes all remote files and
marks the challenge for phase-2 re-upload when the filename sets differ.

### `ChallengeSynchronizer`

```rust
pub struct ChallengeSynchronizer {
    challenge_manager: ChallengeManager,
}
```

`sync(&mut self, show_diff: bool)` orchestrates the full compare-and-apply cycle:

```rust
pub enum SyncAction<'a> {
    Create   { name: String, challenge: &'a Challenge },
    Update   { name: String, local: &'a Challenge, remote: &'a Challenge },
    UpToDate { name: String, challenge: &'a Challenge },
    RemoteOnly { name: String, challenge: &'a Challenge },
}
```

Actions are topologically sorted by `RequirementsQueue::resolve_dependencies()` (Kahn's algorithm).

**Note:** The 4-phase deploy in `main.rs` supersedes the synchronizer for the primary workflow.
The synchronizer is used internally by `deploy` for diff computation.

### Dependency Resolution (`RequirementsQueue`)

Kahn's topological sort:
1. Build `HashMap<name, HashSet<prereq_names>>` from all `Create`/`Update` actions.
2. Seed ready-queue with nodes that have no dependencies.
3. Process queue: for each resolved node, remove from others' dep sets, enqueue newly unblocked.
4. Append `UpToDate` and `RemoteOnly` actions at the end.

Circular dependencies cause the sort to silently drop involved challenges. Use `nervctf validate` first.

---

## `utils` Submodule (inline)

### `validate_challenge_config(config: &Challenge) -> Result<()>`

Lightweight legacy check: `name`, `category`, `value` non-empty/non-zero, at least one flag.
Predates the full `validator` module; prefer `validator::validate_challenges()` for comprehensive checks.

---

## Deploy Workflow

```
1. scan_directory()           — find all challenge.yml files
2. validate_challenges()      — pre-flight checks (errors block deploy)
3. get_challenges()           — fetch remote state (paginated)
4. needs_update()             — compute diff (CREATE / UPDATE / UP-TO-DATE)
── Phase 1 ──────────────────────────────────────────────────────────────
5. POST /challenges           — create/update core fields
6. POST /flags / /tags / /topics / /hints
── Phase 2 ──────────────────────────────────────────────────────────────
7. POST /files                — all files per challenge in one batched multipart request
── Phase 3 ──────────────────────────────────────────────────────────────
8. PATCH /challenges/{id}     — set requirements (names → IDs resolved)
── Phase 4 ──────────────────────────────────────────────────────────────
9. PATCH /challenges/{id}     — set next_id (names → IDs resolved)
── Instance ─────────────────────────────────────────────────────────────
10. POST /instance/build[-compose]  — upload build context to monitor
11. POST /instance/register         — register config on monitor
```

Phases 3 and 4 run after all challenges exist so forward-references always resolve.

---

## Error Handling

- `Result<Option<T>>` distinguishes "not found" (`Ok(None)`) from API/IO errors (`Err`).
- Single broken YAML does not abort the full scan (logged to stderr, skipped).
- Partial updates are safe to retry — `needs_update()` detects incomplete state on next deploy.

---

## Extending

**New challenge fields:**
1. Add to `Challenge` in `models/mod.rs`.
2. Include in payload in `create_challenge_phase1()` / `update_challenge_phase1()` in `main.rs`.
3. Add comparison in `needs_update()` in `sync.rs`.
4. Optionally add validation in `validator.rs`.

**New challenge types:**
1. Extend `ChallengeType` enum in `models/mod.rs`.
2. Handle type-specific payload differences in `create_challenge_phase1()`.

---

## References

- `src/nervctf/src/challenge_manager/mod.rs`
- `src/nervctf/src/challenge_manager/sync.rs`
- `src/nervctf/src/main.rs` — primary deploy orchestration
- [CTFd API Documentation](https://docs.ctfd.io/docs/api/redoc/)
- [Kahn's algorithm](https://en.wikipedia.org/wiki/Topological_sorting#Kahn's_algorithm)
