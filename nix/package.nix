{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "rustdoc-wiki";
  inherit ((lib.importTOML ../Cargo.toml).package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # The tests need the pinned nightly and git, so they run in `checks.tests` instead.
  doCheck = false;

  meta = {
    description = "Sync Rust documentation to a GitHub wiki";
    homepage = "https://github.com/philiptaron/rustdoc-wiki-action";
    license = lib.licenses.asl20;
    mainProgram = "rustdoc-wiki";
  };
}
