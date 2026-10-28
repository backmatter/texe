//! The build fingerprint that lets an unchanged project skip a rebuild.
//!
//! `texe build` sits in an editing loop, where the common case is that nothing
//! has moved since the last successful build. The fingerprint covers every
//! input a rebuild would read — the effective manifest, the resolved toolchain
//! identity, the pinned build timestamp, the composite lock, and every project
//! file that lock names — so a match means the published artifact is already
//! the artifact a rebuild would produce.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::build::generated;
use crate::config::ProjectManifest;
use crate::lockfile::LOCK_NAME;
use crate::toolchain::ToolchainIdentity;

pub const STATE_SCHEMA: &str = "texe.build-state/v1";
pub const STATE_NAME: &str = "build-state.json";
const MAX_FINGERPRINT_FILES: usize = 8192;
const MAX_SOURCE_STAMPS: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildState {
    pub schema: String,
    pub generated_with: String,
    pub fingerprint: String,
    pub environment_fingerprint: String,
    /// Files the build published, project-relative, with the digests they had
    /// when it finished. A hand-edited or deleted artifact fails this check
    /// and forces a rebuild.
    pub outputs: Vec<OutputDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputDigest {
    pub path: PathBuf,
    pub digest: String,
}

/// Read a previously recorded build state. An absent, unreadable, or
/// unsupported state file simply means the fast path is unavailable.
pub fn read(path: &Path) -> Option<BuildState> {
    let bytes = generated::read_local(path, generated::MAX_METADATA).ok()?;
    let state: BuildState = serde_json::from_slice(&bytes).ok()?;
    (state.schema == STATE_SCHEMA && valid_output_layout(&state.outputs)).then_some(state)
}

pub fn write(
    path: &Path,
    fingerprint: &str,
    environment_fingerprint: &str,
    project_root: &Path,
    outputs: &[PathBuf],
) -> Result<(), TexeError> {
    let mut recorded_outputs = outputs
        .iter()
        .map(|output| OutputDigest {
            path: output
                .strip_prefix(project_root)
                .unwrap_or(output)
                .to_owned(),
            digest: String::new(),
        })
        .collect::<Vec<_>>();
    if !valid_output_layout(&recorded_outputs) {
        return Err(TexeError::Build(
            "Build state can only record one published PDF or DVI and its matching SyncTeX file"
                .into(),
        ));
    }
    for (recorded, output) in recorded_outputs.iter_mut().zip(outputs) {
        recorded.digest = digest(&generated::read(
            project_root,
            project_root,
            output,
            generated::MAX_ARTIFACT,
        )?);
    }
    let state = BuildState {
        schema: STATE_SCHEMA.to_string(),
        generated_with: format!("texe/{}", env!("CARGO_PKG_VERSION")),
        fingerprint: fingerprint.to_string(),
        environment_fingerprint: environment_fingerprint.to_string(),
        outputs: recorded_outputs,
    };
    let mut bytes = serde_json::to_vec_pretty(&state).map_err(|source| TexeError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    generated::write(project_root, path, &bytes)
}

/// Digest every input a rebuild would read. Returns `None` when the project has
/// no composite lock yet, which is exactly when there is nothing to reuse.
pub fn build_fingerprint(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ToolchainIdentity,
    command_suite_fingerprint: &str,
    source_date_epoch: u64,
) -> Result<Option<String>, TexeError> {
    let portable_environment = crate::portable::active(project_root)?;
    let lock_path = if let Some(environment) = &portable_environment {
        let authoritative = crate::portable::manifest(environment)?;
        if manifest.project.build_dir != authoritative.project.build_dir {
            return Err(TexeError::Build(
                "The selected project environment changed; retry the build".into(),
            ));
        }
        // Portable builds publish their composite lock beside private state;
        // the researcher's existing root lock belongs to the local project.
        project_root
            .join(&manifest.project.build_dir)
            .join(LOCK_NAME)
    } else if crate::build::process::protection_requested() {
        project_root
            .join(&manifest.project.build_dir)
            .join(LOCK_NAME)
    } else {
        project_root.join(LOCK_NAME)
    };
    let Ok(lock_bytes) = generated::read(
        project_root,
        project_root,
        &lock_path,
        generated::MAX_METADATA,
    ) else {
        return Ok(None);
    };
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&lock_bytes) else {
        return Ok(None);
    };
    let mut hasher = Sha256::new();
    hasher.update(STATE_SCHEMA.as_bytes());
    hasher.update([u8::from(
        portable_environment.is_some() || crate::build::process::protection_requested(),
    )]);
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(source_date_epoch.to_le_bytes());
    hasher.update(canonical_json(manifest, "texe.toml")?);
    hasher.update(canonical_json(toolchain, "toolchain identity")?);
    hasher.update(command_suite_fingerprint.as_bytes());
    hasher.update(&lock_bytes);
    let Some(mut paths) = locked_project_files(&lock) else {
        return Ok(None);
    };
    if portable_environment.is_some() || crate::build::process::protection_requested() {
        let Some((current_sources, aliases)) = portable_source_files(project_root, manifest)?
        else {
            return Ok(None);
        };
        paths.extend(current_sources);
        if paths.len() > MAX_FINGERPRINT_FILES {
            return Ok(None);
        }
        for (path, target) in aliases {
            hasher.update(b"source-alias\0");
            hasher.update(path.as_bytes());
            hasher.update(b"\0");
            hasher.update(target.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        }
    }
    let mut source_bytes = 0_u64;
    for path in paths {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        match read_project_file(project_root, &path)? {
            Some(bytes) => {
                source_bytes = source_bytes.saturating_add(bytes.len() as u64);
                if source_bytes > generated::MAX_ARTIFACT {
                    return Ok(None);
                }
                hasher.update(digest(&bytes));
            }
            None => hasher.update("absent"),
        }
        hasher.update(b"\0");
    }
    Ok(Some(digest_of(hasher)))
}

/// A bounded metadata snapshot of researcher inputs, independent of derived
/// locks and artifacts. It never reads large figures just to detect a save.
pub(crate) fn source_fingerprint(
    project_root: &Path,
    manifest: &ProjectManifest,
) -> Result<Option<String>, TexeError> {
    #[cfg(target_os = "linux")]
    return source_stability_inventory(project_root, manifest);
    #[cfg(not(target_os = "linux"))]
    source_fingerprint_portable(project_root, manifest)
}

/// Held directory descriptors prevent a concurrent directory replacement from
/// making this parent-side inventory follow links outside the saved inputs.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines)]
fn source_stability_inventory(
    project_root: &Path,
    manifest: &ProjectManifest,
) -> Result<Option<String>, TexeError> {
    use rustix::fs::{
        AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags, fstat, open, openat2, readlinkat,
        statat,
    };
    use std::os::unix::ffi::OsStrExt as _;
    fn failure(path: &Path, source: rustix::io::Errno) -> TexeError {
        TexeError::Io {
            path: path.into(),
            source: source.into(),
        }
    }
    fn stamp(stat: &rustix::fs::Stat, hasher: &mut Sha256) {
        hasher.update(stat.st_mode.to_le_bytes());
        hasher.update(stat.st_dev.to_le_bytes());
        hasher.update(stat.st_ino.to_le_bytes());
        hasher.update(stat.st_size.to_le_bytes());
        hasher.update(stat.st_mtime.to_le_bytes());
        hasher.update(stat.st_mtime_nsec.to_le_bytes());
        hasher.update(stat.st_ctime.to_le_bytes());
        hasher.update(stat.st_ctime_nsec.to_le_bytes());
    }
    let filesystem = open(
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| failure(project_root, source))?;
    let resolve = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
    let primary = PathBuf::from(
        manifest
            .project
            .entry
            .file_name()
            .ok_or_else(|| TexeError::Build("Invalid project entry".into()))?,
    )
    .with_extension("pdf");
    let derived = [
        PathBuf::from("texe.lock"),
        primary.clone(),
        primary.with_extension("dvi"),
        primary.with_extension("synctex.gz"),
    ];
    let mut roots = BTreeSet::from([project_root.to_owned()]);
    for source in std::iter::once(&manifest.project.entry)
        .chain(manifest.inputs.roots.iter())
        .chain(manifest.bibliography.roots.iter())
    {
        let path = project_root.join(source);
        let resolved = path
            .canonicalize()
            .map_err(|source| TexeError::Io { path, source })?;
        if !resolved.starts_with(project_root) {
            roots.insert(resolved);
        }
    }
    let mut hasher = Sha256::new();
    let mut count = 0;
    for root in roots {
        let own_project = root == project_root;
        let absolute = root
            .strip_prefix("/")
            .map_err(|_| TexeError::Build("Source root must be absolute".into()))?;
        let anchor = openat2(
            &filesystem,
            absolute,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            resolve,
        )
        .map_err(|source| failure(&root, source))?;
        let metadata = fstat(&anchor).map_err(|source| failure(&root, source))?;
        hasher.update(root.as_os_str().as_bytes());
        hasher.update(b"\0");
        if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
            if count >= MAX_SOURCE_STAMPS {
                return Ok(None);
            }
            stamp(&metadata, &mut hasher);
            count += 1;
            continue;
        }
        let mut pending = vec![PathBuf::from(".")];
        while let Some(relative) = pending.pop() {
            let path = root.join(&relative);
            let directory = openat2(
                &anchor,
                &relative,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                resolve,
            )
            .map_err(|source| failure(&path, source))?;
            let mut entries =
                Dir::read_from(&directory).map_err(|source| failure(&path, source))?;
            let mut names = Vec::new();
            for entry in &mut entries {
                let entry = entry.map_err(|source| failure(&path, source))?;
                let name = entry.file_name().to_bytes();
                if matches!(name, b"." | b"..") {
                    continue;
                }
                if count >= MAX_SOURCE_STAMPS {
                    return Ok(None);
                }
                count += 1;
                names.push(std::ffi::OsStr::from_bytes(name).to_owned());
            }
            names.sort();
            for name in names {
                let child = if relative == Path::new(".") {
                    PathBuf::from(&name)
                } else {
                    relative.join(&name)
                };
                if own_project
                    && (matches!(name.to_str(), Some(".texe" | ".citet" | ".git"))
                        || derived.contains(&child))
                {
                    continue;
                }
                let stat = statat(&directory, &name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|source| failure(&root.join(&child), source))?;
                if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                    pending.push(child);
                    continue;
                }
                hasher.update(child.as_os_str().as_bytes());
                hasher.update(b"\0");
                stamp(&stat, &mut hasher);
                if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink {
                    let target = readlinkat(&directory, &name, Vec::new())
                        .map_err(|source| failure(&root.join(child), source))?;
                    hasher.update(target.as_bytes());
                }
                hasher.update(b"\0");
            }
        }
    }
    Ok(Some(digest_of(hasher)))
}

