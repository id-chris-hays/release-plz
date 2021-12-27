mod git;
mod version;

use crate::{git::Repo, version::NextVersionFromDiff};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use cargo_edit::LocalManifest;
use cargo_metadata::{Package, Version};
use folder_compare::FolderCompare;
use tracing::{debug, instrument};

#[derive(Debug)]
struct LocalPackage {
    package: Package,
    diff: Diff,
}

/// Difference between local and remote crate
#[derive(Debug)]
struct Diff {
    pub commits: Vec<String>,
    /// Whether the crate name exists in the remote crates or not
    pub remote_crate_exists: bool,
}

impl Diff {
    fn new(remote_crate_exists: bool) -> Self {
        Self {
            commits: vec![],
            remote_crate_exists,
        }
    }
}

/// Update a local rust project
#[instrument]
pub fn update(local_manifest: &Path, remote_manifest: &Path) -> anyhow::Result<()> {
    let local_crates = list_crates(local_manifest);
    let remote_crates = list_crates(remote_manifest);
    let mut local_crates = calculate_local_crates(local_crates.into_iter())?;
    let remote_crates = calculate_remote_crates(remote_crates.into_iter())?;
    let mut local_path = local_manifest.to_path_buf();
    local_path.pop();
    let repository = Repo::new(&local_path)?;

    debug!("calculating local packages");
    for (package_path, package) in &mut local_crates {
        debug!("processing local package {}", package.package.name);
        repository.checkout_head()?;
        if let Err(_err) = repository.checkout_last_commit_at_path(package_path) {
            // there are no commits for this package
            break;
        }
        loop {
            let current_commit_message = repository.current_commit_message()?;
            if let Some(remote_crate) = remote_crates.get(&package.package.name) {
                debug!("remote crate {} found", remote_crate.name);
                package.diff.remote_crate_exists = true;
                let mut remote_path = remote_crate.manifest_path.clone();
                remote_path.pop();
                if are_dir_equal(package_path, remote_path.as_ref()) {
                    debug!("directories are equal");
                    // The local crate is identical to the remote one, so go to the next create
                    break;
                } else {
                    debug!("directories differ");
                    package.diff.commits.push(current_commit_message.clone());
                }
            } else {
                package.diff.commits.push(current_commit_message.clone());
            }
            if let Err(_err) = repository.checkout_previous_commit_at_path(package_path) {
                // there are no other commits.
                break;
            }
        }
    }
    debug!("local packages calculated");

    for (package_path, package) in &mut local_crates {
        let current_version = package.package.version.clone();
        debug!("diff: {:?}", &package.diff);
        let next_version = current_version.next_from_diff(&package.diff);

        debug!("next version: {}", next_version);
        if next_version != current_version {
            set_version(package_path, &next_version);
        }
    }

    Ok(())
}

#[instrument]
fn set_version(package_path: &Path, version: &Version) {
    debug!("updating version");
    let mut local_manifest =
        LocalManifest::try_new(&package_path.join("Cargo.toml")).expect("cannot read manifest");
    local_manifest.set_package_version(version);
    local_manifest.write().expect("cannot update manifest");
}

fn list_crates(directory: &Path) -> Vec<Package> {
    cargo_edit::workspace_members(Some(directory)).unwrap()
}

fn calculate_local_crates(
    crates: impl Iterator<Item = Package>,
) -> anyhow::Result<BTreeMap<PathBuf, LocalPackage>> {
    crates
        .map(|c| {
            let mut manifest_path = c.manifest_path.clone();
            debug!("manifest path: {}", manifest_path);
            manifest_path.pop();
            let crate_path: PathBuf = manifest_path.into_std_path_buf();
            debug!("crate path: {:?}", crate_path);
            let local_package = LocalPackage {
                package: c,
                diff: Diff::new(false),
            };
            Ok((crate_path, local_package))
        })
        .collect()
}

/// Return [`BTreeMap`] with "package name" as key
fn calculate_remote_crates(
    crates: impl Iterator<Item = Package>,
) -> anyhow::Result<BTreeMap<String, Package>> {
    crates
        .map(|c| {
            let package_name = c.name.clone();
            Ok((package_name, c))
        })
        .collect()
}

fn are_dir_equal(first: &Path, second: &Path) -> bool {
    let excluded = vec![".git".to_string()];
    let result = FolderCompare::new(first, second, &excluded).unwrap();
    result.changed_files.is_empty() && result.new_files.is_empty()
}