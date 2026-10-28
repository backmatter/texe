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
/// this list is ever a candidate, whatever else the directory holds. Native
/// offline runtimes and their repair archives are deliberately excluded: their
/// live identities belong to projects, rather than the embedded recipe catalog.
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
    append_shared_storage(&home, &mut report)?;
    Ok(report)
}

fn append_shared_storage(home: &Path, report: &mut StorageReport) -> Result<(), TexeError> {
    for (directory, purpose) in [
        ("toolchains", "checksummed LaTeX runtimes"),
        ("native", "verified offline runtimes and repair archives"),
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
    Ok(())
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
    preflight_project_cleanup(project_root, manifest, &targets)?;
    let mut removed = BTreeSet::new();
    for target in targets {
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
    preflight_project_cleanup(project_root, manifest, &targets)?;
    let mut seen = BTreeSet::new();
    for target in targets {
        let path = project_root.join(target);
        if path.exists() && seen.insert(target.clone()) {
            report.freed_bytes += directory_size(&path)?;
            report.project.push(target.clone());
        }
    }
    Ok(())
}

/// Validate the complete plan before removing its first path. Private names
/// alone cannot establish that a path is disposable: a source lookup root can
/// be an alias into it, and a custom manifest can name recovery metadata.
fn preflight_project_cleanup(
    project_root: &Path,
    manifest: &ProjectManifest,
    targets: &[&PathBuf],
) -> Result<(), TexeError> {
    let protected = [
        ".texe/recovery",
        ".texe/editor",
        ".texe/project",
        ".texe/runtime",
        ".texe/descriptors",
        ".texe/environment.json",
        ".texe/published.json",
        ".texe/environment-retention.json",
    ];
    let root = fs::canonicalize(project_root).map_err(|source| TexeError::Io {
        path: project_root.into(),
        source,
    })?;
    let mut resolved_targets = Vec::new();
    for target in targets {
        if protected
            .iter()
            .any(|private| target.starts_with(private) || Path::new(private).starts_with(target))
        {
            return Err(TexeError::Manifest(
                "refusing to clean recovery or editor metadata; sources and the last PDF have been kept"
                    .into(),
            ));
        }
        // An environment's owner record is not a build intermediate. Preserve
        // the container and each environment root even for custom manifests.
        let environments = Path::new(".texe/environments");
        if environments.starts_with(target)
            || (target.starts_with(environments)
                && (target.components().count() <= 3
                    || target
                        .components()
                        .nth(3)
                        .is_some_and(|part| part.as_os_str() == ".texe-environment.json")))
        {
            return Err(TexeError::Manifest(
                "refusing to clean environment ownership metadata; sources and the last PDF have been kept"
                    .into(),
            ));
        }
        reject_symlinked_parent(project_root, target)?;
        let path = root.join(target);
        match fs::canonicalize(&path) {
            Ok(path) => resolved_targets.push(path),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(TexeError::Io { path, source }),
        }
    }
    for (source, is_entry) in std::iter::once((&manifest.project.entry, true)).chain(
        manifest
            .inputs
            .roots
            .iter()
            .chain(manifest.bibliography.roots.iter())
            .map(|source| (source, false)),
    ) {
        let path = root.join(source);
        let resolved = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(TexeError::Io { path, source }),
        };
        // Searching the whole project is normal and private generated state is
        // excluded from that source tree. A narrower explicit root can contain
        // researcher files throughout the tree and must be preserved intact.
        if !is_entry && resolved == root {
            continue;
        }
        if resolved_targets.iter().any(|target| {
            resolved.starts_with(target) || (!is_entry && target.starts_with(&resolved))
        }) {
            return Err(TexeError::Manifest(format!(
                "refusing to clean a declared research source path {}; sources and the last PDF have been kept",
                source.display()
            )));
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
    use crate::clean::{
        CACHE_DIRECTORIES, CleanReport, StorageReport, append_shared_storage, clean_project,
        measure_project, retained,
    };
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
    fn storage_accounts_for_native_runtimes_and_repair_archives_without_sweeping_them() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let native = directory.path().join("native");
        let runtime = native.join("sample/bin/citet-shipout");
        let archive = native.join(".archives/sample.tar.gz");
        for path in [&runtime, &archive] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        fs::write(&runtime, b"runtime").unwrap();
        fs::write(&archive, b"compressed runtime").unwrap();
        let mut report = StorageReport {
            schema: "texe.storage-report/v1".into(),
            project: Vec::new(),
            shared: Vec::new(),
            total_bytes: 0,
        };
        append_shared_storage(directory.path(), &mut report).expect("measure native state");
        assert_eq!(report.shared.len(), 1);
        assert_eq!(report.shared[0].path, native);
        assert_eq!(report.shared[0].bytes, 25);
        assert_eq!(report.total_bytes, 25);
        assert!(report.shared[0].purpose.contains("repair archives"));
        assert!(!CACHE_DIRECTORIES.contains(&"native"));
        assert_eq!(fs::read(&runtime).unwrap(), b"runtime");
        assert_eq!(fs::read(&archive).unwrap(), b"compressed runtime");
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

    #[test]
    fn cleanup_preserves_recovery_editor_and_environment_ownership_metadata() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        for protected in [
            ".texe/recovery",
            ".texe/editor",
            ".texe/project",
            ".texe/runtime",
            ".texe/descriptors",
            ".texe/environment.json",
            ".texe/published.json",
            ".texe/environment-retention.json",
            ".texe/environments",
            ".texe/environments/owned",
            ".texe/environments/owned/.texe-environment.json",
        ] {
            let mut custom = manifest();
            custom.project.build_dir = protected.into();
            custom
                .validate()
                .expect("custom manifest is lexically valid");
            let path = root.join(protected);
            fs::create_dir_all(&path).expect("protected directory");
            fs::write(path.join("research-recovery.txt"), b"irreplaceable work")
                .expect("recovery copy");
            for measure in [false, true] {
                let mut result = report();
                let error = if measure {
                    measure_project(root, &custom, &mut result)
                } else {
                    clean_project(root, &custom, &mut result)
                }
                .expect_err("protected metadata is rejected");
                assert!(error.to_string().contains("metadata"));
                assert!(result.project.is_empty());
                assert_eq!(result.freed_bytes, 0);
                assert_eq!(
                    fs::read(path.join("research-recovery.txt")).unwrap(),
                    b"irreplaceable work"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preflights_all_targets_before_removing_a_declared_source_alias() {
        use std::os::unix::fs::symlink;

        for (source_kind, target) in [
            ("entry", ".texe/build/research.tex"),
            ("inputs", ".texe/texmf"),
            ("bibliography", ".texe/state"),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory");
            let root = directory.path();
            for derived in [
                ".texe/build/output/main.aux",
                ".texe/texmf/research.tex",
                ".texe/state/pqty.lock",
                ".texe/build/research.tex",
            ] {
                let path = root.join(derived);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"research or derived state").unwrap();
            }
            fs::write(root.join("main.pdf"), b"last good PDF").unwrap();
            fs::write(root.join("texe.lock"), b"pinned lock").unwrap();
            symlink(root.join(target), root.join("research")).unwrap();
            let mut custom = manifest();
            match source_kind {
                "entry" => custom.project.entry = "research".into(),
                "inputs" => custom.inputs.roots = vec!["research".into()],
                _ => custom.bibliography.roots = vec!["research".into()],
            }
            for measure in [false, true] {
                let mut result = report();
                let error = if measure {
                    measure_project(root, &custom, &mut result)
                } else {
                    clean_project(root, &custom, &mut result)
                }
                .expect_err("source alias must not be cleaned");
                assert!(error.to_string().contains("research source path"));
                assert!(result.project.is_empty());
                assert_eq!(result.freed_bytes, 0);
                assert!(root.join(".texe/build/output/main.aux").is_file());
                assert!(root.join(".texe/texmf/research.tex").is_file());
                assert!(root.join(".texe/state/pqty.lock").is_file());
                assert_eq!(fs::read(root.join("main.pdf")).unwrap(), b"last good PDF");
                assert_eq!(fs::read(root.join("texe.lock")).unwrap(), b"pinned lock");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_a_narrow_source_alias_containing_derived_targets() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".texe/build")).unwrap();
        fs::write(root.join(".texe/build/research.tex"), b"research").unwrap();
        symlink(root.join(".texe"), root.join("styles")).unwrap();
        let mut custom = manifest();
        custom.inputs.roots = vec!["styles".into()];
        clean_project(root, &custom, &mut report()).expect_err("source tree contains build");
        assert_eq!(
            fs::read(root.join("styles/build/research.tex")).unwrap(),
            b"research"
        );
    }

    #[test]
    fn cleanup_allows_whole_project_lookup_and_owned_environment_intermediates() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut custom = manifest();
        custom.inputs.roots = vec![".".into()];
        custom.project.build_dir = ".texe/environments/owned/build".into();
        custom.packages.texmf = ".texe/environments/owned/texmf".into();
        custom.packages.lock = ".texe/environments/owned/pqty.lock".into();
        fs::create_dir_all(root.join(&custom.project.build_dir)).unwrap();
        fs::write(
            root.join(&custom.project.build_dir).join("main.aux"),
            b"derived",
        )
        .unwrap();
        let marker = root.join(".texe/environments/owned/.texe-environment.json");
        fs::write(&marker, b"owned environment").unwrap();
        fs::write(root.join("main.tex"), b"source").unwrap();
        let mut measured = report();
        measure_project(root, &custom, &mut measured).unwrap();
        assert!(root.join(&custom.project.build_dir).is_dir());
        let mut removed = report();
        clean_project(root, &custom, &mut removed).unwrap();
        assert_eq!(measured.freed_bytes, removed.freed_bytes);
        assert_eq!(fs::read(marker).unwrap(), b"owned environment");
        assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"source");
        assert!(!root.join(&custom.project.build_dir).exists());
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
