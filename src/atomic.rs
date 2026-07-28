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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use crate::atomic::write;

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
