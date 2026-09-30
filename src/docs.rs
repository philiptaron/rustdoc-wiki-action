//! Turns a rustdoc doc comment into Markdown that reads correctly on a wiki page.
//!
//! - Intra-doc links are rewritten to wiki or external URLs. Only the link syntax is touched: every
//!   other byte of the author's Markdown is left as written.
//! - Headings are shifted down so they nest under the item they document.
//! - Code fences get a plain language tag, and rustdoc's hidden `# ` lines are removed.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{BrokenLink, CowStr, Event, Options, Parser, Tag, TagEnd};
use rustdoc_types::Id;

pub struct Processed {
    pub markdown: String,
    /// The plain text of every heading in the original docs, in document order. GitHub numbers
    /// repeated anchors in document order, so the page needs to know about these.
    pub headings: Vec<String>,
}

/// `resolve` gets the link destination as written and the item it resolved to. It returns the URL
/// to link to, or `None` if the target has no home, in which case the link text is kept as plain
/// text.
pub fn process(
    docs: &str,
    links: &HashMap<String, Id>,
    heading_shift: usize,
    resolve: &mut dyn FnMut(&str, Id) -> Option<String>,
) -> Processed {
    let (rewritten, headings) = rewrite_links(docs, links, resolve);
    Processed {
        markdown: fix_lines(&rewritten, heading_shift),
        headings,
    }
}

/// The first paragraph of `docs` on one line, for use in summary lists.
pub fn summary(docs: &str) -> String {
    docs.split("\n\n")
        .next()
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

struct OpenLink {
    dest: String,
    /// The span covering the link text, inside the brackets.
    inner: Option<Range<usize>>,
}

fn rewrite_links(
    docs: &str,
    links: &HashMap<String, Id>,
    resolve: &mut dyn FnMut(&str, Id) -> Option<String>,
) -> (String, Vec<String>) {
    // Like rustdoc, treat an unresolved `[Foo]` as a link whose destination is `Foo`.
    let mut broken = |link: BrokenLink<'_>| {
        Some((
            CowStr::from(link.reference.to_string()),
            CowStr::from(String::new()),
        ))
    };
    let parser = Parser::new_with_broken_link_callback(docs, options(), Some(&mut broken));

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut headings = Vec::new();
    let mut heading: Option<String> = None;
    let mut image_depth = 0usize;
    let mut open: Option<OpenLink> = None;

    for (event, range) in parser.into_offset_iter() {
        match &event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => headings.push(heading.take().unwrap_or_default()),
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth -= 1,
            Event::Text(t) | Event::Code(t) if image_depth == 0 => {
                if let Some(h) = &mut heading {
                    h.push_str(t);
                }
            }
            _ => {}
        }

        match &event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                open = Some(OpenLink {
                    dest: dest_url.to_string(),
                    inner: None,
                });
            }
            Event::End(TagEnd::Link) => {
                let Some(link) = open.take() else { continue };
                let Some(&id) = links.get(&link.dest) else {
                    continue;
                };
                let text = link.inner.map(|r| &docs[r]).unwrap_or_default();
                let replacement = match resolve(&link.dest, id) {
                    Some(url) => format!("[{text}]({url})"),
                    None => text.to_string(),
                };
                edits.push((range, replacement));
            }
            _ => {
                if let Some(link) = &mut open {
                    link.inner = Some(match link.inner.take() {
                        None => range,
                        Some(r) => r.start.min(range.start)..r.end.max(range.end),
                    });
                }
            }
        }
    }

    let mut out = docs.to_string();
    for (range, replacement) in edits.into_iter().rev() {
        out.replace_range(range, &replacement);
    }
    (out, headings)
}

struct Fence {
    marker: char,
    len: usize,
    rust: bool,
}

/// Opening fence: up to three spaces of indentation, then three or more backticks or tildes.
fn open_fence(line: &str) -> Option<(usize, char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = rest.chars().take_while(|&c| c == marker).count();
    if len < 3 {
        return None;
    }
    let info = &rest[len..];
    // A backtick fence's info string cannot contain backticks (that would be inline code).
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some((indent, marker, len, info.trim()))
}

fn closes(line: &str, fence: &Fence) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = line[indent..].trim_end();
    indent <= 3 && rest.len() >= fence.len && rest.chars().all(|c| c == fence.marker)
}

fn is_rust_attribute(token: &str) -> bool {
    matches!(
        token,
        "rust"
            | "ignore"
            | "should_panic"
            | "no_run"
            | "compile_fail"
            | "test_harness"
            | "standalone_crate"
            | "allow_fail"
    ) || token.starts_with("edition")
        || token.starts_with("ignore-")
        || (token.starts_with('E') && token[1..].chars().all(|c| c.is_ascii_digit()))
}

/// The tag to show on a fence, and whether the block is Rust (so hidden lines apply).
fn normalize_info(info: &str) -> (String, bool) {
    let tokens = info
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty());
    match tokens.clone().find(|t| !is_rust_attribute(t)) {
        None => ("rust".into(), true),
        Some(language) => (language.trim_matches(['{', '}', '.']).to_string(), false),
    }
}

