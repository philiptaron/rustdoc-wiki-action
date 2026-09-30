//! Renders this crate's own documentation, the way the Docs workflow publishes it, and checks it.
//!
//! Needs the pinned nightly (for rustdoc JSON), so run inside `nix develop`.

mod common;

use std::path::Path;

use common::{check_wiki_links, rustdoc_json};
use rustdoc_wiki::render::{self, Options, Page};

fn own_pages() -> Vec<Page> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dogfood");
    // A broken intra-doc link would be published as plain text. In our own docs that is a bug, so
    // rustdoc warnings (broken or redundant links, bare URLs) fail the test.
    let json = rustdoc_json(root, "rustdoc_wiki", &target, &["-D", "warnings"]);
    let krate = render::load(&json).expect("own JSON loads");
    let options = Options {
        version: "latest".into(),
        source_base: None,
    };
    render::render(&krate, &options).expect("own docs render")
}

fn page<'a>(pages: &'a [Page], name: &str) -> &'a str {
    &pages
        .iter()
        .find(|p| p.basename == name)
        .unwrap_or_else(|| panic!("no page {name}"))
        .markdown
}

#[test]
fn every_module_has_a_page_and_every_link_between_pages_works() {
    let pages = own_pages();
    let names: Vec<&str> = pages.iter().map(|p| p.basename.as_str()).collect();
    assert_eq!(
        names,
        [
            "latest-rustdoc_wiki",
            "latest-rustdoc_wiki-build",
            "latest-rustdoc_wiki-docs",
            "latest-rustdoc_wiki-external",
            "latest-rustdoc_wiki-names",
            "latest-rustdoc_wiki-publish",
            "latest-rustdoc_wiki-render",
            "latest-rustdoc_wiki-sidebar",
            "latest-rustdoc_wiki-sig",
        ]
    );
    let checked = check_wiki_links(&pages);
    assert!(
        checked >= 20,
        "expected the docs to link around, but only {checked} links were checked"
    );
}

#[test]
fn the_crate_page_links_to_its_modules_and_to_the_design_doc() {
    let pages = own_pages();
    let root = page(&pages, "latest-rustdoc_wiki");
    for module in [
        "build", "render", "publish", "names", "docs", "sig", "external", "sidebar",
    ] {
        assert!(
            root.contains(&format!("(latest-rustdoc_wiki-{module})")),
            "the crate page does not link to {module}"
        );
    }
    assert!(
        root.contains("(https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/DESIGN.md)")
    );
    assert!(
        root.contains("(https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/README.md)")
    );
}

#[test]
fn doc_comments_link_to_the_right_section_of_the_design_doc() {
    let pages = own_pages();
    let design = "https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/DESIGN.md";
    assert!(
        page(&pages, "latest-rustdoc_wiki-names")
            .contains(&format!("({design}#wiki-behavior-verified)"))
    );
    assert!(
        page(&pages, "latest-rustdoc_wiki-publish")
            .contains(&format!("({design}#ownership-rules)"))
    );
}

#[test]
fn intra_doc_links_to_a_dependency_go_to_docs_rs() {
    let pages = own_pages();
    // `render::load` documents itself in terms of `rustdoc_types::FORMAT_VERSION`.
    assert!(page(&pages, "latest-rustdoc_wiki-render").contains(
        "(https://docs.rs/rustdoc_types/latest/rustdoc_types/constant.FORMAT_VERSION.html)"
    ));
}

#[test]
fn no_link_reference_definitions_leak_onto_pages() {
    // A `[label]: url` line left in one item's docs would apply to every item on the page.
    for page in own_pages() {
        for line in page.markdown.lines() {
            let is_definition = line.starts_with('[') && line.contains("]: ");
            assert!(
                !is_definition,
                "{}: leaked definition {line:?}",
                page.basename
            );
        }
    }
}

#[test]
fn summaries_keep_links_whose_definitions_come_later() {
    let pages = own_pages();
    // The first paragraph of `build`'s docs says `[rustdoc JSON]`, defined further down.
    assert!(page(&pages, "latest-rustdoc_wiki").contains(
        "Builds [rustdoc JSON](https://doc.rust-lang.org/nightly/rustdoc/unstable-features.html#-w--output-format-output-format) for the crates to document."
    ));
}