#[cfg(not(target_os = "linux"))]
fn source_fingerprint_portable(
    project_root: &Path,
    manifest: &ProjectManifest,
) -> Result<Option<String>, TexeError> {
    let Some((paths, aliases)) = source_files(project_root, manifest, MAX_SOURCE_STAMPS)? else {
        return Ok(None);
    };
    let mut hasher = Sha256::new();
    let mut count = paths.len() + aliases.len();
    for (path, target) in aliases {
        hasher.update(b"source-alias\0");
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(target.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        source_change_stamp(project_root, &path, &mut hasher)?;
    }
    for path in paths {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        source_change_stamp(project_root, &path, &mut hasher)?;
        hasher.update(b"\0");
    }
    // Standalone protected builds can explicitly grant read-only inputs outside
    // the paper. Include exactly those declared roots, never an ambient HOME.
    let mut outside = BTreeSet::new();
    for source in std::iter::once(&manifest.project.entry)
        .chain(manifest.inputs.roots.iter())
        .chain(manifest.bibliography.roots.iter())
    {
        let path = project_root.join(source);
        let resolved = path
            .canonicalize()
            .map_err(|source| TexeError::Io { path, source })?;
        if !resolved.starts_with(project_root) {
            outside.insert(resolved);
        }
    }
    for root in outside {
        let mut pending = vec![root.clone()];
        while let Some(path) = pending.pop() {
            count += 1;
            if count > MAX_SOURCE_STAMPS {
                return Ok(None);
            }
            let metadata = fs::symlink_metadata(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            hasher.update(path.as_os_str().as_encoded_bytes());
            source_change_stamp_path(&path, &mut hasher)?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                generated::validate_parent(&root, &path.join(".texe-source-scan"))?;
                let mut children = Vec::new();
                for child in fs::read_dir(&path).map_err(|source| TexeError::Io {
                    path: path.clone(),
                    source,
                })? {
                    let child = child.map_err(|source| TexeError::Io {
                        path: path.clone(),
                        source,
                    })?;
                    if count + pending.len() + children.len() >= MAX_SOURCE_STAMPS {
                        return Ok(None);
                    }
                    children.push(child.path());
                }
                children.sort();
                pending.extend(children.into_iter().rev());
            } else if metadata.file_type().is_symlink() {
                let target =
                    fs::read_link(&path).map_err(|source| TexeError::Io { path, source })?;
                hasher.update(target.as_os_str().as_encoded_bytes());
            }
        }
    }
    Ok(Some(digest_of(hasher)))
}

#[cfg(not(target_os = "linux"))]
fn source_change_stamp(root: &Path, path: &str, hasher: &mut Sha256) -> Result<(), TexeError> {
    let path = root.join(path);
    source_change_stamp_path(&path, hasher)
}

#[cfg(not(target_os = "linux"))]
fn source_change_stamp_path(path: &Path, hasher: &mut Sha256) -> Result<(), TexeError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| TexeError::Io {
        path: path.into(),
        source,
    })?;
    hasher.update(metadata.len().to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        // ctime changes even when a save is reverted to identical bytes or a
        // caller restores mtime. This token is only a render stability guard;
        // the normal no-op fingerprint intentionally stays content-based.
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
        hasher.update(metadata.mtime().to_le_bytes());
        hasher.update(metadata.mtime_nsec().to_le_bytes());
        hasher.update(metadata.ctime().to_le_bytes());
        hasher.update(metadata.ctime_nsec().to_le_bytes());
    }
    #[cfg(not(unix))]
    for timestamp in [metadata.modified(), metadata.created()] {
        if let Ok(timestamp) = timestamp
            && let Ok(timestamp) = timestamp.duration_since(std::time::UNIX_EPOCH)
        {
            hasher.update(timestamp.as_nanos().to_le_bytes());
        }
    }
    Ok(())
}

