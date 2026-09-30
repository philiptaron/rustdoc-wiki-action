//! Turns a rustdoc doc comment into Markdown that reads correctly on a wiki page.
//!
//! - [Intra-doc links] are rewritten to wiki or external URLs. Only the link syntax is touched:
//!   every other byte of the author's Markdown is left as written.
//! - Headings are shifted down so they nest under the item they document.
//! - Code fences get a plain language tag, and rustdoc's hidden `# ` lines are removed.
//!
//! [Intra-doc links]: https://doc.rust-lang.org/rustdoc/write-documentation/linking-to-items-by-name.html

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{BrokenLink, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
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

/// A link or image whose end has not been reached yet.
struct Open {
    dest: String,
    title: String,
    link_type: LinkType,
    image: bool,
    /// The span covering the link text, inside the brackets.
    inner: Option<Range<usize>>,
}

/// `url` written so that it survives as an inline link destination.
fn destination(url: &str) -> String {
    if url.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

/// The source of `span` with the edits inside it applied, taking them out of `edits`. A link can
/// contain another link or an image, and the inner one is finished first.
fn splice(docs: &str, span: Range<usize>, edits: &mut Vec<(Range<usize>, String)>) -> String {
    let mut nested = Vec::new();
    while edits
        .last()
        .is_some_and(|(range, _)| range.start >= span.start)
    {
        nested.extend(edits.pop());
    }
    let mut text = docs[span.clone()].to_string();
    nested.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, replacement) in nested {
        text.replace_range(
            range.start - span.start..range.end - span.start,
            &replacement,
        );
    }
    text
}

/// Rewrites the links in `docs`, and collects the text of its headings.
///
/// Every link that has a definition elsewhere in the text (`[text][label]`, `[label][]`, `[label]`
/// with a `[label]: url` line) becomes an inline link, and the definitions are removed. A wiki page
/// holds the docs of many items, and Markdown labels are global to a page, so a definition left in
/// one item's docs could capture a link in another's.
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

    // The definition lines, with the newline that ends each.
    let definitions: Vec<Range<usize>> = parser
        .reference_definitions()
        .iter()
        .map(|(_, definition)| {
            let span = definition.span.clone();
            let end = span.end
                + docs[span.end..]
                    .chars()
                    .next()
                    .filter(|&c| c == '\n')
                    .map_or(0, |_| 1);
            span.start..end
        })
        .collect();

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut headings = Vec::new();
    let mut heading: Option<String> = None;
    let mut image_depth = 0usize;
    let mut open: Vec<Open> = Vec::new();

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

        // Everything between a link's start and end is part of its text.
        let widen = |open: &mut Vec<Open>| {
            for link in open {
                link.inner = Some(match link.inner.take() {
                    None => range.clone(),
                    Some(r) => r.start.min(range.start)..r.end.max(range.end),
                });
            }
        };

        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            }) => {
                widen(&mut open);
                open.push(Open {
                    dest: dest_url.to_string(),
                    title: title.to_string(),
                    link_type,
                    image: false,
                    inner: None,
                });
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                ..
            }) => {
                widen(&mut open);
                open.push(Open {
                    dest: dest_url.to_string(),
                    title: title.to_string(),
                    link_type,
                    image: true,
                    inner: None,
                });
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                let Some(link) = open.pop() else { continue };
                let has_definition = matches!(
                    link.link_type,
                    LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut
                );
                let intra_doc = links.get(&link.dest).filter(|_| !link.image);
                if intra_doc.is_none() && !has_definition {
                    continue;
                }
                let text = match link.inner {
                    Some(inner) => splice(docs, inner, &mut edits),
                    None => String::new(),
                };
                let replacement = match intra_doc {
                    Some(&id) => match resolve(&link.dest, id) {
                        Some(url) => format!("[{text}]({})", destination(&url)),
                        None => text,
                    },
                    None => {
                        let bang = if link.image { "!" } else { "" };
                        let title = if link.title.is_empty() {
                            String::new()
                        } else {
                            format!(" \"{}\"", link.title.replace('"', "\\\""))
                        };
                        format!("{bang}[{text}]({}{title})", destination(&link.dest))
                    }
                };
                // The event for a collapsed reference, `[label][]`, stops before the `[]`.
                let mut range = range;
                if link.link_type == LinkType::Collapsed && docs[range.end..].starts_with("[]") {
                    range.end += 2;
                }
                edits.push((range, replacement));
            }
            _ => widen(&mut open),
        }
    }

    edits.extend(definitions.into_iter().map(|range| (range, String::new())));
    edits.sort_by_key(|(range, _)| range.start);
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
Shortcut [`Foo`](page#anchor1), inline [text](page#anchor2), full [ref text](page#anchor3), and `gone`."
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

    #[test]
    fn inlines_reference_links_and_removes_their_definitions() {
        let docs =
            "See [the guide][g], [g][] and [g].\n\n[g]: https://example.com/guide \"The guide\"\n";
        let inline = "(https://example.com/guide \"The guide\")";
        assert_eq!(
            run(docs, &[], 0).markdown,
            format!("See [the guide]{inline}, [g]{inline} and [g]{inline}.")
        );
    }

    #[test]
    fn a_definition_in_one_item_cannot_capture_a_link_in_another() {
        // Two items on one wiki page, each with its own meaning for `[x]`.
        let a = run("Go [here][x].\n\n[x]: https://a.example\n", &[], 0).markdown;
        let b = run("Go [there][x].\n\n[x]: https://b.example\n", &[], 0).markdown;
        assert_eq!(
            format!("{a}\n\n{b}"),
            "Go [here](https://a.example).\n\nGo [there](https://b.example)."
        );
    }

    #[test]
    fn inlines_reference_images() {
        let docs = "![logo][l]\n\n[l]: https://example.com/l.png\n";
        assert_eq!(
            run(docs, &[], 0).markdown,
            "![logo](https://example.com/l.png)"
        );
    }

    #[test]
    fn intra_doc_definitions_are_resolved_and_removed() {
        let docs = "[a link][helper]\n\n[helper]: crate::util::helper\n";
        let p = run(docs, &[("crate::util::helper", 1)], 0);
        assert_eq!(p.markdown, "[a link](page#anchor1)");
    }

    #[test]
    fn destinations_with_spaces_stay_valid() {
        let docs = "[x][r]\n\n[r]: <https://example.com/a b>\n";
        assert_eq!(run(docs, &[], 0).markdown, "[x](<https://example.com/a b>)");
    }

    #[test]
    fn a_link_around_an_image_keeps_both_rewrites() {
        let docs = "[![logo][l]](crate::bar)\n\n[l]: https://example.com/l.png\n";
        let p = run(docs, &[("crate::bar", 2)], 0);
        assert_eq!(
            p.markdown,
            "[![logo](https://example.com/l.png)](page#anchor2)"
        );
    }

    #[test]
    fn unresolved_brackets_are_not_turned_into_links() {
        let docs = "An array [T; N], an index [0] and a [missing] link.";
        assert_eq!(run(docs, &[], 0).markdown, docs);
    }
}
