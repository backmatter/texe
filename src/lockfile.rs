use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::TexeError;
#[cfg(test)]
use crate::atomic::write as atomic_write;
use crate::build::generated;
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
    runtimes: BTreeMap<String, ToolchainIdentity>,
    packages: serde_json::Value,
}

/// Reads the pinned build timestamp from an existing project lock. Any
/// unreadable or malformed lock yields `None`; `restore_package_lock` reports
/// those failures with their real cause.
pub fn read_source_date_epoch(project_root: &Path) -> Option<u64> {
    if let Some(environment) = crate::portable::active(project_root).ok().flatten() {
        return environment["source_date_epoch"].as_u64();
    }
    let bytes = generated::read(
        project_root,
        project_root,
        &project_root.join(LOCK_NAME),
        generated::MAX_METADATA,
    )
    .ok()?;
    let lock: ProjectLock = serde_json::from_slice(&bytes).ok()?;
    Some(lock.source_date_epoch)
}

fn target_key(target: &str) -> &str {
    match target {
        "x86_64-linux" => "linux-x64",
        "aarch64-linux" => "linux-arm64",
        "universal-darwin" | "aarch64-macos" => "macos-arm64",
        "x86_64-darwin" | "x86_64-macos" => "macos-x64",
        "windows" | "x86_64-windows" => "windows-x64",
        other => other,
    }
}

fn read_project_lock(
    project_root: &Path,
    toolchain: &ToolchainIdentity,
    frozen: bool,
) -> Result<Option<ProjectLock>, TexeError> {
    let path = project_root.join(LOCK_NAME);
    let bytes = generated::optional(project_root, project_root, &path, generated::MAX_METADATA)?;
    let Some(bytes) = bytes else {
        if frozen {
            return Err(TexeError::Build(format!(
                "--frozen requires an existing {}",
                path.display()
            )));
        }
        return Ok(None);
    };
    let lock: ProjectLock =
        serde_json::from_slice(&bytes).map_err(|source| TexeError::Json { path, source })?;
    if lock.schema != LOCK_SCHEMA {
        return Err(TexeError::Build(format!(
            "unsupported lock schema {}; expected {LOCK_SCHEMA}",
            lock.schema
        )));
    }
    validate_runtimes(&lock.runtimes, toolchain)?;
    let target = target_key(&toolchain.target);
    match lock.runtimes.get(target) {
        Some(locked) if locked != toolchain => {
            return Err(TexeError::Build(format!(
                "texe.lock runtime identity for {target} does not match the resolved toolchain"
            )));
        }
        None if frozen => {
            return Err(TexeError::Build(format!(
                "texe.lock has no runtime identity for {target}; run `texe build` on this target to add it without changing packages"
            )));
        }
        _ => {}
    }
    validate_package_lock(&lock.packages)?;
    Ok(Some(lock))
}

/// Validate before consulting cached artifacts, and keep a new target frozen
/// to the existing package selection throughout its first build.
pub(crate) fn adding_target(
    project_root: &Path,
    toolchain: &ToolchainIdentity,
    frozen: bool,
) -> Result<bool, TexeError> {
    Ok(read_project_lock(project_root, toolchain, frozen)?
        .is_some_and(|lock| !lock.runtimes.contains_key(target_key(&toolchain.target))))
}

pub fn restore_package_lock(
    project_root: &Path,
    package_lock: &Path,
    toolchain: &ToolchainIdentity,
    frozen: bool,
) -> Result<(), TexeError> {
    let Some(lock) = read_project_lock(project_root, toolchain, frozen)? else {
        return Ok(());
    };
    generated::validate_parent(project_root, package_lock)?;
    let mut package_bytes =
        serde_json::to_vec_pretty(&lock.packages).map_err(|source| TexeError::Json {
            path: package_lock.to_path_buf(),
            source,
        })?;
    package_bytes.push(b'\n');
    generated::write(project_root, package_lock, &package_bytes)
}

#[cfg(test)]
fn write_project_lock(
    project_root: &Path,
    package_lock: &Path,
    toolchain: &ToolchainIdentity,
    source_date_epoch: u64,
) -> Result<PathBuf, TexeError> {
    let path = project_root.join(LOCK_NAME);
    atomic_write(
        &path,
        &project_lock_bytes(project_root, package_lock, toolchain, source_date_epoch)?,
    )?;
    Ok(path)
}

