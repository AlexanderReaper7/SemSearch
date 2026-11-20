# semsearch. cpuArch is the LLVM CPU name to compile for; nixcfg passes its
# recorded `mine.cpu.arch`, and null builds for the x86-64 baseline.
{
  lib,
  stdenv,
  rustPlatform,
  git,
  ripgrep,
  sqlite,
  cpuArch ? null,
}:
rustPlatform.buildRustPackage {
  pname = "semsearch";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  # ask's tests run its grep and file tools, which call rg and git.
  nativeCheckInputs = [
    git
    ripgrep
  ];

  # The index links nixpkgs' SQLite, not the copy that rusqlite's "bundled"
  # feature compiles, which stays for cargo builds outside Nix. nixpkgs'
  # cargoSetupHook exports LIBSQLITE3_SYS_USE_PKG_CONFIG itself since a change
  # between 2026-10-08 and 2026-11-17 (dontCargoUseSystemLibs in the 26.11
  # release notes), and without sqlite in buildInputs the link then failed with
  # "cannot find -lsqlite3". Set here too, so every nixpkgs links the same
  # SQLite. "bundled" keeps the bindings of its own version (3.53.2 in
  # libsqlite3-sys 0.38.2), so nixpkgs' sqlite must be at least that version.
  buildInputs = [ sqlite ];

  env = {
    LIBSQLITE3_SYS_USE_PKG_CONFIG = "1";
  }
  // lib.optionalAttrs (cpuArch != null) {
    "CARGO_TARGET_${stdenv.hostPlatform.rust.cargoEnvVarTarget}_RUSTFLAGS" = "-C target-cpu=${cpuArch}";
  };

  meta = {
    description = "Local semantic search over code, files and web history, through the hister fork";
    mainProgram = "semsearch";
    platforms = lib.platforms.linux;
  };
}
