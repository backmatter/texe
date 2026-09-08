//! Polling project watcher used by `texe watch`.
//!
//! Polling is deliberately small and dependency-free. A snapshot is metadata
//! only and excludes derived state. Baselines are captured before builds so
//! saves during compilation remain pending for the next build.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

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

/// Track a continuous quiet period, restarting it whenever inputs change.
pub(crate) struct PendingChanges {
    baseline: ProjectSnapshot,
    latest: ProjectSnapshot,
    changed_at: Option<Instant>,
}

impl PendingChanges {
    pub(crate) fn new(snapshot: ProjectSnapshot) -> Self {
        Self {
            baseline: snapshot.clone(),
            latest: snapshot,
            changed_at: None,
        }
    }

    pub(crate) fn observe(
        &mut self,
        snapshot: ProjectSnapshot,
        now: Instant,
        quiet_period: Duration,
    ) -> Option<Vec<PathBuf>> {
        if snapshot != self.latest {
            self.changed_at = Some(now);
            self.latest = snapshot;
        }
        if self.latest == self.baseline {
            self.changed_at = None;
        }
        self.changed_at
            .filter(|changed_at| now.duration_since(*changed_at) >= quiet_period)
            .map(|_| self.baseline.changes_since(&self.latest))
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
        use super::PendingChanges;
        use std::time::{Duration, Instant};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("main.tex"), "before build").unwrap();
        let mut changes = PendingChanges::new(ProjectSnapshot::capture(root, &manifest()).unwrap());
        fs::create_dir_all(root.join(".texe/editor")).unwrap();
        fs::write(root.join("texe.lock"), "new lock").unwrap();
        fs::write(root.join("main.pdf"), "published PDF").unwrap();
        fs::write(root.join(".texe/editor/build-bridge.json"), "bridge").unwrap();
        let now = Instant::now();
        let quiet = Duration::from_millis(250);
        assert!(
            changes
                .observe(
                    ProjectSnapshot::capture(root, &manifest()).unwrap(),
                    now,
                    quiet
                )
                .is_none()
        );
        fs::write(root.join("main.tex"), "saved during compilation").unwrap();
        let saved = ProjectSnapshot::capture(root, &manifest()).unwrap();
        assert!(changes.observe(saved.clone(), now, quiet).is_none());
        assert_eq!(
            changes.observe(saved, now + quiet, quiet),
            Some(vec![PathBuf::from("main.tex")])
        );
    }

    #[test]
    fn continuous_edits_restart_the_quiet_period_without_a_retry_limit() {
        use super::PendingChanges;
        use std::time::{Duration, Instant};

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::write(root.join("main.tex"), b"source").expect("source");
        let baseline = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        let mut changes = PendingChanges::new(baseline.clone());
        let start = Instant::now();
        let quiet = Duration::from_millis(250);
        assert!(changes.observe(baseline.clone(), start, quiet).is_none());
        let mut latest = baseline;
        for tick in 1..=12 {
            fs::write(root.join("main.tex"), "x".repeat(tick)).expect("edit");
            latest = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
            assert!(
                changes
                    .observe(
                        latest.clone(),
                        start + Duration::from_millis(tick as u64 * 100),
                        quiet
                    )
                    .is_none()
            );
        }
        assert!(
            changes
                .observe(latest.clone(), start + Duration::from_millis(1449), quiet)
                .is_none()
        );
        assert_eq!(
            changes.observe(latest, start + Duration::from_millis(1450), quiet),
            Some(vec![PathBuf::from("main.tex")])
        );
    }

    #[test]
    fn reverted_changes_cancel_a_pending_rebuild() {
        use super::PendingChanges;
        use std::time::{Duration, Instant};

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let baseline = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        let mut changes = PendingChanges::new(baseline.clone());
        fs::write(root.join("temporary.tex"), b"source").expect("temporary file");
        let edited = ProjectSnapshot::capture(root, &manifest()).expect("snapshot");
        let start = Instant::now();
        let quiet = Duration::from_millis(250);
        assert!(changes.observe(edited, start, quiet).is_none());
        assert!(changes.observe(baseline, start + quiet, quiet).is_none());
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
