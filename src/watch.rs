//! Project change detection for `texe watch`.
//!
//! The operating system reports file events; a snapshot then decides whether
//! any input really changed. Snapshots are metadata only and exclude derived
//! state, so a build's own outputs wake the loop without rebuilding. Baselines
//! are captured before builds, so saves during compilation stay pending.
#![allow(clippy::trivially_copy_pass_by_ref)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;

use crate::TexeError;
use crate::config::ProjectManifest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSnapshot {
    files: BTreeMap<PathBuf, FileStamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    bytes: u64,
    modified_ns: u128,
}

impl ProjectSnapshot {
    pub(crate) fn capture(
        project_root: &Path,
        manifest: &ProjectManifest,
    ) -> Result<Self, TexeError> {
        let mut excluded = vec![
            Path::new(".texe"),
            Path::new("texe.lock"),
            Path::new(".vscode"),
            manifest.project.build_dir.as_path(),
            manifest.packages.texmf.as_path(),
            manifest.packages.lock.as_path(),
        ];
        let published = manifest
            .project
            .entry
            .file_name()
            .map_or_else(Vec::new, |entry| {
                ["pdf", "dvi", "synctex.gz"]
                    .map(|extension| Path::new(entry).with_extension(extension))
                    .to_vec()
            });
        excluded.extend(published.iter().map(PathBuf::as_path));
        let mut files = BTreeMap::new();
        collect(project_root, project_root, &excluded, &mut files)?;
        Ok(Self { files })
    }

    pub(crate) fn changes_since(&self, newer: &Self) -> Vec<PathBuf> {
        self.files
            .keys()
            .chain(newer.files.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|path| self.files.get(*path) != newer.files.get(*path))
            .map(|path| (*path).clone())
            .collect()
    }
}

/// What wakes the watch loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchSignal {
    /// The filesystem reported activity under the project.
    FilesChanged,
    /// The user asked to stop.
    Stop,
}

/// Forward file events under `root` to `signals` until the result is dropped.
pub(crate) fn watch_files(
    root: &Path,
    signals: Sender<WatchSignal>,
) -> Result<notify::RecommendedWatcher, TexeError> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // The receiver goes away only as the watch ends.
        if event.is_ok_and(|event| file_settled(&event.kind)) {
            let _ = signals.send(WatchSignal::FilesChanged);
        }
    })
    .map_err(|error| TexeError::Build(format!("could not watch the project: {error}")))?;
    notify::Watcher::watch(&mut watcher, root, notify::RecursiveMode::Recursive).map_err(
        |error| {
            TexeError::Build(format!(
                "could not watch {}: {error}. On Linux, raising \
                 fs.inotify.max_user_watches may help",
                root.display()
            ))
        },
    )?;
    Ok(watcher)
}

/// Whether an event means a file reached a state worth reading. Saving often
/// truncates and then writes, so on platforms that report it, wait for the
/// writer to close the file instead of building from partial contents.
fn file_settled(kind: &notify::EventKind) -> bool {
    use notify::EventKind::{Access, Modify};
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    match kind {
        Access(AccessKind::Close(AccessMode::Write)) => true,
        Access(_) => false,
        Modify(ModifyKind::Data(_) | ModifyKind::Any) => !cfg!(target_os = "linux"),
        _ => true,
    }
}

fn collect(
    project_root: &Path,
    directory: &Path,
    excluded: &[&Path],
    files: &mut BTreeMap<PathBuf, FileStamp>,
) -> Result<(), TexeError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(TexeError::Io {
                path: directory.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(TexeError::Io {
                    path: directory.to_path_buf(),
                    source,
                });
            }
        };
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(project_root) else {
            continue;
        };
        if ignored(relative, excluded) {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(TexeError::Io {
                    path: path.clone(),
                    source,
                });
            }
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect(project_root, &path, excluded, files)?;
        } else {
            let modified_ns = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            files.insert(
                relative.to_path_buf(),
                FileStamp {
                    bytes: metadata.len(),
                    modified_ns,
                },
            );
        }
    }
    Ok(())
}

