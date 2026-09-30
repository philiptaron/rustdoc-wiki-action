{
  runCommand,
  jq,
  toolchain,
  rustdoc-wiki,
}:

# The nightly pinned in flake.nix and the `rustdoc-types` version in Cargo.toml must be bumped
# together. This fails, with instructions, when they drift apart.
runCommand "check-format-version"
  {
    nativeBuildInputs = [
      toolchain
      rustdoc-wiki
      jq
    ];
  }
  ''
    export HOME=$TMPDIR CARGO_TARGET_DIR=$TMPDIR/target
    cp -r ${../tests/fixtures/basic} crate
    chmod -R u+w crate
    cd crate
    cargo rustdoc --lib -- -Z unstable-options --output-format json

    json=$(find "$CARGO_TARGET_DIR" -name basic.json | head -n 1)
    emitted=$(jq .format_version "$json")
    supported=$(rustdoc-wiki format-version)
    if [ "$emitted" != "$supported" ]; then
      echo "The pinned nightly emits rustdoc JSON format_version $emitted, but this build of" >&2
      echo "rustdoc-types supports $supported. Bump the nightly date in flake.nix and the" >&2
      echo "rustdoc-types version in Cargo.toml together. Why:" >&2
      echo "https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/DESIGN.md#toolchain-coupling" >&2
      exit 1
    fi
    touch $out
  ''
