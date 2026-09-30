//! URLs for items that are not documented on the wiki: the standard library, and dependencies,
//! which are linked on [docs.rs](https://docs.rs).

use rustdoc_types::{Crate, Id, ItemKind};

/// The standard library is linked at its [stable docs](https://doc.rust-lang.org/stable/std/),
/// which is what most readers use.
const STABLE_ROOT: &str = "https://doc.rust-lang.org/stable/";

/// Crates whose docs live under [`STABLE_ROOT`].
fn is_sysroot(name: &str) -> bool {
    matches!(name, "std" | "core" | "alloc" | "proc_macro" | "test")
}

fn kind_prefix(kind: ItemKind) -> Option<&'static str> {
    Some(match kind {
        ItemKind::Struct => "struct",
        ItemKind::Enum => "enum",
        ItemKind::Union => "union",
        ItemKind::Trait => "trait",
        ItemKind::TraitAlias => "traitalias",
        ItemKind::TypeAlias => "type",
        ItemKind::Constant => "constant",
        ItemKind::Static => "static",
        ItemKind::Macro => "macro",
        ItemKind::ProcAttribute => "attr",
        ItemKind::ProcDerive => "derive",
        ItemKind::Function => "fn",
        ItemKind::Keyword => "keyword",
        ItemKind::ExternType => "foreigntype",
        _ => return None,
    })
}

/// The path segments of an intra-doc link destination as the author wrote it, e.g.
/// `` `std::collections::HashMap` `` gives `["std", "collections", "HashMap"]`.
fn written_path(written: &str) -> Vec<&str> {
    let s = written.trim().trim_matches('`');
    let s = s.split_once('@').map_or(s, |(_, rest)| rest);
    let s = s.split('#').next().unwrap_or(s);
    let s = s.trim_end_matches("()").trim_end_matches('!');
    s.split("::").collect()
}

