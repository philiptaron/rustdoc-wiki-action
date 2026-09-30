//! The managed region of the wiki's [`_Sidebar.md`][sidebar-docs].
//!
//! We own only what is between [`BEGIN`] and [`END`]; anything else in the sidebar is the
//! user's. The contents are derived from the page names in the wiki, so no state is stored.
//!
//! [sidebar-docs]: https://docs.github.com/en/communities/documenting-your-project-with-wikis/creating-a-footer-or-sidebar-for-your-wiki

use std::cmp::Reverse;
use std::collections::BTreeMap;

use anyhow::{Result, bail};

pub const BEGIN: &str = "<!-- rustdoc-wiki:begin -->";
pub const END: &str = "<!-- rustdoc-wiki:end -->";

/// How many module levels to list below each crate.
const TREE_DEPTH: usize = 2;

/// The version whose module tree the sidebar shows, if it exists.
const PRIMARY_VERSION: &str = "latest";

/// Page names (without `.md`) per version, as found in the wiki.
pub type Versions = BTreeMap<String, Vec<String>>;

#[derive(Default)]
struct Node {
    page: Option<String>,
    children: BTreeMap<String, Node>,
}

/// A sort key that puts `latest` first, then releases from newest to oldest, then anything else.
fn version_key(version: &str) -> (u8, Reverse<Vec<u64>>, bool, Reverse<String>) {
    if version == PRIMARY_VERSION {
        return (0, Reverse(vec![]), false, Reverse(String::new()));
    }
    let bare = version.strip_prefix(['v', 'V']).unwrap_or(version);
    let (numbers, pre) = match bare.split_once('-') {
        Some((n, _)) => (n, true),
        None => (bare, false),
    };
    let parsed: Option<Vec<u64>> = numbers.split('.').map(|p| p.parse().ok()).collect();
    match parsed {
        // A release sorts above its own pre-releases, and `Reverse` flips everything to newest-first.
        Some(parts) => (1, Reverse(parts), pre, Reverse(version.to_string())),
        None => (2, Reverse(vec![]), false, Reverse(version.to_string())),
    }
}

/// Versions in display order.
pub fn ordered(versions: &Versions) -> Vec<&String> {
    let mut all: Vec<&String> = versions.keys().collect();
    all.sort_by_key(|v| version_key(v));
    all
}

fn tree(version: &str, pages: &[String]) -> Node {
    let mut root = Node::default();
    let prefix = format!("{version}-");
    for page in pages {
        let Some(path) = page.strip_prefix(&prefix) else {
            continue;
        };
        let mut node = &mut root;
        for segment in path.split('-') {
            node = node.children.entry(segment.to_string()).or_default();
        }
        node.page = Some(page.clone());
    }
    root
}

fn write_tree(node: &Node, depth: usize, out: &mut String) {
    for (name, child) in &node.children {
        let indent = "  ".repeat(depth);
        match &child.page {
            Some(page) => out.push_str(&format!("{indent}* [`{name}`]({page})\n")),
            None => out.push_str(&format!("{indent}* `{name}`\n")),
        }
        if depth < TREE_DEPTH {
            write_tree(child, depth + 1, out);
        }
    }
}

/// The crate root pages of one version.
fn roots(version: &str, pages: &[String]) -> Vec<(String, String)> {
    let prefix = format!("{version}-");
    let mut roots: Vec<(String, String)> = pages
        .iter()
        .filter_map(|p| {
            let name = p.strip_prefix(&prefix)?;
            (!name.contains('-')).then(|| (name.to_string(), p.clone()))
        })
        .collect();
    roots.sort();
    roots
}

/// The sidebar block, markers included. Empty if the wiki has no generated pages.
pub fn block(versions: &Versions) -> String {
    let order = ordered(versions);
    let Some(primary) = order
        .iter()
        .find(|v| v.as_str() == PRIMARY_VERSION)
        .or(order.first())
    else {
        return String::new();
    };

    let mut out = String::new();
    out.push_str(BEGIN);
    out.push('\n');
    if primary.as_str() == PRIMARY_VERSION {
        out.push_str("**API documentation**\n\n");
    } else {
        out.push_str(&format!("**API documentation ({primary})**\n\n"));
    }
    write_tree(&tree(primary, &versions[*primary]), 0, &mut out);

    let links: Vec<String> = order
        .iter()
        .filter_map(|version| {
            let roots = roots(version, &versions[*version]);
            match roots.as_slice() {
                [] => None,
                [(_, page)] => Some(format!("[{version}]({page})")),
                many => {
                    let crates: Vec<String> = many
                        .iter()
                        .map(|(name, page)| format!("[{name}]({page})"))
                        .collect();
                    Some(format!("{version} ({})", crates.join(", ")))
                }
            }
        })
        .collect();
    if links.len() > 1 {
        out.push_str(&format!("\n**Versions:** {}\n", links.join(" · ")));
    }
    out.push_str(END);
    out
}

