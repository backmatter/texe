use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::toolchain::ToolchainIdentity;

pub const LOCK_SCHEMA: &str = "texe.lock/v1";
pub const LOCK_NAME: &str = "texe.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectLock {
    schema: String,
    generated_with: String,
    /// Build timestamp handed to the engine so date-dependent output remains
    /// stable across locked builds.
    source_date_epoch: u64,
    toolchain: ToolchainIdentity,
    packages: serde_json::Value,
}

/// Reads the pinned build timestamp from an existing project lock. Any
/// unreadable or malformed lock yields `None`; `restore_package_lock` reports
/// those failures with their real cause.
pub fn read_source_date_epoch(project_root: &Path) -> Option<u64> {
    let bytes = fs::read(project_root.join(LOCK_NAME)).ok()?;
    let lock: ProjectLock = serde_json::from_slice(&bytes).ok()?;
    Some(lock.source_date_epoch)
}

pub fn restore_package_lock(
    project_root: &Path,
    package_lock: &Path,
    toolchain: &ToolchainIdentity,
    frozen: bool,
) -> Result<(), TexeError> {
    let path = project_root.join(LOCK_NAME);
    if !path.is_file() {
        if frozen {
            return Err(TexeError::Build(format!(
                "--frozen requires an existing {}",
                path.display()
            )));
        }
        return Ok(());
    }
    let bytes = fs::read(&path).map_err(|source| TexeError::Io {
        path: path.clone(),
        source,
    })?;
    let lock: ProjectLock = serde_json::from_slice(&bytes).map_err(|source| TexeError::Json {
        path: path.clone(),
        source,
    })?;
    if lock.schema != LOCK_SCHEMA {
        return Err(TexeError::Build(format!(
            "unsupported lock schema {}; expected {LOCK_SCHEMA}",
            lock.schema
        )));
    }
    if lock.toolchain != *toolchain {
        if frozen {
            return Err(TexeError::Build(
                "texe.lock does not match the resolved toolchain; run `texe build` to update it"
                    .to_string(),
            ));
        }
        if package_lock.is_file() {
            fs::remove_file(package_lock).map_err(|source| TexeError::Io {
                path: package_lock.to_path_buf(),
                source,
            })?;
        }
        return Ok(());
    }
    validate_package_lock(&lock.packages)?;
    let mut package_bytes =
        serde_json::to_vec_pretty(&lock.packages).map_err(|source| TexeError::Json {
            path: package_lock.to_path_buf(),
            source,
        })?;
    package_bytes.push(b'\n');
    atomic_write(package_lock, &package_bytes)
}

pub fn write_project_lock(
    project_root: &Path,
    package_lock: &Path,
    toolchain: &ToolchainIdentity,
    source_date_epoch: u64,
) -> Result<PathBuf, TexeError> {
    let package_bytes = fs::read(package_lock).map_err(|source| TexeError::Io {
        path: package_lock.to_path_buf(),
        source,
    })?;
    let packages: serde_json::Value =
        serde_json::from_slice(&package_bytes).map_err(|source| TexeError::Json {
            path: package_lock.to_path_buf(),
            source,
        })?;
    validate_package_lock(&packages)?;
    let lock = ProjectLock {
        schema: LOCK_SCHEMA.to_string(),
        generated_with: format!("texe/{}", env!("CARGO_PKG_VERSION")),
        source_date_epoch,
        toolchain: toolchain.clone(),
        packages,
    };
    let path = project_root.join(LOCK_NAME);
    let mut bytes = serde_json::to_vec_pretty(&lock).map_err(|source| TexeError::Json {
        path: path.clone(),
        source,
    })?;
    bytes.push(b'\n');
    atomic_write(&path, &bytes)?;
    Ok(path)
}

fn validate_package_lock(packages: &serde_json::Value) -> Result<(), TexeError> {
    if packages.get("schema").and_then(serde_json::Value::as_str) != Some("pqty.lock/v1") {
        return Err(TexeError::Build(
            "texe.lock packages must contain a pqty.lock/v1 object".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::lockfile::{
        LOCK_NAME, LOCK_SCHEMA, read_source_date_epoch, restore_package_lock, write_project_lock,
    };
    use crate::toolchain::ToolchainArtifactIdentity;
    use crate::toolchain::ToolchainIdentity;

    fn identity() -> ToolchainIdentity {
        ToolchainIdentity {
            provider: "managed".to_string(),
            engine: "pdflatex".to_string(),
            channel: "snapshot".to_string(),
            target: "test".to_string(),
            fingerprint: "abc".to_string(),
            registry_url: None,
            registry_metadata_digest: None,
            artifacts: vec![ToolchainArtifactIdentity {
                provider: "pdftex".to_string(),
                sha512: "sha512:abc".to_string(),
            }],
        }
    }

    #[test]
    fn composite_lock_restores_the_internal_package_lock() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let internal = directory.path().join(".texe/state/pqty.lock");
        let packages = serde_json::json!({"schema": "pqty.lock/v1", "closure": []});
        fs::create_dir_all(internal.parent().expect("parent")).expect("state directory");
        fs::write(
            &internal,
            serde_json::to_vec(&packages).expect("serialize packages"),
        )
        .expect("write internal lock");
        write_project_lock(directory.path(), &internal, &identity(), 1_700_000_000)
            .expect("write project lock");
        fs::remove_file(&internal).expect("remove internal lock");
        restore_package_lock(directory.path(), &internal, &identity(), true)
            .expect("restore internal lock");
        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&internal).expect("read restored internal lock"))
                .expect("parse restored internal lock");
        assert_eq!(restored, packages);
    }

    #[test]
    fn build_timestamp_survives_a_lock_rewrite() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let internal = directory.path().join(".texe/state/pqty.lock");
        fs::create_dir_all(internal.parent().expect("parent")).expect("state directory");
        fs::write(&internal, br#"{"schema": "pqty.lock/v1"}"#).expect("write internal lock");

        assert_eq!(read_source_date_epoch(directory.path()), None);
        write_project_lock(directory.path(), &internal, &identity(), 1_700_000_000)
            .expect("write project lock");

        assert_eq!(
            read_source_date_epoch(directory.path()),
            Some(1_700_000_000)
        );
    }

    #[test]
    fn locks_without_a_build_timestamp_are_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let internal = directory.path().join(".texe/state/pqty.lock");
        fs::create_dir_all(internal.parent().expect("parent")).expect("state directory");
        let lock = serde_json::json!({
            "schema": LOCK_SCHEMA,
            "generated_with": "texe/0.1.0",
            "toolchain": identity(),
            "packages": {"schema": "pqty.lock/v1"},
        });
        fs::write(
            directory.path().join(LOCK_NAME),
            serde_json::to_vec(&lock).expect("serialize lock"),
        )
        .expect("write incomplete lock");

        assert_eq!(read_source_date_epoch(directory.path()), None);
        let error = restore_package_lock(directory.path(), &internal, &identity(), true)
            .expect_err("incomplete lock is rejected");
        assert!(error.to_string().contains("source_date_epoch"));
    }
}
