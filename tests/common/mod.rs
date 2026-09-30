//! Helpers for tests that need a wiki: a local bare repository standing in for GitHub's.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub struct Wiki {
    tmp: TempDir,
    pub remote: PathBuf,
}

impl Wiki {
    /// A wiki with a `Home.md` on `master`, like one whose first page was saved in the UI.
    pub fn new() -> Wiki {
        let tmp = TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        git(
            tmp.path(),
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=master",
                "remote.git",
            ],
        );
        let wiki = Wiki { tmp, remote };
        wiki.commit_files(&[("Home.md", "Welcome to the wiki\n")]);
        wiki
    }

    pub fn url(&self) -> String {
        self.remote.display().to_string()
    }

    /// Adds files to the wiki as if a person had edited it.
    pub fn commit_files(&self, files: &[(&str, &str)]) {
        let work = self.tmp.path().join("edit");
        let _ = fs::remove_dir_all(&work);
        git(self.tmp.path(), &["clone", "--quiet", &self.url(), "edit"]);
        for (name, content) in files {
            let path = work.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        git(&work, &["add", "--all"]);
        git(&work, &["commit", "--quiet", "--message", "edit"]);
        git(&work, &["push", "--quiet", "origin", "HEAD:master"]);
    }

    /// A fresh clone, for looking at what was published.
    pub fn checkout(&self) -> PathBuf {
        let dir = self.tmp.path().join("check");
        let _ = fs::remove_dir_all(&dir);
        git(self.tmp.path(), &["clone", "--quiet", &self.url(), "check"]);
        dir
    }

    pub fn commit_count(&self) -> usize {
        git(&self.remote, &["rev-list", "--count", "master"])
            .parse()
            .unwrap()
    }
}
