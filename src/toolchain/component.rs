use std::ffi::OsStr;
use std::fs;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::io::Read as _;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::toolchain::artifact::{
    download_artifact, extract_archive, remove_component_staging, write_new_file,
};
use crate::toolchain::catalog;
use crate::toolchain::managed::{deep_verification_due, record_verification};
use crate::toolchain::{
    COMPONENT_MARKER, ComponentMarker, ManagedBiber, ManagedRuntime, READY_MARKER,
    RuntimeFileIdentity, ToolchainArtifactIdentity, VERIFIED_MARKER, VerificationPolicy,
    biber_component_name, texe_data_home,
};

pub fn ensure_bundled_biber(
    verification: VerificationPolicy,
    offline: bool,
) -> Result<ManagedBiber, TexeError> {
    let home = texe_data_home()?;
    let selection = catalog::select_platform("stable")?;
    ensure_biber_component(
        &home.join("components"),
        &home.join("downloads"),
        selection,
        verification,
        offline,
    )
}

pub fn ensure_managed_biber(runtime: &ManagedRuntime) -> Result<ManagedBiber, TexeError> {
    let selection = catalog::select_platform(&runtime.snapshot)?;
    ensure_biber_component(
        &runtime.component_cache,
        &runtime.downloads,
        selection,
        runtime.verification,
        runtime.offline,
    )
}