pub(crate) fn project_lock_bytes(
    project_root: &Path,
    package_lock: &Path,
    toolchain: &ToolchainIdentity,
    source_date_epoch: u64,
) -> Result<Vec<u8>, TexeError> {
    let package_bytes = generated::read_local(package_lock, generated::MAX_METADATA)?;
    let packages: serde_json::Value =
        serde_json::from_slice(&package_bytes).map_err(|source| TexeError::Json {
            path: package_lock.to_path_buf(),
            source,
        })?;
    validate_package_lock(&packages)?;
    let mut runtimes = BTreeMap::new();
    if let Some(bytes) = generated::optional(
        project_root,
        project_root,
        &project_root.join(LOCK_NAME),
        generated::MAX_METADATA,
    )? {
        let prior: ProjectLock =
            serde_json::from_slice(&bytes).map_err(|source| TexeError::Json {
                path: project_root.join(LOCK_NAME),
                source,
            })?;
        validate_runtimes(&prior.runtimes, toolchain)?;
        // Runtime identities describe this exact closure. A changed closure must
        // be verified again on every target, rather than retaining stale claims.
        if prior.packages == packages {
            runtimes = prior.runtimes;
        } else if !prior.runtimes.contains_key(target_key(&toolchain.target)) {
            return Err(TexeError::Build(
                "adding a runtime target must not change the locked package closure".into(),
            ));
        }
    }
    runtimes.insert(target_key(&toolchain.target).into(), toolchain.clone());
    let lock = ProjectLock {
        schema: LOCK_SCHEMA.to_string(),
        generated_with: format!("texe/{}", env!("CARGO_PKG_VERSION")),
        source_date_epoch,
        runtimes,
        packages,
    };
    let mut bytes = serde_json::to_vec_pretty(&lock).map_err(|source| TexeError::Json {
        path: PathBuf::from(LOCK_NAME),
        source,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_runtimes(
    runtimes: &BTreeMap<String, ToolchainIdentity>,
    toolchain: &ToolchainIdentity,
) -> Result<(), TexeError> {
    if runtimes.is_empty()
        || runtimes
            .iter()
            .any(|(target, runtime)| target != target_key(&runtime.target))
    {
        return Err(TexeError::Build(
            "texe.lock has invalid runtime target identities".into(),
        ));
    }
    if runtimes.values().any(|runtime| {
        runtime.provider != toolchain.provider
            || runtime.engine != toolchain.engine
            || runtime.channel != toolchain.channel
            || runtime.registry_url != toolchain.registry_url
            || runtime.registry_metadata_digest != toolchain.registry_metadata_digest
    }) {
        return Err(TexeError::Build(
            "texe.lock runtimes must use the same engine and package snapshot on every target"
                .into(),
        ));
    }
    Ok(())
}

fn validate_package_lock(packages: &serde_json::Value) -> Result<(), TexeError> {
    if packages.get("schema").and_then(serde_json::Value::as_str) != Some("pqty.lock/v1")
        || packages.get("stage").and_then(serde_json::Value::as_str) != Some("exact")
    {
        return Err(TexeError::Build(
            "texe.lock packages must contain an exact pqty.lock/v1 object".to_string(),
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
        let packages =
            serde_json::json!({"schema": "pqty.lock/v1", "stage": "exact", "closure": []});
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
    fn adding_a_target_preserves_the_exact_package_closure_and_existing_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let internal = root.join("pqty.lock");
        let packages = serde_json::json!({
            "schema": "pqty.lock/v1", "stage": "exact",
            "closure": [{"name": "latex", "revision": 123, "sha512": "abc"}],
        });
        fs::write(&internal, serde_json::to_vec(&packages).unwrap()).unwrap();
        let mut linux = identity();
        linux.target = "linux-x64".into();
        write_project_lock(root, &internal, &linux, 42).unwrap();
        let mut macos = identity();
        macos.target = "macos-arm64".into();
        macos.fingerprint = "mac-runtime".into();
        macos.artifacts[0].sha512 = "sha512:mac".into();
        assert!(super::adding_target(root, &macos, false).unwrap());
        let error = restore_package_lock(root, &internal, &macos, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no runtime identity for macos-arm64")
        );
        restore_package_lock(root, &internal, &macos, false).unwrap();
        write_project_lock(root, &internal, &macos, 42).unwrap();
        let lock: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(LOCK_NAME)).unwrap()).unwrap();
        assert_eq!(lock["packages"], packages);
        assert_eq!(lock["source_date_epoch"], 42);
        assert_eq!(
            lock["runtimes"]["linux-x64"],
            serde_json::to_value(&linux).unwrap()
        );
        assert_eq!(
            lock["runtimes"]["macos-arm64"],
            serde_json::to_value(&macos).unwrap()
        );
        restore_package_lock(root, &internal, &linux, true).unwrap();
        restore_package_lock(root, &internal, &macos, true).unwrap();
        macos.fingerprint = "unverified".into();
        assert!(restore_package_lock(root, &internal, &macos, false).is_err());
    }

    #[test]
    fn tex_live_target_names_use_portable_runtime_keys() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let internal = root.join("pqty.lock");
        fs::write(&internal, br#"{"schema":"pqty.lock/v1","stage":"exact"}"#).unwrap();
        let mut linux = identity();
        linux.target = "x86_64-linux".into();
        write_project_lock(root, &internal, &linux, 42).unwrap();
        let lock: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(LOCK_NAME)).unwrap()).unwrap();
        assert!(lock["runtimes"].get("linux-x64").is_some());
        assert!(!super::adding_target(root, &linux, true).unwrap());
    }

    #[test]
    fn a_changed_closure_requires_other_targets_to_be_verified_again() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let internal = root.join("pqty.lock");
        fs::write(
            &internal,
            br#"{"schema":"pqty.lock/v1","stage":"exact","closure":[]}"#,
        )
        .unwrap();
        let linux = identity();
        write_project_lock(root, &internal, &linux, 42).unwrap();
        let mut macos = identity();
        macos.target = "macos-arm64".into();
        write_project_lock(root, &internal, &macos, 42).unwrap();
        fs::write(
            &internal,
            br#"{"schema":"pqty.lock/v1","stage":"exact","closure":["new-package"]}"#,
        )
        .unwrap();
        write_project_lock(root, &internal, &linux, 42).unwrap();
        restore_package_lock(root, &internal, &linux, true).unwrap();
        assert!(super::adding_target(root, &macos, false).unwrap());
        assert!(restore_package_lock(root, &internal, &macos, true).is_err());
    }

    #[test]
    fn a_new_target_cannot_rewrite_packages() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let internal = root.join("pqty.lock");
        fs::write(
            &internal,
            br#"{"schema":"pqty.lock/v1","stage":"exact","closure":[]}"#,
        )
        .unwrap();
        write_project_lock(root, &internal, &identity(), 42).unwrap();
        let original = fs::read(root.join(LOCK_NAME)).unwrap();
        fs::write(
            &internal,
            br#"{"schema":"pqty.lock/v1","stage":"exact","closure":["new-package"]}"#,
        )
        .unwrap();
        let mut other = identity();
        other.target = "another-target".into();
        assert!(write_project_lock(root, &internal, &other, 42).is_err());
        assert_eq!(fs::read(root.join(LOCK_NAME)).unwrap(), original);
    }

    #[test]
    fn build_timestamp_survives_a_lock_rewrite() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let internal = directory.path().join(".texe/state/pqty.lock");
        fs::create_dir_all(internal.parent().expect("parent")).expect("state directory");
        fs::write(
            &internal,
            br#"{"schema": "pqty.lock/v1", "stage": "exact"}"#,
        )
        .expect("write internal lock");

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
            "runtimes": {"test": identity()},
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

    #[cfg(unix)]
    #[test]
    fn generated_package_lock_aliases_cannot_enter_the_published_lock() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let original = br#"{"schema":"pqty.lock/v1","private":"research"}"#;
        let research = directory.path().join("research.json");
        let internal = directory.path().join("pqty.lock");
        fs::write(&research, original).unwrap();
        symlink(&research, &internal).unwrap();
        assert!(super::project_lock_bytes(directory.path(), &internal, &identity(), 1).is_err());
        fs::remove_file(&internal).unwrap();
        fs::hard_link(&research, &internal).unwrap();
        assert!(super::project_lock_bytes(directory.path(), &internal, &identity(), 1).is_err());
        assert_eq!(fs::read(&research).unwrap(), original);
        assert!(!directory.path().join(LOCK_NAME).exists());
    }

    #[test]
    fn oversized_generated_package_lock_is_rejected_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let internal = directory.path().join("pqty.lock");
        fs::File::create(&internal)
            .unwrap()
            .set_len(super::generated::MAX_METADATA + 1)
            .unwrap();
        assert!(super::project_lock_bytes(directory.path(), &internal, &identity(), 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn restoring_package_lock_cannot_write_through_a_parent_alias() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let research = root.join("research");
        fs::create_dir(&research).unwrap();
        fs::write(research.join("pqty.lock"), b"research source").unwrap();
        let valid = root.join("valid-pqty.lock");
        fs::write(&valid, br#"{"schema":"pqty.lock/v1","stage":"exact"}"#).unwrap();
        write_project_lock(root, &valid, &identity(), 1).unwrap();
        symlink(&research, root.join("derived")).unwrap();
        assert!(
            restore_package_lock(root, &root.join("derived/pqty.lock"), &identity(), true).is_err()
        );
        assert_eq!(
            fs::read(research.join("pqty.lock")).unwrap(),
            b"research source"
        );
    }
}
