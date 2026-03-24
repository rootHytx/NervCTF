# NervCTF — Build & Release Targets
#
# remote-monitor only runs on Linux servers — it is NOT built for ARM,
# Windows, or macOS targets.  nervctf (the CLI) is built for every platform.
#
# All targets below use `nix develop` and are fully self-contained on NixOS —
# the flake provides Rust, C cross-compilers (musl64, aarch64, mingw64),
# pkg-config, and OpenSSL automatically.
#
# Exception: macOS targets require a local toolchain on an actual macOS machine
# (Apple's SDK is not redistributable and cannot be cross-compiled from Linux):
#   rustup target add x86_64-apple-darwin    (macOS only)
#   rustup target add aarch64-apple-darwin   (macOS only)

.DEFAULT_GOAL := help

BINARY_A  := nervctf
BINARY_B  := remote-monitor

NIX := nix develop .\# --command

# ── Targets ───────────────────────────────────────────────────────────────────

.PHONY: help \
        release-nix \
        release-linux release-musl release-arm64 \
        release-macos release-macos-arm \
        release-windows \
        all all-linux all-platforms \
        install install-user \
        fmt check test \
        clean

help: ## Show this help message
	@printf "\033[1mNervCTF build targets\033[0m\n\n"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "  Use 'make all' to build everything cross-compilable from NixOS."

# ── Nix (dev build) ───────────────────────────────────────────────────────────

release-nix: ## [Nix] Native release build — both binaries, no cross
	$(NIX) cargo build --release
	cp target/release/$(BINARY_A) .
	cp target/release/$(BINARY_B) .

# ── Linux (both binaries) ─────────────────────────────────────────────────────

release-linux: ## x86_64 Linux GNU — both binaries
	$(NIX) cargo build --release --target x86_64-unknown-linux-gnu
	@mkdir -p dist
	cp target/x86_64-unknown-linux-gnu/release/$(BINARY_A) dist/$(BINARY_A)-linux-x86_64
	cp target/x86_64-unknown-linux-gnu/release/$(BINARY_B) dist/$(BINARY_B)-linux-x86_64
	@upx --best dist/$(BINARY_A)-linux-x86_64 dist/$(BINARY_B)-linux-x86_64 2>/dev/null || true
	@echo "→ dist/$(BINARY_A)-linux-x86_64  dist/$(BINARY_B)-linux-x86_64"

release-musl: ## x86_64 Linux musl static — both binaries
	$(NIX) cargo build --release --target x86_64-unknown-linux-musl
	@mkdir -p dist
	cp target/x86_64-unknown-linux-musl/release/$(BINARY_A) dist/$(BINARY_A)-linux-x86_64-static
	cp target/x86_64-unknown-linux-musl/release/$(BINARY_B) dist/$(BINARY_B)-linux-x86_64-static
	@upx --best dist/$(BINARY_A)-linux-x86_64-static dist/$(BINARY_B)-linux-x86_64-static 2>/dev/null || true
	@echo "→ dist/$(BINARY_A)-linux-x86_64-static  dist/$(BINARY_B)-linux-x86_64-static"

# ── ARM (nervctf CLI only — remote-monitor is x86_64 Linux only) ──────────────

release-arm64: ## aarch64 Linux GNU — nervctf CLI only
	$(NIX) cargo build --release --target aarch64-unknown-linux-gnu -p nervctf
	@mkdir -p dist
	cp target/aarch64-unknown-linux-gnu/release/$(BINARY_A) dist/$(BINARY_A)-linux-aarch64
	@echo "→ dist/$(BINARY_A)-linux-aarch64"

# ── Windows (nervctf CLI only) ────────────────────────────────────────────────

release-windows: ## x86_64 Windows GNU — nervctf CLI only (MinGW via nix develop)
	$(NIX) cargo build --release --target x86_64-pc-windows-gnu -p nervctf
	@mkdir -p dist
	cp target/x86_64-pc-windows-gnu/release/$(BINARY_A).exe dist/$(BINARY_A)-windows-x86_64.exe
	@upx --best dist/$(BINARY_A)-windows-x86_64.exe 2>/dev/null || true
	@echo "→ dist/$(BINARY_A)-windows-x86_64.exe"

# ── macOS (nervctf CLI only — requires macOS machine) ────────────────────────

release-macos: ## x86_64 macOS — nervctf CLI only (must run on macOS)
	cargo build --release --target x86_64-apple-darwin -p nervctf
	@mkdir -p dist
	cp target/x86_64-apple-darwin/release/$(BINARY_A) dist/$(BINARY_A)-macos-x86_64
	@echo "→ dist/$(BINARY_A)-macos-x86_64"

release-macos-arm: ## aarch64 macOS — nervctf CLI only (must run on macOS)
	cargo build --release --target aarch64-apple-darwin -p nervctf
	@mkdir -p dist
	cp target/aarch64-apple-darwin/release/$(BINARY_A) dist/$(BINARY_A)-macos-aarch64
	@echo "→ dist/$(BINARY_A)-macos-aarch64"

# ── Aggregate ─────────────────────────────────────────────────────────────────

all-linux: release-linux release-musl ## Linux x86_64: GNU + musl static (both binaries)

all: all-linux release-arm64 release-windows ## Everything buildable from NixOS (Linux + ARM + Windows CLI)

all-platforms: all release-macos release-macos-arm ## All targets — macOS requires running on macOS

# ── Install ───────────────────────────────────────────────────────────────────

install: release-nix ## Install both binaries to /usr/local/bin (requires sudo)
	sudo install -m 755 $(BINARY_A) /usr/local/bin/$(BINARY_A)
	sudo install -m 755 $(BINARY_B) /usr/local/bin/$(BINARY_B)
	@echo "Installed to /usr/local/bin/"

install-user: release-nix ## Install both binaries to ~/.local/bin (no sudo required)
	@mkdir -p ~/.local/bin
	install -m 755 $(BINARY_A) ~/.local/bin/$(BINARY_A)
	install -m 755 $(BINARY_B) ~/.local/bin/$(BINARY_B)
	@echo "Installed to ~/.local/bin/ — ensure this is in your PATH"

# ── Dev helpers ───────────────────────────────────────────────────────────────

fmt: ## Format all crates with rustfmt
	$(NIX) cargo fmt --all

check: ## Run cargo check on all workspace crates
	$(NIX) cargo check --all

test: ## Run nervctf unit tests
	$(NIX) cargo test -p nervctf

# ── Clean ─────────────────────────────────────────────────────────────────────

clean: ## Remove build artifacts, dist/, and root-level binaries
	cargo clean
	rm -f $(BINARY_A) $(BINARY_B)
	rm -rf dist/
