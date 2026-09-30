use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use rustdoc_wiki::build::{self, Build};
use rustdoc_wiki::publish::{self, Outcome, Publish};
use rustdoc_wiki::render::{self, Options, Page};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build rustdoc JSON, render it, and publish it to the wiki. This is what the action runs.
    Run(RunArgs),
    /// Build rustdoc JSON for the workspace's library crates.
    Build(BuildArgs),
    /// Render rustdoc JSON files as wiki pages.
    Render(RenderArgs),
    /// Publish rendered pages to the wiki.
    Publish(PublishArgs),
    /// Print the rustdoc JSON format version this build understands.
    FormatVersion,
}

#[derive(Args)]
struct BuildArgs {
    /// A directory in the workspace to document.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    /// Toolchain directory (with `bin/cargo`) to build with instead of the one on PATH.
    #[arg(long)]
    toolchain: Option<PathBuf>,
    /// Workspace packages to document, comma separated. Default: every library.
    #[arg(long, value_delimiter = ',')]
    package: Vec<String>,
    /// Document private items too.
    #[arg(long)]
    private_items: bool,
}

impl BuildArgs {
    fn build(&self) -> Result<Vec<PathBuf>> {
        build::build(&Build {
            workspace: &self.workspace,
            toolchain: self.toolchain.as_deref(),
            packages: &self.package,
            private_items: self.private_items,
        })
    }
}

/// What GitHub Actions tells every step about the repository.
#[derive(Args)]
struct GithubArgs {
    #[arg(long, env = "GITHUB_REPOSITORY")]
    repository: Option<String>,
    #[arg(long, env = "GITHUB_SERVER_URL", default_value = "https://github.com")]
    server_url: String,
    #[arg(long, env = "GITHUB_SHA")]
    sha: Option<String>,
    #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
    token: Option<String>,
}

impl GithubArgs {
    fn wiki_url(&self, explicit: Option<&str>) -> Result<String> {
        if let Some(url) = explicit {
            return Ok(url.to_string());
        }
        let repository = self
            .repository
            .as_deref()
            .context("cannot tell which wiki to publish to: set --wiki-url or GITHUB_REPOSITORY")?;
        Ok(format!(
            "{}/{repository}.wiki.git",
            self.server_url.trim_end_matches('/')
        ))
    }

    fn source_base(&self) -> Option<String> {
        let (repository, sha) = (self.repository.as_deref()?, self.sha.as_deref()?);
        Some(format!(
            "{}/{repository}/blob/{sha}",
            self.server_url.trim_end_matches('/')
        ))
    }

    fn message(&self, version: &str) -> String {
        match &self.sha {
            Some(sha) => format!(
                "Update API docs ({version}) from {}",
                &sha[..sha.len().min(7)]
            ),
            None => format!("Update API docs ({version})"),
        }
    }
}

#[derive(Args)]
struct WikiArgs {
    /// Version name: the first part of every page name and the name of the directory it goes in.
    #[arg(long, default_value = "latest")]
    version: String,
    /// The wiki directory this tool owns.
    #[arg(long, default_value = "api")]
    directory: String,
    /// The wiki's git URL. Default: derived from GITHUB_REPOSITORY.
    #[arg(long)]
    wiki_url: Option<String>,
    /// Commit message.
    #[arg(long)]
    message: Option<String>,
    /// Report what would change without committing or pushing.
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    github: GithubArgs,
}

impl WikiArgs {
    fn publish(&self, pages: &Path) -> Result<()> {
        let wiki_url = self.github.wiki_url(self.wiki_url.as_deref())?;
        let message = self
            .message
            .clone()
            .unwrap_or_else(|| self.github.message(&self.version));
        let outcome = publish::publish(&Publish {
            wiki_url: &wiki_url,
            token: self.github.token.as_deref(),
            directory: &self.directory,
            version: &self.version,
            pages,
            message: &message,
            dry_run: self.dry_run,
        })?;
        match outcome {
            Outcome::Unchanged => eprintln!("the wiki is already up to date"),
            Outcome::Pushed { commit } => eprintln!("pushed {commit} to the wiki"),
            Outcome::DryRun { changes } => {
                eprintln!("dry run: {} files would change", changes.len());
                for change in changes {
                    eprintln!("  {change}");
                }
            }
        }
        Ok(())
    }

    fn render_options(&self) -> Options {
        Options {
            version: self.version.clone(),
            source_base: self.github.source_base(),
        }
    }
}

#[derive(Args)]
struct RenderArgs {
    /// rustdoc JSON files, one per crate.
    #[arg(required = true)]
    json: Vec<PathBuf>,
    /// Directory to write the pages to, one `<page>.md` file each.
    #[arg(long)]
    out: PathBuf,
    /// Version name, which becomes the first part of every page name.
    #[arg(long, default_value = "latest")]
    version: String,
    /// `https://github.com/<owner>/<repo>/blob/<sha>`, to link items to their source.
    #[arg(long)]
    source_base: Option<String>,
}

#[derive(Args)]
struct PublishArgs {
    /// Directory of rendered pages.
    pages: PathBuf,
    #[command(flatten)]
    wiki: WikiArgs,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    build: BuildArgs,
    /// Use the rustdoc JSON files in this directory instead of building.
    #[arg(long)]
    json_dir: Option<PathBuf>,
    #[command(flatten)]
    wiki: WikiArgs,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::FormatVersion => {
            println!("{}", rustdoc_types::FORMAT_VERSION);
            Ok(())
        }
        Command::Build(args) => {
            for json in args.build()? {
                println!("{}", json.display());
            }
            Ok(())
        }
        Command::Render(args) => {
            let options = Options {
                version: args.version.clone(),
                source_base: args.source_base.clone(),
            };
            let pages = render_all(&args.json, &options)?;
            write_pages(&pages, &args.out)
        }
        Command::Publish(args) => args.wiki.publish(&args.pages),
        Command::Run(args) => run(&args),
    }
}

fn run(args: &RunArgs) -> Result<()> {
    let json = match &args.json_dir {
        Some(dir) => json_files(dir)?,
        None => args.build.build()?,
    };
    let pages = render_all(&json, &args.wiki.render_options())?;
    let staging = tempfile::tempdir().context("creating a scratch directory")?;
    write_pages(&pages, staging.path())?;
    args.wiki.publish(staging.path())
}

fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            files.push(path);
        }
    }
    files.sort();
    if files.is_empty() {
        bail!("no .json files in {}", dir.display());
    }
    Ok(files)
}

fn render_all(json: &[PathBuf], options: &Options) -> Result<Vec<Page>> {
    let mut pages = Vec::new();
    for path in json {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let krate = render::load(&bytes).with_context(|| format!("loading {}", path.display()))?;
        pages.extend(render::render(&krate, options)?);
    }
    render::ensure_unique(&pages)?;
    Ok(pages)
}

fn write_pages(pages: &[Page], out: &Path) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    for page in pages {
        let file = out.join(format!("{}.md", page.basename));
        fs::write(&file, &page.markdown).with_context(|| format!("writing {}", file.display()))?;
    }
    eprintln!("wrote {} pages to {}", pages.len(), out.display());
    Ok(())
}
