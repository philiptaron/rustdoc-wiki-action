{
  description = "Sync Rust documentation to a GitHub wiki";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      # The nightly that emits the rustdoc JSON we render. Its `format_version` must equal
      # `rustdoc_types::FORMAT_VERSION` for the `rustdoc-types` release in Cargo.toml; the
      # `format-version` check enforces that. Bump the two together; see
      # https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/DESIGN.md#toolchain-coupling
      nightly = "2026-09-29";

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      packages = forAllSystems (
        pkgs:
        rec {
          # cargo + rustc + rustdoc of the pinned nightly. The action puts this on PATH for the
          # `cargo rustdoc` child process only.
          toolchain = pkgs.rust-bin.nightly.${nightly}.minimal;

          rustdoc-wiki = pkgs.callPackage ./nix/package.nix { };
          default = rustdoc-wiki;
        }
        // nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          # What the action downloads: a static binary that runs on any runner, without Nix. The
          # output check fails the build if a store path ever gets embedded in it.
          rustdoc-wiki-static = (pkgs.pkgsStatic.callPackage ./nix/package.nix { }).overrideAttrs (_: {
            __structuredAttrs = true;
            outputChecks.out.allowedReferences = [ ];
          });
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            (pkgs.rust-bin.nightly.${nightly}.minimal.override {
              extensions = [
                "clippy"
                "rustfmt"
                "rust-src"
              ];
            })
            pkgs.rust-analyzer
            pkgs.cargo-insta
            pkgs.git
          ];
        };
      });

      checks = forAllSystems (
        pkgs:
        let
          inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) toolchain rustdoc-wiki;
        in
        {
          # The whole test suite, on the pinned nightly.
          tests = pkgs.callPackage ./nix/tests.nix { inherit toolchain; };

          # Fails when the pinned nightly and `rustdoc-types` drift apart.
          format-version = pkgs.callPackage ./nix/check-format-version.nix {
            inherit toolchain rustdoc-wiki;
          };
        }
      );
    };
}