/// Confirm that every recorded output is still on disk with the bytes the
/// build left there, and return the primary artifact.
pub fn verify_outputs(project_root: &Path, outputs: &[OutputDigest]) -> Option<PathBuf> {
    if !valid_output_layout(outputs) {
        return None;
    }
    let mut artifact = None;
    for output in outputs {
        let path = project_file(project_root, &output.path.to_string_lossy())?;
        let bytes =
            generated::read(project_root, project_root, &path, generated::MAX_ARTIFACT).ok()?;
        if bytes.is_empty() || digest(&bytes) != output.digest {
            return None;
        }
        artifact.get_or_insert(path);
    }
    artifact
}

/// Every project file the package lock names: the scanned sources, plus the
/// bibliography databases, graphics, and inputs it resolved. Editing any of
/// them must produce a different PDF than the published one.
fn locked_project_files(lock: &serde_json::Value) -> Option<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    let Some(packages) = lock.get("packages") else {
        return Some(paths);
    };
    if let Some(root) = packages.get("root").and_then(serde_json::Value::as_str) {
        paths.insert(root.to_string());
    }
    for source in array(packages, "sources") {
        if let Some(path) = source.get("path").and_then(serde_json::Value::as_str) {
            paths.insert(path.to_string());
            if paths.len() > MAX_FINGERPRINT_FILES {
                return None;
            }
        }
    }
    for field in ["inputs", "graphics", "bibliographies"] {
        for record in array(packages, field) {
            if let Some(path) = record
                .get("resolved_path")
                .and_then(serde_json::Value::as_str)
            {
                paths.insert(path.to_string());
                if paths.len() > MAX_FINGERPRINT_FILES {
                    return None;
                }
            }
        }
    }
    Some(paths)
}

