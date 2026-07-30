//! Removing what a build leaves behind.
//!
//! Everything texe writes outside the project root is derived: a managed
//! runtime is reinstalled from pinned digests, a package store is restored from
//! its lock, a format is regenerated from that package environment, and a
//! materialized TEXMF tree is reproduced from `texe.lock`. None of it is user
//! data, and until something removes it, every recipe change orphans a runtime
//! and every package change mints a format that nothing will ever ask for again.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::TexeError;
use crate::config::ProjectManifest;
use crate::toolchain::{LiveToolchainArtifacts, live_toolchain_artifacts};

/// The `TEXE_HOME` subdirectories a sweep is allowed to touch. Nothing outside
/// this list is ever a candidate, whatever else the directory holds.
const CACHE_DIRECTORIES: &[&str] = &[
    "toolchains",
    "formats",
    "components",
    "downloads",
    "pqty",
    "editor",
    "timings",
];

#[derive(Debug, Clone, Copy, Default)]
pub struct CleanOptions {
    /// Also sweep shared caches below `TEXE_HOME` that no current recipe needs.
    pub caches: bool,
    /// Sweep every managed runtime, format, component, and download, including
    /// the ones current recipes still use. They reinstall on the next build.
    pub all: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanReport {
    pub schema: String,
    /// Project paths removed, relative to the project root.
    pub project: Vec<PathBuf>,
    /// Shared cache paths removed, absolute.
    pub caches: Vec<PathBuf>,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageReport {
    pub schema: String,
    pub project: Vec<StorageEntry>,
    pub shared: Vec<StorageEntry>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageEntry {
    pub path: PathBuf,
    pub bytes: u64,
    pub purpose: String,
}

/// Measure removable project state and managed caches without changing them.
pub fn storage_report(
    project: Option<(&Path, &ProjectManifest)>,
) -> Result<StorageReport, TexeError> {
    let mut report = StorageReport {
        schema: "texe.storage-report/v1".to_string(),
        project: Vec::new(),
        shared: Vec::new(),
        total_bytes: 0,
    };
    if let Some((root, manifest)) = project {
        manifest.validate()?;
        for (relative, purpose) in [
            (&manifest.project.build_dir, "build intermediates"),
            (
                &manifest.packages.texmf,
                "materialized project package tree",
            ),
            (&manifest.packages.lock, "internal package-manager lock"),
        ] {
            let path = root.join(relative);
            if !path.exists() {
                continue;
            }
            let bytes = directory_size(&path)?;
            report.total_bytes += bytes;
            report.project.push(StorageEntry {
                path: relative.clone(),
                bytes,
                purpose: purpose.to_string(),
            });
        }
    }
    let home = crate::toolchain::texe_data_home()?;
    for (directory, purpose) in [
        ("toolchains", "checksummed LaTeX runtimes"),
        ("formats", "generated LaTeX formats"),
        ("components", "checksummed bibliography tools"),
        ("downloads", "verified reusable downloads"),
        ("pqty", "package registry, downloads, and shared store"),
        ("editor", "bundled VS Code integration"),
        ("timings", "local per-project build timing history"),
    ] {
        let path = home.join(directory);
        if !path.exists() {
            continue;
        }
        let bytes = directory_size(&path)?;
        report.total_bytes += bytes;
        report.shared.push(StorageEntry {
            path,
            bytes,
            purpose: purpose.to_string(),
        });
    }
    Ok(report)
}

/// Remove a project's derived build state.
///
/// Keeps `texe.lock` and the published artifact: the first is the project's
/// pinned identity and the second is its output.
///
/// # Errors
///
/// Returns an error when a directory cannot be measured or removed.
pub fn clean_project(
    project_root: &Path,
    manifest: &ProjectManifest,
    report: &mut CleanReport,
) -> Result<(), TexeError> {
    manifest.validate()?;
    // Exactly the manifest-declared derived paths, so an unusual layout loses
    // its build state and nothing else.
    let targets = [
        &manifest.project.build_dir,
        &manifest.packages.texmf,
        &manifest.packages.lock,
    ];
    let mut removed = BTreeSet::new();
    for target in targets {
        reject_symlinked_parent(project_root, target)?;
        let path = project_root.join(target);
        if !path.exists() || !removed.insert(target.clone()) {
            continue;
        }
        report.freed_bytes += remove_tree(&path)?;
        report.project.push(target.clone());
    }
    // A `.texe` left holding nothing is itself derived.
    for ancestor in enclosing_directories(project_root, &targets) {
        let path = project_root.join(&ancestor);
        if is_empty_directory(&path) {
            fs::remove_dir(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            report.project.push(ancestor);
        }
    }
    Ok(())
}

/// Describe the project paths `clean_project` would remove without changing
/// them.
pub fn measure_project(
    project_root: &Path,
    manifest: &ProjectManifest,
    report: &mut CleanReport,
) -> Result<(), TexeError> {
    manifest.validate()?;
    let targets = [
        &manifest.project.build_dir,
        &manifest.packages.texmf,
        &manifest.packages.lock,
    ];
    let mut seen = BTreeSet::new();
    for target in targets {
        reject_symlinked_parent(project_root, target)?;
        let path = project_root.join(target);
        if path.exists() && seen.insert(target.clone()) {
            report.freed_bytes += directory_size(&path)?;
            report.project.push(target.clone());
        }
    }
    Ok(())
}

/// A private target may itself be a symlink—removing that link is confined—but
/// traversing a symlink in one of its parents could redirect recursive removal
/// outside the project.
fn reject_symlinked_parent(project_root: &Path, target: &Path) -> Result<(), TexeError> {
    let mut current = project_root.to_path_buf();
    let Some(parent) = target.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(TexeError::Manifest(format!(
                    "refusing to clean through symlinked derived directory {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(TexeError::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

/// Sweep shared managed caches.
///
/// # Errors
///
/// Returns an error when the storage directory cannot be chosen, measured, or
/// swept.
pub fn clean_caches(options: CleanOptions, report: &mut CleanReport) -> Result<(), TexeError> {
    let live = live_toolchain_artifacts()?;
    for directory in CACHE_DIRECTORIES {
        let root = live.home.join(directory);
        let Some(entries) = owned_cache_entries(&root)? else {
            continue;
        };
        for entry in entries {
            let entry = entry.map_err(|source| TexeError::Io {
                path: root.clone(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !options.all && retained(directory, &name, &live) {
                continue;
            }
            let path = entry.path();
            report.freed_bytes += remove_tree(&path)?;
            report.caches.push(path);
        }
    }
    Ok(())
}

/// Describe the shared paths `clean_caches` would remove without changing
/// them.
pub fn measure_caches(options: CleanOptions, report: &mut CleanReport) -> Result<(), TexeError> {
    let live = live_toolchain_artifacts()?;
    for directory in CACHE_DIRECTORIES {
        let root = live.home.join(directory);
        let Some(entries) = owned_cache_entries(&root)? else {
            continue;
        };
        for entry in entries {
            let entry = entry.map_err(|source| TexeError::Io {
                path: root.clone(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !options.all && retained(directory, &name, &live) {
                continue;
            }
            let path = entry.path();
            report.freed_bytes += directory_size(&path)?;
            report.caches.push(path);
        }
    }
    Ok(())
}

fn owned_cache_entries(root: &Path) -> Result<Option<fs::ReadDir>, TexeError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(TexeError::Io {
                path: root.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TexeError::Build(format!(
            "refusing to clean through non-directory managed cache root {}",
            root.display()
        )));
    }
    fs::read_dir(root)
        .map(Some)
        .map_err(|source| TexeError::Io {
            path: root.to_path_buf(),
            source,
        })
}

/// Whether a cache entry belongs to a recipe this binary can still resolve.
/// Anything else was installed by a recipe that no longer exists here, so no
/// lock can name it and no build can reach it.
fn retained(directory: &str, name: &str, live: &LiveToolchainArtifacts) -> bool {
    match directory {
        "toolchains" => live.runtimes.contains(name),
        "formats" => live.fingerprints.contains(name),
        "components" => live.components.contains(name),
        "downloads" => live.downloads.contains(name),
        // Package objects may still back an explicitly linked project tree, and
        // the editor bridge is tiny. Keep those and any future roots during an
        // orphan sweep; `--all` and uninstall remove named roots explicitly.
        _ => true,
    }
}

/// The directories that would hold `targets`, nearest the root first, so an
/// emptied `.texe` can be removed after its contents are.
fn enclosing_directories(project_root: &Path, targets: &[&PathBuf]) -> Vec<PathBuf> {
    let mut directories = BTreeSet::new();
    for target in targets {
        let mut current = target.parent();
        while let Some(parent) = current {
            if parent.as_os_str().is_empty() || !project_root.join(parent).is_dir() {
                break;
            }
            directories.insert(parent.to_path_buf());
            current = parent.parent();
        }
    }
    // Deepest first: a parent only becomes empty once its children are gone.
    directories.into_iter().rev().collect()
}

fn is_empty_directory(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Remove a file or directory tree, returning the bytes it occupied.
fn remove_tree(path: &Path) -> Result<u64, TexeError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let size = directory_size(path)?;
    let removed = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(size)
}

/// Best-effort size of a tree. A file that cannot be measured contributes
/// nothing rather than failing a removal that is about to succeed. Symlinks are
/// never followed, so a linked TEXMF tree reports what it actually occupies
/// instead of the store it points into.
fn directory_size(path: &Path) -> Result<u64, TexeError> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(0);
    };
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    let mut total = 0;
    let Ok(entries) = fs::read_dir(path) else {
        return Ok(0);
    };
    for entry in entries {
        let entry = entry.map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        total += directory_size(&entry.path())?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::ProjectManifest;
    #[cfg(unix)]
    use crate::clean::owned_cache_entries;
    use crate::clean::{CACHE_DIRECTORIES, CleanReport, clean_project, retained};
    use crate::toolchain::live_toolchain_artifacts;

    fn manifest() -> ProjectManifest {
        toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
        )
        .expect("manifest parses")
    }

    fn report() -> CleanReport {
        CleanReport {
            schema: "texe.clean-report/v1".to_string(),
            project: Vec::new(),
            caches: Vec::new(),
            freed_bytes: 0,
        }
    }

    #[test]
    fn cleaning_keeps_the_lock_and_the_published_artifact() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        for path in [
            ".texe/build/output/main.aux",
            ".texe/texmf/tex/latex/base/article.cls",
            ".texe/state/pqty.lock",
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            fs::write(&path, b"derived").expect("derived file");
        }
        fs::write(root.join("main.tex"), b"source").expect("source");
        fs::write(root.join("main.pdf"), b"%PDF").expect("artifact");
        fs::write(root.join("texe.lock"), b"{}").expect("lock");

        let mut report = report();
        clean_project(root, &manifest(), &mut report).expect("clean");

        assert!(root.join("main.tex").is_file());
        assert!(root.join("main.pdf").is_file());
        assert!(root.join("texe.lock").is_file());
        assert!(
            !root.join(".texe").exists(),
            "an emptied .texe is derived too"
        );
        assert!(report.freed_bytes > 0);
    }

    #[test]
    fn cleaning_a_project_that_was_never_built_is_not_an_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut report = report();
        clean_project(directory.path(), &manifest(), &mut report).expect("clean");
        assert!(report.project.is_empty());
        assert_eq!(report.freed_bytes, 0);
    }

    #[test]
    fn cleaning_revalidates_programmatically_constructed_manifests() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::write(root.join("main.tex"), b"source").expect("source");
        let mut unsafe_manifest = manifest();
        unsafe_manifest.project.build_dir = ".".into();

        let error = clean_project(root, &unsafe_manifest, &mut report()).expect_err("unsafe clean");
        assert!(error.to_string().contains("project.build_dir"));
        assert_eq!(fs::read(root.join("main.tex")).expect("source"), b"source");
    }

    #[cfg(unix)]
    #[test]
    fn cleaning_refuses_to_follow_a_symlinked_private_parent() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::create_dir_all(outside.path().join("build")).expect("outside build");
        fs::write(outside.path().join("build/keep.txt"), b"keep").expect("outside file");
        symlink(outside.path(), directory.path().join(".texe")).expect("private symlink");

        let error = clean_project(directory.path(), &manifest(), &mut report())
            .expect_err("symlinked parent is rejected");
        assert!(error.to_string().contains("symlinked derived directory"));
        assert!(outside.path().join("build/keep.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn cache_sweeps_refuse_a_symlinked_owned_root() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::write(outside.path().join("keep.txt"), b"keep").expect("outside file");
        let root = directory.path().join("pqty");
        symlink(outside.path(), &root).expect("cache symlink");

        let error = owned_cache_entries(&root).expect_err("symlinked root is rejected");
        assert!(
            error
                .to_string()
                .contains("non-directory managed cache root")
        );
        assert!(outside.path().join("keep.txt").is_file());
    }

    #[test]
    fn only_orphaned_cache_entries_are_swept() {
        let live = live_toolchain_artifacts().expect("live artifacts");
        let runtime = live.runtimes.iter().next().expect("a current runtime");
        let fingerprint = live.fingerprints.iter().next().expect("a current recipe");
        let component = live.components.iter().next().expect("a current component");
        let download = live.downloads.iter().next().expect("a current artifact");

        assert!(retained("toolchains", runtime, &live));
        assert!(retained("formats", fingerprint, &live));
        assert!(retained("downloads", download, &live));
        assert!(retained("components", component, &live));
        assert!(retained("pqty", "store", &live));
        assert!(retained("editor", "texe-paper-layout.vsix", &live));

        assert!(!retained(
            "toolchains",
            "texlive-2026-07-26-pdftex-x86_64-linux-deadbeefdeadbeef",
            &live
        ));
        assert!(!retained("formats", &"0".repeat(64), &live));
        assert!(!retained("downloads", "abc.tar.xz", &live));
        assert!(!retained(
            "components",
            "biber-2.20-x86_64-linux-0000",
            &live
        ));
    }

    #[test]
    fn a_sweep_never_leaves_the_known_cache_directories() {
        let live = live_toolchain_artifacts().expect("live artifacts");
        for directory in CACHE_DIRECTORIES {
            assert!(live.home.join(directory).starts_with(&live.home));
        }
        // An unknown directory below TEXE_HOME is not a candidate at all.
        assert!(retained("registries", "anything", &live));
    }
}