/// A URL for `id`, which is not in the crate being documented, or `None` if we cannot build one.
///
/// `written` is the link destination as it appears in the docs. rustdoc reports the path where an
/// item is *defined*, which is often a private module (`std::collections::hash::map::HashMap`) and
/// not a page that exists. When the author wrote the public path, we prefer that.
pub fn url(krate: &Crate, id: Id, written: &str) -> Option<String> {
    let summary = krate.paths.get(&id)?;
    let ext = krate.external_crates.get(&summary.crate_id)?;
    let mut path = summary.path.clone();
    let name = path.last()?.clone();

    let sysroot = is_sysroot(&ext.name);
    // The URL of the crate's own docs, which every item's path continues from. A crate's
    // `html_root_url` (and docs.rs) is the directory that contains the crate's directory.
    let root = if sysroot {
        let top = if matches!(ext.name.as_str(), "std" | "core" | "alloc") {
            "std"
        } else {
            &ext.name
        };
        path[0] = top.to_string();
        format!("{STABLE_ROOT}{top}/")
    } else {
        let base = match ext.html_root_url.as_deref().filter(|u| !u.is_empty()) {
            Some(url) if url.ends_with('/') => url.to_string(),
            Some(url) => format!("{url}/"),
            None => format!("https://docs.rs/{}/latest/", ext.name),
        };
        format!("{base}{}/", ext.name)
    };

    if sysroot {
        let w = written_path(written);
        if w.len() >= 2
            && matches!(w[0], "std" | "core" | "alloc")
            && w.last().copied() == Some(name.as_str())
        {
            path = w.iter().map(|s| s.to_string()).collect();
            path[0] = "std".to_string();
        }
    }

    // Everything below the crate name, as it appears in the URL.
    let below = &path[1..];
    let join = |segments: &[String]| segments.join("/");

    if summary.kind == ItemKind::Primitive {
        return Some(format!("{root}primitive.{name}.html"));
    }
    if summary.kind == ItemKind::Module {
        let dir = if below.is_empty() {
            String::new()
        } else {
            format!("{}/", join(below))
        };
        return Some(format!("{root}{dir}index.html"));
    }

    // Items that live inside a type or trait: `Vec::push`, `Option::Some`, `Iterator::Item`.
    let member = match summary.kind {
        ItemKind::Function => Some("method"),
        ItemKind::Variant => Some("variant"),
        ItemKind::AssocConst => Some("associatedconstant"),
        ItemKind::AssocType => Some("associatedtype"),
        ItemKind::StructField => Some("structfield"),
        _ => None,
    };
    if let Some(member) = member {
        if summary.path.len() >= 3 || summary.kind == ItemKind::Variant {
            let parent_path = &summary.path[..summary.path.len() - 1];
            let parent_kind = krate
                .paths
                .values()
                .find(|s| s.crate_id == summary.crate_id && s.path == parent_path)
                .map(|s| s.kind);
            if let Some(parent_prefix) = parent_kind.and_then(kind_prefix) {
                let parent = &path[..path.len() - 1];
                let dirs = &parent[1..parent.len() - 1];
                let dir = if dirs.is_empty() {
                    String::new()
                } else {
                    format!("{}/", join(dirs))
                };
                let page = &parent[parent.len() - 1];
                return Some(format!(
                    "{root}{dir}{parent_prefix}.{page}.html#{member}.{name}"
                ));
            }
        }
    }

    let prefix = kind_prefix(summary.kind)?;
    let dirs = &below[..below.len().saturating_sub(1)];
    let dir = if dirs.is_empty() {
        String::new()
    } else {
        format!("{}/", join(dirs))
    };
    Some(format!("{root}{dir}{prefix}.{name}.html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_paths() {
        assert_eq!(
            written_path("`std::collections::HashMap`"),
            ["std", "collections", "HashMap"]
        );
        assert_eq!(written_path("struct@Foo"), ["Foo"]);
        assert_eq!(written_path("`Vec::push()`"), ["Vec", "push"]);
        assert_eq!(written_path("vec!"), ["vec"]);
        assert_eq!(written_path("crate::a#anchor"), ["crate", "a"]);
    }

    /// A crate with two external crates: `serde` (id 5, docs.rs) and `alloc` (id 3, standard).
    fn krate(serde_root: Option<&str>) -> Crate {
        serde_json::from_value(serde_json::json!({
            "root": 0, "crate_version": null, "includes_private": false, "index": {},
            "paths": {
                "1": {"crate_id": 5, "path": ["serde"], "kind": "module"},
                "2": {"crate_id": 5, "path": ["serde", "de"], "kind": "module"},
                "3": {"crate_id": 5, "path": ["serde", "de", "Deserialize"], "kind": "trait"},
                "4": {"crate_id": 5, "path": ["serde", "de", "Deserialize", "deserialize"], "kind": "function"},
                "5": {"crate_id": 5, "path": ["serde", "Value"], "kind": "enum"},
                "6": {"crate_id": 5, "path": ["serde", "Value", "Null"], "kind": "variant"},
                "7": {"crate_id": 3, "path": ["alloc", "vec", "Vec"], "kind": "struct"},
                "8": {"crate_id": 3, "path": ["alloc", "vec", "Vec", "push"], "kind": "function"},
                "9": {"crate_id": 1, "path": ["std", "collections", "hash", "map", "HashMap"], "kind": "struct"},
                "10": {"crate_id": 2, "path": ["u32"], "kind": "primitive"},
                "11": {"crate_id": 0, "path": ["mine", "Local"], "kind": "struct"}
            },
            "external_crates": {
                "1": {"name": "std", "html_root_url": null, "path": "/std"},
                "2": {"name": "core", "html_root_url": null, "path": "/core"},
                "3": {"name": "alloc", "html_root_url": null, "path": "/alloc"},
                "5": {"name": "serde", "html_root_url": serde_root, "path": "/serde"}
            },
            "target": {"triple": "x86_64-unknown-linux-gnu", "target_features": []},
            "format_version": rustdoc_types::FORMAT_VERSION
        }))
        .expect("a valid crate")
    }

    fn url_of(krate: &Crate, id: u32, written: &str) -> Option<String> {
        url(krate, Id(id), written)
    }

    #[test]
    fn dependencies_link_to_docs_rs_below_the_crate_name() {
        let k = krate(None);
        assert_eq!(
            url_of(&k, 1, "serde").as_deref(),
            Some("https://docs.rs/serde/latest/serde/index.html")
        );
        assert_eq!(
            url_of(&k, 2, "serde::de").as_deref(),
            Some("https://docs.rs/serde/latest/serde/de/index.html")
        );
        assert_eq!(
            url_of(&k, 3, "`serde::de::Deserialize`").as_deref(),
            Some("https://docs.rs/serde/latest/serde/de/trait.Deserialize.html")
        );
    }

    #[test]
    fn a_crates_html_root_url_is_used_when_it_has_one() {
        for root in [
            "https://docs.rs/serde/1.0.0",
            "https://docs.rs/serde/1.0.0/",
        ] {
            let k = krate(Some(root));
            assert_eq!(
                url_of(&k, 3, "Deserialize").as_deref(),
                Some("https://docs.rs/serde/1.0.0/serde/de/trait.Deserialize.html"),
                "{root}"
            );
        }
    }

    #[test]
    fn members_link_to_an_anchor_on_their_parent() {
        let k = krate(None);
        assert_eq!(
            url_of(&k, 4, "`Deserialize::deserialize`").as_deref(),
            Some("https://docs.rs/serde/latest/serde/de/trait.Deserialize.html#method.deserialize")
        );
        assert_eq!(
            url_of(&k, 6, "`Value::Null`").as_deref(),
            Some("https://docs.rs/serde/latest/serde/enum.Value.html#variant.Null")
        );
        assert_eq!(
            url_of(&k, 8, "`Vec::push`").as_deref(),
            Some("https://doc.rust-lang.org/stable/std/vec/struct.Vec.html#method.push")
        );
    }

    #[test]
    fn the_standard_library_is_linked_at_its_stable_docs() {
        let k = krate(None);
        assert_eq!(
            url_of(&k, 7, "`Vec`").as_deref(),
            Some("https://doc.rust-lang.org/stable/std/vec/struct.Vec.html")
        );
        assert_eq!(
            url_of(&k, 10, "u32").as_deref(),
            Some("https://doc.rust-lang.org/stable/std/primitive.u32.html")
        );
        // Defined in a private module; the path the author wrote is the page that exists.
        assert_eq!(
            url_of(&k, 9, "`std::collections::HashMap`").as_deref(),
            Some("https://doc.rust-lang.org/stable/std/collections/struct.HashMap.html")
        );
    }

    #[test]
    fn local_and_unknown_items_have_no_external_url() {
        let k = krate(None);
        assert_eq!(url_of(&k, 11, "Local"), None);
        assert_eq!(url_of(&k, 99, "Nope"), None);
    }
}