/// Puts `block` into the existing sidebar, replacing a previous managed region if there is one.
pub fn apply(existing: Option<&str>, block: &str) -> Result<String> {
    let existing = existing.unwrap_or_default();
    match (existing.find(BEGIN), existing.find(END)) {
        (Some(begin), Some(end)) if begin < end => {
            let end = end + END.len();
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..begin]);
            out.push_str(block);
            out.push_str(&existing[end..]);
            Ok(out)
        }
        (None, None) if existing.trim().is_empty() => Ok(format!("{block}\n")),
        (None, None) => Ok(format!("{}\n\n{block}\n", existing.trim_end())),
        _ => bail!(
            "_Sidebar.md has a broken managed region: it needs {BEGIN} followed by {END}, or \
             neither. Fix or remove the markers by hand."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(spec: &[(&str, &[&str])]) -> Versions {
        spec.iter()
            .map(|(v, pages)| (v.to_string(), pages.iter().map(|p| p.to_string()).collect()))
            .collect()
    }

    #[test]
    fn versions_sort_latest_then_newest_release_first() {
        let v = versions(&[
            ("v1.2.0", &[]),
            ("latest", &[]),
            ("v1.10.0", &[]),
            ("v1.10.0-rc.1", &[]),
            ("v0.9.0", &[]),
            ("nightly", &[]),
        ]);
        let order: Vec<&str> = ordered(&v).into_iter().map(String::as_str).collect();
        assert_eq!(
            order,
            [
                "latest",
                "v1.10.0",
                "v1.10.0-rc.1",
                "v1.2.0",
                "v0.9.0",
                "nightly"
            ]
        );
    }

    #[test]
    fn block_shows_a_depth_limited_tree_and_the_versions() {
        let v = versions(&[
            (
                "latest",
                &[
                    "latest-basic",
                    "latest-basic-util",
                    "latest-basic-util-nested",
                    "latest-basic-util-nested-deep",
                    "latest-basic-shapes",
                ],
            ),
            ("v1.0.0", &["v1.0.0-basic", "v1.0.0-basic-util"]),
        ]);
        assert_eq!(
            block(&v),
            "\
<!-- rustdoc-wiki:begin -->
**API documentation**

* [`basic`](latest-basic)
  * [`shapes`](latest-basic-shapes)
  * [`util`](latest-basic-util)
    * [`nested`](latest-basic-util-nested)

**Versions:** [latest](latest-basic) · [v1.0.0](v1.0.0-basic)
<!-- rustdoc-wiki:end -->"
        );
    }

    #[test]
    fn a_single_version_has_no_versions_line() {
        let v = versions(&[("latest", &["latest-basic"])]);
        assert!(!block(&v).contains("Versions"));
    }

    #[test]
    fn without_latest_the_newest_release_is_shown() {
        let v = versions(&[("v1.0.0", &["v1.0.0-a"]), ("v2.0.0", &["v2.0.0-a"])]);
        let b = block(&v);
        assert!(b.contains("**API documentation (v2.0.0)**"), "{b}");
        assert!(b.contains("[`a`](v2.0.0-a)"), "{b}");
    }

    #[test]
    fn several_crates_are_listed_per_version() {
        let v = versions(&[
            ("latest", &["latest-a", "latest-b"]),
            ("v1.0.0", &["v1.0.0-a", "v1.0.0-b"]),
        ]);
        assert!(block(&v).contains(
            "**Versions:** latest ([a](latest-a), [b](latest-b)) · v1.0.0 ([a](v1.0.0-a), [b](v1.0.0-b))"
        ));
    }

    #[test]
    fn empty_wiki_gives_no_block() {
        assert_eq!(block(&Versions::new()), "");
    }

    #[test]
    fn apply_creates_appends_and_replaces() {
        let b = format!("{BEGIN}\nnew\n{END}");
        assert_eq!(apply(None, &b).unwrap(), format!("{b}\n"));
        assert_eq!(apply(Some("  \n"), &b).unwrap(), format!("{b}\n"));
        assert_eq!(
            apply(Some("[Home](Home)\n"), &b).unwrap(),
            format!("[Home](Home)\n\n{b}\n")
        );

        let old = format!("[Home](Home)\n\n{BEGIN}\nold\n{END}\n\n[Extra](Extra)\n");
        assert_eq!(
            apply(Some(&old), &b).unwrap(),
            format!("[Home](Home)\n\n{b}\n\n[Extra](Extra)\n")
        );
    }

    #[test]
    fn apply_is_idempotent() {
        let b = format!("{BEGIN}\nx\n{END}");
        let once = apply(Some("[Home](Home)"), &b).unwrap();
        assert_eq!(apply(Some(&once), &b).unwrap(), once);
    }

    #[test]
    fn apply_rejects_broken_markers() {
        let b = format!("{BEGIN}\nx\n{END}");
        assert!(apply(Some(BEGIN), &b).is_err());
        assert!(apply(Some(END), &b).is_err());
        assert!(apply(Some(&format!("{END}\n{BEGIN}")), &b).is_err());
    }
}
