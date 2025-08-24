# `challenge_manager` Module Documentation

## Overview

The `challenge_manager` module is the core of the `nervctf` system, responsible for all high-level operations on CTFd challenges. It provides a robust API for CRUD (Create, Read, Update, Delete) operations, local file system management, requirements/dependency handling, and synchronization between local and remote (CTFd) challenge states.

---

## Module Structure

- **`ChallengeManager`**: Main struct for managing challenges.
- **`sync` submodule**: Handles synchronization logic and dependency resolution.
- **`utils` submodule**: Provides validation and helper functions.

---

## Main Struct: `ChallengeManager`

### Fields

- `client: CtfdClient`
  - The API client for interacting with the remote CTFd instance.
- `base_path: PathBuf`
  - The root directory for local challenge files.
- `requirements_queue: RequirementsQueue`
  - Tracks challenge dependencies for correct sync order.

### Key Methods

#### `new(client: CtfdClient, base_path: &Path) -> Self`
Creates a new manager instance.
**Usage:**
- Instantiate once per session, passing the API client and the local base directory.

#### `get_all_challenges(&self) -> Result<Option<Vec<Challenge>>>`
Fetches all challenges from the remote CTFd instance.

#### `get_challenge(&self, id: u32) -> Result<Option<Challenge>>`
Fetches a specific challenge by its ID.

#### `get_challenge_by_name(&self, name: &str) -> Result<Option<Challenge>>`
Fetches a challenge by its name (case-sensitive).

#### `get_base_path(&self) -> &Path`
Returns the base path for local challenge files.

#### `generate_requirements_list(&mut self, challenges: Vec<Challenge>)`
Populates the requirements queue with challenge dependencies.
**Tricky aspect:**
- This is essential for correct sync order; missing or circular dependencies can cause sync failures.

#### `create_challenge(&self, config: &Challenge) -> Result<Option<Challenge>>`
Creates a new challenge on CTFd using the provided configuration.
- Only basic fields are set; flags, tags, files, and hints must be added separately.

#### `update_challenge(&self, id: u32, config: &Challenge) -> Result<Option<Challenge>>`
Updates an existing challenge and all its related entities (flags, tags, files, hints, requirements, state).
- **Atomic update:** Deletes all existing related entities before recreating them.
- **Tricky aspect:**
  - If any step fails, the challenge may be left in a partially updated state.
  - File uploads are performed using blocking multipart requests.

#### `delete_challenge(&self, id: u32) -> Result<()>`
Deletes a challenge by its ID.

#### `create_flag(&self, challenge_id: u32, flags: Vec<FlagContent>) -> Result<Option<Value>>`
Creates one or more flags for a challenge.

#### `scan_local_challenges(&self) -> Result<Vec<Challenge>>`
Scans the local file system for challenge definitions (`challenge.yml`), parses them, and returns a vector of `Challenge` structs.
- **Tricky aspect:**
  - If a challenge fails to parse, it is skipped with an error message.

#### `get_local_challenge(&self, name: &str) -> Result<Option<Challenge>>`
Returns a local challenge by name.

#### `create_challenge_from_file(&self, yaml_path: &Path) -> Result<Option<Challenge>>`
Creates a challenge on CTFd from a local YAML file.

#### `update_challenge_from_file(&self, challenge_id: u32, yaml_path: &Path) -> Result<Option<Challenge>>`
Updates a challenge on CTFd from a local YAML file.

#### `export_challenges(&self, export_path: &Path) -> Result<()>`
Exports all remote challenges to a local directory structure as YAML files.

#### `synchronizer(&self) -> sync::ChallengeSynchronizer`
Returns a new synchronizer instance for this manager.

---

## Submodule: `sync`

### Main Struct: `ChallengeSynchronizer`

- **Purpose:** Orchestrates the synchronization of local and remote challenges.
- **Key Method:**
  - `sync(&mut self, show_diff: bool) -> Result<()>`
    - Compares local and remote challenges, determines required actions, resolves dependencies, and applies changes.
    - Supports dry-run (diff only) and interactive execution.

### Enum: `SyncAction<'a>`

- **Variants:**
  - `Create { name, challenge }`
  - `Update { name, local, remote }`
  - `UpToDate { name, challenge }`
  - `RemoteOnly { name, challenge }`

- **Usage:**
  - Actions are generated during sync and executed in dependency order.

### Tricky/Advanced Aspects

- **Dependency Resolution:**
  - Uses a topological sort (Kahn's algorithm) to ensure challenges are created/updated in the correct order.
  - If dependencies are missing or circular, sync may fail or hang.

- **Partial Failures:**
  - If an action fails, the synchronizer continues with other actions, logging errors.

---

## Submodule: `utils`

### Function: `validate_challenge_config(config: &Challenge) -> Result<()>`

- **Purpose:**
  - Ensures that a challenge configuration is valid before deployment or sync.
- **Checks:**
  - Name and category are non-empty.
  - Value is non-zero.
  - At least one flag is present.

---

## Error Handling

- Uses the `anyhow` crate for rich error context.
- Most methods return `Result<Option<T>>` to distinguish between "not found" and actual errors.
- When scanning or syncing, errors in individual challenges are logged but do not halt the entire process.

---

## Debugging and Development Notes

- **File Uploads:**
  - File uploads are performed using blocking requests due to limitations in async multipart support. This can cause deadlocks if not handled carefully.
- **Atomic Updates:**
  - When updating a challenge, all related entities are deleted and recreated. If an error occurs mid-update, the challenge may be left in an inconsistent state.
- **Dependency Loops:**
  - Circular dependencies in requirements will cause the synchronizer to hang or fail. Always validate requirements before deployment.
- **Schema Changes:**
  - If the CTFd API or challenge schema changes, update both the Rust structs and the serialization logic.

---

## Example Workflow

1. **Scan Local Challenges:**
   `scan_local_challenges()` reads all `challenge.yml` files and parses them into `Challenge` structs.

2. **Validate Challenges:**
   Use `utils::validate_challenge_config()` to ensure each challenge is well-formed.

3. **Synchronize:**
   Use `synchronizer().sync(show_diff)` to compare local and remote states, resolve dependencies, and apply changes.

4. **Deploy/Update:**
   Use `create_challenge()` or `update_challenge()` to push changes to CTFd.

---

## Extending the Module

- **Adding New Challenge Fields:**
  - Update the `Challenge` struct and ensure serialization/deserialization is correct.
  - Update `create_challenge` and `update_challenge` to include new fields in API payloads.

- **Supporting New Challenge Types:**
  - Extend the `ChallengeType` enum and update logic where challenge types are handled.

- **Custom Validation:**
  - Add new checks to `validate_challenge_config` as needed.

---

## References

- [CTFd API Documentation](https://ctfd.io/api/v1)
- [serde](https://serde.rs/)
- [anyhow](https://docs.rs/anyhow/)
- [walkdir](https://docs.rs/walkdir/)

---

*This documentation is intended for developers and maintainers of the `nervctf` project. For further questions or contributions, please refer to the project repository or contact the maintainers.*
