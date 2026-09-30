//! Prints rustdoc-json types, generics and signatures as Rust source text.
//!
//! Everything here is a pure function of the JSON; it never looks other items up. Paths are printed
//! as the author wrote them (`Path::path`), so `Vec<u8>` stays `Vec<u8>`.

use rustdoc_types::{
    Abi, AssocItemConstraint, AssocItemConstraintKind, Attribute, Constant, DynTrait, Function,
    FunctionHeader, FunctionPointer, GenericArg, GenericArgs, GenericBound, GenericParamDef,
    GenericParamDefKind, Generics, Path, PolyTrait, PreciseCapturingArg, ReprKind, Term,
    TraitBoundModifier, Type, Visibility, WherePredicate,
};

/// Signatures longer than this are wrapped, one parameter per line.
const MAX_LINE: usize = 100;

fn join<I: IntoIterator<Item = String>>(items: I, sep: &str) -> String {
    items.into_iter().collect::<Vec<_>>().join(sep)
}

pub fn ty(t: &Type) -> String {
    match t {
        Type::ResolvedPath(p) => path(p),
        Type::DynTrait(d) => dyn_trait(d),
        Type::Generic(s) | Type::Primitive(s) => s.clone(),
        Type::FunctionPointer(f) => fn_pointer(f),
        Type::Tuple(ts) => match ts.as_slice() {
            [one] => format!("({},)", ty(one)),
            _ => format!("({})", join(ts.iter().map(ty), ", ")),
        },
        Type::Slice(t) => format!("[{}]", ty(t)),
        Type::Array { type_, len } => format!("[{}; {len}]", ty(type_)),
        Type::Pat { type_, .. } => ty(type_),
        Type::ImplTrait(bs) => format!("impl {}", bounds(bs)),
        Type::Infer => "_".into(),
        Type::RawPointer { is_mutable, type_ } => {
            format!(
                "*{} {}",
                if *is_mutable { "mut" } else { "const" },
                pointee(type_)
            )
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let lifetime = lifetime
                .as_ref()
                .map(|l| format!("{l} "))
                .unwrap_or_default();
            let mutable = if *is_mutable { "mut " } else { "" };
            format!("&{lifetime}{mutable}{}", pointee(type_))
        }
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let args = args.as_deref().map(generic_args).unwrap_or_default();
            match trait_ {
                Some(_) if matches!(&**self_type, Type::Generic(s) if s == "Self") => {
                    format!("Self::{name}{args}")
                }
                Some(tr) => format!("<{} as {}>::{name}{args}", ty(self_type), path(tr)),
                None => format!("{}::{name}{args}", ty(self_type)),
            }
        }
    }
}

/// The type behind a `&`/`*`, parenthesized where a `+` would otherwise bind wrongly.
fn pointee(t: &Type) -> String {
    match t {
        Type::DynTrait(d) if d.traits.len() + usize::from(d.lifetime.is_some()) > 1 => {
            format!("({})", ty(t))
        }
        Type::ImplTrait(bs) if bs.len() > 1 => format!("({})", ty(t)),
        _ => ty(t),
    }
}

pub fn path(p: &Path) -> String {
    // Paths written inside a macro, such as the bounds of a `#[derive]`d impl, start with the
    // unhelpful `$crate::default::`. The last segment is what a reader wants.
    let name = match p.path.strip_prefix("$crate::") {
        Some(rest) => rest.rsplit("::").next().unwrap_or(rest),
        None => &p.path,
    };
    format!(
        "{name}{}",
        p.args.as_deref().map(generic_args).unwrap_or_default()
    )
}

fn dyn_trait(d: &DynTrait) -> String {
    let mut parts: Vec<String> = d.traits.iter().map(poly_trait).collect();
    parts.extend(d.lifetime.clone());
    format!("dyn {}", parts.join(" + "))
}

fn poly_trait(p: &PolyTrait) -> String {
    format!("{}{}", hrtb(&p.generic_params), path(&p.trait_))
}

fn fn_pointer(f: &FunctionPointer) -> String {
    let inputs = join(f.sig.inputs.iter().map(|(_, t)| ty(t)), ", ");
    let output = f
        .sig
        .output
        .as_ref()
        .map(|t| format!(" -> {}", ty(t)))
        .unwrap_or_default();
    let header = FunctionHeader {
        is_const: false,
        is_async: false,
        ..f.header.clone()
    };
    format!(
        "{}{}fn({inputs}){output}",
        hrtb(&f.generic_params),
        fn_header(&header)
    )
}