/// rustdoc's rule for a line in a Rust code block: `# code` is hidden, `## code` shows `# code`.
enum Line<'a> {
    Shown(std::borrow::Cow<'a, str>),
    Hidden,
}

fn map_line(line: &str) -> Line<'_> {
    let trimmed = line.trim();
    if trimmed.starts_with("##") {
        Line::Shown(line.replacen("##", "#", 1).into())
    } else if trimmed == "#" || trimmed.starts_with("# ") {
        Line::Hidden
    } else {
        Line::Shown(line.into())
    }
}

/// `#`..`######` followed by a space or the end of the line, with up to three spaces of indent.
fn heading_level(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    let after = &rest[hashes..];
    ((1..=6).contains(&hashes) && (after.is_empty() || after.starts_with([' ', '\t'])))
        .then_some(hashes)
}

fn fix_lines(text: &str, shift: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<Fence> = None;

    for line in text.lines() {
        if let Some(f) = &fence {
            if closes(line, f) {
                out.push_str(line);
                fence = None;
            } else if f.rust {
                match map_line(line) {
                    Line::Hidden => continue,
                    Line::Shown(l) => out.push_str(&l),
                }
            } else {
                out.push_str(line);
            }
        } else if let Some((indent, marker, len, info)) = open_fence(line) {
            let (tag, rust) = normalize_info(info);
            out.push_str(&" ".repeat(indent));
            out.push_str(&marker.to_string().repeat(len));
            out.push_str(&tag);
            fence = Some(Fence { marker, len, rust });
        } else if let Some(level) = heading_level(line) {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let new_level = (level + shift).min(6);
            out.push_str(&line[..indent]);
            out.push_str(&"#".repeat(new_level));
            out.push_str(&line[indent + level..]);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(pairs: &[(&str, u32)]) -> HashMap<String, Id> {
        pairs.iter().map(|(k, v)| (k.to_string(), Id(*v))).collect()
    }

    /// Resolves ids 1..=9 to `page#anchorN`, everything else to nothing.
    fn run(docs: &str, pairs: &[(&str, u32)], shift: usize) -> Processed {
        process(docs, &links(pairs), shift, &mut |_, id| {
            (id.0 <= 9).then(|| format!("page#anchor{}", id.0))
        })
    }

    #[test]
    fn rewrites_every_link_syntax() {
        let docs = "\
Shortcut [`Foo`], inline [text](crate::bar), full [ref text][r], and [`gone`].

[r]: crate::baz
";
        let p = run(
            docs,
            &[
                ("`Foo`", 1),
                ("crate::bar", 2),
                ("crate::baz", 3),
                ("`gone`", 99),
            ],
            0,
        );
        assert_eq!(
            p.markdown,
            "\
Shortcut [`Foo`](page#anchor1), inline [text](page#anchor2), full [ref text](page#anchor3), and `gone`.

[r]: crate::baz"
        );
    }

    #[test]
    fn leaves_other_links_alone() {
        let docs = "See [the web](https://example.com), [0] and <https://example.org>.";
        assert_eq!(run(docs, &[], 0).markdown, docs);
    }

    #[test]
    fn shifts_headings_but_not_code() {
        let docs =
            "# Errors\n\n```text\n# not a heading\n```\n\n## Panics\n\n    # indented code\n";
        let p = run(docs, &[], 3);
        assert_eq!(
            p.markdown,
            "#### Errors\n\n```text\n# not a heading\n```\n\n##### Panics\n\n    # indented code"
        );
        assert_eq!(p.headings, ["Errors", "Panics"]);
    }

    #[test]
    fn heading_shift_is_capped_at_six() {
        assert_eq!(run("#### Deep", &[], 5).markdown, "###### Deep");
    }

    #[test]
    fn strips_hidden_lines_and_unescapes_double_hash() {
        let docs = "```\n# use basic::Config;\n# let hidden = 1;\nlet c = 1;\n## shown\n#\n```";
        assert_eq!(
            run(docs, &[], 0).markdown,
            "```rust\nlet c = 1;\n# shown\n```"
        );
    }

    #[test]
    fn keeps_hash_lines_in_other_languages() {
        let docs = "```toml\n# comment\n[package]\n```";
        assert_eq!(run(docs, &[], 0).markdown, docs);
    }

    #[test]
    fn normalizes_fence_info() {
        for info in [
            "",
            "rust",
            "no_run",
            "ignore,rust",
            "should_panic",
            "edition2021",
            "compile_fail,E0308",
        ] {
            let p = run(&format!("```{info}\nx\n```"), &[], 0);
            assert_eq!(p.markdown, "```rust\nx\n```", "{info:?}");
        }
        assert_eq!(run("```text\nx\n```", &[], 0).markdown, "```text\nx\n```");
        assert_eq!(run("~~~sh\nx\n~~~", &[], 0).markdown, "~~~sh\nx\n~~~");
    }

    #[test]
    fn collects_heading_text_without_formatting() {
        let p = run("# A `b` **c**\n\nSetext\n======\n", &[], 0);
        assert_eq!(p.headings, ["A b c", "Setext"]);
    }

    #[test]
    fn summaries() {
        assert_eq!(
            summary("First line\nsecond line.\n\nMore."),
            "First line second line."
        );
        assert_eq!(summary(""), "");
    }
}
