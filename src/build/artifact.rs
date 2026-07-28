use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::lockfile::read_source_date_epoch;

/// The build timestamp this run hands to the engine, and the one its lock
/// should keep afterwards. They differ exactly when a caller overrides the
/// project's pinned value for one build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BuildTimestamp {
    /// Passed to the engine as `SOURCE_DATE_EPOCH`, and part of the build
    /// fingerprint so an override still invalidates the no-op fast path.
    pub(super) effective: u64,
    /// Written back to `texe.lock`. A project pins this once, when it first
    /// locks, and keeps it from then on.
    pub(super) locked: u64,
}

/// Chooses both timestamps. An inherited `SOURCE_DATE_EPOCH` wins for this
/// build so a caller can pin what the engine renders; otherwise the project
/// reuses the timestamp already recorded in its lock, which is what makes a
/// committed lock reproduce the same PDF on another machine.
///
/// The override deliberately does not reach `locked`. Build environments that
/// export `SOURCE_DATE_EPOCH` globally — Nix, Guix, and reproducible-build CI —
/// would otherwise silently rewrite a committed lock, permanently changing what
/// `\today` renders for every later build on every machine.
pub(super) fn resolve_build_timestamp(project_root: &Path) -> BuildTimestamp {
    build_timestamp(
        read_source_date_epoch(project_root),
        inherited_source_date_epoch(),
        current_unix_time,
    )
}

pub(super) fn build_timestamp(
    pinned: Option<u64>,
    inherited: Option<u64>,
    now: impl FnOnce() -> u64,
) -> BuildTimestamp {
    let effective = inherited.or(pinned).unwrap_or_else(now);
    BuildTimestamp {
        effective,
        // An unlocked project pins whatever this first build used; a locked one
        // keeps what it already pinned.
        locked: pinned.unwrap_or(effective),
    }
}

fn inherited_source_date_epoch() -> Option<u64> {
    std::env::var_os("SOURCE_DATE_EPOCH")
        .as_deref()
        .and_then(OsStr::to_str)
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn current_unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs())
}

/// The files a successful build leaves in the project root.
#[derive(Debug, Clone)]
pub(super) struct PublishedBuild {
    pub(super) artifact: PathBuf,
    pub(super) synctex: Option<PathBuf>,
}

impl PublishedBuild {
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.artifact.clone()];
        paths.extend(self.synctex.clone());
        paths
    }
}

pub(super) fn publish_artifact(
    project_root: &Path,
    internal: &Path,
) -> Result<PublishedBuild, TexeError> {
    let name = internal.file_name().ok_or_else(|| {
        TexeError::Build(format!(
            "built artifact has no filename: {}",
            internal.display()
        ))
    })?;
    let published = project_root.join(name);
    let bytes = fs::read(internal).map_err(|source| TexeError::Io {
        path: internal.to_path_buf(),
        source,
    })?;
    atomic_write(&published, &bytes)?;
    fs::remove_file(internal).map_err(|source| TexeError::Io {
        path: internal.to_path_buf(),
        source,
    })?;
    let synctex = internal.with_extension("synctex.gz");
    let published_synctex = if synctex.is_file() {
        let published_synctex = project_root.join(synctex.file_name().ok_or_else(|| {
            TexeError::Build(format!(
                "SyncTeX artifact has no filename: {}",
                synctex.display()
            ))
        })?);
        let bytes = fs::read(&synctex).map_err(|source| TexeError::Io {
            path: synctex.clone(),
            source,
        })?;
        atomic_write(&published_synctex, &bytes)?;
        fs::remove_file(&synctex).map_err(|source| TexeError::Io {
            path: synctex,
            source,
        })?;
        Some(published_synctex)
    } else {
        None
    };
    Ok(PublishedBuild {
        artifact: published,
        synctex: published_synctex,
    })
}
