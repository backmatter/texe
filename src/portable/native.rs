//! Verified process runtimes for engines with their own I/O and format model.
use super::{error, text};
use crate::{
    TexeError,
    build::{BuildOptions, BuildReport},
    config::ProjectManifest,
    package::PqtyClient,
    progress::Progress,
    toolchain::{ManagedBiber, ResolvedToolchain, ToolchainIdentity, VerificationPolicy},
};
use flate2::bufread::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const READY: &str = ".ready";
const INDEX_SCHEMA: &str = "texe.native-runtime/v1";
const MAX_RUNTIME_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
// Runtime extraction is streamed. Bound disk separately from the compressed
// transfer: the current verified Shipout bundle expands beyond 512 MiB.
const MAX_RUNTIME_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_PATH_METADATA: u64 = 64 * 1024;
const VERIFICATION_INTERVAL: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileMetadata {
    size: u64,
    modified: Option<(u64, u32)>,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    device: u64,
}
impl FileMetadata {
    fn from(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        Self {
            size: metadata.len(),
            modified: metadata.modified().ok().and_then(|time| {
                time.duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
            }),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            #[cfg(unix)]
            mode: metadata.mode(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            device: metadata.dev(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexedFile {
    sha256: String,
    metadata: FileMetadata,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeIndex {
    schema: String,
    sha256: String,
    target: String,
    engine: String,
    directories: BTreeSet<String>,
    files: BTreeMap<String, IndexedFile>,
    verified_at: Option<u64>,
}

fn io(path: &Path, source: std::io::Error) -> TexeError {
    TexeError::Io {
        path: path.to_owned(),
        source,
    }
}
fn now() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}
fn valid_checksum(checksum: &str) -> bool {
    checksum.len() == 64
        && checksum
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn valid_path(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    !value.is_empty()
        && !value.contains(['\\', ':', '\0'])
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        && {
            let mut parts = path.components();
            match parts.next().and_then(|part| part.as_os_str().to_str()) {
                Some("bin" | "assets" | "tex" | "bundle") => true,
                // Trusted prebuilt Tectonic formats travel with the verified
                // runtime, but the compiler uses a project-private copy.
                Some("format-cache") => parts
                    .next()
                    .is_none_or(|part| part.as_os_str() == "formats"),
                _ => false,
            }
        }
}
fn runtime_root(descriptor: &Value) -> Result<PathBuf, TexeError> {
    let checksum = text(&descriptor["runtime"], "sha256")?;
    if !valid_checksum(checksum) {
        return Err(error("Unsafe runtime identity"));
    }
    Ok(crate::toolchain::texe_data_home()?
        .join("native")
        .join(checksum))
}
fn ensure_directory(path: &Path) -> Result<(), TexeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(error(format!(
            "Unsafe native cache directory: {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|source| io(path, source))
        }
        Err(source) => Err(io(path, source)),
    }
}
fn target_name() -> String {
    format!(
        "{}-{}",
        if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(windows) {
            "win32"
        } else {
            std::env::consts::OS
        },
        if std::env::consts::ARCH == "x86_64" {
            "x64"
        } else if std::env::consts::ARCH == "aarch64" {
            "arm64"
        } else {
            std::env::consts::ARCH
        }
    )
}
fn executable(root: &Path, descriptor: &Value) -> Result<PathBuf, TexeError> {
    let name = match text(descriptor, "engine")? {
        "shipout" => "citet-shipout",
        "tectonic" => "tectonic",
        _ => return Err(error("Unsupported native runtime engine")),
    };
    Ok(root
        .join("bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
}
fn hash_reader(reader: &mut impl Read, path: &Path) -> Result<String, TexeError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let size = reader
            .read(&mut buffer)
            .map_err(|source| io(path, source))?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    Ok(hex::encode(hasher.finalize()))
}
fn hash_file(path: &Path, metadata: &FileMetadata) -> Result<String, TexeError> {
    let mut file = fs::File::open(path).map_err(|source| io(path, source))?;
    if FileMetadata::from(&file.metadata().map_err(|source| io(path, source))?) != *metadata {
        return Err(error("Native runtime changed during verification"));
    }
    let checksum = hash_reader(&mut file, path)?;
    let after = fs::symlink_metadata(path).map_err(|source| io(path, source))?;
    if !after.is_file() || after.file_type().is_symlink() || FileMetadata::from(&after) != *metadata
    {
        return Err(error("Native runtime changed during verification"));
    }
    Ok(checksum)
}
fn tree(root: &Path) -> Result<(BTreeSet<String>, BTreeMap<String, FileMetadata>), TexeError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| io(root, source))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(error("Unsafe native runtime cache"));
    }
    let mut directories = BTreeSet::new();
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for item in fs::read_dir(&directory).map_err(|source| io(&directory, source))? {
            let item = item.map_err(|source| io(&directory, source))?;
            let path = item.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| error("Unsafe native cache path"))?;
            if relative == Path::new(READY) {
                continue;
            }
            let name = relative
                .to_str()
                .ok_or_else(|| error("Invalid native runtime cache path"))?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if !valid_path(Path::new(&name)) {
                return Err(error("Unexpected path in native runtime cache"));
            }
            let metadata = fs::symlink_metadata(&path).map_err(|source| io(&path, source))?;
            if metadata.file_type().is_symlink() {
                return Err(error("A native runtime cache entry is a symbolic link"));
            }
            if metadata.is_dir() {
                if directories.len() >= MAX_FILES {
                    return Err(error("Native runtime has too many entries"));
                }
                directories.insert(name);
                pending.push(path);
            } else if metadata.is_file() {
                if files.len() >= MAX_FILES {
                    return Err(error("Native runtime has too many entries"));
                }
                files.insert(name, FileMetadata::from(&metadata));
            } else {
                return Err(error("Unsafe native runtime cache entry"));
            }
        }
    }
    Ok((directories, files))
}
fn write_index(root: &Path, index: &RuntimeIndex) -> Result<(), TexeError> {
    let bytes = serde_json::to_vec(index).map_err(|_| error("Invalid native runtime index"))?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(error("Native runtime index is too large"));
    }
    crate::atomic::write(&root.join(READY), &bytes)
}
fn verify_cache(
    root: &Path,
    descriptor: &Value,
    policy: VerificationPolicy,
) -> Result<(), TexeError> {
    let path = root.join(READY);
    let metadata = fs::symlink_metadata(&path).map_err(|source| io(&path, source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_INDEX_BYTES
    {
        return Err(error("Invalid native runtime index"));
    }
    let mut index: RuntimeIndex = serde_json::from_reader(
        fs::File::open(&path)
            .map_err(|source| io(&path, source))?
            .take(MAX_INDEX_BYTES + 1),
    )
    .map_err(|_| error("Native runtime requires verified preparation"))?;
    if index.schema != INDEX_SCHEMA
        || index.sha256 != text(&descriptor["runtime"], "sha256")?
        || index.target != text(&descriptor["runtime"], "target")?
        || index.engine != text(descriptor, "engine")?
        || index.files.is_empty()
        || index.files.len() > MAX_FILES
        || index.directories.len() > MAX_FILES
        || index.files.keys().any(|name| !valid_path(Path::new(name)))
        || index
            .directories
            .iter()
            .any(|name| !valid_path(Path::new(name)))
        || index
            .files
            .values()
            .any(|file| !valid_checksum(&file.sha256) || file.metadata.size > 128 * 1024 * 1024)
        || index
            .files
            .values()
            .map(|file| file.metadata.size)
            .sum::<u64>()
            > MAX_RUNTIME_BYTES
    {
        return Err(error(
            "Native runtime index does not match its pinned identity",
        ));
    }
    let (directories, files) = tree(root)?;
    if directories != index.directories || !files.keys().eq(index.files.keys()) {
        return Err(error(
            "Native runtime cache contains missing or unexpected files",
        ));
    }
    let time = now();
    let deep = policy == VerificationPolicy::Deep
        || match (time, index.verified_at) {
            (Some(time), Some(verified)) => {
                time < verified || time - verified >= VERIFICATION_INTERVAL
            }
            _ => true,
        };
    let mut changed = false;
    for (name, metadata) in files {
        let file = index.files.get_mut(&name).unwrap();
        #[cfg(unix)]
        if metadata.mode & 0o400 == 0 || (name.starts_with("bin/") && metadata.mode & 0o100 == 0) {
            return Err(error(format!(
                "Native runtime file has unsafe permissions: {name}"
            )));
        }
        if file.metadata.size != metadata.size {
            return Err(error(format!(
                "Native runtime file has changed size: {name}"
            )));
        }
        // Some filesystems coalesce multiple writes into one timestamp tick.
        // Rehash very recent writes so a same-size change in that tick cannot
        // reuse a freshly recorded metadata stamp.
        let recent = match (time, metadata.modified) {
            (Some(time), Some((modified, _))) => modified >= time.saturating_sub(2),
            _ => true,
        };
        if deep || file.metadata != metadata || recent {
            if hash_file(&root.join(&name), &metadata)? != file.sha256 {
                return Err(error(format!(
                    "Native runtime failed installed-file verification: {name}"
                )));
            }
            file.metadata = metadata;
            changed = true;
        }
    }
    if deep {
        index.verified_at = time;
    }
    if changed {
        let _ = write_index(root, &index);
    }
    Ok(())
}
fn remove_cache(path: &Path) -> Result<(), TexeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).map_err(|source| io(path, source))
        }
        Ok(_) => fs::remove_file(path).map_err(|source| io(path, source)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io(path, source)),
    }
}
fn sibling(root: &Path, label: &str) -> PathBuf {
    root.parent().unwrap().join(format!(
        ".{label}-{}-{}-{}",
        root.file_name().unwrap().to_string_lossy(),
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}
fn open_archive(path: &Path, descriptor: &Value) -> Result<fs::File, TexeError> {
    let expected = descriptor["runtime"]["size"]
        .as_u64()
        .filter(|v| *v <= MAX_RUNTIME_ARCHIVE_BYTES)
        .ok_or_else(|| error("Invalid native runtime size"))?;
    let mut file = fs::File::open(path).map_err(|source| io(path, source))?;
    if !file
        .metadata()
        .map_err(|source| io(path, source))?
        .is_file()
        || file.metadata().map_err(|source| io(path, source))?.len() != expected
    {
        return Err(error("Incomplete native runtime download"));
    }
    if hash_reader(&mut Read::by_ref(&mut file).take(expected + 1), path)?
        != text(&descriptor["runtime"], "sha256")?
        || file.metadata().map_err(|source| io(path, source))?.len() != expected
    {
        return Err(error("Native runtime checksum mismatch"));
    }
    file.rewind().map_err(|source| io(path, source))?;
    Ok(file)
}
fn retain_archive(path: &Path, source: &mut fs::File, descriptor: &Value) -> Result<(), TexeError> {
    let temporary = sibling(path, "download");
    let result = (|| {
        source.rewind().map_err(|source| io(path, source))?;
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| io(&temporary, source))?;
        std::io::copy(source, &mut target).map_err(|source| io(&temporary, source))?;
        target.sync_all().map_err(|source| io(&temporary, source))?;
        open_archive(&temporary, descriptor)?;
        fs::rename(&temporary, path).map_err(|source| io(path, source))?;
        sync_parent(path);
        Ok(())
    })();
    let _ = remove_cache(&temporary);
    result
}
fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
}
#[allow(clippy::too_many_lines)]
fn extract_archive(file: fs::File, staging: &Path, descriptor: &Value) -> Result<(), TexeError> {
    let mut archive = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    let mut expected_files = BTreeMap::new();
    let mut path_override = None;
    let mut metadata_pending = false;
    let mut headers = 0;
    for entry in archive
        .entries()
        .map_err(|_| error("Invalid native runtime archive"))?
        .raw(true)
    {
        let mut entry = entry.map_err(|_| error("Invalid native runtime entry"))?;
        headers += 1;
        if headers > MAX_FILES {
            return Err(error("Too many native runtime archive entries"));
        }
        let kind = entry.header().entry_type();
        if kind.is_pax_local_extensions() || kind.is_gnu_longname() {
            // tar's default extension processing reads the complete body before
            // yielding an entry. Read raw headers so this limit comes first.
            if metadata_pending || entry.size() > MAX_PATH_METADATA {
                return Err(error("Native runtime path metadata exceeds its safe limit"));
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| error("Native runtime is too large"))?;
            if total > MAX_RUNTIME_BYTES {
                return Err(error("Native runtime exceeds its unpacked size limit"));
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|_| error("Incomplete native runtime path metadata"))?;
            path_override = archive_path_metadata(kind, &bytes)?;
            metadata_pending = true;
            continue;
        }
        let raw = match path_override.take() {
            Some(path) => path,
            None => entry
                .path()
                .map_err(|_| error("Invalid native runtime path"))?
                .into_owned(),
        };
        metadata_pending = false;
        let path: PathBuf = raw
            .components()
            .filter(|component| *component != std::path::Component::CurDir)
            .collect();
        if path.as_os_str().is_empty() && entry.header().entry_type().is_dir() {
            continue;
        }
        if !valid_path(&path)
            || (!entry.header().entry_type().is_file() && !entry.header().entry_type().is_dir())
            || seen.len() >= MAX_FILES
            || !seen.insert(path.clone())
        {
            return Err(error("Unsafe native runtime archive entry"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| error("Native runtime is too large"))?;
        if total > MAX_RUNTIME_BYTES || entry.size() > 128 * 1024 * 1024 {
            return Err(error("Native runtime exceeds its unpacked size limit"));
        }
        let target = staging.join(&path);
        if entry.header().entry_type().is_dir() {
            if entry.size() != 0 {
                return Err(error("Invalid native runtime directory entry"));
            }
            fs::create_dir_all(&target).map_err(|source| io(&target, source))?;
        } else {
            fs::create_dir_all(target.parent().unwrap()).map_err(|source| io(&target, source))?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|source| io(&target, source))?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 16 * 1024];
            loop {
                let size = entry
                    .read(&mut buffer)
                    .map_err(|source| io(&target, source))?;
                if size == 0 {
                    break;
                }
                hasher.update(&buffer[..size]);
                file.write_all(&buffer[..size])
                    .map_err(|source| io(&target, source))?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(fs::Permissions::from_mode(if path.starts_with("bin") {
                    0o755
                } else {
                    0o644
                }))
                .map_err(|source| io(&target, source))?;
            }
            file.sync_all().map_err(|source| io(&target, source))?;
            let name = path
                .to_str()
                .unwrap()
                .replace(std::path::MAIN_SEPARATOR, "/");
            expected_files.insert(name, hex::encode(hasher.finalize()));
        }
    }
    if metadata_pending {
        return Err(error("Native runtime archive has dangling path metadata"));
    }
    // tar stops at its terminator; consume gzip's trailer as well so bad CRCs
    // and truncated streams cannot become a ready runtime.
    let mut decoder = archive.into_inner();
    let mut tail = Vec::new();
    Read::by_ref(&mut decoder)
        .take(65_537)
        .read_to_end(&mut tail)
        .map_err(|_| error("Incomplete native runtime gzip stream"))?;
    if tail.len() > 65_536 || tail.iter().any(|byte| *byte != 0) {
        return Err(error("Unexpected data after native runtime archive"));
    }
    let mut compressed = decoder.into_inner();
    let mut extra = [0u8; 1];
    if compressed
        .read(&mut extra)
        .map_err(|source| io(staging, source))?
        != 0
    {
        return Err(error(
            "Unexpected compressed data after native runtime archive",
        ));
    }
    super::verify_engine(descriptor, &executable(staging, descriptor)?)?;
    let (directories, files) = tree(staging)?;
    let files = files
        .into_iter()
        .map(|(name, metadata)| {
            let checksum = expected_files
                .remove(&name)
                .ok_or_else(|| error("Unexpected extracted runtime file"))?;
            Ok((
                name,
                IndexedFile {
                    sha256: checksum,
                    metadata,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, TexeError>>()?;
    if !expected_files.is_empty() || files.is_empty() {
        return Err(error("Incomplete extracted native runtime"));
    }
    write_index(
        staging,
        &RuntimeIndex {
            schema: INDEX_SCHEMA.into(),
            sha256: text(&descriptor["runtime"], "sha256")?.into(),
            target: text(&descriptor["runtime"], "target")?.into(),
            engine: text(descriptor, "engine")?.into(),
            directories,
            files,
            verified_at: now(),
        },
    )
}

fn archive_path_metadata(kind: tar::EntryType, bytes: &[u8]) -> Result<Option<PathBuf>, TexeError> {
    if kind.is_gnu_longname() {
        let path = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        if path.contains(&0) {
            return Err(error("Invalid native runtime long path"));
        }
        return std::str::from_utf8(path)
            .map(|path| Some(PathBuf::from(path)))
            .map_err(|_| error("Native runtime paths must use UTF-8"));
    }
    let mut path = None;
    let mut keys = BTreeSet::new();
    for extension in tar::PaxExtensions::new(bytes) {
        let extension = extension.map_err(|_| error("Invalid native runtime path metadata"))?;
        let key = extension
            .key()
            .map_err(|_| error("Invalid native runtime metadata key"))?;
        if !keys.insert(key.to_owned()) {
            return Err(error("Duplicate native runtime metadata key"));
        }
        match key {
            "path" => {
                path = Some(PathBuf::from(
                    extension
                        .value()
                        .map_err(|_| error("Native runtime paths must use UTF-8"))?,
                ));
            }
            // Ownership and timestamps are deliberately replaced when sealing
            // the runtime. Size, links and sparse metadata are never accepted.
            "mtime" | "atime" | "ctime" | "uid" | "gid" | "uname" | "gname" => {}
            _ => return Err(error("Unsupported native runtime path metadata")),
        }
    }
    Ok(path)
}
fn install_at(
    root: &Path,
    descriptor: &Value,
    input: Option<&Path>,
    policy: VerificationPolicy,
) -> Result<(), TexeError> {
    if text(&descriptor["runtime"], "target")? != target_name() {
        return Err(error(format!(
            "This native runtime is unavailable for {}",
            target_name()
        )));
    }
    let parent = root
        .parent()
        .ok_or_else(|| error("Unsafe native runtime cache path"))?;
    ensure_directory(parent)?;
    let locks = parent.join(".locks");
    ensure_directory(&locks)?;
    let lock = locks.join(text(&descriptor["runtime"], "sha256")?);
    ensure_directory(&lock)?;
    let lock_file = lock.join(crate::guard::LOCK_NAME);
    match fs::symlink_metadata(&lock_file) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(error("Unsafe native runtime cache lock"));
        }
        Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
            return Err(io(&lock_file, source));
        }
        _ => {}
    }
    let _guard = crate::guard::BuildGuard::acquire(&lock)?;
    let backup = parent.join(format!(
        ".previous-{}",
        text(&descriptor["runtime"], "sha256")?
    ));
    // Complete or roll back an interrupted cache swap before using its files.
    if fs::symlink_metadata(&backup).is_ok() {
        if fs::symlink_metadata(root).is_err() {
            fs::rename(&backup, root).map_err(|source| io(root, source))?;
            sync_parent(root);
        } else {
            // An old derived cache is disposable; inability to clean it does
            // not make the current verified runtime unusable.
            let _ = remove_cache(&backup);
        }
    }
    if verify_cache(root, descriptor, policy).is_ok() {
        return Ok(());
    }
    let archives = parent.join(".archives");
    ensure_directory(&archives)?;
    let cached = archives.join(format!(
        "{}.tar.gz",
        text(&descriptor["runtime"], "sha256")?
    ));
    let source_path = input.unwrap_or(&cached);
    let mut source = open_archive(source_path, descriptor).map_err(|failure| {
        error(format!("Native runtime could not be repaired from its verified download: {failure}. Prepare this project again while connected."))
    })?;
    let staging = sibling(root, "native");
    fs::create_dir(&staging).map_err(|source| io(&staging, source))?;
    let result = (|| {
        extract_archive(
            source
                .try_clone()
                .map_err(|source| io(source_path, source))?,
            &staging,
            descriptor,
        )?;
        verify_cache(&staging, descriptor, VerificationPolicy::Deep)?;
        if input.is_some() {
            retain_archive(&cached, &mut source, descriptor)?;
        }
        if fs::symlink_metadata(root).is_ok() {
            fs::rename(root, &backup).map_err(|source| io(root, source))?;
        }
        if let Err(source) = fs::rename(&staging, root) {
            if fs::symlink_metadata(&backup).is_ok() {
                fs::rename(&backup, root).map_err(|restore| error(format!("Could not publish native runtime: {source}; could not restore prior cache: {restore}")))?;
            }
            return Err(io(root, source));
        }
        sync_parent(root);
        let _ = remove_cache(&backup);
        Ok(())
    })();
    let _ = remove_cache(&staging);
    result
}
pub(super) fn install(
    descriptor: &Value,
    input: Option<&Path>,
    policy: VerificationPolicy,
) -> Result<(), TexeError> {
    install_at(&runtime_root(descriptor)?, descriptor, input, policy)
}
pub(crate) fn resolve(
    root: &Path,
    manifest: &ProjectManifest,
    descriptor: &Value,
    verification: VerificationPolicy,
    offline: bool,
) -> Result<ResolvedToolchain, TexeError> {
    install(descriptor, None, verification)?;
    let runtime = runtime_root(descriptor)?;
    let engine = text(descriptor, "engine")?;
    let executable = executable(&runtime, descriptor)?;
    super::verify_engine(descriptor, &executable)?;
    Ok(ResolvedToolchain {
        provider: "portable".into(),
        engine: engine.into(),
        engine_executable: executable,
        kpsewhich_executable: PathBuf::new(),
        texmf_dist: root.join(&manifest.packages.texmf),
        engine_roots: vec![runtime],
        identity: ToolchainIdentity {
            provider: "portable".into(),
            engine: engine.into(),
            channel: text(&descriptor["profile"], "id")?.into(),
            target: text(&descriptor["runtime"], "target")?.into(),
            fingerprint: text(&descriptor["runtime"], "sha256")?.into(),
            registry_url: None,
            registry_metadata_digest: None,
            artifacts: vec![],
        },
        managed: None,
        verification,
        offline,
    })
}
pub(super) fn prepared_biber(
    descriptor: &Value,
    verification: VerificationPolicy,
    offline: bool,
) -> Result<Option<ManagedBiber>, TexeError> {
    if descriptor["engine"] != "tectonic" || !super::requires_biber(descriptor) {
        return Ok(None);
    }
    crate::toolchain::ensure_biber_for_snapshot(
        text(&descriptor["profile"], "runtimeRecipe")?,
        verification,
        offline,
    )
    .map(Some)
}

fn add_biber_environment(
    environment: &mut Vec<(OsString, OsString)>,
    biber: &ManagedBiber,
    runtime: &Path,
    cache: &Path,
) -> Result<(), TexeError> {
    let binary_dir = biber
        .executable
        .parent()
        .ok_or_else(|| error("The prepared bibliography tool has no binary directory"))?;
    let path = std::env::join_paths([binary_dir, runtime.join("bin").as_path()])
        .map_err(|_| error("The bibliography tool path cannot be represented in PATH"))?;
    environment.push(("PATH".into(), path));
    environment.push((
        "LD_LIBRARY_PATH".into(),
        biber.library_dir.as_os_str().into(),
    ));
    environment.push(("PAR_GLOBAL_TMPDIR".into(), cache.as_os_str().into()));
    environment.push(("PAR_GLOBAL_TEMP".into(), cache.as_os_str().into()));
    Ok(())
}

fn prepare_tectonic_formats(
    root: &Path,
    output_relative: &Path,
    runtime: &Path,
) -> Result<(), TexeError> {
    let sealed = runtime.join("format-cache/formats");
    match fs::symlink_metadata(&sealed) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io(&sealed, source)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(error("The sealed Tectonic format cache is unsafe")),
    }
    let relative = output_relative.join(".tectonic-cache/formats");
    super::private_directory(root, &relative)?;
    let destination = root.join(relative);
    for entry in fs::read_dir(&sealed).map_err(|source| io(&sealed, source))? {
        let entry = entry.map_err(|source| io(&sealed, source))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| io(&path, source))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || path.extension().is_none_or(|extension| extension != "fmt")
            || metadata.len() > 128 * 1024 * 1024
        {
            return Err(error(
                "The sealed Tectonic format cache contains an unsafe file",
            ));
        }
        // Atomic replacement also removes a hostile destination symlink or
        // hardlink without changing its target. Only the private copy can be
        // regenerated by the compiler; the shared runtime stays read-only.
        let bytes = fs::read(&path).map_err(|source| io(&path, source))?;
        crate::build::generated::write(root, &destination.join(entry.file_name()), &bytes)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)] // Native builds share the selected project, tools and publication context.
