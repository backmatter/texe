//! The exclusive lock a build holds on its build directory.
//!
//! An editor that builds on save and a person who runs `texe build` in a
//! terminal share one output directory, one auxiliary file set, and one
//! published PDF. Two engines writing them at once produce neither project's
//! output, and the loser can leave auxiliary state that costs the next build a
//! pass to repair. The lock makes the second caller wait for the first.

use std::fs;
use std::path::Path;

use crate::TexeError;

pub const LOCK_NAME: &str = ".texe-build.lock";

/// An exclusive lock on one project's build directory, released when dropped
/// and, because the operating system owns it, also when the process dies.
#[derive(Debug)]
pub struct BuildGuard {
    file: fs::File,
}

impl BuildGuard {
    /// Take the lock for `build_root`, waiting for whoever already holds it.
    ///
    /// # Errors
    ///
    /// Returns an error when the lock file cannot be created or locked.
    pub fn acquire(build_root: &Path) -> Result<Self, TexeError> {
        fs::create_dir_all(build_root).map_err(|source| TexeError::Io {
            path: build_root.to_path_buf(),
            source,
        })?;
        let path = build_root.join(LOCK_NAME);
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => return Ok(Self { file }),
            Err(fs::TryLockError::WouldBlock) => {}
            Err(fs::TryLockError::Error(source)) => {
                return Err(TexeError::Io { path, source });
            }
        }
        eprintln!("texe: waiting for another build in this project to finish");
        file.lock().map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self { file })
    }
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        // Releasing is best effort: the operating system drops the lock with
        // the file descriptor either way.
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::guard::{BuildGuard, LOCK_NAME};

    #[test]
    fn a_second_caller_cannot_take_a_held_build_lock() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let build_root = directory.path().join(".texe/build");
        let held = BuildGuard::acquire(&build_root).expect("first guard");
        let lock_path = build_root.join(LOCK_NAME);
        assert!(lock_path.is_file());

        let contended = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open lock file");
        assert!(
            matches!(contended.try_lock(), Err(fs::TryLockError::WouldBlock)),
            "a held build lock must not be acquirable"
        );

        drop(held);
        assert!(
            contended.try_lock().is_ok(),
            "a released build lock must be acquirable"
        );
    }
}
