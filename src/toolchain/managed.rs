use std::fs;
use std::path::Path;

use crate::TexeError;
use crate::atomic;
use crate::toolchain::artifact::{
    download_artifact, extract_archive, remove_staging, write_new_file,
};
use crate::toolchain::catalog::ManagedArtifact;
use crate::toolchain::component::runtime_file_identities;
use crate::toolchain::{
    READY_MARKER, RuntimeMarker, ToolchainIdentity, VERIFICATION_INTERVAL_SECONDS,
    VERIFICATION_SCHEMA, VERIFIED_MARKER, VerificationPolicy, VerificationStamp,
};

pub(super) fn install_managed_runtime(
    home: &Path,
    root: &Path,
    identity: &ToolchainIdentity,
    artifacts: &[ManagedArtifact],
    sources: &[String],
    verification: VerificationPolicy,
    offline: bool,
) -> Result<(), TexeError> {
    if root.join(READY_MARKER).is_file() {
        return verify_installed_runtime(root, identity, verification);
    }
    if root.exists() {
        return Err(TexeError::Toolchain(format!(
            "managed runtime exists without a ready marker: {}; remove that exact directory and \
             retry",
            root.display()
        )));
    }

    let downloads = home.join("downloads");
    let parent = root.parent().ok_or_else(|| {
        TexeError::Toolchain(format!(
            "managed runtime path has no parent: {}",
            root.display()
        ))
    })?;
    fs::create_dir_all(&downloads).map_err(|source| TexeError::Io {
        path: downloads.clone(),
        source,
    })?;
    fs::create_dir_all(parent).map_err(|source| TexeError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let staging = parent.join(format!(
        ".runtime-{}.{}.tmp",
        identity.fingerprint,
        std::process::id()
    ));
    if staging.exists() {
        remove_staging(&staging)?;
    }
    fs::create_dir(&staging).map_err(|source| TexeError::Io {
        path: staging.clone(),
        source,
    })?;

    let result = (|| {
        for artifact in artifacts {
            let archive = download_artifact(&downloads, artifact, sources, offline)?;
            extract_archive(&archive, &staging)?;
        }
        let marker = RuntimeMarker {
            schema: "texe.runtime/v1".to_string(),
            identity: identity.clone(),
            files: runtime_file_identities(&staging)?,
        };
        let marker = serde_json::to_vec_pretty(&marker).map_err(|source| TexeError::Json {
            path: staging.join(READY_MARKER),
            source,
        })?;
        write_new_file(&staging.join(READY_MARKER), &marker, None)?;
        match fs::rename(&staging, root) {
            Ok(()) => Ok(()),
            Err(_) if root.join(READY_MARKER).is_file() => {
                remove_staging(&staging)?;
                Ok(())
            }
            Err(source) => Err(TexeError::Io {
                path: root.to_path_buf(),
                source,
            }),
        }
    })();
    if result.is_err() && staging.exists() {
        let _ = remove_staging(&staging);
    }
    result?;
    // The marker was built by hashing every extracted file, so the runtime is
    // verified as of now.
    record_verification(root, &identity.fingerprint);
    Ok(())
}

pub(super) fn verify_installed_runtime(
    root: &Path,
    identity: &ToolchainIdentity,
    verification: VerificationPolicy,
) -> Result<(), TexeError> {
    let marker_path = root.join(READY_MARKER);
    let bytes = fs::read(&marker_path).map_err(|source| TexeError::Io {
        path: marker_path.clone(),
        source,
    })?;
    let marker: RuntimeMarker =
        serde_json::from_slice(&bytes).map_err(|source| TexeError::Json {
            path: marker_path,
            source,
        })?;
    if marker.schema != "texe.runtime/v1" || marker.identity != *identity {
        return Err(TexeError::Toolchain(format!(
            "managed runtime identity does not match its pinned recipe: {}",
            root.display()
        )));
    }
    if !deep_verification_due(root, &identity.fingerprint, verification) {
        return Ok(());
    }
    let actual = runtime_file_identities(root)?;
    if actual != marker.files {
        return Err(TexeError::Toolchain(format!(
            "managed runtime failed installed-file verification: {}",
            root.display()
        )));
    }
    record_verification(root, &identity.fingerprint);
    Ok(())
}

/// Decide whether an already-identified runtime or component is due for a full
/// rehash. Anything unknown — no stamp, a stamp from another recipe, an
/// unreadable or backwards clock — reads as due, so the cheap path is only
/// taken on evidence that the deep check ran recently.
pub(super) fn deep_verification_due(
    root: &Path,
    fingerprint: &str,
    verification: VerificationPolicy,
) -> bool {
    if verification == VerificationPolicy::Deep {
        return true;
    }
    let Some(stamp) = read_verification_stamp(root) else {
        return true;
    };
    if stamp.schema != VERIFICATION_SCHEMA || stamp.fingerprint != fingerprint {
        return true;
    }
    let Some(now) = unix_time() else {
        return true;
    };
    now < stamp.verified_at || now - stamp.verified_at >= VERIFICATION_INTERVAL_SECONDS
}

fn read_verification_stamp(root: &Path) -> Option<VerificationStamp> {
    let bytes = fs::read(root.join(VERIFIED_MARKER)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Record that every installed file below `root` hashed as expected. Best
/// effort: a runtime directory that cannot be stamped — a read-only cache, a
/// stopped clock — simply gets the deep check again on the next resolve.
pub(super) fn record_verification(root: &Path, fingerprint: &str) {
    let Some(verified_at) = unix_time() else {
        return;
    };
    let stamp = VerificationStamp {
        schema: VERIFICATION_SCHEMA.to_string(),
        fingerprint: fingerprint.to_string(),
        verified_at,
    };
    if let Ok(mut bytes) = serde_json::to_vec_pretty(&stamp) {
        bytes.push(b'\n');
        let _ = atomic::write(&root.join(VERIFIED_MARKER), &bytes);
    }
}

pub(super) fn unix_time() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|since_epoch| since_epoch.as_secs())
}
