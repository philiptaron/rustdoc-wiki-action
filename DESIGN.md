# Design

Status: draft from the design interview. Items marked **(decided)** were chosen explicitly;
items marked **(proposed)** are my extrapolation and have not been confirmed.

## Goal

A GitHub Action that turns a Rust workspace's API documentation into pages in the repository's
GitHub wiki. It runs on CI for the default branch (`latest`) and optionally on releases
(frozen snapshots).

## Decisions

| Area | Decision |
| --- | --- |
| Input | rustdoc JSON (`cargo rustdoc -- -Z unstable-options --output-format json`), rendered to Markdown by us |
| Implementation | Rust binary, shipped via a composite action |
| Toolchain | Nix pins the nightly. The flake is the lock, so the action's ref decides both the nightly and the matching `rustdoc-types` |
| Distribution | Prebuilt binaries attached to GitHub Releases, built with Nix `pkgsStatic` and checked for no `/nix/store` references. Nix supplies the toolchain at run time |
| Page layout | One page per module |
| Wiki ownership | A configurable directory (default `api/`) plus a managed region of `_Sidebar.md` |
| Naming | **(decided, revised after the wiki spike)** `api/<version>/<version>-<crate>-<mod>-<submod>.md`, with the version in every name, `latest` included. The original plan (`api/<version>/<crate>-<mod>.md`) collides across versions, because a wiki page's name is its basename (see "Wiki behavior") |
| Versions | `version` input, default `latest`. The workflow owns the triggers. A run replaces `api/<version>/` wholesale, even for an existing snapshot |
| Scope | `packages` input (default: all workspace lib crates). Public items only by default |
| Features | Honor each crate's `[package.metadata.docs.rs]`. No feature inputs |
| Page content | Signatures, docs, source links, trait impls per type, `deprecated` badge. `cfg`/feature badges are cut from the MVP |
| Escape hatch | `json-dir` input skips the build. No cargo-args or toolchain override in v1 |
| Target | Current repo's wiki only, via the workflow token (`contents: write`) |

### Layout in the wiki repo

```
_Sidebar.md                      # only the block between the markers is ours
api/
  latest/
    latest-my_crate.md           # crate root module
    latest-my_crate-foo.md
    latest-my_crate-foo-bar.md
  v1.2.0/                        # frozen only by convention: re-running overwrites it
    v1.2.0-my_crate.md
    ...
```

GitHub identifies a wiki page by its basename alone, so the directory is not part of any URL. Here
it only marks ownership (delete `api/<version>/` safely) and keeps the git tree tidy. The version
goes in the basename so names stay unique across versions.

Hyphen is a safe separator: Rust path segments cannot contain `-`, and the version is known from the
directory name, so `<version>-<crate>-<mod>...` splits unambiguously. The module tree can be
recovered from file names alone, so the sidebar needs no stored state.

### Ownership rules

- A run with `version: X` deletes and rewrites `api/X/`. Other version directories are untouched.
- `_Sidebar.md`: we replace only what is between `<!-- rustdoc-wiki:begin -->` and
  `<!-- rustdoc-wiki:end -->`. If the file does not exist, we create it with the markers.
  The block holds the crates, a depth-limited module tree for `latest`, and a list of versions,
  all derived from directory listings in the wiki (proposed). Links use the flat form
  `[name](basename)`, which was verified to work from the sidebar.
- Nothing else in the wiki is touched, including `Home`.
- Hand edits inside `api/<version>/` are overwritten.
- Before writing, check that no basename in the resulting wiki is used twice, counting files we do
  not own (for example a hand-written page called `latest-my_crate`). Fail instead of letting one
  page silently shadow the other.

### Page contents

Per module page: title `crate::path`, module docs, then sections (modules, structs, enums,
unions, traits, functions, type aliases, constants, statics, macros, re-exports).

- Re-exports are listed as links, not inlined **(decided)**.
- Each item: signature block, docs, GitHub source permalink (`span` + `GITHUB_SHA`), and a
  `deprecated` badge (from the dedicated `deprecation` field). Feature-gating (`cfg`) badges are
  out of the MVP, and so are badges that need parsing `attrs` (`must_use`, `non_exhaustive`),
  since the `attrs` representation changes between format versions. Revisit only if trivial.
- Types list their inherent methods and trait impls. Blanket and auto-trait impls are
  summarized, not expanded (proposed).
- Traits: no "implementors" list in v1 (not selected).
- Doc-comment headings are shifted down so they nest under the item heading (proposed).
- Doctest hidden lines (`# ...`) are stripped and code-fence info strings normalized (proposed).
- Intra-doc links resolve through the JSON `links` map to `[text](basename#anchor)`, with no
  directory and no `.md` (see "Wiki behavior"). Items outside the
  documented crates link to docs.rs, and `std`/`core`/`alloc` to doc.rust-lang.org (proposed).
  Links to items that are not documented degrade to plain code.

## Toolchain coupling

- The flake pins the nightly (date) and `flake.lock`. The binary depends on the matching
  `rustdoc-types` release.
- CI guard (proposed): build a fixture crate with the pinned toolchain and assert that the emitted
  `format_version` equals `rustdoc_types::FORMAT_VERSION`.
