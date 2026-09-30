//! URLs for items that are not documented on the wiki: the standard library and dependencies.

use rustdoc_types::{Crate, Id, ItemKind};

/// The standard library is linked at its stable docs, which is what most readers use.
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
    let root = if sysroot {
        let top = if matches!(ext.name.as_str(), "std" | "core" | "alloc") {
            "std"
        } else {
            &ext.name
        };
        path[0] = top.to_string();
        format!("{STABLE_ROOT}{top}/")
    } else {
        match ext.html_root_url.as_deref().filter(|u| !u.is_empty()) {
            Some(url) if url.ends_with('/') => url.to_string(),
            Some(url) => format!("{url}/"),
            None => format!("https://docs.rs/{}/latest/", ext.name),
        }
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
}