fn valid_output_layout(outputs: &[OutputDigest]) -> bool {
    let Some(primary) = outputs.first() else {
        return false;
    };
    if outputs.len() > 2
        || primary.path.components().count() != 1
        || !matches!(primary.path.components().next(), Some(Component::Normal(_)))
        || !matches!(
            primary.path.extension().and_then(|value| value.to_str()),
            Some("pdf" | "dvi")
        )
    {
        return false;
    }
    outputs
        .get(1)
        .is_none_or(|sync| sync.path == primary.path.with_extension("synctex.gz"))
}

type PortableSources = (BTreeSet<String>, BTreeMap<String, PathBuf>);
fn portable_source_files(
    project_root: &Path,
    manifest: &ProjectManifest,
) -> Result<Option<PortableSources>, TexeError> {
    source_files(project_root, manifest, MAX_FINGERPRINT_FILES)
}

fn source_files(
    project_root: &Path,
    manifest: &ProjectManifest,
    limit: usize,
) -> Result<Option<PortableSources>, TexeError> {
    let name = manifest
        .project
        .entry
        .file_name()
        .ok_or_else(|| TexeError::Build("Invalid project entry".into()))?;
    let primary = PathBuf::from(name).with_extension("pdf");
    let dvi = primary.with_extension("dvi");
    let synctex = primary.with_extension("synctex.gz");
    let mut files = BTreeSet::new();
    let mut aliases = BTreeMap::new();
    let mut directories = vec![project_root.to_owned()];
    let mut entries_seen = 0_usize;
    while let Some(directory) = directories.pop() {
        generated::validate_parent(project_root, &directory.join(".texe-source-scan"))?;
        let entries = fs::read_dir(&directory).map_err(|source| TexeError::Io {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| TexeError::Io {
                path: directory.clone(),
                source,
            })?;
            entries_seen += 1;
            if entries_seen > limit {
                return Ok(None);
            }
            let path = entry.path();
            let relative = path.strip_prefix(project_root).expect("project descendant");
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".texe" | ".citet" | ".git")
            ) || relative.components().count() == 1
                && (relative == Path::new("texe.lock")
                    || relative == primary
                    || relative == dvi
                    || relative == synctex)
            {
                continue;
            }
            let kind = entry.file_type().map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() {
                let relative = relative.to_str().ok_or_else(|| {
                    TexeError::Build("Project source paths must use UTF-8".into())
                })?;
                files.insert(relative.replace('\\', "/"));
            } else if kind.is_symlink() {
                let relative = relative.to_str().ok_or_else(|| {
                    TexeError::Build("Project source paths must use UTF-8".into())
                })?;
                aliases.insert(
                    relative.replace('\\', "/"),
                    fs::read_link(&path).map_err(|source| TexeError::Io {
                        path: path.clone(),
                        source,
                    })?,
                );
            }
            // Do not follow unrelated aliases. Explicit locked inputs still
            // use the confined reader below, and local targets are themselves
            // included by this complete source-tree scan.
        }
    }
    Ok(Some((files, aliases)))
}

