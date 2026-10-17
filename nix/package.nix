# semsearch. cpuArch is the LLVM CPU name to compile for; nixcfg passes its
# recorded `mine.cpu.arch`, and null builds for the x86-64 baseline.
{
  lib,
  stdenv,
  rustPlatform,
  git,
  ripgrep,
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

  env = lib.optionalAttrs (cpuArch != null) {
    "CARGO_TARGET_${stdenv.hostPlatform.rust.cargoEnvVarTarget}_RUSTFLAGS" = "-C target-cpu=${cpuArch}";
  };

  meta = {
    description = "Local semantic search over code, files and web history, through the hister fork";
    mainProgram = "semsearch";
    platforms = lib.platforms.linux;
  };
}
