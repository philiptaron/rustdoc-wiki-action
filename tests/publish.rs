//! Publishing against a local bare repository standing in for the wiki.

mod common;

use std::fs;

use common::{Wiki, git};
use rustdoc_wiki::publish::{Outcome, Publish, publish};
use rustdoc_wiki::sidebar::{BEGIN, END};
use tempfile::TempDir;

fn pages(names: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in names {
        fs::write(dir.path().join(format!("{name}.md")), format!("# {name}\n")).unwrap();
    }
    dir
}

fn publish_pages(wiki: &Wiki, version: &str, names: &[&str]) -> anyhow::Result<Outcome> {
    let dir = pages(names);
    publish(&Publish {
        wiki_url: &wiki.url(),
        token: None,
        directory: "api",
        version,
        pages: dir.path(),
        message: "Update API docs",
        dry_run: false,
    })
}

#[test]
fn first_publish_adds_pages_and_a_sidebar_and_leaves_other_pages_alone() {
    let wiki = Wiki::new();
    let outcome = publish_pages(&wiki, "latest", &["latest-a", "latest-a-b"]).unwrap();
    assert!(matches!(outcome, Outcome::Pushed { .. }), "{outcome:?}");

    let check = wiki.checkout();
    assert_eq!(
        fs::read_to_string(check.join("Home.md")).unwrap(),
        "Welcome to the wiki\n"
    );
    assert!(check.join("api/latest/latest-a.md").is_file());
    assert!(check.join("api/latest/latest-a-b.md").is_file());
    let sidebar = fs::read_to_string(check.join("_Sidebar.md")).unwrap();
    assert!(
        sidebar.starts_with(BEGIN) && sidebar.trim_end().ends_with(END),
        "{sidebar}"
    );
    assert!(
        sidebar.contains("* [`a`](latest-a)\n  * [`b`](latest-a-b)"),
        "{sidebar}"
    );

    assert_eq!(
        git(&check, &["log", "-1", "--format=%an"]),
        "github-actions[bot]"
    );
    assert_eq!(
        git(&check, &["log", "-1", "--format=%s"]),
        "Update API docs"
    );
}

#[test]
fn publishing_the_same_pages_again_changes_nothing() {
    let wiki = Wiki::new();
    publish_pages(&wiki, "latest", &["latest-a"]).unwrap();
    let commits = wiki.commit_count();
    assert_eq!(
        publish_pages(&wiki, "latest", &["latest-a"]).unwrap(),
        Outcome::Unchanged
    );
    assert_eq!(wiki.commit_count(), commits);
}

#[test]
fn a_run_replaces_its_version_and_leaves_other_versions_alone() {
    let wiki = Wiki::new();
    publish_pages(&wiki, "latest", &["latest-a", "latest-a-gone"]).unwrap();
    publish_pages(&wiki, "v1.0.0", &["v1.0.0-a"]).unwrap();
    publish_pages(&wiki, "latest", &["latest-a"]).unwrap();

    let check = wiki.checkout();
    assert!(check.join("api/latest/latest-a.md").is_file());
    assert!(
        !check.join("api/latest/latest-a-gone.md").exists(),
        "stale page should be removed"
    );
    assert!(check.join("api/v1.0.0/v1.0.0-a.md").is_file());

    let sidebar = fs::read_to_string(check.join("_Sidebar.md")).unwrap();
    assert!(
        sidebar.contains("**Versions:** [latest](latest-a) · [v1.0.0](v1.0.0-a)"),
        "{sidebar}"
    );
}