fn ignored(relative: &Path, excluded: &[&Path]) -> bool {
    relative.components().next().is_some_and(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | "node_modules" | "target")
        )
    }) || excluded
        .iter()
        .any(|excluded| relative == *excluded || relative.starts_with(excluded))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::ProjectManifest;
    use crate::watch::ProjectSnapshot;

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
        .expect("manifest")
    }

    #[test]
    fn saves_during_a_build_remain_pending_but_its_outputs_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("main.tex"), "before build").unwrap();
        let baseline = ProjectSnapshot::capture(root, &manifest()).unwrap();
        fs::create_dir_all(root.join(".texe/editor")).unwrap();
        fs::write(root.join("texe.lock"), "new lock").unwrap();
        fs::write(root.join("main.pdf"), "published PDF").unwrap();
        fs::write(root.join(".texe/editor/build-bridge.json"), "bridge").unwrap();
        let outputs = ProjectSnapshot::capture(root, &manifest()).unwrap();
        assert!(baseline.changes_since(&outputs).is_empty());
        fs::write(root.join("main.tex"), "saved during compilation").unwrap();
        let saved = ProjectSnapshot::capture(root, &manifest()).unwrap();
        assert_eq!(
            baseline.changes_since(&saved),
            vec![PathBuf::from("main.tex")]
        );
    }

    #[test]
    fn a_temporary_file_that_vanishes_leaves_nothing_to_rebuild() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let baseline = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        fs::write(root.join("temporary.tex"), b"source").expect("temporary file");
        let edited = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        assert!(!baseline.changes_since(&edited).is_empty());
        fs::remove_file(root.join("temporary.tex")).expect("remove");
        let restored = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        assert!(baseline.changes_since(&restored).is_empty());
    }

    #[test]
    fn published_outputs_are_excluded_by_exact_name() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let mut manifest = manifest();
        manifest.project.entry = PathBuf::from("sources/main.v2.tex");
        for name in [
            "main.v2.pdf",
            "main.v2.synctex.gz",
            "main.v2.gz",
            "main.pdf",
        ] {
            fs::write(root.join(name), b"content").expect("file");
        }
        let snapshot = ProjectSnapshot::capture(root, &manifest).expect("snapshot");
        assert!(!snapshot.files.contains_key(Path::new("main.v2.pdf")));
        assert!(!snapshot.files.contains_key(Path::new("main.v2.synctex.gz")));
        assert!(snapshot.files.contains_key(Path::new("main.v2.gz")));
        assert!(snapshot.files.contains_key(Path::new("main.pdf")));
    }

    #[test]
    fn snapshots_track_sources_but_ignore_all_derived_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        for (path, bytes) in [
            ("texe.toml", b"manifest".as_slice()),
            ("main.tex", b"source"),
            ("main.pdf", b"artifact"),
            (".texe/build/output/main.aux", b"auxiliary"),
            (".texe/texmf/tex/latex/base/article.cls", b"package"),
            (".texe/state/pqty.lock", b"lock"),
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            fs::write(path, bytes).expect("file");
        }
        let first = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        assert!(first.files.contains_key(Path::new("texe.toml")));

        fs::write(root.join(".texe/build/output/main.aux"), b"changed").expect("derived edit");
        fs::write(root.join("main.pdf"), b"changed").expect("artifact edit");
        assert_eq!(
            first,
            ProjectSnapshot::capture(root, &manifest()).expect("snapshot")
        );

        fs::write(root.join("main.tex"), b"changed").expect("source edit");
        assert_ne!(
            first,
            ProjectSnapshot::capture(root, &manifest()).expect("snapshot")
        );
    }

    #[test]
    fn change_reasons_include_added_changed_and_removed_inputs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::write(root.join("main.tex"), b"first").expect("source");
        let first = ProjectSnapshot::capture(root, &manifest()).expect("first");
        fs::write(root.join("main.tex"), b"second").expect("changed source");
        fs::write(root.join("figure.dat"), b"figure").expect("new input");
        let second = ProjectSnapshot::capture(root, &manifest()).expect("second");
        let changes = first.changes_since(&second);
        assert!(changes.contains(&PathBuf::from("main.tex")));
        assert!(changes.contains(&PathBuf::from("figure.dat")));

        fs::remove_file(root.join("figure.dat")).expect("remove input");
        let third = ProjectSnapshot::capture(root, &manifest()).expect("third");
        assert_eq!(
            second.changes_since(&third),
            vec![PathBuf::from("figure.dat")]
        );
    }
}
