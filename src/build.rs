//! Builds rustdoc JSON for the crates to document.
//!
//! Which crates, and with which features, comes from `cargo metadata` and each crate's
//! `[package.metadata.docs.rs]`, so a crate documents itself the way it does on docs.rs.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub struct Build<'a> {
    /// A directory inside the workspace (usually its root).
    pub workspace: &'a Path,
    /// A toolchain directory with `bin/cargo`, `bin/rustc` and `bin/rustdoc`. When set, it is used
    /// instead of whatever is on `PATH`, for the child processes only.
    pub toolchain: Option<&'a Path>,
    /// Package names to document. Empty means every library in the workspace.
    pub packages: &'a [String],
    pub private_items: bool,
}

/// A library to document, and how docs.rs would build it.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    package: String,
    /// The library's crate name, which names the JSON file.
    lib_name: String,
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    rustdoc_args: Vec<String>,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The libraries in `metadata`'s workspace, in name order, restricted to `wanted` if it is not
/// empty.
fn select(metadata: &Value, wanted: &[String]) -> Result<Vec<Target>> {
    let members = strings(&metadata["workspace_members"]);
    let mut available = Vec::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let (Some(id), Some(name)) = (package["id"].as_str(), package["name"].as_str()) else {
            continue;
        };
        if !members.iter().any(|m| m == id) {
            continue;
        }
        let lib = package["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| {
                strings(&t["kind"])
                    .iter()
                    .any(|k| matches!(k.as_str(), "lib" | "rlib" | "proc-macro"))
            });
        let Some(lib) = lib else {
            available.push((name.to_string(), None));
            continue;
        };
        let docs_rs = &package["metadata"]["docs.rs"];
        let target = Target {
            package: name.to_string(),
            lib_name: lib["name"].as_str().unwrap_or(name).replace('-', "_"),
            features: strings(&docs_rs["features"]),
            all_features: docs_rs["all-features"].as_bool().unwrap_or(false),
            no_default_features: docs_rs["no-default-features"].as_bool().unwrap_or(false),
            rustdoc_args: strings(&docs_rs["rustdoc-args"]),
        };
        available.push((name.to_string(), Some(target)));
    }
    available.sort_by(|a, b| a.0.cmp(&b.0));

    if wanted.is_empty() {
        let libs: Vec<Target> = available.into_iter().filter_map(|(_, t)| t).collect();
        if libs.is_empty() {
            bail!("the workspace has no library crates to document");
        }
        return Ok(libs);
    }

    let mut selected = Vec::new();
    for name in wanted {
        match available.iter().position(|(n, _)| n == name) {
            None => {
                let names: Vec<&str> = available.iter().map(|(n, _)| n.as_str()).collect();
                bail!(
                    "no workspace package named {name:?}; the workspace has: {}",
                    names.join(", ")
                )
            }
            Some(i) => match available.remove(i).1 {
                Some(target) => selected.push(target),
                None => {
                    bail!("package {name:?} has no library target, so there is nothing to document")
                }
            },
        }
    }
    Ok(selected)
}

fn cargo(toolchain: Option<&Path>) -> Command {
    let Some(toolchain) = toolchain else {
        return Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    };
    let bin = toolchain.join("bin");
    let mut path = OsString::from(&bin);
    if let Some(existing) = std::env::var_os("PATH") {
        path.push(":");
        path.push(existing);
    }
    let mut command = Command::new(bin.join("cargo"));
    command
        .env("PATH", path)
        // Name the compilers outright so that a rustup shim earlier on PATH cannot be picked.
        .env("RUSTC", bin.join("rustc"))
        .env("RUSTDOC", bin.join("rustdoc"))
        .env_remove("RUSTUP_TOOLCHAIN");
    command
}

fn rustdoc_command(b: &Build, target: &Target) -> Command {
    let mut command = cargo(b.toolchain);
    command
        .current_dir(b.workspace)
        .args(["rustdoc", "--lib", "--package", &target.package]);
    if !target.features.is_empty() {
        command.args(["--features", &target.features.join(",")]);
    }
    if target.all_features {
        command.arg("--all-features");
    }
    if target.no_default_features {
        command.arg("--no-default-features");
    }
    command.args(["--", "-Z", "unstable-options", "--output-format", "json"]);
    if b.private_items {
        command.arg("--document-private-items");
    }
    command.args(&target.rustdoc_args);
    command
}

