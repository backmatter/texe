//! Package trees shared between hosted Papers.
//!
//! A protected build normally copies its locked package environment into the
//! Paper (`.texe/texmf`). A hosted operator builds many Papers in fresh
//! directories, so `TEXE_HOSTED=1` protected builds instead materialize each
//! environment once below `TEXE_HOME/package-trees/<fingerprint>/texmf` and
//! point every engine, processor and trace at that tree.
//!
//! The pqty environment fingerprint is a SHA-256 over every provider's
//! version and container integrity and every locked file path, and pqty
//! verifies each container against that integrity while materializing, so one
//! fingerprint names exactly one tree. Only this trusted parent writes a tree:
//! under a guard, into a staging directory that is renamed into place only
//! after the install succeeded, every entry was checked to be a plain file or
//! directory, and a completion marker naming the fingerprint was written.
//! Compilers receive the tree as a Landlock read root and cannot write it. A
//! Paper cannot name the tree either: protected package paths must stay below
//! the Paper's `.texe`, and shared trees are always installed from the trusted
//! default store, never a project-configured one.
use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::config::ProjectManifest;
use crate::package::{PackageEnvironment, PqtyClient};
use crate::progress::Progress;
use crate::toolchain::ResolvedToolchain;

const TREES: &str = "package-trees";
const TREE: &str = "texmf";
const READY_MARKER: &str = ".texe-tree-ready";
const READY_SCHEMA: &str = "texe.shared-package-tree/v1";
/// Entries a shared tree may hold; a locked environment has far fewer.
const MAX_TREE_ENTRIES: usize = 500_000;

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReadyMarker {
    schema: String,
    fingerprint: String,
}

/// Whether this build materializes its packages into a shared tree: hosted,
/// protected builds of engines whose package tree texe owns (Shipout serves
/// its own).
pub(crate) fn enabled(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
) -> bool {
    cfg!(unix)
        && std::env::var_os("TEXE_HOSTED").is_some()
        && toolchain.engine != "shipout"
        && super::process::is_portable_build(
            project_root,
            &project_root.join(&manifest.project.build_dir),
        )
}

/// The shared tree for `fingerprint`, materialized and verified complete,
/// registered as a read-only root for every compiler of this build.
pub(crate) fn ensure(
    tools: &PqtyClient,
    project_root: &Path,
    toolchain: &ResolvedToolchain,
    lock: &Path,
    environment: &PackageEnvironment,
    progress: &Progress,
) -> Result<PathBuf, TexeError> {
    let trees = trees_root(project_root)?;
    let key = fingerprint_key(&environment.fingerprint)?;
    let directory = trees.join(key);
    let tree = directory.join(TREE);
    if !is_ready(&directory, &environment.fingerprint) {
        let _guard = crate::guard::BuildGuard::acquire(&trees)?;
        if !is_ready(&directory, &environment.fingerprint) {
            materialize(
                tools,
                project_root,
                toolchain,
                lock,
                &trees,
                &directory,
                key,
                &environment.fingerprint,
                progress,
            )?;
        }
    }
    super::process::add_shared_read_root(tree.clone());
    Ok(tree)
}

/// `TEXE_HOME/package-trees`, canonical and outside the Paper.
fn trees_root(project_root: &Path) -> Result<PathBuf, TexeError> {
    trees_root_in(&crate::toolchain::texe_data_home()?, project_root)
}

fn trees_root_in(home: &Path, project_root: &Path) -> Result<PathBuf, TexeError> {
    let trees = home.join(TREES);
    fs::create_dir_all(&trees).map_err(|source| TexeError::Io {
        path: trees.clone(),
        source,
    })?;
    let trees = fs::canonicalize(&trees).map_err(|source| TexeError::Io {
        path: trees.clone(),
        source,
    })?;
    let project = fs::canonicalize(project_root).map_err(|source| TexeError::Io {
        path: project_root.to_path_buf(),
        source,
    })?;
    if trees.starts_with(&project) || project.starts_with(&trees) {
        return Err(TexeError::Build(
            "Shared package trees must lie outside the Paper; set TEXE_HOME elsewhere".into(),
        ));
    }
    Ok(trees)
}

fn fingerprint_key(fingerprint: &str) -> Result<&str, TexeError> {
    let key = fingerprint.strip_prefix("sha256:").unwrap_or_default();
    if key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(key)
    } else {
        Err(TexeError::Build(format!(
            "pqty emitted an unexpected environment fingerprint: {fingerprint}"
        )))
    }
}

/// A complete tree for `fingerprint`: real directories (no links) and a
/// marker this parent wrote after verifying the installation.
fn is_ready(directory: &Path, fingerprint: &str) -> bool {
    let real_directory =
        |path: &Path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
    let marker = directory.join(READY_MARKER);
    real_directory(directory)
        && real_directory(&directory.join(TREE))
        && fs::symlink_metadata(&marker).is_ok_and(|metadata| metadata.is_file())
        && fs::read(&marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ReadyMarker>(&bytes).ok())
            .is_some_and(|ready| ready.schema == READY_SCHEMA && ready.fingerprint == fingerprint)
}