pub(crate) fn build(
    root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    tools: &PqtyClient,
    descriptor: &Value,
    _options: BuildOptions,
    progress: &Progress,
    publish: bool,
) -> Result<BuildReport, TexeError> {
    if toolchain.engine == "tectonic"
        && (!manifest.project.generated.is_empty()
            || !manifest.inputs.roots.is_empty()
            || !manifest.bibliography.roots.is_empty())
    {
        return Err(error(
            "The offline Tectonic runtime cannot apply this project's custom input roots or generated files in its protected mode. Use Shipout or a TeX Live engine for this project. The source and last PDF have been kept.",
        ));
    }
    super::private_directory(root, &manifest.project.build_dir)?;
    let _guard = crate::guard::BuildGuard::acquire(&root.join(&manifest.project.build_dir))?;
    let rendered_sources = crate::build::capture_render_sources(root, manifest)?;
    let started = std::time::Instant::now();
    crate::build::prepare_portable(root, manifest, toolchain, tools, descriptor, progress)?;
    let output_relative = manifest.project.build_dir.join("output");
    super::private_directory(root, &output_relative)?;
    let output_dir = root.join(&output_relative);
    if toolchain.engine == "shipout" {
        super::private_directory(root, &manifest.project.build_dir.join("formats"))?;
    } else {
        super::private_directory(root, &output_relative.join(".cache"))?;
        super::private_directory(root, &output_relative.join(".tectonic-cache"))?;
    }
    crate::build::materialize_portable_generated(&output_dir, manifest)?;
    let source = super::entry_for_build(root, manifest, &output_dir, Some(descriptor))?;
    let output = output_dir
        .join(manifest.project.entry.file_name().unwrap())
        .with_extension("pdf");
    for artifact in [&output, &output.with_extension("synctex.gz")] {
        crate::build::generated::remove(root, artifact)?;
    }
    let runtime = runtime_root(descriptor)?;
    if toolchain.engine == "tectonic" {
        prepare_tectonic_formats(root, &output_relative, &runtime)?;
    }
    let args: Vec<OsString> = if toolchain.engine == "shipout" {
        let input_roots = std::iter::once(output_dir.join(".texe-generated"))
            .chain(manifest.inputs.roots.iter().map(|path| root.join(path)))
            .chain(
                manifest
                    .bibliography
                    .roots
                    .iter()
                    .map(|path| root.join(path)),
            )
            .collect::<Vec<_>>();
        let input_roots =
            serde_json::to_string(&input_roots).map_err(|_| error("Invalid native input roots"))?;
        vec![
            source.as_os_str().into(),
            "--pdf".into(),
            output.as_os_str().into(),
            "--texmf".into(),
            root.join(&manifest.packages.texmf).into_os_string(),
            "--project-root".into(),
            root.as_os_str().into(),
            "--source-root".into(),
            root.join(&manifest.project.entry)
                .parent()
                .unwrap()
                .as_os_str()
                .into(),
            "--input-roots-json".into(),
            input_roots.into(),
            "--format-cache".into(),
            root.join(&manifest.project.build_dir)
                .join("formats")
                .into_os_string(),
        ]
    } else {
        vec![
            "--color".into(),
            "never".into(),
            "--synctex".into(),
            "--keep-logs".into(),
            "--keep-intermediates".into(),
            "--bundle".into(),
            runtime.join("bundle").into_os_string(),
            "--only-cached".into(),
            "--untrusted".into(),
            "--outdir".into(),
            output_dir.as_os_str().into(),
            source.into_os_string(),
        ]
    };
    let mut environment = vec![
        (
            "SHIPOUT_ASSETS_ROOT".into(),
            runtime.join("assets/shipout-core").into_os_string(),
        ),
        (
            "SOURCE_DATE_EPOCH".into(),
            descriptor["source_date_epoch"]
                .as_u64()
                .unwrap()
                .to_string()
                .into(),
        ),
        (
            "TECTONIC_CACHE_DIR".into(),
            output_dir.join(".tectonic-cache").into_os_string(),
        ),
        (
            "XDG_CACHE_HOME".into(),
            output_dir.join(".cache").into_os_string(),
        ),
    ];
    environment.extend(crate::compiler_sandbox::font_environment(
        root,
        &output_dir,
        &[
            runtime.join("bundle"),
            root.join(&manifest.packages.texmf).join("fonts"),
        ],
    )?);
    let biber = prepared_biber(descriptor, toolchain.verification, toolchain.offline)?;
    if let Some(biber) = &biber {
        let cache_relative = output_relative.join(".biber-cache");
        super::private_directory(root, &cache_relative)?;
        add_biber_environment(
            &mut environment,
            biber,
            &runtime,
            &root.join(&cache_relative),
        )?;
        if !publish {
            // Readiness includes launching the pinned processor and warming
            // its private PAR cache, even when this source has no citations yet.
            // Previous untrusted jobs can write this cache. A trusted launch
            // must never execute those old unpacked helper binaries.
            remove_cache(&root.join(&cache_relative))?;
            super::private_directory(root, &cache_relative)?;
            let cache = root.join(&cache_relative);
            let mut warm = crate::process::command(&biber.executable);
            warm.env_clear()
                .args(["--noconf", "--version"])
                .current_dir(&cache);
            for (name, value) in &environment {
                warm.env(name, value);
            }
            for name in ["HOME", "TMPDIR", "TEMP", "TMP"] {
                warm.env(name, &cache);
            }
            let warmed = warm.output().map_err(|source| TexeError::Spawn {
                tool: biber.executable.clone(),
                source,
            })?;
            if !warmed.status.success() {
                return Err(error(format!(
                    "The project's bibliography tool could not be prepared. The last environment and PDF have been kept. {}",
                    String::from_utf8_lossy(&warmed.stderr)
                )));
            }
            crate::compiler_sandbox::prepare_biber_native_libraries(root, &cache)?;
        }
    }
    let working_directory = if toolchain.engine == "shipout" {
        &output_dir
    } else {
        root
    };
    let mut read_roots = vec![runtime.clone()];
    let mut execute_roots = vec![runtime.join("bin")];
    if let Some(biber) = &biber {
        read_roots.push(biber.library_dir.clone());
        execute_roots.push(biber.executable.parent().unwrap().to_owned());
        execute_roots.push(root.join(&output_relative).join(".biber-cache"));
    }
    let policy = crate::compiler_sandbox::Policy::new(
        root,
        &[root.join(&manifest.project.build_dir)],
        &read_roots,
        &execute_roots,
    )?
    .with_shipout_arena(toolchain.engine == "shipout");
    let result = crate::compiler_sandbox::run(
        &toolchain.engine_executable,
        &args,
        working_directory,
        &environment,
        &policy,
    )?;
    crate::build::generated::validate_parent(root, &output.with_extension("log"))?;
    crate::build::generated::write(
        root,
        &output.with_extension("log"),
        &[result.stdout.as_slice(), result.stderr.as_slice()].concat(),
    )?;
    let regular_pdf = crate::build::generated::read(
        root,
        &output_dir,
        &output,
        crate::build::generated::MAX_ARTIFACT,
    )
    .is_ok_and(|bytes| crate::build::generated::valid_pdf(&bytes));
    if result.status.success() && regular_pdf {
        crate::build::generated::optional(
            root,
            &output_dir,
            &output.with_extension("synctex.gz"),
            crate::build::generated::MAX_ARTIFACT,
        )?;
    }
    if !result.status.success() || !regular_pdf {
        return Err(error(format!(
            "{} build failed. The last PDF has been kept. {}",
            toolchain.engine,
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    let lock = serde_json::to_vec(descriptor).map_err(|_| error("Invalid portable environment"))?;
    let published = if publish {
        crate::build::publish_if_sources_unchanged(root, manifest, &rendered_sources, || {
            crate::build::publish_native(
                root,
                &output,
                &lock,
                &root
                    .join(&manifest.project.build_dir)
                    .join("execution.lock"),
            )
        })?
    } else {
        crate::build::verify_render_sources(root, manifest, &rendered_sources)?;
        output
    };
    Ok(BuildReport {
        schema: "texe.build-report/v1".into(),
        engine: toolchain.engine.clone(),
        artifact: published,
        engine_passes: 1,
        bibliography_runs: 0,
        index_runs: 0,
        convergence_rounds: 0,
        environment_fingerprint: text(descriptor, "id")?.into(),
        cached: false,
        duration_millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        warning_count: 0,
        warnings: vec![],
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use serde_json::json;

    fn capsule(directory: &Path) -> (PathBuf, Value) {
        let encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        for (name, contents) in [
            (
                "bin/citet-shipout",
                &b"#!/bin/sh\necho 'shipout/test-native'\n"[..],
            ),
            (
                "assets/shipout-core/tex/kernel.tex",
                &b"kernel contents"[..],
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, name, contents)
                .expect("archive entry");
        }
        let bytes = archive
            .into_inner()
            .expect("tar terminator")
            .finish()
            .expect("gzip footer");
        let path = directory.join("runtime.tar.gz");
        fs::write(&path, &bytes).expect("runtime archive");
        let descriptor = json!({
            "engine": "shipout",
            "profile": { "engines": { "shipout": { "version": "shipout/test-native" } } },
            "runtime": { "sha256": crate::portable::digest(&bytes), "size": bytes.len(), "target": target_name() },
        });
        (path, descriptor)
    }
    fn cache(directory: &Path, descriptor: &Value) -> PathBuf {
        directory
            .join("native")
            .join(descriptor["runtime"]["sha256"].as_str().unwrap())
    }
    fn prepared() -> (tempfile::TempDir, PathBuf, Value) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (archive, descriptor) = capsule(directory.path());
        let root = cache(directory.path(), &descriptor);
        install_at(
            &root,
            &descriptor,
            Some(&archive),
            VerificationPolicy::Interval,
        )
        .expect("prepare native cache");
        (directory, root, descriptor)
    }

    #[test]
    fn oversized_raw_path_metadata_is_rejected_before_reading_its_body() {
        for kind in [tar::EntryType::XHeader, tar::EntryType::GNULongName] {
            let (directory, root, descriptor) = prepared();
            fs::write(directory.path().join("main.tex"), b"research source").unwrap();
            fs::write(directory.path().join("main.pdf"), b"last PDF").unwrap();
            let previous = fs::read(root.join(READY)).unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_path(".path-metadata").unwrap();
            header.set_size(1024 * 1024 * 1024);
            header.set_cksum();
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(header.as_bytes()).unwrap();
            // Deliberately no body: rejecting its declared size must precede
            // decompression/allocation or a missing-body error.
            let bytes = encoder.finish().unwrap();
            let archive = directory.path().join("oversized.tar.gz");
            fs::write(&archive, &bytes).unwrap();
            let mut bad = descriptor.clone();
            bad["runtime"]["sha256"] = crate::portable::digest(&bytes).into();
            bad["runtime"]["size"] = bytes.len().into();
            let failure =
                install_at(&root, &bad, Some(&archive), VerificationPolicy::Interval).unwrap_err();
            assert!(
                failure
                    .to_string()
                    .contains("path metadata exceeds its safe limit"),
                "{failure}"
            );
            assert_eq!(fs::read(root.join(READY)).unwrap(), previous);
            assert_eq!(
                fs::read(directory.path().join("main.tex")).unwrap(),
                b"research source"
            );
            assert_eq!(
                fs::read(directory.path().join("main.pdf")).unwrap(),
                b"last PDF"
            );
            verify_cache(&root, &descriptor, VerificationPolicy::Deep).unwrap();
        }
    }

    #[test]
    fn bounded_pax_and_gnu_long_paths_remain_supported() {
        for kind in [tar::EntryType::XHeader, tar::EntryType::GNULongName] {
            let directory = tempfile::tempdir().unwrap();
            let (original, mut descriptor) = capsule(directory.path());
            let mut base = tar::Archive::new(GzDecoder::new(BufReader::new(
                fs::File::open(original).unwrap(),
            )));
            let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
            for entry in base.entries().unwrap() {
                let mut entry = entry.unwrap();
                let header = entry.header().clone();
                archive.append(&header, &mut entry).unwrap();
            }
            let name = format!("assets/{}/long font name.otf", "font-source-".repeat(12));
            let body = if kind.is_gnu_longname() {
                let mut bytes = name.as_bytes().to_vec();
                bytes.push(0);
                bytes
            } else {
                let record = format!(" path={name}\n");
                let mut size = record.len() + 1;
                while size != record.len() + size.to_string().len() {
                    size = record.len() + size.to_string().len();
                }
                format!("{size}{record}").into_bytes()
            };
            let mut extension = tar::Header::new_gnu();
            extension.set_entry_type(kind);
            extension.set_path(".path-metadata").unwrap();
            extension.set_size(body.len() as u64);
            extension.set_cksum();
            archive.append(&extension, body.as_slice()).unwrap();
            let bytes = b"sealed font";
            let mut header = tar::Header::new_gnu();
            header.set_path("assets/short-name.otf").unwrap();
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            archive.append(&header, bytes.as_slice()).unwrap();
            let bytes = archive.into_inner().unwrap().finish().unwrap();
            let path = directory.path().join("long-path.tar.gz");
            fs::write(&path, &bytes).unwrap();
            descriptor["runtime"]["sha256"] = crate::portable::digest(&bytes).into();
            descriptor["runtime"]["size"] = bytes.len().into();
            let root = cache(directory.path(), &descriptor);
            install_at(
                &root,
                &descriptor,
                Some(&path),
                VerificationPolicy::Interval,
            )
            .unwrap();
            assert_eq!(fs::read(root.join(name)).unwrap(), b"sealed font");
            verify_cache(&root, &descriptor, VerificationPolicy::Deep).unwrap();
        }
    }

    #[test]
    fn sealed_tectonic_formats_use_private_copies_and_keep_research_unchanged() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let runtime = fixture.path().join("runtime");
        let sealed = runtime.join("format-cache/formats");
        fs::create_dir_all(&sealed).unwrap();
        fs::write(sealed.join("latex-pinned.fmt"), b"trusted format").unwrap();
        let output_relative = Path::new(".texe/environments/test/build/output");
        let private = root.join(output_relative).join(".tectonic-cache/formats");
        fs::create_dir_all(&private).unwrap();
        let research = root.join("main.tex");
        fs::write(&research, b"research source").unwrap();
        std::os::unix::fs::symlink(&research, private.join("latex-pinned.fmt")).unwrap();

        prepare_tectonic_formats(&root, output_relative, &runtime).unwrap();
        assert_eq!(fs::read(&research).unwrap(), b"research source");
        assert_eq!(
            fs::read(private.join("latex-pinned.fmt")).unwrap(),
            b"trusted format"
        );
        assert!(
            !fs::symlink_metadata(private.join("latex-pinned.fmt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::write(
            private.join("latex-pinned.fmt"),
            b"document-modified format",
        )
        .unwrap();
        assert_eq!(
            fs::read(sealed.join("latex-pinned.fmt")).unwrap(),
            b"trusted format"
        );
        prepare_tectonic_formats(&root, output_relative, &runtime).unwrap();
        assert_eq!(
            fs::read(private.join("latex-pinned.fmt")).unwrap(),
            b"trusted format"
        );
    }

    #[test]
    fn a_linked_private_tectonic_format_directory_cannot_publish_into_sources() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let runtime = fixture.path().join("runtime");
        let sealed = runtime.join("format-cache/formats");
        fs::create_dir_all(&sealed).unwrap();
        fs::write(sealed.join("latex-pinned.fmt"), b"trusted format").unwrap();
        let output_relative = Path::new(".texe/environments/test/build/output");
        let cache = root.join(output_relative).join(".tectonic-cache");
        fs::create_dir_all(&cache).unwrap();
        let sources = root.join("research");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("notes.tex"), b"research source").unwrap();
        std::os::unix::fs::symlink(&sources, cache.join("formats")).unwrap();
        assert!(prepare_tectonic_formats(&root, output_relative, &runtime).is_err());
        assert_eq!(
            fs::read(sources.join("notes.tex")).unwrap(),
            b"research source"
        );
        assert!(!sources.join("latex-pinned.fmt").exists());
    }

    #[test]
    fn native_archives_allow_only_the_sealed_tectonic_format_subtree() {
        for name in [
            "format-cache",
            "format-cache/formats",
            "format-cache/formats/latex-pinned.fmt",
        ] {
            assert!(valid_path(Path::new(name)), "{name}");
        }
        for name in [
            "format-cache/other",
            "format-cache/../source.tex",
            "format-cache/formats/../../source.tex",
            "other/formats/latex.fmt",
        ] {
            assert!(!valid_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn native_bibliography_prefetch_is_unused_without_tectonic_biblatex() {
        for descriptor in [
            json!({"engine":"shipout", "packages":{"closure":[{"provider":"biblatex"}]}}),
            json!({"engine":"tectonic", "packages":{"closure":[{"provider":"latex"}]}}),
        ] {
            assert!(
                prepared_biber(&descriptor, VerificationPolicy::Deep, true)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn tectonic_receives_only_pinned_bibliography_paths_and_private_par_cache() {
        let biber = ManagedBiber {
            executable: PathBuf::from("/data/pinned-biber/bin/biber"),
            library_dir: PathBuf::from("/data/pinned-biber/lib"),
            cache_dir: PathBuf::from("/data/shared-unused-par-cache"),
        };
        let mut environment = Vec::new();
        add_biber_environment(
            &mut environment,
            &biber,
            Path::new("/data/native"),
            Path::new("/project/.texe/build/output/.biber-cache"),
        )
        .unwrap();
        let environment = environment.into_iter().collect::<BTreeMap<_, _>>();
        assert_eq!(
            environment[&OsString::from("PATH")],
            OsString::from("/data/pinned-biber/bin:/data/native/bin")
        );
        assert_eq!(
            environment[&OsString::from("LD_LIBRARY_PATH")],
            OsString::from("/data/pinned-biber/lib")
        );
        assert_eq!(
            environment[&OsString::from("PAR_GLOBAL_TMPDIR")],
            OsString::from("/project/.texe/build/output/.biber-cache")
        );
        assert_eq!(
            environment[&OsString::from("PAR_GLOBAL_TEMP")],
            environment[&OsString::from("PAR_GLOBAL_TMPDIR")]
        );
    }

    #[test]
    fn changed_files_missing_files_and_extra_files_repair_from_the_verified_archive() {
        let (_directory, root, descriptor) = prepared();
        let asset = root.join("assets/shipout-core/tex/kernel.tex");
        let original = fs::read(&asset).expect("original asset");
        fs::write(&asset, b"corrupt content").expect("damage an asset");
        assert!(verify_cache(&root, &descriptor, VerificationPolicy::Interval).is_err());
        install_at(&root, &descriptor, None, VerificationPolicy::Interval)
            .expect("repair without a download");
        assert_eq!(fs::read(&asset).unwrap(), original);
        fs::remove_file(&asset).expect("remove an asset");
        install_at(&root, &descriptor, None, VerificationPolicy::Interval)
            .expect("restore missing asset");
        fs::write(root.join("assets/foreign.tex"), b"unexpected").expect("extra asset");
        assert!(verify_cache(&root, &descriptor, VerificationPolicy::Interval).is_err());
        install_at(&root, &descriptor, None, VerificationPolicy::Interval)
            .expect("remove unexpected asset by replacement");
        assert!(!root.join("assets/foreign.tex").exists());
        verify_cache(&root, &descriptor, VerificationPolicy::Deep)
            .expect("fully verified repaired runtime");
    }

    #[test]
    fn deep_verification_hashes_files_even_when_the_recorded_metadata_matches() {
        let (_directory, root, descriptor) = prepared();
        let name = "assets/shipout-core/tex/kernel.tex";
        let path = root.join(name);
        fs::write(&path, b"corrupt content").expect("damage an asset");
        fs::File::open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let mut index: RuntimeIndex =
            serde_json::from_slice(&fs::read(root.join(READY)).unwrap()).unwrap();
        index.files.get_mut(name).unwrap().metadata =
            FileMetadata::from(&fs::metadata(&path).unwrap());
        write_index(&root, &index).expect("record the same metadata");
        verify_cache(&root, &descriptor, VerificationPolicy::Interval).expect("metadata fast path");
        assert!(verify_cache(&root, &descriptor, VerificationPolicy::Deep).is_err());
    }

    #[test]
    fn symlinked_assets_are_replaced_without_changing_their_target() {
        let (directory, root, descriptor) = prepared();
        let outside = directory.path().join("research.tex");
        fs::write(&outside, b"research source").unwrap();
        let path = root.join("assets/shipout-core/tex/kernel.tex");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(verify_cache(&root, &descriptor, VerificationPolicy::Interval).is_err());
        install_at(&root, &descriptor, None, VerificationPolicy::Interval)
            .expect("repair a symlinked cache");
        assert_eq!(fs::read(&outside).unwrap(), b"research source");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_truncated_gzip_footer_keeps_the_prior_cache_and_published_work() {
        let (directory, root, descriptor) = prepared();
        let source = directory.path().join("main.tex");
        let pdf = directory.path().join("main.pdf");
        fs::write(&source, b"research source").unwrap();
        fs::write(&pdf, b"last published PDF").unwrap();
        let prior_index = fs::read(root.join(READY)).unwrap();
        let mut bytes = fs::read(directory.path().join("runtime.tar.gz")).unwrap();
        bytes.truncate(bytes.len() - 4);
        let truncated = directory.path().join("truncated.tar.gz");
        fs::write(&truncated, &bytes).unwrap();
        let mut bad_descriptor = descriptor.clone();
        bad_descriptor["runtime"]["sha256"] = crate::portable::digest(&bytes).into();
        bad_descriptor["runtime"]["size"] = bytes.len().into();
        assert!(
            install_at(
                &root,
                &bad_descriptor,
                Some(&truncated),
                VerificationPolicy::Interval
            )
            .is_err()
        );
        assert_eq!(fs::read(root.join(READY)).unwrap(), prior_index);
        assert_eq!(fs::read(&source).unwrap(), b"research source");
        assert_eq!(fs::read(&pdf).unwrap(), b"last published PDF");
        verify_cache(&root, &descriptor, VerificationPolicy::Deep)
            .expect("old runtime remains intact");
    }

    #[test]
    fn a_corrupt_repair_archive_keeps_the_existing_cache() {
        let (_directory, root, descriptor) = prepared();
        let asset = root.join("assets/shipout-core/tex/kernel.tex");
        fs::write(&asset, b"damaged content").unwrap();
        let archive = root.parent().unwrap().join(".archives").join(format!(
            "{}.tar.gz",
            descriptor["runtime"]["sha256"].as_str().unwrap()
        ));
        fs::write(&archive, b"corrupt archive").unwrap();
        let old_index = fs::read(root.join(READY)).unwrap();
        assert!(install_at(&root, &descriptor, None, VerificationPolicy::Interval).is_err());
        assert_eq!(fs::read(&asset).unwrap(), b"damaged content");
        assert_eq!(fs::read(root.join(READY)).unwrap(), old_index);
    }

    #[test]
    fn an_unexecutable_engine_repairs_from_its_verified_archive() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_directory, root, descriptor) = prepared();
        let engine = root.join("bin/citet-shipout");
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(verify_cache(&root, &descriptor, VerificationPolicy::Interval).is_err());
        install_at(&root, &descriptor, None, VerificationPolicy::Interval)
            .expect("restore the executable");
        super::super::verify_engine(&descriptor, &engine).expect("the repaired engine runs");
    }

    #[test]
    fn appended_compressed_bytes_do_not_publish_a_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let (archive, mut descriptor) = capsule(directory.path());
        let mut bytes = fs::read(&archive).unwrap();
        bytes.extend_from_slice(b"unrecognized compressed data");
        fs::write(&archive, &bytes).unwrap();
        descriptor["runtime"]["sha256"] = crate::portable::digest(&bytes).into();
        descriptor["runtime"]["size"] = bytes.len().into();
        let root = cache(directory.path(), &descriptor);
        assert!(
            install_at(
                &root,
                &descriptor,
                Some(&archive),
                VerificationPolicy::Interval
            )
            .is_err()
        );
        assert!(!root.exists());
    }

    #[test]
    fn an_interrupted_cache_swap_recovers_the_prior_verified_directory() {
        let (_directory, root, descriptor) = prepared();
        let backup = root.parent().unwrap().join(format!(
            ".previous-{}",
            descriptor["runtime"]["sha256"].as_str().unwrap()
        ));
        fs::rename(&root, &backup).unwrap();
        install_at(&root, &descriptor, None, VerificationPolicy::Interval)
            .expect("recover an interrupted rename");
        assert!(!backup.exists());
        verify_cache(&root, &descriptor, VerificationPolicy::Deep).expect("recovered runtime");
    }
}