/// Where `cargo rustdoc` wrote `<lib_name>.json` below `target_dir`.
///
/// Normally that is `<target>/doc/`, but when a build target is configured (`CARGO_BUILD_TARGET`,
/// or `build.target` in `.cargo/config.toml`) cargo writes to `<target>/<triple>/doc/` instead. If
/// several exist, the newest is the one we just built.
pub fn find_json(target_dir: &Path, lib_name: &str) -> Option<PathBuf> {
    let file = format!("{lib_name}.json");
    let mut candidates = vec![target_dir.join("doc").join(&file)];
    for entry in fs::read_dir(target_dir).into_iter().flatten().flatten() {
        candidates.push(entry.path().join("doc").join(&file));
    }
    candidates
        .into_iter()
        .filter_map(|path| Some((fs::metadata(&path).ok()?.modified().ok()?, path)))
        .max()
        .map(|(_, path)| path)
}

/// Builds rustdoc JSON, returning the path of one file per crate.
pub fn build(b: &Build) -> Result<Vec<PathBuf>> {
    let output = cargo(b.toolchain)
        .current_dir(b.workspace)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .stderr(Stdio::inherit())
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!("cargo metadata failed in {}", b.workspace.display());
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).context("parsing cargo metadata")?;
    let target_dir = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .context("cargo metadata has no target_directory")?,
    );

    let mut files = Vec::new();
    for target in select(&metadata, b.packages)? {
        eprintln!("documenting {}", target.package);
        let status = rustdoc_command(b, &target)
            .status()
            .context("running cargo rustdoc")?;
        if !status.success() {
            bail!(
                "`cargo rustdoc` failed for {}. Rustdoc's JSON output needs the nightly toolchain \
                 this action pins; when running by hand, pass --toolchain.",
                target.package
            );
        }
        let Some(json) = find_json(&target_dir, &target.lib_name) else {
            bail!(
                "rustdoc did not write {}.json below {}",
                target.lib_name,
                target_dir.display()
            );
        };
        files.push(json);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        json!({
            "workspace_members": ["path+file:///w/zeta#0.1.0", "path+file:///w/alpha#0.1.0", "path+file:///w/cli#0.1.0"],
            "packages": [
                {
                    "id": "path+file:///w/zeta#0.1.0", "name": "zeta",
                    "targets": [{"name": "zeta", "kind": ["lib"]}],
                    "metadata": {"docs.rs": {
                        "features": ["a", "b"], "all-features": true,
                        "rustdoc-args": ["--cfg", "docsrs"]
                    }}
                },
                {
                    "id": "path+file:///w/alpha#0.1.0", "name": "my-alpha",
                    "targets": [{"name": "my-alpha", "kind": ["proc-macro"]}],
                    "metadata": null
                },
                {
                    "id": "path+file:///w/cli#0.1.0", "name": "cli",
                    "targets": [{"name": "cli", "kind": ["bin"]}]
                },
                {
                    "id": "registry+dependency#1.0.0", "name": "dep",
                    "targets": [{"name": "dep", "kind": ["lib"]}]
                }
            ]
        })
    }

    #[test]
    fn selects_workspace_libraries_by_default() {
        let targets = select(&metadata(), &[]).unwrap();
        let names: Vec<&str> = targets.iter().map(|t| t.package.as_str()).collect();
        assert_eq!(names, ["my-alpha", "zeta"]);
        assert_eq!(targets[0].lib_name, "my_alpha");
    }

    #[test]
    fn reads_docs_rs_metadata() {
        let targets = select(&metadata(), &["zeta".into()]).unwrap();
        assert_eq!(
            targets,
            [Target {
                package: "zeta".into(),
                lib_name: "zeta".into(),
                features: vec!["a".into(), "b".into()],
                all_features: true,
                no_default_features: false,
                rustdoc_args: vec!["--cfg".into(), "docsrs".into()],
            }]
        );
    }

    #[test]
    fn finds_json_in_the_default_and_the_per_target_directory() {
        use std::time::{Duration, SystemTime};

        let dir = tempfile::tempdir().unwrap();
        let write = |relative: &str, age: u64| {
            let path = dir.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = fs::File::create(&path).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(age))
                .unwrap();
            path
        };
        assert_eq!(find_json(dir.path(), "a"), None);

        let plain = write("doc/a.json", 10);
        assert_eq!(find_json(dir.path(), "a"), Some(plain));

        let per_target = write("aarch64-apple-darwin/doc/a.json", 1);
        assert_eq!(find_json(dir.path(), "a"), Some(per_target));
        assert_eq!(find_json(dir.path(), "b"), None);
    }

    #[test]
    fn explains_bad_selections() {
        let unknown = select(&metadata(), &["nope".into()])
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("cli, my-alpha, zeta"), "{unknown}");
        let no_lib = select(&metadata(), &["cli".into()])
            .unwrap_err()
            .to_string();
        assert!(no_lib.contains("no library target"), "{no_lib}");
        // Dependencies are not workspace members.
        assert!(select(&metadata(), &["dep".into()]).is_err());
    }
}
