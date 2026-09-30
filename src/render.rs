//! Renders a rustdoc JSON crate as wiki pages, one page per module.
//!
//! Rendering happens in two stages:
//!
//! 1. **Plan.** Walk the module tree and decide which page every item lives on ("home"), expanding
//!    `pub use` re-exports. An item that is only reachable through a re-export from a private
//!    module is documented where it is re-exported, like rustdoc does.
//! 2. **Emit.** Write the Markdown. A link to an item needs the anchor GitHub will give its heading,
//!    which depends on every heading before it on that page. So the pages are emitted twice: the
//!    first pass records each item's anchor, and the second pass uses them.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use rustdoc_types::{
    Crate, FORMAT_VERSION, Id, Impl, Item, ItemEnum, MacroKind, StructKind, Type, VariantKind,
    Visibility,
};

use crate::docs;
use crate::external;
use crate::names::{self, Slugger};
use crate::sig;

pub struct Options {
    pub version: String,
    /// `https://github.com/<owner>/<repo>/blob/<sha>`. Items link to their source when this is set.
    pub source_base: Option<String>,
}

pub struct Page {
    pub basename: String,
    pub markdown: String,
}

/// Parses rustdoc JSON and checks that its `format_version` is the one this build understands.
pub fn load(json: &[u8]) -> Result<Crate> {
    let unsupported = |found: u64| {
        anyhow::anyhow!(
            "unsupported rustdoc JSON format_version {found}: this build understands version \
             {FORMAT_VERSION}. Build the docs with the nightly toolchain this action pins."
        )
    };
    match serde_json::from_slice::<Crate>(json) {
        Ok(krate) if krate.format_version == FORMAT_VERSION => Ok(krate),
        Ok(krate) => Err(unsupported(krate.format_version.into())),
        Err(error) => {
            let found = serde_json::from_slice::<serde_json::Value>(json)
                .ok()
                .and_then(|v| v.get("format_version")?.as_u64());
            match found {
                Some(v) if v != u64::from(FORMAT_VERSION) => Err(unsupported(v)),
                _ => Err(error).context("parsing rustdoc JSON"),
            }
        }
    }
}

pub fn render(krate: &Crate, options: &Options) -> Result<Vec<Page>> {
    names::validate_version(&options.version)?;
    let model = Model::plan(krate, options)?;
    let (_, anchors) = model.emit(&HashMap::new());
    let (mut pages, _) = model.emit(&anchors);
    // Module order in the JSON is an accident of rustdoc's traversal; sort so that output is stable.
    pages.sort_by(|a, b| a.basename.cmp(&b.basename));
    Ok(pages)
}

struct PageSpec {
    module: Id,
    /// The module path, crate name first.
    path: Vec<String>,
    basename: String,
    /// Items defined in this module.
    defs: Vec<Id>,
    submodules: Vec<Id>,
    reexports: Vec<ReExport>,
    /// Items documented here because this is where they are re-exported, with their public name.
    inlined: Vec<(Id, String)>,
}

struct ReExport {
    source: String,
    is_glob: bool,
    /// The public names this `use` brings in, and what they resolve to.
    entries: Vec<(String, Id)>,
}

struct Model<'a> {
    krate: &'a Crate,
    version: &'a str,
    source_base: Option<&'a str>,
    pages: Vec<PageSpec>,
    module_page: HashMap<Id, usize>,
    /// The page that holds each item's heading.
    home: HashMap<Id, usize>,
    /// Where to link for items that have no heading of their own: fields, variants, and the
    /// members of trait impls.
    parent: HashMap<Id, Id>,
}

const GROUPS: [&str; 9] = [
    "Macros",
    "Structs",
    "Enums",
    "Unions",
    "Traits",
    "Functions",
    "Type aliases",
    "Constants",
    "Statics",
];

fn group_of(item: &Item) -> Option<usize> {
    Some(match &item.inner {
        ItemEnum::Macro(_) | ItemEnum::ProcMacro(_) => 0,
        ItemEnum::Struct(_) => 1,
        ItemEnum::Enum(_) => 2,
        ItemEnum::Union(_) => 3,
        ItemEnum::Trait(_) => 4,
        ItemEnum::Function(_) => 5,
        ItemEnum::TypeAlias(_) => 6,
        ItemEnum::Constant { .. } => 7,
        ItemEnum::Static(_) => 8,
        _ => return None,
    })
}

