//! Polling project watcher used by `texe watch`.
//!
//! Polling is deliberately small and dependency-free. A snapshot is metadata
//! only, excludes the manifest-declared derived trees, and is taken again after
//! every build so texe's own lock/artifact writes do not trigger a loop.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
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
        let excluded = [
            manifest.project.build_dir.as_path(),
            manifest.packages.texmf.as_path(),
            manifest.packages.lock.as_path(),
        ];
        let published_stem = manifest.project.entry.file_stem();
        let mut files = BTreeMap::new();
        collect(
            project_root,
            project_root,
            &excluded,
            published_stem,
            &mut files,
        )?;
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

fn collect(
    project_root: &Path,
    directory: &Path,
    excluded: &[&Path],
    published_stem: Option<&std::ffi::OsStr>,
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
        if ignored(relative, excluded, published_stem) {
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
            collect(project_root, &path, excluded, published_stem, files)?;
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

fn ignored(relative: &Path, excluded: &[&Path], published_stem: Option<&std::ffi::OsStr>) -> bool {
    if relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
        || excluded
            .iter()
            .any(|excluded| relative == *excluded || relative.starts_with(excluded))
    {
        return true;
    }
    let Some(stem) = published_stem else {
        return false;
    };
    relative
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
        && relative.file_stem() == Some(stem)
        && matches!(
            relative.extension().and_then(std::ffi::OsStr::to_str),
            Some("pdf" | "dvi" | "gz")
        )
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
