# Design

Status: the MVP is implemented and its tests pass locally (`nix flake check` on aarch64-darwin). It
has not run on GitHub yet: the workflows, the static Linux build and the action itself are
untested until the first push. Items marked **(decided)** were chosen explicitly in the design
interview; **(proposed)** items are my extrapolation and have not been confirmed.

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
| Distribution | Prebuilt Linux binaries attached to GitHub Releases, built with Nix `pkgsStatic` and checked for no `/nix/store` references. Nix supplies the toolchain at run time. macOS and Windows runners are not supported |
| Page layout | One page per module |
| Wiki ownership | A configurable directory (default `api/`) plus a managed region of `_Sidebar.md` |
| Naming | `api/<version>/<version>-<crate>-<mod>-<submod>.md`, with the version in every name, `latest` included. Revised after the wiki spike: the original plan (`api/<version>/<crate>-<mod>.md`) collides across versions, because a wiki page's name is its basename (see "Wiki behavior") |
| Versions | `version` input, default `latest`. The workflow owns the triggers. A run replaces `api/<version>/` wholesale, even for an existing snapshot |
| Scope | `packages` input (default: all workspace lib crates). Public items only by default (`private-items` opts in) |
| Features | Honor each crate's `[package.metadata.docs.rs]`. No feature inputs |
| Page content | Signatures, docs, source links, trait impls per type, a `deprecated` note. `cfg`/feature badges are cut from the MVP |
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
  `<!-- rustdoc-wiki:end -->`. If the file does not exist, we create it with the markers. A
  sidebar with only one of the markers is an error, not something to guess at. The block holds
  each crate and its modules two levels deep for `latest` (or, without one, the newest release), and
  a list of versions (`latest`, then releases newest first), all derived from the wiki's directory
  listing. Links use the flat form `[name](basename)`.
- Nothing else in the wiki is touched, including `Home`.
- Hand edits inside `api/<version>/` are overwritten.
- Before writing, check that no basename in the resulting wiki is used twice, counting files we do
  not own, in any directory and ignoring case. Fail instead of letting one page silently shadow the
  other.

### Page contents

Per module page: title `crate::path`, module docs, then sections: modules, re-exports, macros,
structs, enums, unions, traits, functions, type aliases, constants, statics.

- Re-exports are listed as links, not inlined **(decided)**. The exception is an item that is only
  reachable through a `pub use` from a private module: it has no page of its own, so it is
  documented in full where it is re-exported, like rustdoc does.
- Each item: a signature block, a `[source]` link to the commit that was built (`span` and
  `GITHUB_SHA`), a `> **Deprecated**` note, docs, and members (fields, variants, methods, trait
  items).
- Attributes go above the signature as source text: `#[non_exhaustive]`, `#[must_use]`, `#[repr]`,
  `#[macro_export]`, `#[no_mangle]`, `#[export_name]`, `#[link_section]`, `#[target_feature]`. Format
  61 represents these as structured data, so this is cheap and safe. Attributes the JSON only has as
  strings (`Attribute::Other`) are not shown, because their form is not covered by the format
  version.
- Types list their inherent methods with headings, and their trait implementations as a list.
  Auto-trait and blanket implementations are summarized as a list of trait names in a collapsed
  `<details>` block, without their conditions.
- Traits: associated types, constants, required and provided methods. No "implementors" list in v1.
- Doc-comment headings are shifted down so they nest under the item heading, and the page's
  anchors account for them.
- Doctest hidden lines (`# ...`) are stripped, `## ` shows `# `, and code-fence info strings are
  normalized to `rust` or the block's language.
- Intra-doc links resolve through the JSON `links` map to `[text](basename#anchor)`, with no
  directory and no `.md`. Items outside the documented crates link to docs.rs, and
  `std`/`core`/`alloc` to `doc.rust-lang.org/stable`. A link to an item that is not documented is
  left as plain text.
- Signatures are in code blocks, so the types inside them are not links.

## Toolchain coupling

- The flake pins the nightly (date) and `flake.lock`. The binary depends on the matching
  `rustdoc-types` release (`=0.61.0`, format 61).
- `checks.format-version` builds a fixture crate with the pinned toolchain and asserts that the
  emitted `format_version` equals `rustdoc_types::FORMAT_VERSION`. It fails with instructions
  when the two drift apart.
- At runtime the binary checks `format_version` on every input, including `json-dir`, and fails
  with a clear error on mismatch.
- The pinned `cargo`/`rustc`/`rustdoc` are used for the child processes only, not for later steps in
  the user's job. `RUSTC` and `RUSTDOC` are set to them so that a rustup shim earlier on `PATH` is
  never picked. The user's `rust-toolchain.toml` is ignored. Native dependencies are the user's job:
  install them in a step before this action.
- Not written yet: a scheduled workflow that bumps the nightly, runs the tests and opens a PR
  (proposed).

## Implementation

One crate, `rustdoc-wiki`, a library and a binary:

