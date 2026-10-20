# Developer notes

Build details, gotchas, and implementation notes that are not obvious from the code. Routes and data models live in [`remote-monitor.md`](remote-monitor.md) and [`ctfd_api.md`](ctfd_api.md); this file does not repeat them.

## Build

Use the Nix devShell for every cargo command:

```sh
nix develop .# --command cargo build --release \
    --target x86_64-unknown-linux-musl -p nervctf -p remote-monitor
nix develop .# --command cargo test
```

The shell provides rustc, cargo, rustfmt, clippy, pkg-config, OpenSSL, and Ansible. Verify with the musl release build, not a native debug build.

## Cross-compilation

Each cross-stdenv sets `CC` globally, and the last one wins. Pin every target through `CC_<triple>` and reset `CC` to native gcc in the shell hook; `CC_x86_64_unknown_linux_gnu` is mandatory. Without it, ring and cc-rs emit COFF objects for ELF targets.

For Windows, inject `libpthread.a` via `NIX_LDFLAGS_x86_64_w64_mingw32`. `RUSTFLAGS -L` and `-C link-arg` do not reach the external cross-linker.

## Configuration

Precedence: CLI flags > `MONITOR_URL` / `MONITOR_TOKEN` > `.nervctf.yml`. `load_config` walks up from the current directory; `save_config` writes mode `0600` because the file can hold the admin password.

## CTFd deployment requirements

- Challenge visibility must be **Public**. Private mode blocks even valid API tokens from `GET /api/v1/challenges`.
- The uploads directory needs `chown -R 1001:1001 <CTFd>/.data/CTFd/uploads`, or `POST /api/v1/files` returns 500.
- Upload every file for a challenge in one multipart request (multiple `file` parts). One request per file returns 500.
- `GET /api/v1/challenges` paginates at 20. `get_challenges()` follows `meta.pagination.next`; without that, re-deploys duplicate challenges.

## CTFd schema and passwords

- CTFd builds have shipped both `configs` and `config` as the settings table. `ctfd_db::config_table()` checks `information_schema`; the plugin uses the ORM.
- Some builds store passwords with passlib `bcrypt_sha256` (`$bcrypt-sha256$...`) and hash on assignment through the `Users.password` setter. `_maybe_complete_setup()` probes a transient `Users` instance and either passes plaintext or hashes with `bcrypt_sha256`.
- `Configs` in some builds has no `type` attribute. The plugin sets only `key` and `value`.
- `challenges.next_id` and the inline scoring columns are absent on some versions; `ctfd_db.rs` detects them and substitutes NULL/omits them. See the compatibility matrix in [`ctfd-dependency-audit.md`](ctfd-dependency-audit.md).

## Implementation notes

- Instance names are `ctf-<sanitized>-<6 random chars>`; the random suffix prevents orphan-checker races.
- `pick_free_ports` allocates atomically in 40000–60000 from the `allocated` set and the DB's used ports, so ports within one instance never collide.
- `extra_ports` is `None` for single-port and `{"<internal>": <host>}` for multi-port.
- Compose `up()` receives fully resolved `service_mappings: &HashMap<String, Vec<(u16, u32)>>`; `compose.rs` does not parse the config format.
- `CHALLENGES_BASE_DIR` must be identical on the host and in the monitor container. Challenge compose bind mounts use the host path.
- The monitor writes lifecycle rows to `events`; flag submissions stay in `flag_attempts` to avoid a second write on the hot path.
- The upgrade playbook recreates the whole Compose stack, not just `ctfd`. A restart leaves the old upstream IP in nginx and produces 502s.

## Setup

`setup.rs` finds the plugin next to the executable, in the workspace, or from the embedded copy (extracted to `$XDG_CACHE_HOME/nervctf/ctfd-plugin`). It generates the monitor token from `/dev/urandom`, resolves CTFd's published HTTP port from Compose, runs the preflight, and writes `.nervctf.yml` before provisioning. It prompts for the first admin and passes `NERVCTF_ADMIN_*` / `NERVCTF_EVENT_*` to the playbooks.

See `git log` for historical changes.
