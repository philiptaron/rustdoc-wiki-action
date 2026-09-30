//! Renders the fixture crates with the pinned nightly and checks the resulting pages.
//!
//! These tests run `cargo rustdoc` with `-Z unstable-options`, so they need the pinned nightly:
//! run them inside `nix develop`.

mod common;

use std::path::Path;

use common::{check_wiki_links, rustdoc_json};
use rustdoc_wiki::render::{self, Options, Page};

fn fixture_json(name: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("fixtures")
        .join(name);
    rustdoc_json(&dir, name, &target, &[])
}

fn render_fixture(name: &str) -> Vec<Page> {
    let krate = render::load(&fixture_json(name)).expect("fixture JSON loads");
    let options = Options {
        version: "latest".into(),
        source_base: Some("https://github.com/example/repo/blob/0123abc".into()),
    };
    render::render(&krate, &options).expect("fixture renders")
}

#[test]
fn basic_pages_match_snapshots() {
    let pages = render_fixture("basic");
    let names: Vec<&str> = pages.iter().map(|p| p.basename.as_str()).collect();
    assert_eq!(
        names,
        [
            "latest-basic",
            "latest-basic-shapes",
            "latest-basic-util",
            "latest-basic-util-nested"
        ]
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_omit_expression(true);
    settings.bind(|| {
        for page in &pages {
            insta::assert_snapshot!(page.basename.clone(), &page.markdown);
        }
    });
}

#[test]
fn every_wiki_link_resolves_to_a_page_and_heading() {
    let checked = check_wiki_links(&render_fixture("basic"));
    assert!(
        checked >= 15,
        "expected the fixture to exercise many links, saw {checked}"
    );
}

#[test]
fn external_links_use_the_written_path() {
    let pages = render_fixture("basic");
    let root = &pages[0].markdown;
    // `Vec` is only known by its definition path, alloc::vec::Vec, which maps onto std.
    assert!(root.contains("(https://doc.rust-lang.org/stable/std/vec/struct.Vec.html)"));
    // HashMap is defined in a private module; the path the author wrote is the real page.
    assert!(
        root.contains("(https://doc.rust-lang.org/stable/std/collections/struct.HashMap.html)")
    );
}

#[test]
fn unresolvable_links_become_plain_text() {
    let pages = render_fixture("basic");
    let util = pages
        .iter()
        .find(|p| p.basename == "latest-basic-util")
        .unwrap();
    // `missing` does not exist, so rustdoc reported no target and it stays as written.
    assert!(util.markdown.contains("`missing`"));
    assert!(!util.markdown.contains("[`missing`]("));
}

#[test]
fn unsupported_format_versions_are_reported_clearly() {
    let error = render::load(br#"{"format_version": 1, "root": 0}"#).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("format_version 1"), "{message}");
    assert!(message.contains("pins"), "{message}");
}
