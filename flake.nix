{
  description = "NervCTF — CTF challenge deployment and sync tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
          # allowUnsupportedSystem lets us access Windows packages (e.g. winpthreads)
          # from Linux for cross-compilation purposes.
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
        musl64  = pkgs.pkgsCross.musl64;
        aarch64 = pkgs.pkgsCross.aarch64-multiplatform;
        mingw64 = pkgs.pkgsCross.mingwW64;


        # Used both for nix packages and devShell
        commonBuildInputs      = with pkgs; [ openssl ];
        commonNativeBuildInputs = [ pkgs.pkg-config rustToolchain ];
        commonEnv = {
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
        };

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        nervctf = rustPlatform.buildRustPackage (commonEnv // {
          pname = "nervctf";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          buildInputs = commonBuildInputs;
          nativeBuildInputs = commonNativeBuildInputs;
          cargoBuildFlags = [ "--package" "nervctf" ];
          cargoTestFlags = [ "--package" "nervctf" ];
        });

        remote-monitor = rustPlatform.buildRustPackage (commonEnv // {
          pname = "remote-monitor";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          buildInputs = commonBuildInputs;
          nativeBuildInputs = commonNativeBuildInputs;
          cargoBuildFlags = [ "--package" "remote-monitor" ];
          doCheck = false;
        });
      in
      {
        # `nix shell`               → nervctf on PATH
        # `nix shell .#remote-monitor` → remote-monitor on PATH
        packages.default       = nervctf;
        packages.nervctf       = nervctf;
        packages.remote-monitor = remote-monitor;

        # `nix develop` → full dev shell: Rust (all targets) + C cross toolchains + Ansible
        devShells.default = pkgs.mkShell (commonEnv // {
          name = "nervctf-dev";
          packages = [
            rustToolchain
            pkgs.pkg-config
            pkgs.openssl
            pkgs.openssl.dev
            pkgs.ansible
            # C cross-compilers (linker + cc for ring/cc-rs build scripts)
            musl64.stdenv.cc
            aarch64.stdenv.cc
            mingw64.stdenv.cc
          ];
          # Tell cc-rs and cargo which C compiler/linker to use per target.
          # The cross-compiler setup hooks (musl64, aarch64, mingw64) each set
          # CC to their own cross-compiler; the last one (mingw64) wins.  We
          # must therefore pin every target's C compiler explicitly so that
          # ring/cc-rs never falls back to the polluted generic CC.
          CC_x86_64_unknown_linux_gnu =
            "${pkgs.stdenv.cc}/bin/${pkgs.stdenv.cc.targetPrefix}cc";
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER =
            "${pkgs.stdenv.cc}/bin/${pkgs.stdenv.cc.targetPrefix}cc";

          CC_x86_64_unknown_linux_musl =
            "${musl64.stdenv.cc}/bin/${musl64.stdenv.cc.targetPrefix}cc";
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER =
            "${musl64.stdenv.cc}/bin/${musl64.stdenv.cc.targetPrefix}cc";

          CC_aarch64_unknown_linux_gnu =
            "${aarch64.stdenv.cc}/bin/${aarch64.stdenv.cc.targetPrefix}cc";
          CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER =
            "${aarch64.stdenv.cc}/bin/${aarch64.stdenv.cc.targetPrefix}cc";

          CC_x86_64_pc_windows_gnu =
            "${mingw64.stdenv.cc}/bin/${mingw64.stdenv.cc.targetPrefix}cc";
          CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER =
            "${mingw64.stdenv.cc}/bin/${mingw64.stdenv.cc.targetPrefix}cc";

          # ring 0.16 links -l:libpthread.a on Windows/GNU targets.
          # The winpthreads derivation can't always be built from this nixpkgs
          # pin (stdenv-linux bootstrap issue), so we locate it at shell-entry
          # time from whatever is already in the store.
          shellHook = ''
            # The cross-compiler setup hooks set CC to their own compilers;
            # restore CC to the native Linux GCC so host-targeted tools work.
            export CC="${pkgs.stdenv.cc}/bin/${pkgs.stdenv.cc.targetPrefix}cc"

            _pthread=$(find /nix/store -maxdepth 3 -name "libpthread.a" \
                         -path "*x86_64-w64-mingw32*" 2>/dev/null | head -1)
            if [ -n "$_pthread" ]; then
              # NIX_LDFLAGS_<target> is read by the Nix gcc-wrapper and appended
              # to every linker invocation for that target, which is how we
              # inject the winpthreads search path without patching ring.
              export NIX_LDFLAGS_x86_64_w64_mingw32="-L$(dirname "$_pthread")"
            else
              echo "⚠️  libpthread.a not in store — Windows build may fail."
              echo "   Run:  nix-shell -p 'pkgsCross.mingwW64.windows.pthreads'"
            fi
            unset _pthread
          '';
        });
      });
}
