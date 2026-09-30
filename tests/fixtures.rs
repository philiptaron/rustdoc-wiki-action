//! Renders the fixture crates with the pinned nightly and checks the resulting pages.
//!
//! These tests run `cargo rustdoc` with `-Z unstable-options`, so they need the pinned nightly:
//! run them inside `nix develop`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use pulldown_cmark::{Event, Options as MarkdownOptions, Parser, Tag, TagEnd};
use rustdoc_wiki::names::Slugger;
use rustdoc_wiki::render::{self, Options, Page};

fn fixture_json(name: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("fixtures")
        .join(name);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", &target)
        .args(["rustdoc", "--lib", "--quiet", "--"])
        .args(["-Z", "unstable-options", "--output-format", "json"])
        .output()
        .expect("failed to run cargo");
    assert!(
        output.status.success(),
        "`cargo rustdoc` failed for fixture {name}. These tests need the pinned nightly; run them \
         inside `nix develop`.\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = rustdoc_wiki::build::find_json(&target, name).expect("rustdoc wrote no JSON");
    fs::read(json).expect("JSON is readable")
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

/// The anchors GitHub will assign to the headings of `markdown`.
fn anchors(markdown: &str) -> HashSet<String> {
    let options = MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_FOOTNOTES
        | MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TASKLISTS;
    let mut slugger = Slugger::default();
    let mut found = HashSet::new();
    let mut heading: Option<String> = None;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => {
                found.insert(slugger.anchor(&heading.take().unwrap_or_default()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(h) = &mut heading {
                    h.push_str(&t);
                }
            }
            _ => {}
        }
    }
    found
}

/// Every link that stays inside the wiki: `page`, `page#anchor` or `#anchor`.
fn wiki_links(markdown: &str) -> Vec<String> {
    Parser::new_ext(markdown, MarkdownOptions::ENABLE_TABLES)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. })
                if !dest_url.contains(':') && !dest_url.contains('/') =>
            {
                Some(dest_url.to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn every_wiki_link_resolves_to_a_page_and_heading() {
    let pages = render_fixture("basic");
    let by_name: HashMap<&str, HashSet<String>> = pages
        .iter()
        .map(|p| (p.basename.as_str(), anchors(&p.markdown)))
        .collect();

    let mut checked = 0;
    for page in &pages {
        for link in wiki_links(&page.markdown) {
            let (target, anchor) = match link.split_once('#') {
                Some((t, a)) => (
                    if t.is_empty() {
                        page.basename.as_str()
                    } else {
                        t
                    },
                    Some(a),
                ),
                None => (link.as_str(), None),
            };
            let headings = by_name
                .get(target)
                .unwrap_or_else(|| panic!("{}: link to unknown page {link:?}", page.basename));
            if let Some(anchor) = anchor {
                assert!(
                    headings.contains(anchor),
                    "{}: link {link:?} has no matching heading on {target}",
                    page.basename
                );
            }
            checked += 1;
        }
    }
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