- At runtime the binary checks `format_version` on every input, including `json-dir`, and fails
  with a clear error on mismatch.
- The pinned `cargo`/`rustc` is put on `PATH` for the child processes only, not for later steps in
  the user's job. It ignores the user's `rust-toolchain.toml`. Native dependencies are the
  user's job: install them in a step before this action.
- A scheduled workflow bumps the nightly, runs the fixture tests, and opens a PR (proposed).

## Action shape (proposed)

Binary: `rustdoc-wiki`, with subcommands `build` (cargo → JSON), `render` (JSON → Markdown tree),
`publish` (tree → wiki commit/push), and `run` (all three; what the action calls). Keeping the stages
separate makes `json-dir`, local preview, and testing straightforward.

Composite action steps: install Nix → `nix build <action_path>#toolchain` → fetch the release
binary for the runner's OS/arch → `rustdoc-wiki run`.

Inputs: `version`, `packages`, `json-dir`, `directory` (default `api`), `private-items`
(default false), `dry-run`, `github-token` (defaults to the workflow token).

Publishing: clone `<repo>.wiki.git` with the token, detect the default branch from the remote HEAD
(do not hardcode `master`), apply the owned paths, commit as `github-actions[bot]`, skip if the tree
is unchanged, push. On a non-fast-forward rejection, fetch, re-apply and retry (we own our paths, so
there is nothing to merge). The build and render are all-or-nothing: if any selected crate fails,
nothing is published. If the wiki is disabled or has no first page (GitHub creates the wiki repo
lazily), fail with instructions.

## Wiki behavior (verified)

Tested on 2026-09-30 against this repo's own wiki, with scratch pages that have since been removed.

1. **The namespace is flat.** `api/latest/x.md` is served at `/wiki/x`. `/wiki/api/latest/x` does
   not exist, and the page list shows basenames only. Two files with the same basename in different
   directories collide silently: one shadows the other, and the winner is not something to rely on.
2. **Markdown links are passed through untouched.** Every page sits at the same URL depth, so
   `[text](basename)` and `[text](basename#anchor)` work from any page, from the sidebar, and from
   `Home`. These all produce dead links: a link with a directory (`api/latest/x`), an absolute
   `/…/wiki/api/latest/x`, and a link ending in `.md`.
3. **`[[wikilinks]]`:** `[[basename]]` works, but `[[a/b]]` becomes `a-b`, a nonexistent page. We use
   plain Markdown links only.
4. **Anchors follow GFM slugging.** Lowercase, spaces become `-`, and punctuation other than `-` and
   `_` is dropped (`Bar::new` → `barnew`, `Foo<T>` → `foot`, `v0.1.0` → `v010`). Repeated headings on
   one page get `-1`, `-2`, … in document order. We must slug the same way, and compute the suffixes,
   when linking across pages.
5. **Names with dots and hyphens work** as basenames, including `v1.0.0-rc.1-x`. The page list shows
   hyphens as spaces.
6. **Sidebars.** The root `_Sidebar.md` applies to every page. A `_Sidebar.md` inside a directory
   replaces the root sidebar for pages in that directory, which makes per-version sidebars possible
   but hides hand-written root links on those pages. Not tested: nested or parent-directory lookup.
   HTML comments (our markers) are stripped from the rendered page but stay in the file.
7. **The wiki repo** does not exist until the first page is created in the web UI (clone gives
   "Repository not found"). Its default branch here is `master`.

## Risks and things to verify early

1. ~~**Subdirectories in GitHub wikis.**~~ Resolved: see "Wiki behavior". The layout was revised.
2. **Nix-built binaries are not portable by default.** A plain Nix build links against
   `/nix/store`. Plan: build release artifacts from `pkgsStatic` with `__structuredAttrs = true` and
   `outputChecks.out.allowedReferences = [ ]`, so the build fails if any store path leaks in.
   If that proves insufficient, fall back to <https://github.com/gabyx/self-hoisted-nix>.
   macOS runners are out of scope for the action: Release artifacts are Linux only. Running the
   binary on a Mac laptop via a local Nix build is still supported for development.
3. ~~**What rustdoc JSON exposes for `cfg`/feature gating.**~~ Cut from the MVP.
4. **Floating tags.** If users reference `@v1`, the action must still resolve a binary version
   (for example from a `VERSION` file in the action checkout).
5. ~~**Anchors.**~~ Slug rules are known (see "Wiki behavior"). Still to do: choose heading text for
   items, methods and impls that stays unique and stable, since only heading text drives the anchor.
6. **Platform support.** The action supports Linux runners only (x86_64 and, if cheap, aarch64).
   Nix excludes Windows, and macOS runners are not a target.
7. **`targets` from docs.rs metadata.** Host build only in v1; `targets`/`default-target` ignored.

## Not decided yet

- Sidebar depth limit and ordering (crates alphabetical? `latest` first, then versions by semver?).
- Whether to emit a per-version `_Sidebar.md`. It would work, but shadows the root sidebar there.
- Whether multi-crate workspaces get a combined landing page.
- Name of the wiki commit message (`Update API docs (<version>) from <sha>` is the proposal).
- Testing: golden-file tests over small fixture crates, plus dogfooding this repo's own wiki (proposed).