/// `for<'a, 'b> ` (with the trailing space), or nothing.
fn hrtb(params: &[GenericParamDef]) -> String {
    if params.is_empty() {
        return String::new();
    }
    format!(
        "for<{}> ",
        join(params.iter().map(|p| p.name.clone()), ", ")
    )
}

pub fn generic_args(a: &GenericArgs) -> String {
    match a {
        GenericArgs::AngleBracketed { args, constraints } => {
            if args.is_empty() && constraints.is_empty() {
                return String::new();
            }
            let all = args
                .iter()
                .map(generic_arg)
                .chain(constraints.iter().map(constraint));
            format!("<{}>", join(all, ", "))
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let output = output
                .as_ref()
                .map(|t| format!(" -> {}", ty(t)))
                .unwrap_or_default();
            format!("({}){output}", join(inputs.iter().map(ty), ", "))
        }
        GenericArgs::ReturnTypeNotation => "(..)".into(),
    }
}

fn generic_arg(a: &GenericArg) -> String {
    match a {
        GenericArg::Lifetime(l) => l.clone(),
        GenericArg::Type(t) => ty(t),
        GenericArg::Const(c) => constant(c),
        GenericArg::Infer => "_".into(),
    }
}

fn constant(c: &Constant) -> String {
    c.expr.clone()
}

fn constraint(c: &AssocItemConstraint) -> String {
    let args = c.args.as_deref().map(generic_args).unwrap_or_default();
    match &c.binding {
        AssocItemConstraintKind::Equality(term) => {
            format!("{}{args} = {}", c.name, term_text(term))
        }
        AssocItemConstraintKind::Constraint(bs) => format!("{}{args}: {}", c.name, bounds(bs)),
    }
}

fn term_text(t: &Term) -> String {
    match t {
        Term::Type(t) => ty(t),
        Term::Constant(c) => constant(c),
    }
}

pub fn bounds(bs: &[GenericBound]) -> String {
    join(bs.iter().map(bound), " + ")
}

fn bound(b: &GenericBound) -> String {
    match b {
        GenericBound::TraitBound {
            trait_,
            generic_params,
            modifier,
        } => {
            let modifier = match modifier {
                TraitBoundModifier::None => "",
                TraitBoundModifier::Maybe => "?",
                TraitBoundModifier::MaybeConst => "[const] ",
            };
            format!("{}{modifier}{}", hrtb(generic_params), path(trait_))
        }
        GenericBound::Outlives(l) => l.clone(),
        GenericBound::Use(args) => {
            let args = args.iter().map(|a| match a {
                PreciseCapturingArg::Lifetime(s) | PreciseCapturingArg::Param(s) => s.clone(),
            });
            format!("use<{}>", join(args, ", "))
        }
    }
}

/// `<'a, T: Bound = Default, const N: usize>`, or nothing. Compiler-introduced parameters (from
/// `impl Trait` in argument position) are left out because they do not appear in the source.
pub fn generics_decl(g: &Generics) -> String {
    let params: Vec<String> = g
        .params
        .iter()
        .filter(|p| {
            !matches!(
                p.kind,
                GenericParamDefKind::Type {
                    is_synthetic: true,
                    ..
                }
            )
        })
        .map(|p| match &p.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                if outlives.is_empty() {
                    p.name.clone()
                } else {
                    format!("{}: {}", p.name, outlives.join(" + "))
                }
            }
            GenericParamDefKind::Type {
                bounds: bs,
                default,
                ..
            } => {
                let mut s = p.name.clone();
                if !bs.is_empty() {
                    s.push_str(&format!(": {}", bounds(bs)));
                }
                if let Some(d) = default {
                    s.push_str(&format!(" = {}", ty(d)));
                }
                s
            }
            GenericParamDefKind::Const { type_, default } => {
                let mut s = format!("const {}: {}", p.name, ty(type_));
                if let Some(d) = default {
                    s.push_str(&format!(" = {d}"));
                }
                s
            }
        })
        .collect();
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

pub fn where_predicates(g: &Generics) -> Vec<String> {
    g.where_predicates
        .iter()
        .map(|p| match p {
            WherePredicate::BoundPredicate {
                type_,
                bounds: bs,
                generic_params,
            } => {
                format!("{}{}: {}", hrtb(generic_params), ty(type_), bounds(bs))
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                format!("{lifetime}: {}", outlives.join(" + "))
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                format!("{} == {}", ty(lhs), term_text(rhs))
            }
        })
        .collect()
}

