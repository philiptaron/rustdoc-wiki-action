//! Runs the real binary from a workspace to a wiki, the way the action does.
//!
//! Needs the pinned nightly (for rustdoc JSON), so run inside `nix develop`.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::{Wiki, git};
use tempfile::TempDir;

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .display()
        .to_string()
}

/// `rustdoc-wiki run` on the `basic` fixture, published to `wiki`, with the environment GitHub
/// Actions provides.
fn run(wiki: &Wiki, target: &TempDir, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustdoc-wiki"))
        .arg("run")
        .args(["--workspace", &fixture("basic")])
        .args(["--wiki-url", &wiki.url()])
        .args(extra)
        .env("CARGO_TARGET_DIR", target.path())
        .env("GITHUB_REPOSITORY", "example/repo")
        .env("GITHUB_SHA", "0123456789abcdef0123456789abcdef01234567")
        .env_remove("GITHUB_TOKEN")
        .output()
        .expect("binary runs")
}

#[test]
fn builds_renders_and_publishes_a_workspace() {
    let wiki = Wiki::new();
    let target = TempDir::new().unwrap();
    let output = run(&wiki, &target, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let check = wiki.checkout();
    let root = fs::read_to_string(check.join("api/latest/latest-basic.md")).unwrap();
    // Source links come from GITHUB_REPOSITORY and GITHUB_SHA.
    assert!(
        root.contains("https://github.com/example/repo/blob/0123456789abcdef0123456789abcdef01234567/src/lib.rs#L"),
        "{root}"
    );
    for page in [
        "latest-basic-shapes",
        "latest-basic-util",
        "latest-basic-util-nested",
    ] {
        assert!(
            check.join(format!("api/latest/{page}.md")).is_file(),
            "{page}"
        );
    }
    let sidebar = fs::read_to_string(check.join("_Sidebar.md")).unwrap();
    assert!(sidebar.contains("* [`basic`](latest-basic)"), "{sidebar}");
    assert_eq!(
        git(&check, &["log", "-1", "--format=%s"]),
        "Update API docs (latest) from 0123456"
    );
    assert!(check.join("Home.md").is_file());
}

#[test]
fn a_release_snapshot_sits_next_to_latest() {
    let wiki = Wiki::new();
    let target = TempDir::new().unwrap();
    assert!(run(&wiki, &target, &[]).status.success());
    let output = run(&wiki, &target, &["--version", "v0.3.1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let check = wiki.checkout();
    assert!(check.join("api/latest/latest-basic.md").is_file());
    assert!(check.join("api/v0.3.1/v0.3.1-basic.md").is_file());
    let sidebar = fs::read_to_string(check.join("_Sidebar.md")).unwrap();
    assert!(
        sidebar.contains("**Versions:** [latest](latest-basic) · [v0.3.1](v0.3.1-basic)"),
        "{sidebar}"
    );
}

#[test]
fn running_twice_publishes_once() {
    let wiki = Wiki::new();
    let target = TempDir::new().unwrap();
    assert!(run(&wiki, &target, &[]).status.success());
    let commits = wiki.commit_count();
    let output = run(&wiki, &target, &[]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already up to date"));
    assert_eq!(wiki.commit_count(), commits);
}

#[test]
fn a_dry_run_leaves_the_wiki_alone() {
    let wiki = Wiki::new();
    let target = TempDir::new().unwrap();
    let commits = wiki.commit_count();
    let output = run(&wiki, &target, &["--dry-run"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("dry run"));
    assert_eq!(wiki.commit_count(), commits);
}

#[test]
fn an_unknown_package_lists_the_choices() {
    let wiki = Wiki::new();
    let target = TempDir::new().unwrap();
    let output = run(&wiki, &target, &["--package", "nope"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no workspace package named \"nope\""),
        "{stderr}"
    );
    assert!(stderr.contains("basic"), "{stderr}");
}

/// With a build target configured, cargo writes to `<target dir>/<triple>/doc/`.
#[test]
fn works_when_a_build_target_is_configured() {
    let rustc = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("rustc runs");
    let info = String::from_utf8(rustc.stdout).unwrap();
    let host = info
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("rustc reports a host");

    let target = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rustdoc-wiki"))
        .args(["build", "--workspace", &fixture("basic")])
        .env("CARGO_TARGET_DIR", target.path())
        .env("CARGO_BUILD_TARGET", host)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = String::from_utf8(output.stdout).unwrap();
    assert!(
        json.trim().ends_with(&format!("{host}/doc/basic.json")),
        "{json}"
    );
}

#[test]
fn json_dir_skips_the_build() {
    let wiki = Wiki::new();
    let target = TempDir::new().unwrap();
    // Build once to get JSON, then publish from it without building again.
    let built = Command::new(env!("CARGO_BIN_EXE_rustdoc-wiki"))
        .args(["build", "--workspace", &fixture("basic")])
        .env("CARGO_TARGET_DIR", target.path())
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let json = String::from_utf8(built.stdout).unwrap();
    let json = json.trim();
    assert!(json.ends_with("basic.json"), "{json}");

    let dir = TempDir::new().unwrap();
    fs::copy(json, dir.path().join("basic.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rustdoc-wiki"))
        .args(["run", "--json-dir"])
        .arg(dir.path())
        .args(["--wiki-url", &wiki.url()])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GITHUB_REPOSITORY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(wiki.checkout().join("api/latest/latest-basic.md").is_file());
}
