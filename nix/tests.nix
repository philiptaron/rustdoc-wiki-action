{
  lib,
  makeRustPlatform,
  toolchain,
  git,
}:

# Runs the whole test suite on the pinned nightly. The fixture tests generate rustdoc JSON, which
# only the nightly can do, so unlike the shipped package this is built with it too.
let
  rustPlatform = makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  };
in
rustPlatform.buildRustPackage {
  pname = "rustdoc-wiki-tests";
  inherit ((lib.importTOML ../Cargo.toml).package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../tests
      # `tests/links.rs` checks the links in the docs, the config and the workflows.
      ../README.md
      ../DESIGN.md
      ../LICENSE
      ../action.yml
      ../flake.nix
      ../nix
      ../.github
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeCheckInputs = [ git ];
  doCheck = true;

  # Never rewrite snapshots from inside a build.
  env.INSTA_UPDATE = "no";

  preCheck = ''
    # Cargo wants a writable home even when the fixture crates have no dependencies.
    export HOME=$TMPDIR
  '';

  installPhase = ''
    touch $out
  '';
}