/// `\nwhere\n    A,\n    B,` or nothing.
pub fn where_block(g: &Generics) -> String {
    let preds = where_predicates(g);
    if preds.is_empty() {
        return String::new();
    }
    format!(
        "\nwhere\n{}",
        join(preds.iter().map(|p| format!("    {p},")), "\n")
    )
}

/// ` where A, B` on one line, or nothing.
pub fn where_inline(g: &Generics) -> String {
    let preds = where_predicates(g);
    if preds.is_empty() {
        String::new()
    } else {
        format!(" where {}", preds.join(", "))
    }
}

pub fn visibility(v: &Visibility) -> String {
    match v {
        Visibility::Public => "pub ".into(),
        Visibility::Default => String::new(),
        Visibility::Crate => "pub(crate) ".into(),
        Visibility::Restricted { path, .. } => match path.as_str() {
            "crate" => "pub(crate) ".into(),
            "super" | "self" => format!("pub({path}) "),
            _ => format!("pub(in {path}) "),
        },
    }
}

fn abi_name(abi: &Abi) -> Option<String> {
    let (name, unwind) = match abi {
        Abi::Rust => return None,
        Abi::C { unwind } => ("C", *unwind),
        Abi::Cdecl { unwind } => ("cdecl", *unwind),
        Abi::Stdcall { unwind } => ("stdcall", *unwind),
        Abi::Fastcall { unwind } => ("fastcall", *unwind),
        Abi::Aapcs { unwind } => ("aapcs", *unwind),
        Abi::Win64 { unwind } => ("win64", *unwind),
        Abi::SysV64 { unwind } => ("sysv64", *unwind),
        Abi::System { unwind } => ("system", *unwind),
        Abi::Other(s) => return Some(s.clone()),
    };
    Some(if unwind {
        format!("{name}-unwind")
    } else {
        name.into()
    })
}

/// `const async unsafe extern "C" ` (with the trailing space), or nothing.
fn fn_header(h: &FunctionHeader) -> String {
    let mut s = String::new();
    if h.is_const {
        s.push_str("const ");
    }
    if h.is_async {
        s.push_str("async ");
    }
    if h.is_unsafe {
        s.push_str("unsafe ");
    }
    if let Some(abi) = abi_name(&h.abi) {
        s.push_str(&format!("extern \"{abi}\" "));
    }
    s
}

fn param(name: &str, t: &Type) -> String {
    if name != "self" {
        return format!("{name}: {}", ty(t));
    }
    match t {
        Type::Generic(s) if s == "Self" => "self".into(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } if matches!(&**type_, Type::Generic(s) if s == "Self") => {
            let lifetime = lifetime
                .as_ref()
                .map(|l| format!("{l} "))
                .unwrap_or_default();
            let mutable = if *is_mutable { "mut " } else { "" };
            format!("&{lifetime}{mutable}self")
        }
        _ => format!("self: {}", ty(t)),
    }
}

/// A function signature without a body or trailing `;`.
pub fn function(name: &str, vis: &str, f: &Function) -> String {
    let mut params: Vec<String> = f.sig.inputs.iter().map(|(n, t)| param(n, t)).collect();
    if f.sig.is_c_variadic {
        params.push("...".into());
    }
    let output = f
        .sig
        .output
        .as_ref()
        .map(|t| format!(" -> {}", ty(t)))
        .unwrap_or_default();
    let head = format!(
        "{vis}{}fn {name}{}",
        fn_header(&f.header),
        generics_decl(&f.generics)
    );

    let one_line = format!("{head}({}){output}", params.join(", "));
    let signature = if one_line.len() > MAX_LINE && params.len() > 1 {
        let lines = join(params.iter().map(|p| format!("    {p},")), "\n");
        format!("{head}(\n{lines}\n){output}")
    } else {
        one_line
    };
    format!("{signature}{}", where_block(&f.generics))
}

