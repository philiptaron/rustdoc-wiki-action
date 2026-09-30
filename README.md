# rustdoc-wiki-action

Publish your Rust workspace's API documentation to your repository's GitHub wiki.

The action builds [rustdoc JSON](https://doc.rust-lang.org/nightly/rustdoc/unstable-features.html#-w--output-format-output-format)
for your library crates, renders it as Markdown (one wiki page per module), and pushes the pages to
`<repo>.wiki.git`. Signatures, docs, intra-doc links and source links all come out linked and
readable on github.com.

## Quick start

```yaml
# .github/workflows/docs.yml
name: Docs
on:
  push:
    branches: [main]

concurrency:
  group: wiki
  cancel-in-progress: false

permissions:
  contents: write # to push to the wiki

jobs:
  wiki:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: philiptaron/rustdoc-wiki-action@v0
```

Before the first run, enable **Wikis** in the repository settings and **create a first page in the
web UI**. GitHub only creates a wiki's git repository when its first page is saved, and the action
cannot do that for you.

### Release snapshots

`version` names a set of docs. `latest` follows your default branch; a release tag gives you a
snapshot that later runs leave alone:

```yaml
on:
  push:
    branches: [main]
  release:
    types: [published]

jobs:
  wiki:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: philiptaron/rustdoc-wiki-action@v0
        with:
          version: ${{ github.event_name == 'release' && github.event.release.tag_name || 'latest' }}
```

## Inputs

| Input | Default | |
| --- | --- | --- |
| `version` | `latest` | Names this set of docs. It is the first part of every page name and the wiki directory the pages go in. Letters, digits, `.`, `_` and `-` only. |
| `packages` | every library | Workspace packages to document, separated by commas or newlines. |
| `workspace` | `.` | Directory of the Cargo workspace. |
| `directory` | `api` | The wiki directory the action owns. |
| `private-items` | `false` | Document private items too. |
| `json-dir` | | Use rustdoc JSON files from this directory instead of building. Their `format_version` must match the pinned nightly. |
| `dry-run` | `false` | Report what would change, without committing or pushing. |
| `github-token` | `github.token` | Needs `contents: write`. |

## What ends up in the wiki

```
_Sidebar.md                       # only the block between the markers is ours
api/
  latest/
    latest-my_crate.md            # the crate's root module
    latest-my_crate-parser.md     # my_crate::parser
    latest-my_crate-parser-lexer.md
  v1.2.0/
    v1.2.0-my_crate.md
    ...
```

- **Ownership.** A run replaces `api/<version>/` and nothing else, apart from the block of
  `_Sidebar.md` between `<!-- rustdoc-wiki:begin -->` and `<!-- rustdoc-wiki:end -->`. Your `Home`
  page and the rest of your sidebar are untouched. Hand edits inside `api/<version>/` are
  overwritten.
- **Page names.** GitHub identifies a wiki page by its file name alone and ignores the directory, so
  every generated name starts with the version. Before writing, the action checks that no page it
  would create shares a name with any other page in the wiki (in any directory, ignoring case), and
  fails instead of silently shadowing it.
- **Sidebar.** The managed block lists each crate and its modules two levels deep, for `latest`
  (or, without one, the newest release), and links every version.
- **No churn.** Unchanged docs make no commit. Concurrent pushes to the wiki are retried on top of
  the new tip.

Each module page has the module's docs, then its modules, re-exports, macros, structs, enums, unions,
traits, functions, type aliases, constants and statics. For each item: its signature, a link to its
source at the commit that was built, a `deprecated` note, its docs, and its members (fields,
variants, methods, trait items). Types list their trait implementations; auto-trait and blanket
implementations are summarized.

## What gets documented

- **Crates.** Every library crate in the workspace, or those named in `packages`.
- **Items.** Public items only, unless `private-items` is set. Items that are only reachable
  through a `pub use` from a private module are documented where they are re-exported.
- **Features.** Each crate's `[package.metadata.docs.rs]` is honored (`features`, `all-features`,
  `no-default-features`, `rustdoc-args`), so a crate documents itself the way it does on docs.rs.
  `targets` is ignored: docs are built for the host.
- **Links.** Intra-doc links become links to the wiki page and heading. Items from other crates link
  to docs.rs, and the standard library to doc.rust-lang.org. A link to something that is not
  documented is left as plain text.

## Requirements and limitations

- **Linux runners.** x86_64 and aarch64. The toolchain is provided through
  [Nix](https://nixos.org), which the action installs if the runner does not have it. Windows and
  macOS runners are not supported.
- **The pinned nightly.** rustdoc's JSON output is nightly-only and changes often, so each release
  of this action pins one nightly and the matching `rustdoc-types`. Your crates must build with it;
  a `rust-toolchain.toml` in your repository is ignored. If they cannot, build the JSON yourself and
  pass `json-dir`.
- **Native dependencies** of your crates are yours to install in an earlier step.
- Signatures are shown in code blocks, so the types inside them are not hyperlinks.
- Trait pages do not list implementors.

## Running locally

```console
$ nix build .#toolchain --out-link toolchain
$ nix run . -- run --toolchain ./toolchain --dry-run --wiki-url git@github.com:you/repo.wiki.git
```

`rustdoc-wiki` also has the stages as separate commands: `build` (rustdoc JSON for the workspace),
`render` (JSON to Markdown files, useful for looking at the output), and `publish`.

## Development

```console
$ nix develop        # the pinned nightly, rustfmt, clippy, cargo-insta
$ cargo test         # unit, snapshot and end-to-end tests
$ cargo insta review # after an intended change to the rendered output
$ nix flake check    # the same tests, hermetically, plus the format-version check
```

The nightly in `flake.nix` and the `rustdoc-types` version in `Cargo.toml` are bumped together;
`nix flake check` fails if they disagree. [DESIGN.md](DESIGN.md) records the decisions behind all
of this, including how GitHub's wiki behaves, which shaped the page layout.

## License

Apache-2.0. See [LICENSE](LICENSE).
