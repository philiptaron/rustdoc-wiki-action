//! Publishes rendered pages to the wiki's git repository.
//!
//! The wiki is a plain git repo. We clone it, replace the directory we own, refresh the
//! [`sidebar`]'s managed region, and push. See [DESIGN.md, "Ownership rules"][ownership].
//!
//! [ownership]: https://github.com/philiptaron/rustdoc-wiki-action/blob/HEAD/DESIGN.md#ownership-rules

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::sidebar::{self, Versions};

const BOT_NAME: &str = "github-actions[bot]";
const BOT_EMAIL: &str = "41898282+github-actions[bot]@users.noreply.github.com";
const PUSH_ATTEMPTS: usize = 5;

pub struct Publish<'a> {
    /// Where to clone from: `https://github.com/<owner>/<repo>.wiki.git`, or a local path.
    pub wiki_url: &'a str,
    /// Token for HTTPS authentication. Sent as a header, never put in the URL.
    pub token: Option<&'a str>,
    /// The wiki directory we own, such as `api`. Pages go in `<directory>/<version>/`.
    pub directory: &'a str,
    pub version: &'a str,
    /// A directory of rendered `<page>.md` files.
    pub pages: &'a Path,
    pub message: &'a str,
    /// Work out what would change, but do not commit or push.
    pub dry_run: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The wiki already had exactly these pages.
    Unchanged,
    Pushed {
        commit: String,
    },
    /// Files that would change.
    DryRun {
        changes: Vec<String>,
    },
}

pub fn publish(p: &Publish) -> Result<Outcome> {
    let directory = validated_directory(p.directory)?;
    crate::names::validate_version(p.version)?;
    let pages = read_pages(p.pages, p.version)?;

    let scratch = tempfile::tempdir().context("creating a scratch directory")?;
    let root = scratch.path().join("wiki");
    clone(p, &root)?;
    let git = Git {
        dir: &root,
        token: p.token,
    };
    let branch = git.run(&["symbolic-ref", "--short", "HEAD"])?;

    for attempt in 1..=PUSH_ATTEMPTS {
        apply(&root, &directory, p.version, &pages)?;
        git.run(&["add", "--all"])?;
        let status = git.run(&["status", "--porcelain"])?;
        if status.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        if p.dry_run {
            let changes = status.lines().map(str::to_string).collect();
            return Ok(Outcome::DryRun { changes });
        }
        git.run(&[
            "-c",
            &format!("user.name={BOT_NAME}"),
            "-c",
            &format!("user.email={BOT_EMAIL}"),
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--message",
            p.message,
        ])?;
        match git.run(&["push", "--quiet", "origin", &format!("HEAD:{branch}")]) {
            Ok(_) => {
                let commit = git.run(&["rev-parse", "HEAD"])?;
                return Ok(Outcome::Pushed { commit });
            }
            Err(error) if attempt < PUSH_ATTEMPTS && is_rejection(&error) => {
                // Someone else pushed first. We only touch paths we own, so there is nothing to
                // merge: start again from their tree and redo our change.
                eprintln!("push was rejected (attempt {attempt}); retrying on top of the new tip");
                git.run(&["fetch", "--quiet", "origin", &branch])?;
                git.run(&["reset", "--hard", "--quiet", "FETCH_HEAD"])?;
            }
            Err(error) => return Err(error.context("pushing to the wiki")),
        }
    }
    bail!("could not push to the wiki after {PUSH_ATTEMPTS} attempts")
}

fn is_rejection(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("rejected") || text.contains("non-fast-forward") || text.contains("fetch first")
}

/// `api` or `docs/api`: relative, and staying inside the wiki.
fn validated_directory(directory: &str) -> Result<PathBuf> {
    let path = Path::new(directory);
    let ok = !directory.is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && !directory.starts_with(['_', '.']);
    if !ok {
        bail!(
            "invalid directory {directory:?}: use a relative path inside the wiki such as \"api\", \
             without \"..\", and not starting with '_' or '.'"
        );
    }
    Ok(path.to_path_buf())
}

/// The `<page>.md` files in `dir`, as (page name, path), checked to belong to `version`.
fn read_pages(dir: &Path, version: &str) -> Result<Vec<(String, PathBuf)>> {
    let mut pages = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            if !name.starts_with(&format!("{version}-")) {
                bail!(
                    "{} does not start with \"{version}-\"; were the pages rendered with \
                     --version {version}?",
                    path.display()
                );
            }
            pages.push((name, path));
        }
    }
    if pages.is_empty() {
        bail!("no pages found in {}", dir.display());
    }
    pages.sort();
    Ok(pages)
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e == "md" || e == "markdown")
}