/// Attributes that are worth showing above a signature, as source text.
pub fn attributes(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|a| match a {
            Attribute::NonExhaustive => Some("#[non_exhaustive]".into()),
            Attribute::MustUse { reason: None } => Some("#[must_use]".into()),
            Attribute::MustUse { reason: Some(r) } => Some(format!("#[must_use = {r:?}]")),
            Attribute::MacroExport => Some("#[macro_export]".into()),
            Attribute::ExportName(n) => Some(format!("#[export_name = {n:?}]")),
            Attribute::LinkSection(n) => Some(format!("#[link_section = {n:?}]")),
            Attribute::NoMangle => Some("#[no_mangle]".into()),
            Attribute::TargetFeature { enable } => {
                let enables = join(enable.iter().map(|f| format!("enable = {f:?}")), ", ");
                Some(format!("#[target_feature({enables})]"))
            }
            Attribute::Repr(r) => {
                let mut parts = Vec::new();
                match r.kind {
                    ReprKind::Rust => {}
                    ReprKind::C => parts.push("C".to_string()),
                    ReprKind::Transparent => parts.push("transparent".to_string()),
                    ReprKind::Simd => parts.push("simd".to_string()),
                }
                match r.packed {
                    Some(1) => parts.push("packed".to_string()),
                    Some(n) => parts.push(format!("packed({n})")),
                    None => {}
                }
                if let Some(n) = r.align {
                    parts.push(format!("align({n})"));
                }
                parts.extend(r.int.clone());
                (!parts.is_empty()).then(|| format!("#[repr({})]", parts.join(", ")))
            }
            // Not covered by the format version, so its shape can change without notice.
            Attribute::Other(_) | Attribute::AutomaticallyDerived => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(s: &str) -> Type {
        Type::Primitive(s.into())
    }

    fn resolved(name: &str, args: Vec<GenericArg>) -> Type {
        Type::ResolvedPath(Path {
            path: name.into(),
            id: rustdoc_types::Id(0),
            args: (!args.is_empty()).then(|| {
                Box::new(GenericArgs::AngleBracketed {
                    args,
                    constraints: vec![],
                })
            }),
        })
    }

    #[test]
    fn simple_types() {
        assert_eq!(ty(&Type::Tuple(vec![])), "()");
        assert_eq!(ty(&Type::Tuple(vec![prim("u8")])), "(u8,)");
        assert_eq!(
            ty(&Type::Tuple(vec![prim("u8"), prim("bool")])),
            "(u8, bool)"
        );
        assert_eq!(ty(&Type::Slice(Box::new(prim("u8")))), "[u8]");
        assert_eq!(
            ty(&Type::Array {
                type_: Box::new(prim("u8")),
                len: "4".into()
            }),
            "[u8; 4]"
        );
        assert_eq!(ty(&Type::Infer), "_");
    }

    #[test]
    fn references_and_pointers() {
        let r = Type::BorrowedRef {
            lifetime: Some("'a".into()),
            is_mutable: true,
            type_: Box::new(prim("str")),
        };
        assert_eq!(ty(&r), "&'a mut str");
        let p = Type::RawPointer {
            is_mutable: false,
            type_: Box::new(prim("u8")),
        };
        assert_eq!(ty(&p), "*const u8");
    }

    #[test]
    fn generic_arguments() {
        let t = resolved(
            "HashMap",
            vec![
                GenericArg::Type(resolved("String", vec![])),
                GenericArg::Lifetime("'static".into()),
                GenericArg::Infer,
            ],
        );
        assert_eq!(ty(&t), "HashMap<String, 'static, _>");
    }

    #[test]
    fn dyn_in_reference_is_parenthesized_when_it_has_several_bounds() {
        let tr = |n: &str| PolyTrait {
            trait_: Path {
                path: n.into(),
                id: rustdoc_types::Id(0),
                args: None,
            },
            generic_params: vec![],
        };
        let one = Type::DynTrait(DynTrait {
            traits: vec![tr("Debug")],
            lifetime: None,
        });
        let two = Type::DynTrait(DynTrait {
            traits: vec![tr("Debug"), tr("Send")],
            lifetime: None,
        });
        let r = |t| Type::BorrowedRef {
            lifetime: None,
            is_mutable: false,
            type_: Box::new(t),
        };
        assert_eq!(ty(&r(one)), "&dyn Debug");
        assert_eq!(ty(&r(two)), "&(dyn Debug + Send)");
    }

    #[test]
    fn visibilities() {
        assert_eq!(visibility(&Visibility::Public), "pub ");
        assert_eq!(visibility(&Visibility::Default), "");
        assert_eq!(visibility(&Visibility::Crate), "pub(crate) ");
    }

    #[test]
    fn repr_attributes() {
        let repr = |kind, align, packed, int| {
            Attribute::Repr(rustdoc_types::AttributeRepr {
                kind,
                align,
                packed,
                int,
            })
        };
        let attrs = [
            repr(ReprKind::C, None, None, None),
            repr(ReprKind::Rust, Some(8), None, None),
            repr(ReprKind::C, None, Some(1), Some("u8".into())),
            repr(ReprKind::Rust, None, None, None),
        ];
        assert_eq!(
            attributes(&attrs),
            ["#[repr(C)]", "#[repr(align(8))]", "#[repr(C, packed, u8)]"]
        );
    }
}
