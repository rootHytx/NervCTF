# `validator` Module Documentation

## Overview

The `validator` module performs static analysis over parsed `Challenge` structs. It runs
automatically before every `deploy` (errors block deployment) and is exposed as `nervctf validate`.

---

## Architecture

```
src/validator.rs
├── Severity          (Error | Warning)
├── Issue             (per-finding record)
├── ValidationReport  (result set + print helper)
├── RENDERED          (set of known field paths — suppresses unknown-key warnings)
├── validate_challenges()   ← public entry point
└── validate_one()          ← per-challenge checks (private)
```

---

## Public API

### `validate_challenges(base_dir: &Path, fix_mode: bool) -> Result<()>`

Scans `base_dir` via `DirectoryScanner`, validates all found challenges, prints the report,
and returns `Err` if any errors are present.

```sh
nervctf validate --base-dir ./challenges
echo $?   # 0 = clean or warnings only, 1 = errors present
```

---

### `ValidationReport`

| Method | Description |
|--------|-------------|
| `has_errors() -> bool` | True if any `Severity::Error` is present |
| `is_clean() -> bool` | True if `issues` is empty |
| `error_count() -> usize` | |
| `warning_count() -> usize` | |
| `print()` | Pretty-prints to stdout |

Output format:
```
  ❌ ERROR   [challenge-name.field] message
  ⚠️  WARN   [challenge-name.field] message

  N error(s), M warning(s)
```

---

## Checks Reference

### Cross-challenge

| Check | Severity |
|-------|----------|
| Duplicate challenge names | Error |

### Per-challenge

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
| `flags` | None defined (for non-random-instance challenges) | Error |
| `flags[i]` | Empty content | Error |
| `files` | Referenced path not found on disk | Error |
| `requirements` | Prerequisite is the challenge itself | Error |
| `requirements` | Named prerequisite not in local set | Warning |
| `next` | Points to the challenge itself | Error |
| `next` | Target not found in local set | Warning |
| `unknown_yaml_keys` | Unrecognised top-level key | Warning |

### `type: instance` specific checks

| Field | Check | Severity |
|-------|-------|----------|
| `instance` | Block missing | Error |
| `instance.internal_port` | Missing or 0 | Error |
| `instance.connection` | Missing or empty | Error |
| `instance.flag_mode: random` | `flags:` list also present (conflict) | Warning |
| `instance.backend: docker` | `image` missing or empty | Error |
| `instance.backend: lxc` | `lxc_image` missing or empty | Error |
| `instance.backend: vagrant` | `vagrantfile` missing or empty | Error |
| `instance.backend: compose` | No `compose_service` set | Warning |
| `instance.flag_delivery: file` | `flag_file_path` not set | Warning |

**Notes:**
- Numeric requirement IDs (e.g. `- 1`) are skipped during local validation (assumed remote CTFd IDs).
- Missing prerequisites are warnings (not errors) because they may already exist on the remote instance.

---

## `RENDERED` constant

A `HashSet<&str>` of all known field paths. Any top-level YAML key not in this set (and not in
`RENDERED`) produces an `unknown_yaml_keys` warning. Includes all `instance.*` subfields:
`instance.flag_delivery`, `instance.flag_file_path`, `instance.flag_service`,
`instance.flag_prefix`, `instance.flag_suffix`, `instance.random_flag_length`,
`instance.compose_file`, `instance.compose_service`, `instance.lxc_image`,
`instance.vagrantfile`, `instance.timeout_minutes`,
`instance.max_renewals`, `instance.command`.

---

## Integration with `deploy`

```
scan_directory()
    └─ validate_challenges()
           ├─ has_errors? → print report, return early (no API calls made)
           └─ warnings only? → print report, continue to diff
```

---

## Extending

Add calls in `validate_one()` for per-challenge checks, or in `validate_challenges()` for
cross-challenge checks:

```rust
if some_condition {
    issues.push(Issue::error(name, "field_name", "message"));
}
if other_condition {
    issues.push(Issue::warn(name, "field_name", "message"));
}
```

For global issues (not tied to a single challenge), use `Issue::error_global()`.

---

## References

- `src/nervctf/src/validator.rs`
- `src/nervctf/src/main.rs` — `validate_command()`, pre-deploy integration