#[allow(clippy::too_many_arguments)]
fn materialize(
    tools: &PqtyClient,
    project_root: &Path,
    toolchain: &ResolvedToolchain,
    lock: &Path,
    trees: &Path,
    directory: &Path,
    key: &str,
    fingerprint: &str,
    progress: &Progress,
) -> Result<(), TexeError> {
    // A previous attempt that did not finish, or anything else in the way.
    remove_entry(directory)?;
    let staging = trees.join(format!(".{key}.{}.tmp", std::process::id()));
    remove_entry(&staging)?;
    let result = (|| {
        let tree = staging.join(TREE);
        fs::create_dir_all(&tree).map_err(|source| TexeError::Io {
            path: tree.clone(),
            source,
        })?;
        tools.install_shared(project_root, toolchain, lock, &tree, progress)?;
        verify_plain_tree(&tree)?;
        let marker = serde_json::to_vec(&ReadyMarker {
            schema: READY_SCHEMA.to_string(),
            fingerprint: fingerprint.to_string(),
        })
        .map_err(|source| TexeError::Json {
            path: staging.join(READY_MARKER),
            source,
        })?;
        crate::atomic::write(&staging.join(READY_MARKER), &marker)?;
        fs::rename(&staging, directory).map_err(|source| TexeError::Io {
            path: directory.to_path_buf(),
            source,
        })
    })();
    if result.is_err() {
        let _ = remove_entry(&staging);
    }
    result
}

fn remove_entry(path: &Path) -> Result<(), TexeError> {
    let removed = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => Err(source),
    };
    removed.map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Every entry of a freshly installed tree is a plain directory or regular
/// file: no link can lead a later reader outside it.
fn verify_plain_tree(tree: &Path) -> Result<(), TexeError> {
    let mut pending = vec![tree.to_path_buf()];
    let mut entries = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|source| TexeError::Io {
            path: directory.clone(),
            source,
        })? {
            entries += 1;
            if entries > MAX_TREE_ENTRIES {
                return Err(TexeError::Build(
                    "The shared package tree has too many entries".into(),
                ));
            }
            let entry = entry.map_err(|source| TexeError::Io {
                path: directory.clone(),
                source,
            })?;
            let kind = entry.file_type().map_err(|source| TexeError::Io {
                path: entry.path(),
                source,
            })?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if !kind.is_file() {
                return Err(TexeError::Build(format!(
                    "The shared package tree may only hold files and directories: {}",
                    entry.path().display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        READY_MARKER, READY_SCHEMA, ReadyMarker, TREE, is_ready, trees_root_in, verify_plain_tree,
    };

    const FINGERPRINT: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn ready_tree(root: &std::path::Path) -> std::path::PathBuf {
        let directory = root.join("tree");
        fs::create_dir_all(directory.join(TREE).join("tex")).unwrap();
        fs::write(
            directory.join(READY_MARKER),
            serde_json::to_vec(&ReadyMarker {
                schema: READY_SCHEMA.into(),
                fingerprint: FINGERPRINT.into(),
            })
            .unwrap(),
        )
        .unwrap();
        directory
    }

    #[test]
    fn only_a_completed_tree_for_the_same_environment_is_used() {
        let root = tempfile::tempdir().unwrap();
        let directory = ready_tree(root.path());
        assert!(is_ready(&directory, FINGERPRINT));
        // Another environment's tree, or one whose installation never finished.
        assert!(!is_ready(&directory, &FINGERPRINT.replace("0123", "3210")));
        fs::remove_file(directory.join(READY_MARKER)).unwrap();
        assert!(!is_ready(&directory, FINGERPRINT));
    }

    #[cfg(unix)]
    #[test]
    fn linked_trees_are_never_used_or_published() {
        let root = tempfile::tempdir().unwrap();
        let directory = ready_tree(root.path());
        // A tree directory replaced by a link to somewhere a Paper can write.
        let elsewhere = root.path().join("paper/.texe/texmf");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::rename(directory.join(TREE), root.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, directory.join(TREE)).unwrap();
        assert!(!is_ready(&directory, FINGERPRINT));
        // A freshly installed tree containing a link fails verification.
        let fresh = root.path().join("fresh");
        fs::create_dir_all(fresh.join("tex/latex")).unwrap();
        fs::write(fresh.join("tex/latex/a.sty"), b"").unwrap();
        assert!(verify_plain_tree(&fresh).is_ok());
        std::os::unix::fs::symlink(&elsewhere, fresh.join("tex/latex/b.sty")).unwrap();
        assert!(verify_plain_tree(&fresh).is_err());
    }

    #[test]
    fn shared_trees_never_lie_inside_a_paper() {
        let root = tempfile::tempdir().unwrap();
        let paper = root.path().join("paper");
        fs::create_dir_all(&paper).unwrap();
        // A TEXE_HOME inside the Paper would let its files name shared trees.
        assert!(trees_root_in(&paper.join(".texe/home"), &paper).is_err());
        // Nor may a Paper be built from inside the shared trees.
        let home = root.path().join("home");
        let nested = home.join("package-trees/paper");
        fs::create_dir_all(&nested).unwrap();
        assert!(trees_root_in(&home, &nested).is_err());
        assert!(trees_root_in(&home, &paper).is_ok());
    }
}
