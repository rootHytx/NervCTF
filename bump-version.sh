#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 <new-version>"
    echo "Example: $0 2.2.0"
    echo ""
    echo "Updates version in:"
    echo "  Cargo.toml (workspace)"
    echo "  src/nervctf/Cargo.toml"
    echo "  src/remote-monitor/Cargo.toml"
    echo "  flake.nix (both derivations)"
    echo "  src/nervctf/src/main.rs (--version string)"
    exit 1
}

[ $# -ne 1 ] && usage

NEW="$1"

if ! [[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: version must be X.Y.Z (got: $NEW)" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

CURRENT="$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')"

if [ "$CURRENT" = "$NEW" ]; then
    echo "already at $NEW, nothing to do."
    exit 0
fi

echo "$CURRENT -> $NEW"

# Cargo.toml files: only lines that start with `version = ` (the [package] declaration,
# not indented dependency specs)
sed -i 's|^version = ".*"|version = "'"$NEW"'"|' Cargo.toml
sed -i 's|^version = ".*"|version = "'"$NEW"'"|' src/nervctf/Cargo.toml
sed -i 's|^version = ".*"|version = "'"$NEW"'"|' src/remote-monitor/Cargo.toml

# flake.nix: `version = "X.Y.Z";`  (semicolon distinguishes it from Cargo/TOML syntax)
sed -i 's|version = "[0-9]\+\.[0-9]\+\.[0-9]\+";|version = "'"$NEW"'";|g' flake.nix

# main.rs clap attribute: #[command(version = "X.Y.Z")]
sed -i 's|#\[command(version = "[0-9]\+\.[0-9]\+\.[0-9]\+")\]|#[command(version = "'"$NEW"'")]|' \
    src/nervctf/src/main.rs

echo "done."