| Module | Job |
| --- | --- |
| `names` | Heading slugs the way GitHub computes them, per-page anchor de-duplication, page names, version validation |
| `sig` | Pure functions that print rustdoc-json types, generics and signatures as Rust source |
| `docs` | Doc-comment processing: link rewriting from source offsets (other bytes untouched), heading shift, fences, hidden lines |
| `external` | URLs for items that are not on the wiki |
| `render` | Plan (which page each item lives on, re-exports, globs) then emit |
| `sidebar` | The managed region of `_Sidebar.md` |
| `publish` | Clone, replace the owned directory, sidebar, commit, push with retry |
| `build` | `cargo metadata`, docs.rs metadata, `cargo rustdoc` |

**Two-pass emit.** A link to an item needs the anchor GitHub will give its heading, and that depends
on every heading before it on its page, including headings inside doc comments. So each page is
emitted twice: the first pass records each item's anchor, and the second uses them. An integration
test recomputes every anchor from the final Markdown with independent code and checks that each
wiki link resolves.

**CLI.** `run` (build, render, publish; what the action calls), `build`, `render`, `publish`,
`format-version`. `GITHUB_REPOSITORY`, `GITHUB_SERVER_URL`, `GITHUB_SHA` and `GITHUB_TOKEN` are the
defaults for the wiki URL, source links, commit message and authentication.

**Action.** A composite action: install Nix if the runner has none, `nix build
<action_path>#toolchain`, get the binary, run it. The binary comes from the release whose version
matches `Cargo.toml` at the commit the action was checked out at, so `@v0` and pinned tags both
work. Commits without a release, such as a branch or the local `uses: ./` used by this repo's own
workflow, build the binary with Nix instead.

**Publishing.** Clone `<repo>.wiki.git` (the token goes in through git's environment config, never
the URL or argv), use the branch the clone checked out (not a hardcoded `master`), apply the owned
paths, commit as `github-actions[bot]`, skip if the tree is unchanged, push. On a rejected push,
fetch, reset to the new tip, redo the change and retry (we only touch paths we own, so there is
nothing to merge). Build and render are all-or-nothing: if any selected crate fails, nothing is
published. If the wiki is disabled or has no first page, fail with instructions.

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
   one page get `-1`, `-2`, … in document order. We slug the same way, and compute the suffixes,
   when linking across pages.
5. **Names with dots and hyphens work** as basenames, including `v1.0.0-rc.1-x`. The page list shows
   hyphens as spaces.
6. **Sidebars.** The root `_Sidebar.md` applies to every page. A `_Sidebar.md` inside a directory
   replaces the root sidebar for pages in that directory, which makes per-version sidebars possible
   but hides hand-written root links on those pages. Not tested: nested or parent-directory lookup.
   HTML comments (our markers) are stripped from the rendered page but stay in the file.
7. **The wiki repo** does not exist until the first page is created in the web UI (clone gives
   "Repository not found"). Its default branch here is `master`.

## Rustdoc JSON, format 61 (verified)

Things that are not obvious from the `rustdoc-types` documentation:

- The `links` map is keyed by the destination exactly as written, backticks included: `` [`Foo`] ``
  gives the key `` "`Foo`" ``, and `[text](crate::a)` gives `"crate::a"`. Unresolvable links have no
  key.
- A `Use` item has no `name`; the public name is `inner.use.name`. For a glob, `id` is the module
  (or enum) being globbed, and its items are not enumerated: consumers expand them.
- An item re-exported from a private module is in `index` (with `visibility: public`) but is not
  listed in any module's `items`, and its `paths` entry has the private path.
- External items are only in `paths`. `paths` reports where an item is defined, which is often a
  private module (`std::collections::hash::map::HashMap`) and not a page that exists, so the path
  the author wrote is preferred when it is a public `std` path.
- For a blanket impl, `for_` is the concrete type it was found on and `blanket_impl` is the type the
  impl is really for.
- `Constant.expr` is `_` for anything but simple literals; `value` then holds the evaluated
  number (`1_024usize`).
- Derived impls put `$crate::default::Default` in their bounds.
- Module order in `items` is an accident of traversal, so pages are sorted by name.
- With a build target configured (`CARGO_BUILD_TARGET`, or `build.target` in `.cargo/config.toml`)
  the JSON is at `<target>/<triple>/doc/`, not `<target>/doc/`. The Nix build sandbox sets it.

## Risks and open items

1. **The static Linux build is unverified.** `packages.rustdoc-wiki-static` is defined and
   evaluates, but only the first CI run will show whether `outputChecks.out.allowedReferences = [ ]`
   passes for `pkgsStatic`. Fallback: <https://github.com/gabyx/self-hoisted-nix>.
2. **The action and workflows have not run.** `actionlint` and `shellcheck` are clean. The dogfood
   workflow will publish this repository's own docs to its wiki on the first push to `lex`.
3. **Standard library links** rely on the author-written path. An item that is only known by a
   private definition path, and was not written out in full, may link to a page that does not exist.
4. **`targets` from docs.rs metadata** is ignored: docs are built for the host.
5. **Nightly bumps are manual** until the scheduled workflow exists. A bump can change the rendered
   output; the snapshot tests show how.
6. **Linux runners only.** Nix excludes Windows, and macOS runners are not a target.

Not decided yet:

- Whether multi-crate workspaces get a combined landing page.
- Whether to emit a per-version `_Sidebar.md`. It would work, but shadows the root sidebar there.
- Whether the sidebar depth should be an input (it is fixed at two levels).
- Hyperlinked types inside signatures. They would need HTML instead of code fences.
- A trait "implementors" list.
