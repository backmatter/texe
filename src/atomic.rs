//! Crash-safe file replacement, shared by every writer in the crate.

use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::TexeError;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Replace `path` with `bytes`, leaving either the old or the new contents
/// behind but never a partial file.
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), TexeError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| TexeError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        TexeError::Build(format!(
            "cannot write a file without a UTF-8 name: {}",
            path.display()
        ))
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{name}.texe-{}.{}.{}.tmp",
        std::process::id(),
        nonce,
        sequence
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|source| TexeError::Io {
                path: temporary.clone(),
                source,
            })?;
        file.write_all(bytes).map_err(|source| TexeError::Io {
            path: temporary.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| TexeError::Io {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, path).map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() && temporary.is_file() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Replace related files, restoring completed writes if a later write fails.
/// Each replacement is atomic; a process crash can still interrupt the group.
pub(crate) fn replace_files(files: &[(&Path, Option<&[u8]>)]) -> Result<(), TexeError> {
    replace_files_with(files, replace_contents)
}

fn replace_contents(path: &Path, bytes: Option<&[u8]>) -> Result<(), TexeError> {
    if let Some(bytes) = bytes {
        write(path, bytes)
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(TexeError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

fn replace_files_with(
    files: &[(&Path, Option<&[u8]>)],
    mut replace: impl FnMut(&Path, Option<&[u8]>) -> Result<(), TexeError>,
) -> Result<(), TexeError> {
    // Read every previous file before changing any destination.
    let previous = files
        .iter()
        .map(|(path, _)| match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(TexeError::Io {
                path: (*path).to_path_buf(),
                source,
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, (path, bytes)) in files.iter().enumerate() {
        if let Err(error) = replace(path, *bytes) {
            let mut rollback_errors = Vec::new();
            for ((restored, _), old) in files[..index].iter().zip(&previous[..index]).rev() {
                if let Err(rollback) = replace_contents(restored, old.as_deref()) {
                    rollback_errors.push(rollback.to_string());
                }
            }
            return if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(TexeError::Build(format!(
                    "{error}; could not restore previous outputs: {}",
                    rollback_errors.join("; ")
                )))
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use crate::atomic::write;

    #[test]
    fn failed_publication_restores_replaced_deleted_and_new_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let lock = root.join("texe.lock");
        let sync = root.join("main.synctex.gz");
        let new = root.join("new-file");
        let pdf = root.join("main.pdf");
        fs::write(&lock, b"old lock").expect("lock");
        fs::write(&sync, b"old sync").expect("sync");
        fs::write(&pdf, b"old PDF").expect("PDF");
        let result = super::replace_files_with(
            &[
                (&lock, Some(b"new lock")),
                (&sync, None),
                (&new, Some(b"new")),
                (&pdf, Some(b"new PDF")),
            ],
            |path, bytes| {
                if path == pdf {
                    Err(crate::TexeError::Io {
                        path: path.to_path_buf(),
                        source: std::io::Error::other("injected write failure"),
                    })
                } else {
                    super::replace_contents(path, bytes)
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(lock).expect("lock"), b"old lock");
        assert_eq!(fs::read(sync).expect("sync"), b"old sync");
        assert_eq!(fs::read(pdf).expect("PDF"), b"old PDF");
        assert!(!new.exists());
        assert_eq!(fs::read_dir(root).expect("directory").count(), 3);
    }

    #[test]
    fn replacement_leaves_no_temporary_behind() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("state.json");
        write(&path, b"first").expect("initial write");
        write(&path, b"second").expect("replacing write");
        assert_eq!(fs::read(&path).expect("contents"), b"second");
        let leftovers = fs::read_dir(directory.path())
            .expect("directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != OsStr::new("state.json"))
            .count();
        assert_eq!(leftovers, 0);
    }
}