impl<'a> Model<'a> {
    fn plan(krate: &'a Crate, options: &'a Options) -> Result<Self> {
        let mut model = Model {
            krate,
            version: &options.version,
            source_base: options.source_base.as_deref(),
            pages: Vec::new(),
            module_page: HashMap::new(),
            home: HashMap::new(),
            parent: HashMap::new(),
        };
        let root = krate
            .index
            .get(&krate.root)
            .context("root module missing from index")?;
        let name = root.name.clone().context("root module has no name")?;
        model.visit_module(krate.root, vec![name]);
        model.resolve_reexports();
        Ok(model)
    }

    fn visible(&self, item: &Item) -> bool {
        matches!(item.visibility, Visibility::Public) || self.krate.includes_private
    }

    fn visit_module(&mut self, id: Id, path: Vec<String>) -> usize {
        let krate = self.krate;
        let page = self.pages.len();
        self.pages.push(PageSpec {
            module: id,
            basename: names::page_basename(self.version, &path),
            path: path.clone(),
            defs: Vec::new(),
            submodules: Vec::new(),
            reexports: Vec::new(),
            inlined: Vec::new(),
        });
        self.module_page.insert(id, page);

        let ItemEnum::Module(module) = &krate.index[&id].inner else {
            unreachable!()
        };
        for &child in &module.items {
            let Some(item) = krate.index.get(&child) else {
                continue;
            };
            if !self.visible(item) {
                continue;
            }
            match &item.inner {
                ItemEnum::Module(m) => {
                    if m.is_stripped {
                        continue;
                    }
                    let Some(name) = &item.name else { continue };
                    let mut child_path = path.clone();
                    child_path.push(name.clone());
                    self.visit_module(child, child_path);
                    self.pages[page].submodules.push(child);
                }
                ItemEnum::Use(_) | ItemEnum::Impl(_) => {}
                _ if group_of(item).is_some() => {
                    self.register(page, child);
                    self.pages[page].defs.push(child);
                }
                _ => {}
            }
        }
        page
    }

    /// Records `id` as living on `page`, along with the members that get their own heading there.
    fn register(&mut self, page: usize, id: Id) {
        self.home.insert(id, page);
        let krate = self.krate;
        let Some(item) = krate.index.get(&id) else {
            return;
        };
        match &item.inner {
            ItemEnum::Struct(s) => {
                match &s.kind {
                    StructKind::Unit => {}
                    StructKind::Tuple(fields) => {
                        for field in fields.iter().flatten() {
                            self.parent.insert(*field, id);
                        }
                    }
                    StructKind::Plain { fields, .. } => {
                        for field in fields {
                            self.parent.insert(*field, id);
                        }
                    }
                }
                self.register_impls(page, id, &s.impls);
            }
            ItemEnum::Union(u) => {
                for field in &u.fields {
                    self.parent.insert(*field, id);
                }
                self.register_impls(page, id, &u.impls);
            }
            ItemEnum::Enum(e) => {
                for variant in &e.variants {
                    self.parent.insert(*variant, id);
                    let Some(Item {
                        inner: ItemEnum::Variant(v),
                        ..
                    }) = krate.index.get(variant)
                    else {
                        continue;
                    };
                    let fields: Vec<Id> = match &v.kind {
                        VariantKind::Plain => vec![],
                        VariantKind::Tuple(fields) => fields.iter().flatten().copied().collect(),
                        VariantKind::Struct { fields, .. } => fields.clone(),
                    };
                    for field in fields {
                        self.parent.insert(field, id);
                    }
                }
                self.register_impls(page, id, &e.impls);
            }
            ItemEnum::Trait(t) => {
                for member in &t.items {
                    self.home.insert(*member, page);
                }
            }
            _ => {}
        }
    }

    fn register_impls(&mut self, page: usize, ty: Id, impls: &[Id]) {
        let krate = self.krate;
        for imp in impls {
            let Some(Item {
                inner: ItemEnum::Impl(i),
                ..
            }) = krate.index.get(imp)
            else {
                continue;
            };
            for member in &i.items {
                if i.trait_.is_none() {
                    self.home.insert(*member, page);
                } else {
                    self.parent.insert(*member, ty);
                }
            }
        }
    }