fn ensure_biber_component(
    component_cache: &Path,
    downloads: &Path,
    selection: catalog::PlatformSelection<'_>,
    verification: VerificationPolicy,
    offline: bool,
) -> Result<ManagedBiber, TexeError> {
    let artifact = &selection.platform.biber;
    let short_digest = &artifact.sha512[..16];
    let target = component_cache.join(biber_component_name(selection));
    let executable = target
        .join("bin")
        .join(selection.target)
        .join(format!("biber{}", selection.platform.executable_suffix));
    let library_dir = target.join("lib");
    let cache_dir = component_cache
        .join(".par-cache")
        .join(biber_component_name(selection));
    let expected_artifacts = biber_artifact_identities(selection);
    if target.join(COMPONENT_MARKER).is_file() {
        verify_managed_component(&target, &expected_artifacts, verification)?;
        return Ok(ManagedBiber {
            executable,
            library_dir,
            cache_dir,
        });
    }
    if target.exists() {
        return Err(TexeError::Toolchain(format!(
            "managed Biber component exists without a ready marker: {}; remove that exact \
             directory and retry",
            target.display()
        )));
    }

    fs::create_dir_all(downloads).map_err(|source| TexeError::Io {
        path: downloads.to_path_buf(),
        source,
    })?;
    fs::create_dir_all(component_cache).map_err(|source| TexeError::Io {
        path: component_cache.to_path_buf(),
        source,
    })?;
    let staging = component_cache.join(format!(".biber-{short_digest}.{}.tmp", std::process::id()));
    if staging.exists() {
        remove_component_staging(&staging)?;
    }
    fs::create_dir(&staging).map_err(|source| TexeError::Io {
        path: staging.clone(),
        source,
    })?;

    let result = (|| {
        let biber_archive =
            download_artifact(downloads, artifact, &selection.snapshot.sources, offline)?;
        extract_archive(&biber_archive, &staging)?;
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        if let Some(entry) = &selection.platform.biber_compatibility_library_entry {
            extract_biber_compatibility_library(
                &staging.join("bin").join(selection.target).join("biber"),
                &staging.join("lib"),
                entry,
            )?;
        }
        let marker = ComponentMarker {
            schema: "texe.component/v1".to_string(),
            artifacts: expected_artifacts.clone(),
            files: runtime_file_identities(&staging)?,
        };
        let bytes = serde_json::to_vec_pretty(&marker).map_err(|source| TexeError::Json {
            path: staging.join(COMPONENT_MARKER),
            source,
        })?;
        write_new_file(&staging.join(COMPONENT_MARKER), &bytes, None)?;
        match fs::rename(&staging, &target) {
            Ok(()) => Ok(()),
            Err(_) if target.join(COMPONENT_MARKER).is_file() => {
                remove_component_staging(&staging)?;
                verify_managed_component(&target, &expected_artifacts, VerificationPolicy::Deep)
            }
            Err(source) => Err(TexeError::Io {
                path: target.clone(),
                source,
            }),
        }
    })();
    if result.is_err() && staging.exists() {
        let _ = remove_component_staging(&staging);
    }
    result?;
    record_verification(&target, &artifact.sha512);
    Ok(ManagedBiber {
        executable,
        library_dir,
        cache_dir,
    })
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(super) fn extract_biber_compatibility_library(
    executable: &Path,
    destination: &Path,
    entry: &str,
) -> Result<(), TexeError> {
    let file = fs::File::open(executable).map_err(|source| TexeError::Io {
        path: executable.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        TexeError::Toolchain(format!(
            "managed Biber component is not a readable PAR archive: {error}"
        ))
    })?;
    let mut library = archive.by_name(entry).map_err(|error| {
        TexeError::Toolchain(format!(
            "managed Biber component is missing {entry}: {error}"
        ))
    })?;
    if library.size() > 1024 * 1024 {
        return Err(TexeError::Toolchain(format!(
            "managed Biber compatibility library is unexpectedly large: {} bytes",
            library.size()
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(library.size()).unwrap_or_default());
    library
        .read_to_end(&mut bytes)
        .map_err(|source| TexeError::Io {
            path: executable.to_path_buf(),
            source,
        })?;
    fs::create_dir_all(destination).map_err(|source| TexeError::Io {
        path: destination.to_path_buf(),
        source,
    })?;
    write_new_file(&destination.join("libcrypt.so.1"), &bytes, Some(0o644))
}

pub(super) fn biber_artifact_identities(
    selection: catalog::PlatformSelection<'_>,
) -> Vec<ToolchainArtifactIdentity> {
    let artifact = &selection.platform.biber;
    vec![ToolchainArtifactIdentity {
        provider: artifact.provider.clone(),
        sha512: format!("sha512:{}", artifact.sha512),
    }]
}

pub(super) fn verify_managed_component(
    root: &Path,
    expected_artifacts: &[ToolchainArtifactIdentity],
    verification: VerificationPolicy,
) -> Result<(), TexeError> {
    let marker_path = root.join(COMPONENT_MARKER);
    let bytes = fs::read(&marker_path).map_err(|source| TexeError::Io {
        path: marker_path.clone(),
        source,
    })?;
    let marker: ComponentMarker =
        serde_json::from_slice(&bytes).map_err(|source| TexeError::Json {
            path: marker_path,
            source,
        })?;
    if marker.schema != "texe.component/v1" || marker.artifacts != expected_artifacts {
        return Err(TexeError::Toolchain(format!(
            "managed component identity does not match its pinned recipe: {}",
            root.display()
        )));
    }
    let artifact_digest = expected_artifacts
        .first()
        .and_then(|artifact| artifact.sha512.strip_prefix("sha512:"))
        .ok_or_else(|| {
            TexeError::Toolchain("managed component recipe has no SHA-512 identity".to_string())
        })?;
    if !deep_verification_due(root, artifact_digest, verification) {
        return Ok(());
    }
    if runtime_file_identities(root)? != marker.files {
        return Err(TexeError::Toolchain(format!(
            "managed component failed installed-file verification: {}",
            root.display()
        )));
    }
    record_verification(root, artifact_digest);
    Ok(())
}

pub(super) fn runtime_file_identities(root: &Path) -> Result<Vec<RuntimeFileIdentity>, TexeError> {
    let mut files = Vec::new();
    collect_runtime_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_runtime_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<RuntimeFileIdentity>,
) -> Result<(), TexeError> {
    let entries = fs::read_dir(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| TexeError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() {
            return Err(TexeError::Toolchain(format!(
                "managed runtime contains an unexpected symlink: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_runtime_files(root, &path, files)?;
        } else if file_type.is_file() {
            if matches!(
                path.file_name().and_then(OsStr::to_str),
                Some(READY_MARKER | COMPONENT_MARKER | VERIFIED_MARKER)
            ) {
                continue;
            }
            let relative = path.strip_prefix(root).map_err(|_| {
                TexeError::Toolchain(format!(
                    "managed runtime file escaped its root: {}",
                    path.display()
                ))
            })?;
            let portable = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = fs::read(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            files.push(RuntimeFileIdentity {
                path: portable,
                sha256: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
            });
        } else {
            return Err(TexeError::Toolchain(format!(
                "managed runtime contains an unsupported filesystem entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}