fn array<'a>(value: &'a serde_json::Value, field: &str) -> &'a [serde_json::Value] {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn read_project_file(project_root: &Path, path: &str) -> Result<Option<Vec<u8>>, TexeError> {
    let physical_root = project_root
        .canonicalize()
        .map_err(|source| TexeError::Io {
            path: project_root.into(),
            source,
        })?;
    let project_root = physical_root.as_path();
    let Some(path) = project_file(project_root, path) else {
        return Ok(None);
    };
    // Source links within the paper remain valid user inputs. Resolve their
    // target first, then use the strict reader for the confined regular file.
    // Generated outputs and metadata never get this alias exception.
    let resolved = match path.canonicalize() {
        Ok(resolved) if resolved.starts_with(project_root) => resolved,
        Ok(_) => {
            return Err(TexeError::Build(
                "Project source inputs must remain within the paper's source directory".into(),
            ));
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(TexeError::Io { path, source }),
    };
    generated::optional(
        project_root,
        project_root,
        &resolved,
        generated::MAX_ARTIFACT,
    )
}

/// Locked paths are project-relative by construction. One that is not stays
/// out of the fingerprint rather than reaching outside the project.
fn project_file(project_root: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(project_root.join(relative))
}

fn canonical_json<T: Serialize>(value: &T, what: &str) -> Result<Vec<u8>, TexeError> {
    serde_json::to_vec(value).map_err(|source| TexeError::Json {
        path: PathBuf::from(what),
        source,
    })
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    digest_of(hasher)
}

