//! Page names and heading anchors.
//!
//! GitHub identifies a wiki page by its file basename alone and derives heading anchors from the
//! heading text (see DESIGN.md, "Wiki behavior"). Every rule that affects a link target lives here.

use std::collections::HashMap;

use anyhow::{Result, bail};

/// The anchor GitHub gives a heading whose text content is `text`, before de-duplication.
///
/// Lowercases, turns spaces into `-`, and drops everything except letters, digits, `-` and `_`.
/// Verified against GitHub's wiki renderer: `Bar::new` gives `barnew`, `Foo<T>` gives `foot`.
pub fn slug(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c == '-' || c == '_' || c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// Assigns anchors to the headings of one page, in document order, the way GitHub does:
/// a repeated slug gets `-1`, `-2`, ... appended.
#[derive(Debug, Default)]
pub struct Slugger {
    occurrences: HashMap<String, usize>,
}

impl Slugger {
    pub fn anchor(&mut self, text: &str) -> String {
        let original = slug(text);
        let mut candidate = original.clone();
        while self.occurrences.contains_key(&candidate) {
            let count = self
                .occurrences
                .get_mut(&original)
                .expect("original was seen");
            *count += 1;
            candidate = format!("{original}-{count}");
        }
        self.occurrences.insert(candidate.clone(), 0);
        candidate
    }
}

/// Checks that `version` can be embedded in a page name and used as a directory name.
pub fn validate_version(version: &str) -> Result<()> {
    let ok = !version.is_empty()
        && !version.starts_with(['.', '-'])
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !ok {
        bail!(
            "invalid version {version:?}: use only ASCII letters, digits, '.', '_' and '-', \
             and do not start with '.' or '-'"
        );
    }
    Ok(())
}

/// The wiki page name for a module: `<version>-<crate>-<mod>-<submod>`.
///
/// `module_path` starts with the crate name. A wiki page is identified by its basename alone, so
/// the version has to be part of it. Rust path segments cannot contain `-`, so the result splits
/// unambiguously once the version is known.
pub fn page_basename(version: &str, module_path: &[String]) -> String {
    let mut name = String::from(version);
    for segment in module_path {
        name.push('-');
        name.push_str(segment.strip_prefix("r#").unwrap_or(segment));
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_github() {
        assert_eq!(slug("struct Bar"), "struct-bar");
        assert_eq!(slug("Bar::new"), "barnew");
        assert_eq!(slug("fn with_code"), "fn-with_code");
        assert_eq!(slug("v0.1.0-spike_crate"), "v010-spike_crate");
        assert_eq!(
            slug("Heading with  & symbols, Foo<T>"),
            "heading-with---symbols-foot"
        );
    }

    #[test]
    fn repeated_headings_get_numeric_suffixes() {
        let mut s = Slugger::default();
        assert_eq!(s.anchor("struct Bar"), "struct-bar");
        assert_eq!(s.anchor("Bar::new"), "barnew");
        assert_eq!(s.anchor("struct Bar"), "struct-bar-1");
        assert_eq!(s.anchor("Bar::new"), "barnew-1");
        assert_eq!(s.anchor("struct Bar"), "struct-bar-2");
    }

    #[test]
    fn suffix_collision_with_a_real_heading_is_skipped() {
        let mut s = Slugger::default();
        assert_eq!(s.anchor("a"), "a");
        assert_eq!(s.anchor("a-1"), "a-1");
        assert_eq!(s.anchor("a"), "a-2");
    }

    #[test]
    fn versions() {
        for ok in ["latest", "v1.2.0", "v1.0.0-rc.1", "2026_09"] {
            validate_version(ok).unwrap();
        }
        for bad in ["", "-x", ".x", "a/b", "a b", "a#b", "v1+build"] {
            assert!(validate_version(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn basenames() {
        let path: Vec<String> = ["basic", "util", "nested"].map(String::from).into();
        assert_eq!(page_basename("latest", &path), "latest-basic-util-nested");
        assert_eq!(
            page_basename("v1.0.0-rc.1", &path[..1]),
            "v1.0.0-rc.1-basic"
        );
    }
}
