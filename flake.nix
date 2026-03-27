{
  description = "NervCTF — CTF challenge deployment and sync tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # crane: builds cargo deps as a separate cached derivation.
    # Deps are only recompiled when Cargo.lock changes; source changes
    # only recompile the app crates.
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
          config.allowUnsupportedSystem = true;
        };

        # Rust toolchain with all cross-compilation targets pre-installed
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [
            "x86_64-unknown-linux-gnu"
            "x86_64-unknown-linux-musl"
            "aarch64-unknown-linux-gnu"
            "x86_64-pc-windows-gnu"
          ];
        };

        # C cross-compilation toolchains (linker + C compiler for each target)
        musl64 = pkgs.pkgsCross.musl64;
        aarch64 = pkgs.pkgsCross.aarch64-multiplatform;
        mingw64 = pkgs.pkgsCross.mingwW64;

        commonBuildInputs = with pkgs; [
          openssl
          sqlite
        ];
        commonNativeBuildInputs = [
          pkgs.pkg-config
          rustToolchain
        ];
        commonEnv = {
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.sqlite.dev}/lib/pkgconfig";
          LIBRARY_PATH = "${pkgs.sqlite}/lib";
        };

        # ── crane setup ──────────────────────────────────────────────────────
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Source filter: crane's default keeps only *.rs + Cargo files, but
        # nervctf embeds assets (playbook, compose, sh) via include_str!.
        # Extend the filter to keep those too.
        src = pkgs.lib.cleanSourceWith {
          src = craneLib.path ./.;
          filter =
            path: type:
            (craneLib.filterCargoSources path type)
            || (builtins.match ".*\\.yml$" path != null)
            || (builtins.match ".*\\.yaml$" path != null)
            || (builtins.match ".*\\.sh$" path != null)
            || (builtins.match ".*\\.html$" path != null)
            || (builtins.match ".*\\.css$" path != null);
        };

        commonArgs = commonEnv // {
          inherit src;
          buildInputs = commonBuildInputs;
          nativeBuildInputs = commonNativeBuildInputs;
          cargoExtraArgs = "--offline";
          # Silence "could not find system library 'openssl'" during dep build
          OPENSSL_NO_VENDOR = "1";
        };

        # Build ALL workspace dependencies once.
        # This derivation is cached by Nix and only rebuilt when Cargo.lock changes.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        nervctf = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            pname = "nervctf";
            version = "2.2.1";
            cargoExtraArgs = "--package nervctf";
          }
        );

        remote-monitor = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            pname = "remote-monitor";
            version = "2.2.1";
            cargoExtraArgs = "--package remote-monitor";
            doCheck = false;
          }
        );
      in
      {
        # `nix build`                  → nervctf binary
        # `nix build .#remote-monitor` → remote-monitor binary
        packages.default = nervctf;
        packages.nervctf = nervctf;
        packages.remote-monitor = remote-monitor;

        # `nix develop` → full dev shell with sccache for fast cargo builds
        devShells.default = pkgs.mkShell (
          commonEnv
          // {
            name = "nervctf-dev";
            packages = [
              rustToolchain
              pkgs.pkg-config
              pkgs.openssl
              pkgs.openssl.dev
              pkgs.ansible
              pkgs.sccache
              pkgs.upx
              pkgs.sqlite
              pkgs.sqlite.dev
              # C cross-compilers (linker + cc for ring/cc-rs build scripts)
              musl64.stdenv.cc
              aarch64.stdenv.cc
              mingw64.stdenv.cc
            ];
            # Pin every target's C compiler so the cross-compiler setup hooks
            # (which each overwrite CC) don't pollute native builds.
            CC_x86_64_unknown_linux_gnu = "${pkgs.stdenv.cc}/bin/${pkgs.stdenv.cc.targetPrefix}cc";
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = "${pkgs.stdenv.cc}/bin/${pkgs.stdenv.cc.targetPrefix}cc";

            CC_x86_64_unknown_linux_musl = "${musl64.stdenv.cc}/bin/${musl64.stdenv.cc.targetPrefix}cc";
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${musl64.stdenv.cc}/bin/${musl64.stdenv.cc.targetPrefix}cc";

            CC_aarch64_unknown_linux_gnu = "${aarch64.stdenv.cc}/bin/${aarch64.stdenv.cc.targetPrefix}cc";
            CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = "${aarch64.stdenv.cc}/bin/${aarch64.stdenv.cc.targetPrefix}cc";

            CC_x86_64_pc_windows_gnu = "${mingw64.stdenv.cc}/bin/${mingw64.stdenv.cc.targetPrefix}cc";
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${mingw64.stdenv.cc}/bin/${mingw64.stdenv.cc.targetPrefix}cc";

            shellHook = ''
              # Restore CC to native GCC (cross-compiler hooks each overwrite it)
              export CC="${pkgs.stdenv.cc}/bin/${pkgs.stdenv.cc.targetPrefix}cc"

              # sccache: cache compiled Rust crates across cargo invocations.
              # The cache lives in ~/.cache/sccache (up to 10 GB by default).
              export RUSTC_WRAPPER="${pkgs.sccache}/bin/sccache"
              echo "sccache enabled — $(sccache --show-stats 2>/dev/null | grep -E 'Cache (hits|misses)' || echo 'stats unavailable')"

              # Provide libpthread.a for x86_64-pc-windows-gnu cross builds.
              # Preferred: official winpthreads (pkgsCross.mingwW64.windows.pthreads).
              # Fallback: mcfgthread (pthreads-compatible, already in store as a
              #   transitive dep of mingw64.stdenv.cc).  We symlink it into a stable
              #   temp dir so the MinGW linker can find it via -l:libpthread.a.
              _pthread=$(find /nix/store -maxdepth 4 -name "libpthread.a" \
                           -path "*x86_64-w64-mingw32*" 2>/dev/null | head -1)
              if [ -n "$_pthread" ]; then
                export NIX_LDFLAGS_x86_64_w64_mingw32="-L$(dirname "$_pthread")"
              else
                _mcf=$(find /nix/store -maxdepth 2 \
                         -name "*mcfgthread-x86_64-w64-mingw32-*" \
                         -not -name "*-dev" -not -name "*.drv" -type d 2>/dev/null | head -1)
                if [ -n "$_mcf" ] && [ -f "$_mcf/lib/libmcfgthread.a" ]; then
                  mkdir -p /tmp/nervctf-win-libs
                  ln -sf "$_mcf/lib/libmcfgthread.a" /tmp/nervctf-win-libs/libpthread.a
                  export NIX_LDFLAGS_x86_64_w64_mingw32="-L/tmp/nervctf-win-libs"
                  echo "ℹ️   Windows pthreads: using mcfgthread as libpthread.a shim"
                else
                  echo "⚠️  libpthread.a not in store — Windows build may fail."
                  echo "   Run:  nix-shell -p 'pkgsCross.mingwW64.windows.pthreads'"
                fi
                unset _mcf
              fi
              unset _pthread
            '';
          }
        );
      }
    );
}
