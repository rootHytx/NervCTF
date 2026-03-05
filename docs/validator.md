# `validator` Module Documentation

## Overview

The `validator` module provides a static analysis pass over a collection of parsed
`Challenge` structs. It is called automatically before every `deploy` (blocking on
errors) and is also exposed as the standalone `nervctf validate` subcommand.

Unlike the scanner (which catches YAML parse failures) or `nervctf fix` (which
patches missing fields interactively), the validator works on fully-parsed
challenges and focuses on logical and referential correctness.

---

## Architecture

```
src/
└── validator.rs
    ├── Severity          (Error | Warning)
    ├── Issue             (per-finding record)
    ├── ValidationReport  (result set + print helper)
    ├── validate_challenges()   ← public entry point
    └── validate_one()          ← per-challenge checks (private)
```

---

## Public API

### `validate_challenges(challenges: &[Challenge]) -> ValidationReport`

The main entry point. Accepts a slice of already-parsed challenges and returns a
`ValidationReport` containing all findings sorted errors-first, then alphabetically
by challenge name.

```rust
let challenges = scanner.scan_directory(&base_dir)?;
let report = validate_challenges(&challenges);
report.print();
if report.has_errors() {
    // block deployment
}
```

Cross-challenge checks (duplicate names) run first, then per-challenge checks for
each entry.

---

### `ValidationReport`

```rust
pub struct ValidationReport {
    pub issues: Vec<Issue>,
}
```

| Method | Description |
|--------|-------------|
| `has_errors() -> bool` | True if any `Severity::Error` is present |
| `is_clean() -> bool` | True if `issues` is empty |
| `error_count() -> usize` | Number of errors |
| `warning_count() -> usize` | Number of warnings |
| `print()` | Pretty-prints all issues to stdout |

`print()` output format:
```
  ❌ ERROR   [challenge-name.field] message
  ⚠️  WARN   [challenge-name.field] message

  N error(s), M warning(s)
```

---

### `Issue`

```rust
pub struct Issue {
    pub severity: Severity,
    pub challenge: String,   // empty string for cross-challenge issues
    pub field: Option<String>,
    pub message: String,
}
```

---

### `Severity`

```rust
pub enum Severity { Error, Warning }
```

`Error < Warning` in ordering (errors sort first in the report).

---

## Checks Reference

### Cross-challenge checks

| Check | Severity | Description |
|-------|----------|-------------|
| Duplicate names | Error | Two or more challenges share the same `name` |

### Per-challenge checks

| Field | Check | Severity |
|-------|-------|----------|
| `name` | Empty or whitespace | Error |
| `category` | Empty or whitespace | Error |
| `value` | == 0 for `standard` type | Error |
| `extra` | Missing for `dynamic` type | Error |
| `extra.initial` | Missing or 0 for `dynamic` | Error |
| `extra.decay` | Missing or 0 for `dynamic` | Error |
| `extra.minimum` | Not set for `dynamic` | Warning |
| `description` | Missing or empty | Warning |
| `flags` | None defined | Error |
| `flags[i]` | Empty content | Error |
| `files` | Referenced path not found on disk | Error |
| `requirements` | Prerequisite is the challenge itself | Error |
| `requirements` | Named prerequisite not in local set | Warning |
| `next` | Points to the challenge itself | Error |
| `next` | Target not found in local set | Warning |

**Note on numeric requirement IDs**: prerequisites that parse as `u32` (e.g.
`- 1`) are assumed to reference remote CTFd IDs and are skipped during local
validation.

**Note on `requirements` warnings vs. errors**: missing prerequisites are
warnings rather than errors because they might exist on the remote CTFd instance
already (e.g. imported from a previous campaign). If strict enforcement is
desired, treat any warning as a failure in CI by checking the exit code and
`error_count()`.

---

## Integration with `deploy`

`deploy_challenges()` in `main.rs` runs `validate_challenges()` immediately after
scanning local challenges, before computing the diff or making any API call:

```
scan_directory()
    └─ validate_challenges()
           ├─ has_errors? → print report, return early (no API calls made)
           └─ warnings only? → print report, continue to diff
```

This ensures bad challenge data never reaches CTFd and gives actionable feedback
before any network I/O.

---

## Standalone `nervctf validate`

The `validate` subcommand does not require CTFd credentials — it is handled before
credential resolution in `main.rs`. Exit code is 1 when any error is present,
making it suitable for use in CI:

```sh
nervctf validate --base-dir ./challenges
echo $?   # 0 = clean or warnings only, 1 = errors present
```

---

## Extending the Validator

To add a new check, add a call to `validate_one()` in `validator.rs`. Use the
`Issue::error()` or `Issue::warn()` constructors:

```rust
// Error example
if some_condition {
    issues.push(Issue::error(name, "field_name", "human-readable message"));
}

// Warning example
if some_other_condition {
    issues.push(Issue::warn(name, "field_name", "human-readable message"));
}
```

For cross-challenge checks (e.g. checking that all `next:` targets form a DAG),
add logic in `validate_challenges()` before the per-challenge loop, using
`Issue::error_global()` for issues that don't belong to a single challenge.

---

## References

- `src/nervctf/src/validator.rs` — implementation
- `src/nervctf/src/main.rs` — `validate_command()`, pre-deploy integration
- [ctfcli challenge spec](https://github.com/CTFd/ctfcli)
