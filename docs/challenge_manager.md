# The `challenge_manager` module

## Overview

`challenge_manager` is the mid-level orchestration layer for CTFd challenge operations. It wraps `CtfdClient` to give one API for scanning local files, fetching remote state, and running CRUD operations.

The primary deploy workflow lives in `src/main.rs` (`deploy_challenges()`), which uses `CtfdClient` directly for full control over the four-phase order. `ChallengeManager` is used for programmatic access and by `ChallengeSynchronizer`.

---

## Module structure

```
src/challenge_manager/
├── mod.rs     — ChallengeManager struct, CRUD helpers, inline utils submodule
└── sync.rs    — ChallengeSynchronizer, SyncAction, needs_update()
```

---

## `ChallengeManager`

### Fields

| Field | Type | Description |
|---|---|---|
| `client` | `CtfdClient` | Async API client (targets the remote monitor, which writes to CTFd's MariaDB) |
| `base_path` | `PathBuf` | Root directory for local challenge files |
| `requirements_queue` | `RequirementsQueue` | Dependency tracking for sync order |

### Key methods

#### `new(client: CtfdClient, base_path: &Path) -> Self`

#### `scan_local_challenges(&self) -> Result<Vec<Challenge>>`

Recursively walks `base_path/challenges/` for `challenge.yml` files, parses each, and sets `source_path`. Parse failures are printed to stderr and skipped.

#### `get_all_challenges(&self) -> Result<Option<Vec<Challenge>>>`

Fetches all challenges from CTFd via `GET /api/v1/challenges` (paginated).

#### `create_challenge(&self, data: &Challenge) -> Result<Option<Challenge>>`

POSTs the core fields to CTFd and returns the created challenge with its `id`. Flags, hints, tags, and files are added separately.

#### `update_challenge(&self, id: u32, data: &Challenge) -> Result<Option<Challenge>>`

Full update: deletes existing flags, tags, hints, and files, then recreates them from the config. It also patches requirements and state.

#### `delete_challenge(&self, id: u32) -> Result<()>`

#### `generate_requirements_list(&mut self, challenges: Vec<Challenge>)`

Fills `requirements_queue` with dependency information for topological ordering.

#### `synchronizer(&self) -> sync::ChallengeSynchronizer`

---

## The `sync` submodule

### `needs_update(remote: &Challenge, local: &Challenge) -> bool`

A public free function. Returns `true` when any significant field differs.

**Fields compared:**

| Field | Comparison key | Notes |
|---|---|---|
| `category`, `value`, `description`, `state` | exact | |
| `connection_info`, `attempts` | exact | |
| `extra` | JSON-serialized | |
| `flags` | sorted `(content, type, data)` tuples | detects type and data changes |
| `tags` | sorted tag value strings | |
| `hints` | sorted `(content, cost)` tuples | detects cost changes |
| `requirements` | sorted prerequisite name/ID list | |
| `instance` | JSON-serialized | |
| `files` | sorted basenames | triggers when local has files; the remote never has this field populated |
| `topics` | sorted topic strings | triggers when local has topics; the remote never has this field populated |

**Files and topics.** CTFd's `/challenges` list endpoint never includes `files` or `topics`, so `remote.files` and `remote.topics` are always `None`. When local has files or topics, `needs_update` returns `true` unconditionally so `update_challenge_phase1` runs; `sync_files()` and `replace_topics()` then do the real idempotent diff against the live CTFd API and skip the update when nothing changed.

**Flags, hints, and tags.** The list endpoint does not return these sub-resources either, so their comparisons fire only when both sides are `Some`, which happens in tests and when the deploy loop populates the remote struct from a detail call.

`ChallengeSynchronizer::needs_update()` delegates to this free function to avoid duplication. Both the `sync` command and the test suite call the free function.

---

## Sub-resource sync strategy

`update_challenge_phase1()` in `main.rs` syncs sub-resources (flags, hints, tags, topics, files) after the core challenge PATCH. Each has its own replace helper.

### Comparison keys per sub-resource

| Sub-resource | Comparison key | Where compared |
|---|---|---|
| flags | sorted `(content, type, data)` tuples | `replace_flags()` and `needs_update()` |
| hints | sorted `(content, cost)` tuples | `replace_hints()` and `needs_update()` |
| tags | sorted value strings | `replace_tags()` and `needs_update()` |
| topics | sorted value strings | `replace_topics()` (idempotent diff) and `needs_update()` |
| files | sorted basenames | `sync_files()` (idempotent diff) and `needs_update()` |

### Which sub-resources trigger `needs_update`

All five are checked. For flags, hints, tags, and requirements the check is symmetric (`if let (Some, Some)`). For files and topics it is asymmetric: any non-empty local list returns `true`, because the remote `Challenge` never has those fields populated from the list endpoint.

### Replace-function flow

`replace_flags`, `replace_hints`, and `replace_tags` all follow the same pattern:

1. Fetch the current remote sub-resources from a dedicated CTFd endpoint.
2. Build sorted comparison vectors from both sides using the full comparison key.
3. Return early when the vectors are equal.
4. Otherwise delete every existing remote sub-resource for the challenge.
5. POST the full local set as replacements.

`replace_topics` uses a differential approach (delete extras, add missing), because CTFd's topics endpoint has no bulk delete. `sync_files` deletes all remote files and marks the challenge for phase-2 re-upload when the filename sets differ.

### `ChallengeSynchronizer`

```rust
pub struct ChallengeSynchronizer {
    challenge_manager: ChallengeManager,
}
```

`sync(&mut self, show_diff: bool)` runs the full compare-and-apply cycle:

```rust
pub enum SyncAction<'a> {
    Create   { name: String, challenge: &'a Challenge },
    Update   { name: String, local: &'a Challenge, remote: &'a Challenge },
    UpToDate { name: String, challenge: &'a Challenge },
    RemoteOnly { name: String, challenge: &'a Challenge },
}
```

Actions are topologically sorted by `RequirementsQueue::resolve_dependencies()` (Kahn's algorithm).

The four-phase deploy in `main.rs` supersedes the synchronizer for the primary workflow. The synchronizer is used internally by `deploy` to compute the diff.

### Dependency resolution (`RequirementsQueue`)

Kahn's topological sort:

1. Build `HashMap<name, HashSet<prereq_names>>` from all `Create` and `Update` actions.
2. Seed the ready queue with nodes that have no dependencies.
3. Process the queue: for each resolved node, remove it from the other nodes' dependency sets and enqueue anything newly unblocked.
4. Append `UpToDate` and `RemoteOnly` actions at the end.

Circular dependencies make the sort drop the challenges involved, silently. Run `nervctf validate` first.

---

## The inline `utils` submodule

### `validate_challenge_config(config: &Challenge) -> Result<()>`

A lightweight legacy check: `name`, `category`, and `value` non-empty and non-zero, plus at least one flag. It predates the full `validator` module; prefer `validator::validate_challenges()`.

---

## Deploy workflow

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
8. PATCH /challenges/{id}     — set requirements (names resolved to IDs)
── Phase 4 ──────────────────────────────────────────────────────────────
9. PATCH /challenges/{id}     — set next_id (names resolved to IDs)
── Instance ─────────────────────────────────────────────────────────────
10. POST /instance/build[-compose]  — upload build context to monitor
11. POST /instance/register         — register config on monitor
```

Phases 3 and 4 run after every challenge exists, so forward references always resolve.

---

## Error handling

- `Result<Option<T>>` separates "not found" (`Ok(None)`) from API and IO errors (`Err`).
- A single broken YAML file does not abort the scan; it is logged to stderr and skipped.
- Partial updates are safe to retry. `needs_update()` detects the incomplete state on the next deploy.

---

## Extending

**New challenge fields:**

1. Add the field to `Challenge` in `models/mod.rs`.
2. Include it in the payload in `create_challenge_phase1()` and `update_challenge_phase1()` in `main.rs`.
3. Add it to the comparison in `needs_update()` in `sync.rs`.
4. Optionally add a validation rule in `validator.rs`.

**New challenge types:**

1. Extend the `ChallengeType` enum in `models/mod.rs`.
2. Handle the type-specific payload differences in `create_challenge_phase1()`.

---

## References

- `src/nervctf/src/challenge_manager/mod.rs`
- `src/nervctf/src/challenge_manager/sync.rs`
- `src/nervctf/src/main.rs` — primary deploy orchestration
- [CTFd API documentation](https://docs.ctfd.io/docs/api/redoc/)
- [Kahn's algorithm](https://en.wikipedia.org/wiki/Topological_sorting#Kahn's_algorithm)
