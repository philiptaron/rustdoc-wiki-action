//! Renders rustdoc JSON as GitHub wiki pages.
//!
//! This is the engine of [rustdoc-wiki-action](https://github.com/philiptaron/rustdoc-wiki-action).
//! Its [README](https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/README.md) explains how to use the
//! action, and [DESIGN.md](https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/DESIGN.md)
//! records why it works the way it does.
//!
//! The pipeline has three stages, each of which can also be run on its own from the command line:
//!
//! 1. [`build`] runs `cargo rustdoc` to produce rustdoc JSON.
//! 2. [`render`] turns that JSON into Markdown pages, one per module.
//! 3. [`publish`] puts the pages in the wiki's git repository.
//!
//! They share a few helpers:
//!
//! - [`names`]: page names, and the heading anchors GitHub assigns.
//! - [`docs`]: turns doc comments into wiki Markdown, rewriting intra-doc links.
//! - [`sig`]: prints types and signatures as Rust source.
//! - [`external`]: URLs for items that are not on the wiki.
//! - [`sidebar`]: the managed region of `_Sidebar.md`.

pub mod build;
pub mod docs;
pub mod external;
pub mod names;
pub mod publish;
pub mod render;
pub mod sidebar;
pub mod sig;

/// The project's home on GitHub. Error messages point readers at its documentation from here.
pub const HOMEPAGE: &str = "https://github.com/philiptaron/rustdoc-wiki-action";
