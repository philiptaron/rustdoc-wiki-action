//! Keeps the links in the documentation honest.
//!
//! Relative links must point at files that exist, `#anchors` at headings that exist (computed with
//! the same slug rules the tool uses for the wiki), and the section links that code comments, error
//! messages and workflows give must still name a real heading. External URLs are not checked here:
//! that needs a network.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::anchors;
use pulldown_cmark::{Event, Options, Parser, Tag};

const DOCS: [&str; 2] = ["README.md", "DESIGN.md"];

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH
}

/// Every link destination in `markdown`, with reference-style links already resolved.
fn destinations(markdown: &str) -> Vec<String> {
    Parser::new_ext(markdown, options())
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. }) => Some(dest_url.to_string()),
            _ => None,
        })
        .collect()
}

#[derive(Default)]
struct Report {
    /// Links that stay in the repository.
    checked: usize,
    problems: Vec<String>,
}

fn check_file(root: &Path, file: &str) -> Report {
    let source = fs::read_to_string(root.join(file)).unwrap();
    let dir = Path::new(file).parent().unwrap_or(Path::new(""));
    let mut report = Report::default();
    for dest in destinations(&source) {
        if dest.contains("://") || dest.starts_with("mailto:") {
            continue;
        }
        report.checked += 1;
        let (path, anchor) = match dest.split_once('#') {
            Some((path, anchor)) => (path, Some(anchor)),
            None => (dest.as_str(), None),
        };
        let target: PathBuf = if path.is_empty() {
            file.into()
        } else {
            dir.join(path)
        };
        let full = root.join(&target);
        if !full.exists() {
            report.problems.push(format!(
                "{file}: {dest}: {} does not exist",
                target.display()
            ));
            continue;
        }
        let Some(anchor) = anchor else { continue };
        if full.extension().is_none_or(|e| e != "md") {
            report.problems.push(format!(
                "{file}: {dest}: {} is not Markdown",
                target.display()
            ));
        } else if !anchors(&fs::read_to_string(&full).unwrap()).contains(anchor) {
            report.problems.push(format!(
                "{file}: {dest}: {} has no heading #{anchor}",
                target.display()
            ));
        }
    }
    report
}

#[test]
fn links_in_the_docs_resolve() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0;
    let mut problems = Vec::new();
    for file in DOCS {
        let report = check_file(root, file);
        checked += report.checked;
        problems.extend(report.problems);
    }
    assert!(
        problems.is_empty(),
        "broken links:\n{}",
        problems.join("\n")
    );
    assert!(
        checked >= 20,
        "expected the docs to link around, but only {checked} links were checked"
    );
}

#[test]
fn the_checker_finds_broken_links() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("a.md"),
        "[ok](b.md#hello-world) [file](missing.md) [heading](b.md#nope) [here](#absent) \
         [external](https://example.com/x#y) [self](#a)\n\n# A\n",
    )
    .unwrap();
    fs::write(dir.path().join("b.md"), "# Hello, World\n").unwrap();

    let report = check_file(dir.path(), "a.md");
    assert_eq!(report.checked, 5, "external links are not counted");
    assert_eq!(report.problems.len(), 3, "{:#?}", report.problems);
    assert!(report.problems[0].contains("missing.md does not exist"));
    assert!(report.problems[1].contains("no heading #nope"));
    assert!(report.problems[2].contains("no heading #absent"));
}

/// Files whose comments and messages link to sections of the docs.
fn files_that_point_at_the_docs(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .extension()
                .is_some_and(|e| matches!(e.to_str(), Some("rs" | "nix" | "yml" | "toml")))
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for dir in ["src", "nix", ".github"] {
        walk(&root.join(dir), &mut files);
    }
    files.extend(["Cargo.toml", "flake.nix", "action.yml"].map(|f| root.join(f)));
    files
}

/// The section names that follow each occurrence of `marker` in `text`.
fn sections_after(text: &str, marker: &str) -> Vec<String> {
    text.match_indices(marker)
        .map(|(at, _)| {
            text[at + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                .collect()
        })
        .collect()
}

#[test]
fn section_links_in_code_and_config_name_real_anchors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let design = anchors(&fs::read_to_string(root.join("DESIGN.md")).unwrap());
    let readme = anchors(&fs::read_to_string(root.join("README.md")).unwrap());

    let mut checked = 0;
    let mut problems = Vec::new();
    for file in files_that_point_at_the_docs(root) {
        let text = fs::read_to_string(&file).unwrap();
        // `{}#` is how the error messages append a README section to `HOMEPAGE`.
        let targets = [
            ("DESIGN.md#", "DESIGN.md", &design),
            ("rustdoc-wiki-action#", "README.md", &readme),
            ("{}#", "README.md", &readme),
        ];
        for (marker, doc, anchors) in targets {
            for section in sections_after(&text, marker) {
                checked += 1;
                if !anchors.contains(&section) {
                    let name = file.strip_prefix(root).unwrap().display();
                    problems.push(format!("{name}: {doc} has no heading #{section}"));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "stale section links:\n{}",
        problems.join("\n")
    );
    assert!(
        checked >= 8,
        "expected several section links, found {checked}"
    );
}