/// Every Markdown page in the tree outside `skip`, keyed by lowercase name. GitHub identifies a
/// page by its file name alone and ignores the directory.
fn existing_pages(root: &Path, skip: &Path) -> Result<HashMap<String, String>> {
    fn walk(dir: &Path, root: &Path, skip: &Path, out: &mut HashMap<String, String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path == skip || path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, skip, out)?;
            } else if is_markdown(&path) {
                let name = path.file_stem().unwrap().to_string_lossy().to_lowercase();
                let relative = path.strip_prefix(root).unwrap().display().to_string();
                out.insert(name, relative);
            }
        }
        Ok(())
    }
    let mut out = HashMap::new();
    walk(root, root, skip, &mut out)?;
    Ok(out)
}

/// The versions in `<directory>`, with the page names in each.
fn scan_versions(root: &Path, directory: &Path) -> Result<Versions> {
    let mut versions = BTreeMap::new();
    let base = root.join(directory);
    if !base.is_dir() {
        return Ok(versions);
    }
    for entry in fs::read_dir(&base)? {
        let version = entry?.path();
        if !version.is_dir() {
            continue;
        }
        let mut names = Vec::new();
        for page in fs::read_dir(&version)? {
            let page = page?.path();
            if is_markdown(&page) {
                names.push(page.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
        names.sort();
        versions.insert(
            version.file_name().unwrap().to_string_lossy().into_owned(),
            names,
        );
    }
    Ok(versions)
}

/// Rewrites the paths we own in the working tree at `root`. Idempotent.
fn apply(root: &Path, directory: &Path, version: &str, pages: &[(String, PathBuf)]) -> Result<()> {
    let owned = root.join(directory).join(version);

    let existing = existing_pages(root, &owned)?;
    for (name, _) in pages {
        if let Some(other) = existing.get(&name.to_lowercase()) {
            bail!(
                "the wiki already has a page {other:?} that would be shadowed by the generated \
                 page {name:?}: GitHub identifies a wiki page by its file name alone. Rename or \
                 remove it."
            );
        }
    }

    if owned.exists() {
        fs::remove_dir_all(&owned).with_context(|| format!("removing {}", owned.display()))?;
    }
    fs::create_dir_all(&owned)?;
    for (name, source) in pages {
        fs::copy(source, owned.join(format!("{name}.md")))
            .with_context(|| format!("copying {}", source.display()))?;
    }

    let block = sidebar::block(&scan_versions(root, directory)?);
    if !block.is_empty() {
        let path = root.join("_Sidebar.md");
        let current = fs::read_to_string(&path).ok();
        fs::write(&path, sidebar::apply(current.as_deref(), &block)?)?;
    }
    Ok(())
}

fn clone(p: &Publish, destination: &Path) -> Result<()> {
    let output = Git::command(p.token)
        .args(["clone", "--quiet", p.wiki_url])
        .arg(destination)
        .output()
        .context("running git")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not found") || stderr.contains("does not exist") {
        bail!(
            "could not clone the wiki ({}). Either the repository's wiki is disabled, or it has \
             no pages yet: GitHub only creates the wiki's git repository when the first page is \
             saved. Enable Wikis in the repository settings and create a first page in the web \
             UI, then run this again (see {}#quick-start).\n{}",
            p.wiki_url,
            crate::HOMEPAGE,
            stderr.trim()
        );
    }
    bail!("git clone {} failed:\n{}", p.wiki_url, stderr.trim())
}

struct Git<'a> {
    dir: &'a Path,
    token: Option<&'a str>,
}

impl Git<'_> {
    fn command(token: Option<&str>) -> Command {
        let mut command = Command::new("git");
        command.env("GIT_TERMINAL_PROMPT", "0");
        if let Some(token) = token {
            // Through the environment, so the credential never appears in a process listing.
            let credentials = base64(format!("x-access-token:{token}").as_bytes());
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraheader")
                .env(
                    "GIT_CONFIG_VALUE_0",
                    format!("AUTHORIZATION: basic {credentials}"),
                );
        }
        command
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = Self::command(self.token)
            .current_dir(self.dir)
            .args(args)
            .output()
            .context("running git")?;
        if !output.status.success() {
            let verb = args
                .iter()
                .find(|a| !a.starts_with('-') && !a.contains('='))
                .unwrap_or(&"?");
            bail!(
                "git {verb} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
        }
    }

    #[test]
    fn directories() {
        for ok in ["api", "docs/api", "Reference"] {
            validated_directory(ok).unwrap();
        }
        for bad in ["", "/abs", "../up", "a/../b", "_hidden", ".git", "./x"] {
            assert!(validated_directory(bad).is_err(), "{bad:?}");
        }
    }
}