    /// The names a glob import of module or enum `target` brings in.
    fn glob_entries(&self, target: Id, seen: &mut HashSet<Id>) -> Vec<(String, Id)> {
        let krate = self.krate;
        let mut out = Vec::new();
        if !seen.insert(target) {
            return out;
        }
        let Some(item) = krate.index.get(&target) else {
            return out;
        };
        match &item.inner {
            ItemEnum::Module(m) => {
                for child in &m.items {
                    let Some(c) = krate.index.get(child) else {
                        continue;
                    };
                    if !self.visible(c) {
                        continue;
                    }
                    match &c.inner {
                        ItemEnum::Use(u) if u.is_glob => {
                            if let Some(id) = u.id {
                                out.extend(self.glob_entries(id, seen));
                            }
                        }
                        ItemEnum::Use(u) => out.extend(u.id.map(|id| (u.name.clone(), id))),
                        ItemEnum::Impl(_) => {}
                        _ => out.extend(c.name.clone().map(|n| (n, *child))),
                    }
                }
            }
            ItemEnum::Enum(e) => {
                for variant in &e.variants {
                    if let Some(name) = krate.index.get(variant).and_then(|v| v.name.clone()) {
                        out.push((name, *variant));
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn is_inlinable(item: &Item) -> bool {
        item.crate_id == 0 && group_of(item).is_some()
    }

    fn resolve_reexports(&mut self) {
        let krate = self.krate;
        for page in 0..self.pages.len() {
            let ItemEnum::Module(module) = &krate.index[&self.pages[page].module].inner else {
                continue;
            };
            for &child in &module.items {
                let Some(item) = krate.index.get(&child) else {
                    continue;
                };
                let ItemEnum::Use(u) = &item.inner else {
                    continue;
                };
                if !self.visible(item) {
                    continue;
                }
                let raw: Vec<(String, Id)> = match (u.is_glob, u.id) {
                    (true, Some(target)) => self.glob_entries(target, &mut HashSet::new()),
                    (false, Some(target)) => vec![(u.name.clone(), target)],
                    _ => vec![],
                };
                let mut entries = Vec::new();
                for (name, target) in raw {
                    let known =
                        self.module_page.contains_key(&target) || self.home.contains_key(&target);
                    match krate.index.get(&target) {
                        Some(t) if !known && Self::is_inlinable(t) => {
                            self.register(page, target);
                            self.pages[page].inlined.push((target, name));
                        }
                        _ => entries.push((name, target)),
                    }
                }
                if !entries.is_empty() || u.is_glob {
                    self.pages[page].reexports.push(ReExport {
                        source: u.source.clone(),
                        is_glob: u.is_glob,
                        entries,
                    });
                }
            }
        }
    }

    /// Where to link for `id`: a wiki page and anchor, or an external URL.
    fn href(&self, anchors: &HashMap<Id, String>, id: Id, written: &str) -> Option<String> {
        let mut current = id;
        loop {
            if let Some(&page) = self.module_page.get(&current) {
                return Some(self.pages[page].basename.clone());
            }
            if let Some(&page) = self.home.get(&current) {
                let base = &self.pages[page].basename;
                return Some(match anchors.get(&current) {
                    Some(anchor) => format!("{base}#{anchor}"),
                    None => base.clone(),
                });
            }
            match self.parent.get(&current) {
                Some(&parent) => current = parent,
                None => break,
            }
        }
        external::url(self.krate, id, written)
    }

    fn emit(&self, anchors_in: &HashMap<Id, String>) -> (Vec<Page>, HashMap<Id, String>) {
        let mut anchors_out = HashMap::new();
        let pages = self
            .pages
            .iter()
            .map(|spec| {
                let mut emit = Emit {
                    m: self,
                    anchors_in,
                    anchors_out: HashMap::new(),
                    slug: Slugger::default(),
                };
                let markdown = emit.module_page(spec);
                anchors_out.extend(emit.anchors_out);
                Page {
                    basename: spec.basename.clone(),
                    markdown,
                }
            })
            .collect();
        (pages, anchors_out)
    }
}

struct Emit<'a> {
    m: &'a Model<'a>,
    anchors_in: &'a HashMap<Id, String>,
    anchors_out: HashMap<Id, String>,
    slug: Slugger,
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn code_block(lines: &str) -> String {
    format!("```rust\n{lines}\n```")
}

impl Emit<'_> {
    fn href(&self, id: Id, written: &str) -> Option<String> {
        self.m.href(self.anchors_in, id, written)
    }

    fn section(&mut self, title: &str) -> String {
        self.slug.anchor(title);
        format!("## {title}")
    }

    /// A heading for an item, whose anchor other pages can link to.
    fn heading(&mut self, level: usize, id: Id, kind: &str, name: &str) -> String {
        let anchor = self.slug.anchor(&format!("{kind} {name}"));
        self.anchors_out.insert(id, anchor);
        format!("{} {kind} `{name}`", "#".repeat(level))
    }

    /// The item's docs as wiki Markdown, with headings shifted down by `shift`.
    fn docs(&mut self, item: &Item, shift: usize) -> Option<String> {
        let text = item.docs.as_deref().filter(|d| !d.trim().is_empty())?;
        let processed = docs::process(text, &item.links, shift, &mut |dest, id| {
            self.href(id, dest)
        });
        for heading in &processed.headings {
            self.slug.anchor(heading);
        }
        Some(processed.markdown)
    }

    /// The first paragraph of the docs, on one line, with links resolved.
    fn summary(&self, item: &Item) -> String {
        let Some(text) = item.docs.as_deref() else {
            return String::new();
        };
        let first = docs::summary(text);
        docs::process(&first, &item.links, 0, &mut |dest, id| self.href(id, dest)).markdown
    }

    fn source_link(&self, item: &Item) -> Option<String> {
        let base = self.m.source_base?;
        let span = item.span.as_ref().filter(|_| item.crate_id == 0)?;
        let file = span.filename.to_string_lossy().replace('\\', "/");
        if file.starts_with('/') || file.split('/').any(|part| part == "..") {
            return None;
        }
        let lines = if span.begin.0 == span.end.0 {
            format!("L{}", span.begin.0)
        } else {
            format!("L{}-L{}", span.begin.0, span.end.0)
        };
        Some(format!("[source]({base}/{file}#{lines})"))
    }

    fn deprecation(item: &Item) -> Option<String> {
        let d = item.deprecation.as_ref()?;
        let mut text = String::from("> **Deprecated**");
        if let Some(since) = &d.since {
            text.push_str(&format!(" since {since}"));
        }
        if let Some(note) = &d.note {
            text.push_str(&format!(": {}", note.replace('\n', " ")));
        }
        Some(text)
    }

    fn module_page(&mut self, spec: &PageSpec) -> String {
        let krate = self.m.krate;
        let module = &krate.index[&spec.module];
        let title = spec.path.join("::");
        self.slug.anchor(&title);

        let mut blocks = vec![format!("# `{title}`")];
        if spec.path.len() == 1 {
            if let Some(version) = &krate.crate_version {
                blocks.push(format!("Version {version}"));
            }
        }
        blocks.extend(self.docs(module, 1));

        if !spec.submodules.is_empty() {
            blocks.push(self.section("Modules"));
            let mut submodules: Vec<(String, Id)> = spec
                .submodules
                .iter()
                .map(|&id| (krate.index[&id].name.clone().unwrap_or_default(), id))
                .collect();
            submodules.sort_by_key(|(name, _)| (name.to_lowercase(), name.clone()));
            let lines: Vec<String> = submodules
                .iter()
                .map(|(name, id)| {
                    let page = &self.m.pages[self.m.module_page[id]];
                    let summary = self.summary(&krate.index[id]);
                    let summary = if summary.is_empty() {
                        String::new()
                    } else {
                        format!(" — {summary}")
                    };
                    format!("- [`{name}`]({}){summary}", page.basename)
                })
                .collect();
            blocks.push(lines.join("\n"));
        }

        if !spec.reexports.is_empty() {
            blocks.push(self.section("Re-exports"));
            let lines: Vec<String> = spec
                .reexports
                .iter()
                .map(|r| {
                    let links: Vec<String> = r
                        .entries
                        .iter()
                        .map(|(name, id)| match self.href(*id, &r.source) {
                            Some(url) => format!("[`{name}`]({url})"),
                            None => format!("`{name}`"),
                        })
                        .collect();
                    let source = format!(
                        "`pub use {}{}`",
                        r.source,
                        if r.is_glob { "::*" } else { "" }
                    );
                    if links.is_empty() {
                        format!("- {source}")
                    } else {
                        format!("- {source} — {}", links.join(", "))
                    }
                })
                .collect();
            blocks.push(lines.join("\n"));
        }

        let mut groups: Vec<Vec<(Id, String)>> = vec![Vec::new(); GROUPS.len()];
        let named = spec
            .defs
            .iter()
            .filter_map(|&id| Some((id, krate.index[&id].name.clone()?)))
            .chain(spec.inlined.iter().cloned());
        for (id, name) in named {
            if let Some(group) = group_of(&krate.index[&id]) {
                groups[group].push((id, name));
            }
        }
        for (title, mut entries) in GROUPS.iter().zip(groups) {
            if entries.is_empty() {
                continue;
            }
            entries.sort_by_key(|(_, name)| (name.to_lowercase(), name.clone()));
            blocks.push(self.section(title));
            for (id, name) in entries {
                blocks.push(self.item(id, &name));
            }
        }

        let mut out = blocks.join("\n\n");
        out.push('\n');
        out
    }

    /// The heading, signature, docs and members of one top-level item.
    fn item(&mut self, id: Id, name: &str) -> String {
        let krate = self.m.krate;
        let item = &krate.index[&id];
        let (kind, display) = match &item.inner {
            ItemEnum::Struct(_) => ("struct", name.to_string()),
            ItemEnum::Enum(_) => ("enum", name.to_string()),
            ItemEnum::Union(_) => ("union", name.to_string()),
            ItemEnum::Trait(_) => ("trait", name.to_string()),
            ItemEnum::Function(_) => ("fn", name.to_string()),
            ItemEnum::TypeAlias(_) => ("type", name.to_string()),
            ItemEnum::Constant { .. } => ("const", name.to_string()),
            ItemEnum::Static(_) => ("static", name.to_string()),
            ItemEnum::Macro(_) => ("macro", format!("{name}!")),
            ItemEnum::ProcMacro(pm) => match pm.kind {
                MacroKind::Bang => ("macro", format!("{name}!")),
                MacroKind::Attr => ("attribute macro", name.to_string()),
                MacroKind::Derive => ("derive macro", name.to_string()),
            },
            _ => unreachable!("only grouped items reach here"),
        };

        let mut blocks = vec![self.heading(3, id, kind, &display)];
        blocks.push(code_block(&self.signature(item, name)));
        blocks.extend(Self::deprecation(item));
        blocks.extend(self.source_link(item));
        blocks.extend(self.docs(item, 3));
        blocks.extend(self.members(id, item, name));
        blocks.join("\n\n")
    }

    fn attribute_lines(item: &Item) -> String {
        let attrs = sig::attributes(&item.attrs);
        if attrs.is_empty() {
            String::new()
        } else {
            format!("{}\n", attrs.join("\n"))
        }
    }

    fn field_type(&self, id: Id) -> Option<&Type> {
        match &self.m.krate.index.get(&id)?.inner {
            ItemEnum::StructField(t) => Some(t),
            _ => None,
        }
    }

    fn plain_fields(&self, fields: &[Id], stripped: bool) -> String {
        let mut lines: Vec<String> = fields
            .iter()
            .filter_map(|&id| {
                let field = self.m.krate.index.get(&id)?;
                let ty = self.field_type(id)?;
                Some(format!(
                    "    {}{}: {},",
                    sig::visibility(&field.visibility),
                    field.name.as_deref().unwrap_or("_"),
                    sig::ty(ty)
                ))
            })
            .collect();
        if stripped {
            lines.push("    // some fields omitted".into());
        }
        if lines.is_empty() {
            "{}".into()
        } else {
            format!("{{\n{}\n}}", lines.join("\n"))
        }
    }

    fn tuple_fields(&self, fields: &[Option<Id>]) -> String {
        let parts: Vec<String> = fields
            .iter()
            .map(|field| {
                let Some(id) = field else {
                    return "/* private */".to_string();
                };
                let vis = self
                    .m
                    .krate
                    .index
                    .get(id)
                    .map(|f| sig::visibility(&f.visibility));
                match self.field_type(*id) {
                    Some(ty) => format!("{}{}", vis.unwrap_or_default(), sig::ty(ty)),
                    None => "_".to_string(),
                }
            })
            .collect();
        format!("({})", parts.join(", "))
    }

    fn signature(&self, item: &Item, name: &str) -> String {
        let attrs = Self::attribute_lines(item);
        let vis = sig::visibility(&item.visibility);
        let body = match &item.inner {
            ItemEnum::Struct(s) => {
                let generics = sig::generics_decl(&s.generics);
                match &s.kind {
                    StructKind::Unit => {
                        format!(
                            "{vis}struct {name}{generics}{};",
                            sig::where_inline(&s.generics)
                        )
                    }
                    StructKind::Tuple(fields) => format!(
                        "{vis}struct {name}{generics}{}{};",
                        self.tuple_fields(fields),
                        sig::where_inline(&s.generics)
                    ),
                    StructKind::Plain {
                        fields,
                        has_stripped_fields,
                    } => {
                        let where_block = sig::where_block(&s.generics);
                        let open = if where_block.is_empty() { " " } else { "\n" };
                        format!(
                            "{vis}struct {name}{generics}{where_block}{open}{}",
                            self.plain_fields(fields, *has_stripped_fields)
                        )
                    }
                }
            }
            ItemEnum::Union(u) => {
                let where_block = sig::where_block(&u.generics);
                let open = if where_block.is_empty() { " " } else { "\n" };
                format!(
                    "{vis}union {name}{}{where_block}{open}{}",
                    sig::generics_decl(&u.generics),
                    self.plain_fields(&u.fields, u.has_stripped_fields)
                )
            }
            ItemEnum::Enum(e) => {
                let mut lines: Vec<String> = e
                    .variants
                    .iter()
                    .filter_map(|v| self.m.krate.index.get(v))
                    .map(|v| format!("    {},", self.variant_signature(v)))
                    .collect();
                if e.has_stripped_variants {
                    lines.push("    // some variants omitted".into());
                }
                let where_block = sig::where_block(&e.generics);
                let open = if where_block.is_empty() { " " } else { "\n" };
                let body = if lines.is_empty() {
                    "{}".into()
                } else {
                    format!("{{\n{}\n}}", lines.join("\n"))
                };
                format!(
                    "{vis}enum {name}{}{where_block}{open}{body}",
                    sig::generics_decl(&e.generics)
                )
            }
            ItemEnum::Trait(t) => {
                let mut head = vis.clone();
                if t.is_unsafe {
                    head.push_str("unsafe ");
                }
                if t.is_auto {
                    head.push_str("auto ");
                }
                let bounds = if t.bounds.is_empty() {
                    String::new()
                } else {
                    format!(": {}", sig::bounds(&t.bounds))
                };
                format!(
                    "{head}trait {name}{}{bounds}{}",
                    sig::generics_decl(&t.generics),
                    sig::where_block(&t.generics)
                )
            }
            ItemEnum::Function(f) => sig::function(name, &vis, f),
            ItemEnum::TypeAlias(a) => format!(
                "{vis}type {name}{} = {}{};",
                sig::generics_decl(&a.generics),
                sig::ty(&a.type_),
                sig::where_inline(&a.generics)
            ),
            ItemEnum::Constant { type_, const_ } => {
                // rustdoc only stringifies simple expressions; otherwise `expr` is `_` and the
                // evaluated `value` (numeric types only) is all there is.
                let value = if const_.expr != "_" {
                    Some(&const_.expr)
                } else {
                    const_.value.as_ref()
                };
                let value = value.map(|v| format!(" = {v}")).unwrap_or_default();
                format!("{vis}const {name}: {}{value};", sig::ty(type_))
            }
            ItemEnum::Static(s) => format!(
                "{vis}static {}{name}: {};",
                if s.is_mutable { "mut " } else { "" },
                sig::ty(&s.type_)
            ),
            ItemEnum::Macro(source) => source.clone(),
            ItemEnum::ProcMacro(pm) => match pm.kind {
                MacroKind::Bang => format!("{name}!(..)"),
                MacroKind::Attr => format!("#[{name}]"),
                MacroKind::Derive if pm.helpers.is_empty() => format!("#[derive({name})]"),
                MacroKind::Derive => {
                    format!(
                        "#[derive({name})]\n// helper attributes: {}",
                        pm.helpers.join(", ")
                    )
                }
            },
            _ => unreachable!("only grouped items reach here"),
        };
        format!("{attrs}{body}")
    }

    fn variant_signature(&self, variant: &Item) -> String {
        let ItemEnum::Variant(v) = &variant.inner else {
            return String::new();
        };
        let name = variant.name.as_deref().unwrap_or("_");
        let mut text = match &v.kind {
            VariantKind::Plain => name.to_string(),
            VariantKind::Tuple(fields) => format!("{name}{}", self.tuple_fields(fields)),
            VariantKind::Struct {
                fields,
                has_stripped_fields,
            } => {
                let mut parts: Vec<String> = fields
                    .iter()
                    .filter_map(|&id| {
                        let field = self.m.krate.index.get(&id)?;
                        Some(format!(
                            "{}: {}",
                            field.name.as_deref().unwrap_or("_"),
                            sig::ty(self.field_type(id)?)
                        ))
                    })
                    .collect();
                if *has_stripped_fields {
                    parts.push("..".into());
                }
                format!("{name} {{ {} }}", parts.join(", "))
            }
        };
        if let Some(d) = &v.discriminant {
            text.push_str(&format!(" = {}", d.expr));
        }
        text
    }

    /// Docs for a list entry, indented to sit under its bullet.
    fn bullet_docs(&mut self, item: &Item, spaces: usize) -> Option<String> {
        self.docs(item, 4)
            .map(|d| format!("\n\n{}", indent(&d, spaces)))
    }

    fn members(&mut self, id: Id, item: &Item, name: &str) -> Vec<String> {
        let krate = self.m.krate;
        let own = (id, name);
        let mut blocks = Vec::new();
        match &item.inner {
            ItemEnum::Struct(s) => {
                let fields: Vec<Id> = match &s.kind {
                    StructKind::Unit => vec![],
                    StructKind::Tuple(fields) => fields.iter().flatten().copied().collect(),
                    StructKind::Plain { fields, .. } => fields.clone(),
                };
                blocks.extend(self.field_list(&fields));
                blocks.extend(self.impl_sections(own, &s.impls));
            }
            ItemEnum::Union(u) => {
                blocks.extend(self.field_list(&u.fields));
                blocks.extend(self.impl_sections(own, &u.impls));
            }
            ItemEnum::Enum(e) => {
                let mut lines = Vec::new();
                for variant in &e.variants {
                    let Some(v) = krate.index.get(variant) else {
                        continue;
                    };
                    let mut entry = format!("- `{}`", self.variant_signature(v));
                    entry.extend(self.bullet_docs(v, 2));
                    lines.push(entry);
                }
                if !lines.is_empty() {
                    blocks.push("**Variants**".to_string());
                    blocks.push(lines.join("\n"));
                }
                blocks.extend(self.impl_sections(own, &e.impls));
            }
            ItemEnum::Trait(t) => blocks.extend(self.trait_members(name, &t.items)),
            _ => {}
        }
        blocks
    }

    fn field_list(&mut self, fields: &[Id]) -> Vec<String> {
        let krate = self.m.krate;
        let mut lines = Vec::new();
        for (index, id) in fields.iter().enumerate() {
            let Some(field) = krate.index.get(id) else {
                continue;
            };
            let Some(ty) = self.field_type(*id) else {
                continue;
            };
            // Tuple fields have no name; their visibility is already in the signature above.
            let mut entry = match &field.name {
                Some(name) => {
                    format!(
                        "- `{}{name}: {}`",
                        sig::visibility(&field.visibility),
                        sig::ty(ty)
                    )
                }
                None => format!("- `{index}: {}`", sig::ty(ty)),
            };
            entry.extend(self.bullet_docs(field, 2));
            lines.push(entry);
        }
        if lines.is_empty() {
            return vec![];
        }
        vec!["**Fields**".to_string(), lines.join("\n")]
    }

    /// `impl<T: Bound> Trait for Type<T> where ...` on one line. `own` is the type the impl was
    /// found on, and the name it is documented under (which differs for renamed re-exports).
    fn impl_header(i: &Impl, own: (Id, &str)) -> String {
        let unsafety = if i.is_unsafe { "unsafe " } else { "" };
        let generics = sig::generics_decl(&i.generics);
        // For a blanket impl, `for_` is the concrete type it was found on; the impl itself is for
        // the blanket type.
        let target = match i.blanket_impl.as_ref().unwrap_or(&i.for_) {
            Type::ResolvedPath(p) if p.id == own.0 => {
                sig::ty(&Type::ResolvedPath(rustdoc_types::Path {
                    path: own.1.to_string(),
                    ..p.clone()
                }))
            }
            other => sig::ty(other),
        };
        let head = match &i.trait_ {
            Some(t) => {
                let negative = if i.is_negative { "!" } else { "" };
                format!(
                    "{unsafety}impl{generics} {negative}{} for {target}",
                    sig::path(t)
                )
            }
            None => format!("{unsafety}impl{generics} {target}"),
        };
        format!("{head}{}", sig::where_inline(&i.generics))
    }

    /// Inherent methods with their own headings, then summaries of the trait impls.
    fn impl_sections(&mut self, own: (Id, &str), impls: &[Id]) -> Vec<String> {
        let krate = self.m.krate;
        let mut blocks = Vec::new();
        let mut traits = Vec::new();
        // The conditions on auto and blanket impls are noise; the trait names are the useful part.
        let mut auto = Vec::new();
        let mut blanket = Vec::new();

        for id in impls {
            let Some(Item {
                inner: ItemEnum::Impl(i),
                ..
            }) = krate.index.get(id)
            else {
                continue;
            };
            let Some(trait_) = &i.trait_ else {
                blocks.extend(self.inherent_members(own, i));
                continue;
            };
            if i.is_synthetic {
                auto.push(format!("`{}`", sig::path(trait_)));
            } else if i.blanket_impl.is_some() {
                blanket.push(format!("`{}`", sig::path(trait_)));
            } else {
                traits.push(format!("- `{}`", Self::impl_header(i, own)));
            }
        }

        traits.sort();
        if !traits.is_empty() {
            blocks.push("**Trait implementations**".to_string());
            blocks.push(traits.join("\n"));
        }
        for (title, mut names) in [
            ("Auto trait implementations", auto),
            ("Blanket implementations", blanket),
        ] {
            names.sort();
            names.dedup();
            if !names.is_empty() {
                blocks.push(format!(
                    "<details>\n<summary>{title}</summary>\n\n{}\n\n</details>",
                    names.join(", ")
                ));
            }
        }
        blocks
    }

    fn inherent_members(&mut self, own: (Id, &str), i: &Impl) -> Vec<String> {
        let krate = self.m.krate;
        let type_name = own.1;
        let header = Self::impl_header(i, own);
        let mut blocks = Vec::new();
        for id in &i.items {
            let Some(member) = krate.index.get(id) else {
                continue;
            };
            let Some(member_name) = member.name.as_deref() else {
                continue;
            };
            let Some((kind, text)) = self.member_signature(member, member_name) else {
                continue;
            };
            let attrs = Self::attribute_lines(member);
            let body = indent(&format!("{attrs}{text};"), 4);
            blocks.push(self.heading(4, *id, kind, &format!("{type_name}::{member_name}")));
            blocks.push(code_block(&format!("{header} {{\n{body}\n}}")));
            blocks.extend(Self::deprecation(member));
            blocks.extend(self.source_link(member));
            blocks.extend(self.docs(member, 4));
        }
        blocks
    }

    /// The keyword for an associated item's heading and its signature without a trailing `;`.
    fn member_signature(&self, member: &Item, name: &str) -> Option<(&'static str, String)> {
        let vis = sig::visibility(&member.visibility);
        match &member.inner {
            ItemEnum::Function(f) => Some(("fn", sig::function(name, &vis, f))),
            ItemEnum::AssocConst { type_, value, .. } => {
                let value = value
                    .as_ref()
                    .map(|v| format!(" = {v}"))
                    .unwrap_or_default();
                Some((
                    "const",
                    format!("{vis}const {name}: {}{value}", sig::ty(type_)),
                ))
            }
            ItemEnum::AssocType {
                generics,
                bounds,
                type_,
                ..
            } => {
                let bounds = if bounds.is_empty() {
                    String::new()
                } else {
                    format!(": {}", sig::bounds(bounds))
                };
                let value = type_
                    .as_ref()
                    .map(|t| format!(" = {}", sig::ty(t)))
                    .unwrap_or_default();
                Some((
                    "type",
                    format!(
                        "{vis}type {name}{}{bounds}{value}{}",
                        sig::generics_decl(generics),
                        sig::where_inline(generics)
                    ),
                ))
            }
            _ => None,
        }
    }

    fn trait_members(&mut self, trait_name: &str, members: &[Id]) -> Vec<String> {
        let krate = self.m.krate;
        let mut sections: [(&str, Vec<Id>); 4] = [
            ("Associated types", vec![]),
            ("Associated constants", vec![]),
            ("Required methods", vec![]),
            ("Provided methods", vec![]),
        ];
        for id in members {
            let Some(member) = krate.index.get(id) else {
                continue;
            };
            match &member.inner {
                ItemEnum::AssocType { .. } => sections[0].1.push(*id),
                ItemEnum::AssocConst { .. } => sections[1].1.push(*id),
                ItemEnum::Function(f) if f.has_body => sections[3].1.push(*id),
                ItemEnum::Function(_) => sections[2].1.push(*id),
                _ => {}
            }
        }

        let mut blocks = Vec::new();
        for (title, ids) in sections {
            if ids.is_empty() {
                continue;
            }
            blocks.push(format!("**{title}**"));
            for id in ids {
                let member = &krate.index[&id];
                let Some(member_name) = member.name.as_deref() else {
                    continue;
                };
                let Some((kind, text)) = self.member_signature(member, member_name) else {
                    continue;
                };
                let attrs = Self::attribute_lines(member);
                blocks.push(self.heading(4, id, kind, &format!("{trait_name}::{member_name}")));
                blocks.push(code_block(&format!("{attrs}{text};")));
                blocks.extend(Self::deprecation(member));
                blocks.extend(self.source_link(member));
                blocks.extend(self.docs(member, 4));
            }
        }
        blocks
    }
}

/// Checks that no two pages share a name. Used by callers that combine crates.
pub fn ensure_unique(pages: &[Page]) -> Result<()> {
    let mut seen = HashSet::new();
    for page in pages {
        if !seen.insert(&page.basename) {
            bail!("two pages are both called {:?}", page.basename);
        }
    }
    Ok(())
}