#[test]
fn hand_written_sidebar_content_is_preserved() {
    let wiki = Wiki::new();
    wiki.commit_files(&[("_Sidebar.md", "[Home](Home)\n[Guide](Guide)\n")]);
    publish_pages(&wiki, "latest", &["latest-a"]).unwrap();

    let sidebar = fs::read_to_string(wiki.checkout().join("_Sidebar.md")).unwrap();
    assert!(
        sidebar.starts_with("[Home](Home)\n[Guide](Guide)\n\n"),
        "{sidebar}"
    );
    assert!(sidebar.contains(BEGIN), "{sidebar}");

    // A later run edits only the managed block.
    let edited = sidebar.replace("[Guide](Guide)", "[Guide](Guide)\n[Extra](Extra)");
    wiki.commit_files(&[("_Sidebar.md", &edited)]);
    publish_pages(&wiki, "latest", &["latest-a", "latest-b"]).unwrap();
    let after = fs::read_to_string(wiki.checkout().join("_Sidebar.md")).unwrap();
    assert!(after.contains("[Extra](Extra)"), "{after}");
    assert!(after.contains("[`b`](latest-b)"), "{after}");
    assert_eq!(after.matches(BEGIN).count(), 1, "{after}");
}

#[test]
fn a_page_that_would_be_shadowed_is_an_error_and_nothing_is_pushed() {
    let wiki = Wiki::new();
    wiki.commit_files(&[("Docs/Latest-A.md", "hand written\n")]);
    let commits = wiki.commit_count();

    let error = publish_pages(&wiki, "latest", &["latest-a"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("shadowed"), "{error}");
    assert!(error.contains("Docs/Latest-A.md"), "{error}");
    assert_eq!(wiki.commit_count(), commits);
}

#[test]
fn pages_must_belong_to_the_version() {
    let wiki = Wiki::new();
    let error = publish_pages(&wiki, "v1.0.0", &["latest-a"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("--version"), "{error}");
}

#[test]
fn a_missing_wiki_explains_how_to_create_it() {
    let tmp = TempDir::new().unwrap();
    let dir = pages(&["latest-a"]);
    let error = publish(&Publish {
        wiki_url: &tmp.path().join("nope.git").display().to_string(),
        token: None,
        directory: "api",
        version: "latest",
        pages: dir.path(),
        message: "m",
        dry_run: false,
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("first page"), "{error}");
}

#[test]
fn a_dry_run_reports_changes_without_pushing() {
    let wiki = Wiki::new();
    let commits = wiki.commit_count();
    let dir = pages(&["latest-a"]);
    let outcome = publish(&Publish {
        wiki_url: &wiki.url(),
        token: None,
        directory: "api",
        version: "latest",
        pages: dir.path(),
        message: "m",
        dry_run: true,
    })
    .unwrap();
    let Outcome::DryRun { changes } = outcome else {
        panic!("expected a dry run")
    };
    assert!(
        changes.iter().any(|c| c.contains("api/latest/latest-a.md")),
        "{changes:?}"
    );
    assert_eq!(wiki.commit_count(), commits);
}

#[test]
fn a_rejected_push_is_retried_on_top_of_the_new_tip() {
    let wiki = Wiki::new();
    // On its first run, this hook lands someone else's commit just before our push does, which
    // makes ours a non-fast-forward.
    let hook = wiki.remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\n\
         if [ ! -e raced ]; then\n\
           touch raced\n\
           unset GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES\n\
           export GIT_INDEX_FILE=\"$PWD/racer-index\"\n\
           export GIT_AUTHOR_NAME=racer GIT_AUTHOR_EMAIL=r@example.com\n\
           export GIT_COMMITTER_NAME=racer GIT_COMMITTER_EMAIL=r@example.com\n\
           blob=$(echo raced | git hash-object -w --stdin)\n\
           git read-tree master\n\
           git update-index --add --cacheinfo 100644,$blob,Raced.md\n\
           tree=$(git write-tree)\n\
           parent=$(git rev-parse master)\n\
           commit=$(git commit-tree -p \"$parent\" -m racer \"$tree\")\n\
           git update-ref refs/heads/master \"$commit\"\n\
         fi\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let outcome = publish_pages(&wiki, "latest", &["latest-a"]).unwrap();
    assert!(matches!(outcome, Outcome::Pushed { .. }), "{outcome:?}");
    let check = wiki.checkout();
    assert!(
        check.join("Raced.md").is_file(),
        "the competing commit must survive"
    );
    assert!(check.join("Home.md").is_file(), "so must everything else");
    assert!(
        check.join("api/latest/latest-a.md").is_file(),
        "our pages must land on top of it"
    );
}
