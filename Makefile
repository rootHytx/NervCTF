# NervCTF — Build & Release Targets
#
# Targets using `nix develop` (release-nix, release-musl, release-arm64,
# release-windows, fmt, check, test) are fully self-contained — the flake
# provides Rust, C cross-compilers, pkg-config, and OpenSSL automatically.
#
# macOS targets (release-macos, release-macos-arm) require a local toolchain
# on an actual macOS machine — Apple's SDK is not redistributable and cannot
# be cross-compiled from Linux:
#   rustup target add x86_64-apple-darwin    (macOS only)
#   rustup target add aarch64-apple-darwin   (macOS only)
#
# Linux and Windows cross-compilation (release-linux, release-musl,
# release-arm64, release-windows) is fully handled by nix develop.

.DEFAULT_GOAL := help

BINARY_A  := nervctf
BINARY_B  := remote-monitor

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
	@echo "  Cross-compilation uses 'cross' when available (requires Docker)."
	@echo "  Without cross, the Rust target must already be installed via rustup."

# ── Nix (recommended) ─────────────────────────────────────────────────────────

release-nix: ## [Nix] Release build inside nix dev shell — all deps bundled
	nix develop .# --command cargo build --release
	cp target/release/$(BINARY_A) .
	cp target/release/$(BINARY_B) .

# ── Linux ─────────────────────────────────────────────────────────────────────

release-linux: ## [Debian/Ubuntu/Fedora/Arch/RHEL] x86_64 Linux GNU binary
	cargo build --release --target x86_64-unknown-linux-gnu
	@mkdir -p dist
	cp target/x86_64-unknown-linux-gnu/release/$(BINARY_A) dist/$(BINARY_A)-linux-x86_64
	cp target/x86_64-unknown-linux-gnu/release/$(BINARY_B) dist/$(BINARY_B)-linux-x86_64
	@upx --best dist/$(BINARY_A)-linux-x86_64 dist/$(BINARY_B)-linux-x86_64 2>/dev/null || true
	@echo "→ dist/$(BINARY_A)-linux-x86_64"

release-musl: ## [Alpine/containers/any Linux] Static x86_64 binary — via nix develop
	nix develop .# --command cargo build --release --target x86_64-unknown-linux-musl
	@mkdir -p dist
	cp target/x86_64-unknown-linux-musl/release/$(BINARY_A) dist/$(BINARY_A)-linux-x86_64-static
	cp target/x86_64-unknown-linux-musl/release/$(BINARY_B) dist/$(BINARY_B)-linux-x86_64-static
	@upx --best dist/$(BINARY_A)-linux-x86_64-static dist/$(BINARY_B)-linux-x86_64-static 2>/dev/null || true
	@echo "→ dist/$(BINARY_A)-linux-x86_64-static"

release-arm64: ## [Raspberry Pi 4/5, AWS Graviton, Oracle ARM] aarch64 GNU — via nix develop
	nix develop .# --command cargo build --release --target aarch64-unknown-linux-gnu
	@mkdir -p dist
	cp target/aarch64-unknown-linux-gnu/release/$(BINARY_A) dist/$(BINARY_A)-linux-aarch64
	cp target/aarch64-unknown-linux-gnu/release/$(BINARY_B) dist/$(BINARY_B)-linux-aarch64
	@echo "→ dist/$(BINARY_A)-linux-aarch64"

all-linux: release-linux release-musl release-arm64 ## Build all Linux release targets

# ── macOS ─────────────────────────────────────────────────────────────────────

release-macos: ## [macOS Intel] x86_64 Apple Darwin binary
	cargo build --release --target x86_64-apple-darwin
	@mkdir -p dist
	cp target/x86_64-apple-darwin/release/$(BINARY_A) dist/$(BINARY_A)-macos-x86_64
	cp target/x86_64-apple-darwin/release/$(BINARY_B) dist/$(BINARY_B)-macos-x86_64
	@echo "→ dist/$(BINARY_A)-macos-x86_64"

release-macos-arm: ## [macOS Apple Silicon] aarch64 Apple Darwin binary
	cargo build --release --target aarch64-apple-darwin
	@mkdir -p dist
	cp target/aarch64-apple-darwin/release/$(BINARY_A) dist/$(BINARY_A)-macos-aarch64
	cp target/aarch64-apple-darwin/release/$(BINARY_B) dist/$(BINARY_B)-macos-aarch64
	@echo "→ dist/$(BINARY_A)-macos-aarch64"

# ── Windows ───────────────────────────────────────────────────────────────────

release-windows: ## [Windows] x86_64 GNU Windows binary — via nix develop (MinGW)
	nix develop .# --command cargo build --release --target x86_64-pc-windows-gnu
	@mkdir -p dist
	cp target/x86_64-pc-windows-gnu/release/$(BINARY_A).exe dist/$(BINARY_A)-windows-x86_64.exe
	cp target/x86_64-pc-windows-gnu/release/$(BINARY_B).exe dist/$(BINARY_B)-windows-x86_64.exe
	@upx --best dist/$(BINARY_A)-windows-x86_64.exe dist/$(BINARY_B)-windows-x86_64.exe 2>/dev/null || true
	@echo "→ dist/$(BINARY_A)-windows-x86_64.exe"

# ── All platforms ─────────────────────────────────────────────────────────────

all: all-linux release-windows ## Build all Linux + Windows targets (cross-compilable from Linux via nix develop)

all-platforms: all release-macos release-macos-arm ## Build every target — macOS targets require running on macOS (Apple SDK not redistributable)

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
	nix develop .# --command cargo fmt --all

check: ## Run cargo check on all workspace crates
	nix develop .# --command cargo check --all

test: ## Run nervctf unit tests
	nix develop .# --command cargo test -p nervctf

# ── Clean ─────────────────────────────────────────────────────────────────────

clean: ## Remove build artifacts, dist/, and root-level binaries
	cargo clean
	rm -f $(BINARY_A) $(BINARY_B)
	rm -rf dist/