fn digest_of(hasher: Sha256) -> String {
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::generated;
    use super::source_fingerprint;
    use sha2::Digest as _;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::ProjectManifest;
    use crate::lockfile::LOCK_NAME;
    use crate::state::{
        OutputDigest, STATE_NAME, build_fingerprint, project_file, read, verify_outputs, write,
    };
    use crate::toolchain::ToolchainIdentity;

    fn identity() -> ToolchainIdentity {
        ToolchainIdentity {
            provider: "managed".to_string(),
            engine: "pdflatex".to_string(),
            channel: "snapshot".to_string(),
            target: "x86_64-linux".to_string(),
            fingerprint: "abc".to_string(),
            registry_url: None,
            registry_metadata_digest: None,
            artifacts: Vec::new(),
        }
    }

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
        .expect("manifest parses")
    }

    fn project() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::write(root.join("main.tex"), b"\\documentclass{article}").expect("entry");
        fs::write(root.join("references.bib"), b"@book{knuth,}").expect("bibliography");
        let lock = serde_json::json!({
            "schema": "texe.lock/v1",
            "generated_with": "texe/test",
            "toolchain": identity(),
            "packages": {
                "schema": "pqty.lock/v1",
                "root": "main.tex",
                "sources": [{"path": "main.tex", "digest": "sha256:0"}],
                "bibliographies": [
                    {"kind": "bibliography", "name": "references",
                     "resolved_path": "references.bib", "source": {}},
                    {"kind": "bibliography-style", "name": "plain",
                     "resolved_path": null, "source": {}}
                ],
            },
        });
        fs::write(
            root.join(LOCK_NAME),
            serde_json::to_vec_pretty(&lock).expect("lock"),
        )
        .expect("write lock");
        directory
    }

    fn fingerprint(root: &Path) -> String {
        build_fingerprint(
            root,
            &manifest(),
            &identity(),
            "sha256:command-suite",
            1_700_000_000,
        )
        .expect("fingerprint")
        .expect("locked project has a fingerprint")
    }

    #[test]
    fn an_unlocked_project_has_no_fingerprint() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            build_fingerprint(
                directory.path(),
                &manifest(),
                &identity(),
                "sha256:command-suite",
                0,
            )
            .expect("fingerprint"),
            None
        );
    }

    #[test]
    fn protected_local_cache_uses_private_lock_and_current_unlocked_sources() {
        let fixture = project();
        let root = fixture.path();
        let private_lock = root.join(".texe/build/texe.lock");
        fs::create_dir_all(private_lock.parent().unwrap()).unwrap();
        let original_lock = fs::read(root.join(LOCK_NAME)).unwrap();
        fs::write(&private_lock, &original_lock).unwrap();
        let standalone = fingerprint(root);
        {
            let _protection = crate::build::process::ProtectionScope::enter(true);
            let original = fingerprint(root);
            assert_ne!(original, standalone);
            fs::write(
                root.join(LOCK_NAME),
                b"researcher root lock stays untouched",
            )
            .unwrap();
            assert_eq!(fingerprint(root), original);
            fs::write(root.join("new-input.tex"), b"newly included research").unwrap();
            let new_input = fingerprint(root);
            assert_ne!(new_input, original);
            fs::write(root.join("new-input.tex"), b"edited new research").unwrap();
            assert_ne!(fingerprint(root), new_input);
        }
        assert_eq!(
            fs::read(root.join(LOCK_NAME)).unwrap(),
            b"researcher root lock stays untouched"
        );
    }

    #[test]
    fn editing_any_locked_project_file_changes_the_fingerprint() {
        let directory = project();
        let root = directory.path();
        let initial = fingerprint(root);
        assert_eq!(initial, fingerprint(root));

        fs::write(root.join("main.tex"), b"\\documentclass{report}").expect("edit entry");
        let edited_entry = fingerprint(root);
        assert_ne!(initial, edited_entry);

        fs::write(root.join("main.tex"), b"\\documentclass{article}").expect("restore entry");
        assert_eq!(initial, fingerprint(root));

        fs::write(root.join("references.bib"), b"@book{lamport,}").expect("edit bibliography");
        assert_ne!(initial, fingerprint(root));
    }

    #[test]
    fn editing_a_nested_entry_changes_the_fingerprint() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::create_dir_all(root.join("paper")).expect("paper directory");
        fs::write(root.join("paper/main.tex"), b"first").expect("nested entry");
        let lock = serde_json::json!({
            "schema": "texe.lock/v1",
            "generated_with": "texe/test",
            "toolchain": identity(),
            "packages": {
                "schema": "pqty.lock/v1",
                "root": "paper/main.tex",
                "sources": [{"path": "paper/main.tex", "digest": "sha256:0"}],
            },
        });
        fs::write(
            root.join(LOCK_NAME),
            serde_json::to_vec_pretty(&lock).expect("lock"),
        )
        .expect("write lock");
        let mut manifest = manifest();
        manifest.project.entry = PathBuf::from("paper/main.tex");
        let initial = build_fingerprint(
            root,
            &manifest,
            &identity(),
            "sha256:command-suite",
            1_700_000_000,
        )
        .expect("fingerprint")
        .expect("locked project");

        fs::write(root.join("paper/main.tex"), b"second").expect("edit nested entry");
        let edited = build_fingerprint(
            root,
            &manifest,
            &identity(),
            "sha256:command-suite",
            1_700_000_000,
        )
        .expect("fingerprint")
        .expect("locked project");
        assert_ne!(initial, edited);
    }

    #[test]
    fn a_deleted_locked_file_changes_the_fingerprint() {
        let directory = project();
        let root = directory.path();
        let initial = fingerprint(root);
        fs::remove_file(root.join("references.bib")).expect("remove bibliography");
        assert_ne!(initial, fingerprint(root));
    }

    #[test]
    fn the_timestamp_toolchain_and_manifest_all_participate() {
        let directory = project();
        let root = directory.path();
        let initial = fingerprint(root);

        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &manifest(),
                &identity(),
                "sha256:command-suite",
                1_700_000_001,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );

        let mut other = identity();
        other.fingerprint = "def".to_string();
        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &manifest(),
                &other,
                "sha256:command-suite",
                1_700_000_000,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );

        let mut other_manifest = manifest();
        other_manifest.toolchain.max_passes += 1;
        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &other_manifest,
                &identity(),
                "sha256:command-suite",
                1_700_000_000,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );

        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &manifest(),
                &identity(),
                "sha256:different-command-suite",
                1_700_000_000,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );
    }

    #[test]
    fn recorded_outputs_must_still_be_on_disk_unchanged() {
        let directory = project();
        let root = directory.path();
        let artifact = root.join("main.pdf");
        fs::write(&artifact, b"%PDF-test").expect("artifact");
        fs::write(root.join("main.synctex.gz"), b"sync").expect("synctex");
        let state_path = root.join(".texe/build").join(STATE_NAME);
        write(
            &state_path,
            "sha256:fingerprint",
            "sha256:environment",
            root,
            &[artifact.clone(), root.join("main.synctex.gz")],
        )
        .expect("state is recorded");

        let state = read(&state_path).expect("state is readable");
        assert_eq!(state.fingerprint, "sha256:fingerprint");
        assert_eq!(state.outputs[0].path, Path::new("main.pdf"));
        assert_eq!(verify_outputs(root, &state.outputs), Some(artifact.clone()));

        fs::write(&artifact, b"%PDF-edited").expect("edit artifact");
        assert_eq!(verify_outputs(root, &state.outputs), None);

        fs::remove_file(&artifact).expect("remove artifact");
        assert_eq!(verify_outputs(root, &state.outputs), None);
    }

    #[test]
    fn locked_paths_cannot_reach_outside_the_project() {
        assert_eq!(project_file(Path::new("/project"), "../secret"), None);
        assert_eq!(project_file(Path::new("/project"), "/etc/passwd"), None);
        assert_eq!(
            project_file(Path::new("/project"), "sections/one.tex"),
            Some(PathBuf::from("/project/sections/one.tex"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn aliased_outputs_cannot_validate_or_enter_a_build_state() {
        use std::os::unix::fs::symlink;
        let directory = project();
        let root = directory.path();
        let research = root.join("research.pdf");
        let original = b"last good researcher PDF";
        fs::write(&research, original).unwrap();
        let artifact = root.join("main.pdf");
        let state_path = root.join(".texe/build").join(STATE_NAME);
        let output = OutputDigest {
            path: PathBuf::from("main.pdf"),
            digest: super::digest(original),
        };
        symlink(&research, &artifact).unwrap();
        assert_eq!(verify_outputs(root, std::slice::from_ref(&output)), None);
        assert!(
            write(
                &state_path,
                "fingerprint",
                "environment",
                root,
                std::slice::from_ref(&artifact)
            )
            .is_err()
        );
        fs::remove_file(&artifact).unwrap();
        fs::hard_link(&research, &artifact).unwrap();
        assert_eq!(verify_outputs(root, std::slice::from_ref(&output)), None);
        assert!(write(&state_path, "fingerprint", "environment", root, &[artifact]).is_err());
        assert_eq!(fs::read(&research).unwrap(), original);
        assert!(!state_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cache_parent_alias_cannot_overwrite_research_and_last_good_pdf() {
        use std::os::unix::fs::symlink;
        let directory = project();
        let root = directory.path();
        let research = root.join("research");
        fs::create_dir(&research).unwrap();
        let source = research.join(STATE_NAME);
        fs::write(&source, b"research notes").unwrap();
        let artifact = root.join("main.pdf");
        fs::write(&artifact, b"last good PDF").unwrap();
        symlink(&research, root.join(".texe")).unwrap();
        let state_path = root.join(".texe").join(STATE_NAME);
        assert!(read(&state_path).is_none());
        assert!(
            write(
                &state_path,
                "fingerprint",
                "environment",
                root,
                std::slice::from_ref(&artifact)
            )
            .is_err()
        );
        assert_eq!(fs::read(&source).unwrap(), b"research notes");
        assert_eq!(fs::read(&artifact).unwrap(), b"last good PDF");
    }

    #[cfg(unix)]
    #[test]
    fn aliased_locked_sources_are_rejected_before_fingerprinting() {
        use std::os::unix::fs::symlink;
        let directory = project();
        let root = directory.path();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("research.bib"), b"private research").unwrap();
        fs::remove_file(root.join("references.bib")).unwrap();
        symlink(
            outside.path().join("research.bib"),
            root.join("references.bib"),
        )
        .unwrap();
        assert!(build_fingerprint(root, &manifest(), &identity(), "suite", 1_700_000_000).is_err());
        assert_eq!(
            fs::read(outside.path().join("research.bib")).unwrap(),
            b"private research"
        );
    }

    #[cfg(unix)]
    #[test]
    fn researcher_source_aliases_within_the_paper_remain_valid_inputs() {
        use std::os::unix::fs::symlink;
        let directory = project();
        let root = directory.path();
        fs::rename(
            root.join("references.bib"),
            root.join("actual-references.bib"),
        )
        .unwrap();
        symlink("actual-references.bib", root.join("references.bib")).unwrap();
        let initial = fingerprint(root);
        fs::write(
            root.join("actual-references.bib"),
            b"edited researcher bibliography",
        )
        .unwrap();
        assert_ne!(initial, fingerprint(root));
    }

    #[test]
    fn portable_fingerprints_use_private_lock_and_preserve_the_researcher_root_lock() {
        let directory = project();
        let root = directory.path();
        let original_root_lock = fs::read(root.join(LOCK_NAME)).unwrap();
        let mut profile = serde_json::json!({"schema": "texe.hosted-profile/v1", "runtimeRecipe": "texlive-2026-07-26"});
        profile["id"] = format!(
            "sha256:{}",
            hex::encode(sha2::Sha256::digest(serde_json::to_vec(&profile).unwrap()))
        )
        .into();
        let mut descriptor = serde_json::json!({"schema":"texe.environment/v1", "projectId":"test-paper", "profile":profile,
            "engine":"pdflatex", "entry":"main.tex", "source_date_epoch":1_700_000_000,
            "packages":{"schema":"pqty.lock/v1", "stage":"exact", "root":"main.tex", "closure":[]}});
        descriptor["id"] = format!(
            "sha256:{}",
            hex::encode(sha2::Sha256::digest(
                serde_json::to_vec(&descriptor).unwrap()
            ))
        )
        .into();
        fs::create_dir(root.join(".texe")).unwrap();
        fs::write(
            root.join(crate::portable::ACTIVE),
            serde_json::to_vec(&descriptor).unwrap(),
        )
        .unwrap();
        let manifest = crate::portable::manifest(&descriptor).unwrap();
        let private_lock = root.join(&manifest.project.build_dir).join(LOCK_NAME);
        fs::create_dir_all(private_lock.parent().unwrap()).unwrap();
        fs::write(&private_lock, &original_root_lock).unwrap();
        let fingerprint = || {
            build_fingerprint(root, &manifest, &identity(), "suite", 1_700_000_000)
                .unwrap()
                .unwrap()
        };
        let first = fingerprint();
        assert_eq!(first, fingerprint());
        fs::write(root.join(LOCK_NAME), b"unrelated original project lock").unwrap();
        assert_eq!(first, fingerprint());
        fs::write(root.join("references.bib"), b"edited reference").unwrap();
        assert_ne!(first, fingerprint());
        // Reused package capsules can predate a new source file. Later edits
        // must still invalidate the local PDF's cached build.
        fs::write(root.join("new-input.tex"), b"new chapter").unwrap();
        let with_new_input = fingerprint();
        fs::write(root.join("new-input.tex"), b"edited new chapter").unwrap();
        assert_ne!(with_new_input, fingerprint());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("references.bib", root.join("source-alias.bib")).unwrap();
            let first_alias = fingerprint();
            fs::remove_file(root.join("source-alias.bib")).unwrap();
            std::os::unix::fs::symlink("new-input.tex", root.join("source-alias.bib")).unwrap();
            assert_ne!(first_alias, fingerprint());
        }
        fs::remove_file(root.join(LOCK_NAME)).unwrap();
        assert!(
            build_fingerprint(root, &manifest, &identity(), "suite", 1_700_000_000)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn cached_state_cannot_nominate_sources_multiple_pdfs_or_unrelated_sync_files() {
        let directory = project();
        let root = directory.path();
        for paths in [
            vec!["main.tex"],
            vec!["main.pdf", "other.pdf"],
            vec!["main.pdf", "other.synctex.gz"],
            vec!["nested/main.pdf"],
            vec!["main.synctex.gz"],
            Vec::<&str>::new(),
        ] {
            let outputs = paths
                .iter()
                .map(|path| OutputDigest {
                    path: PathBuf::from(path),
                    digest: "digest".into(),
                })
                .collect::<Vec<_>>();
            assert_eq!(verify_outputs(root, &outputs), None);
            let state = super::BuildState {
                schema: super::STATE_SCHEMA.into(),
                generated_with: "test".into(),
                fingerprint: "fp".into(),
                environment_fingerprint: "env".into(),
                outputs,
            };
            let state_path = root.join("state.json");
            fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
            assert!(read(&state_path).is_none());
        }
        assert_eq!(
            fs::read(root.join("main.tex")).unwrap(),
            b"\\documentclass{article}"
        );
    }

    #[test]
    fn too_many_locked_inputs_disable_the_cache_without_blocking_a_build() {
        let directory = project();
        let root = directory.path();
        let mut lock: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(LOCK_NAME)).unwrap()).unwrap();
        lock["packages"]["sources"] = (0..=super::MAX_FINGERPRINT_FILES)
            .map(|number| serde_json::json!({"path":format!("source-{number}.tex")}))
            .collect();
        fs::write(root.join(LOCK_NAME), serde_json::to_vec(&lock).unwrap()).unwrap();
        assert!(
            build_fingerprint(root, &manifest(), &identity(), "suite", 1)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fs::read(root.join("main.tex")).unwrap(),
            b"\\documentclass{article}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn render_inventory_is_metadata_only_and_tracks_only_declared_outside_inputs() {
        use std::os::unix::fs::symlink;
        let directory = project();
        let root = directory.path();
        let large = fs::File::create(root.join("large-figure.pdf")).unwrap();
        large.set_len(generated::MAX_ARTIFACT + 1).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            root.join("unused-input-pipe"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        let outside = tempfile::tempdir().unwrap();
        let input = outside.path().join("shared-input.tex");
        fs::write(&input, b"outside original").unwrap();
        symlink(outside.path(), root.join("unrelated-link")).unwrap();
        let initial = source_fingerprint(root, &manifest()).unwrap().unwrap();
        fs::write(&input, b"outside edit").unwrap();
        assert_eq!(
            source_fingerprint(root, &manifest()).unwrap().unwrap(),
            initial,
            "an unrelated source link must not traverse outside the paper"
        );
        let mut declared = manifest();
        declared.inputs.roots.push(input.clone());
        let declared_initial = source_fingerprint(root, &declared).unwrap().unwrap();
        fs::write(&input, b"declared outside edit").unwrap();
        assert_ne!(
            source_fingerprint(root, &declared).unwrap().unwrap(),
            declared_initial
        );
        assert_eq!(
            fs::metadata(root.join("large-figure.pdf")).unwrap().len(),
            generated::MAX_ARTIFACT + 1
        );
        fs::remove_file(root.join("unused-input-pipe")).unwrap();
    }

    #[test]
    fn render_inventory_ignores_nested_private_metadata_and_primary_output_changes() {
        let directory = project();
        let root = directory.path();
        fs::create_dir_all(root.join("chapter/.git")).unwrap();
        fs::write(root.join("chapter/text.tex"), b"research").unwrap();
        fs::write(root.join("chapter/.git/index"), b"index").unwrap();
        let initial = source_fingerprint(root, &manifest()).unwrap().unwrap();
        fs::write(root.join("chapter/.git/index"), b"modified index").unwrap();
        fs::write(root.join("main.pdf"), b"derived PDF").unwrap();
        fs::create_dir_all(root.join(".texe/build/output")).unwrap();
        fs::write(root.join(".texe/build/output/main.log"), b"derived log").unwrap();
        assert_eq!(
            source_fingerprint(root, &manifest()).unwrap().unwrap(),
            initial
        );
        fs::write(root.join("chapter/text.tex"), b"new research").unwrap();
        assert_ne!(
            source_fingerprint(root, &manifest()).unwrap().unwrap(),
            initial
        );
    }
}
