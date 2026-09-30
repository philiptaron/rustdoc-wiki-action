//! Helpers for tests that need a wiki: a local bare repository standing in for GitHub's.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rustdoc_wiki::names::Slugger;
use rustdoc_wiki::render::Page;
use tempfile::TempDir;

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub struct Wiki {
    tmp: TempDir,
    pub remote: PathBuf,
}

impl Wiki {
    /// A wiki with a `Home.md` on `master`, like one whose first page was saved in the UI.
    pub fn new() -> Wiki {
        let tmp = TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        git(
            tmp.path(),
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=master",
                "remote.git",
            ],
        );
        let wiki = Wiki { tmp, remote };
        wiki.commit_files(&[("Home.md", "Welcome to the wiki\n")]);
        wiki
    }

    pub fn url(&self) -> String {
        self.remote.display().to_string()
    }

    /// Adds files to the wiki as if a person had edited it.
    pub fn commit_files(&self, files: &[(&str, &str)]) {
        let work = self.tmp.path().join("edit");
        let _ = fs::remove_dir_all(&work);
        git(self.tmp.path(), &["clone", "--quiet", &self.url(), "edit"]);
        for (name, content) in files {
            let path = work.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        git(&work, &["add", "--all"]);
        git(&work, &["commit", "--quiet", "--message", "edit"]);
        git(&work, &["push", "--quiet", "origin", "HEAD:master"]);
    }

    /// A fresh clone, for looking at what was published.
    pub fn checkout(&self) -> PathBuf {
        let dir = self.tmp.path().join("check");
        let _ = fs::remove_dir_all(&dir);
        git(self.tmp.path(), &["clone", "--quiet", &self.url(), "check"]);
        dir
    }

    pub fn commit_count(&self) -> usize {
        git(&self.remote, &["rev-list", "--count", "master"])
            .parse()
            .unwrap()
    }
}

/// Runs `cargo rustdoc` on the library in `dir`, building into `target`, and returns the JSON for
/// the crate named `lib_name`. Needs the pinned nightly, so tests that call this run inside
/// `nix develop`.
pub fn rustdoc_json(dir: &Path, lib_name: &str, target: &Path, rustdoc_args: &[&str]) -> Vec<u8> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target)
        .args(["rustdoc", "--lib", "--quiet", "--"])
        .args(["-Z", "unstable-options", "--output-format", "json"])
        .args(rustdoc_args)
        .output()
        .expect("failed to run cargo");
    assert!(
        output.status.success(),
        "`cargo rustdoc` failed in {}. These tests need the pinned nightly; run them inside \
         `nix develop`.\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = rustdoc_wiki::build::find_json(target, lib_name).expect("rustdoc wrote no JSON");
    fs::read(json).expect("JSON is readable")
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

/// The anchors GitHub assigns to the headings of `markdown`, computed independently of the
/// renderer's own bookkeeping.
pub fn anchors(markdown: &str) -> HashSet<String> {
    let mut slugger = Slugger::default();
    let mut found = HashSet::new();
    let mut heading: Option<String> = None;
    for event in Parser::new_ext(markdown, markdown_options()) {
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
pub fn wiki_links(markdown: &str) -> Vec<String> {
    Parser::new_ext(markdown, markdown_options())
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

/// Asserts that every link between wiki pages points at a page that exists and, if it has an
/// anchor, at a heading that exists on it. Returns how many links were checked.
pub fn check_wiki_links(pages: &[Page]) -> usize {
    let by_name: HashMap<&str, HashSet<String>> = pages
        .iter()
        .map(|p| (p.basename.as_str(), anchors(&p.markdown)))
        .collect();
    let mut checked = 0;
    for page in pages {
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
    checked
}
