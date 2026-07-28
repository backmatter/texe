use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha512};
use xz2::read::XzDecoder;
use xz2::stream::Stream;

use crate::TexeError;
use crate::toolchain::catalog::ManagedArtifact;
use crate::toolchain::{CONNECT_TIMEOUT, DOWNLOAD_ATTEMPTS, READ_TIMEOUT, RETRY_BACKOFF};

/// The dated TeX Live archive admits standard download-client identities
/// without a browser challenge. Keep texe identifiable while retaining the
/// compatibility product that the archive expects from automated downloads.
const SNAPSHOT_USER_AGENT: &str = concat!(
    "Wget/1.21.4 (compatible; texe/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/backmatter/texe)"
);
const MAX_ARCHIVE_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_XZ_DECODER_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;

#[derive(Debug, thiserror::Error)]
enum FetchFailure {
    #[error("{url}: {source}")]
    Request {
        url: String,
        #[source]
        source: ureq::Error,
    },
    #[error("{url}: {source}")]
    Body {
        url: String,
        #[source]
        source: io::Error,
    },
    #[error("{url}: {source}")]
    Verification {
        url: String,
        #[source]
        source: Box<TexeError>,
    },
}

impl FetchFailure {
    const fn is_network(&self) -> bool {
        matches!(self, Self::Request { .. } | Self::Body { .. })
    }
}

#[derive(Debug)]
struct DownloadFailures(Vec<FetchFailure>);

impl fmt::Display for DownloadFailures {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, failure) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{failure}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DownloadFailures {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0
            .first()
            .map(|failure| failure as &(dyn std::error::Error + 'static))
    }
}

pub(super) fn download_artifact(
    downloads: &Path,
    artifact: &ManagedArtifact,
    sources: &[String],
    offline: bool,
) -> Result<PathBuf, TexeError> {
    let path = downloads.join(format!("{}.tar.xz", artifact.sha512));
    if path.is_file() {
        verify_archive(&path, artifact)?;
        return Ok(path);
    }
    if offline {
        return Err(TexeError::Toolchain(format!(
            "offline mode requires the cached runtime component {} at {}",
            artifact.provider,
            path.display()
        )));
    }

    let bytes = fetch_artifact(artifact, sources)?;

    let temporary = downloads.join(format!(".{}.{}.tmp", artifact.sha512, std::process::id()));
    let result = (|| {
        write_new_file(&temporary, &bytes, None)?;
        match fs::rename(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(_) if path.is_file() => {
                fs::remove_file(&temporary).map_err(|source| TexeError::Io {
                    path: temporary.clone(),
                    source,
                })?;
                verify_archive(&path, artifact)
            }
            Err(source) => Err(TexeError::Io {
                path: path.clone(),
                source,
            }),
        }
    })();
    if result.is_err() && temporary.is_file() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| path)
}

/// Fetch one container, trying every equivalent snapshot source in turn and
/// sweeping the list again a few times before giving up. Every response is
/// verified before it is returned, so a source serving the wrong bytes is
/// simply another failure to move past.
fn fetch_artifact(artifact: &ManagedArtifact, sources: &[String]) -> Result<Vec<u8>, TexeError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(SNAPSHOT_USER_AGENT)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_body(Some(READ_TIMEOUT))
        .build()
        .into();
    let mut failures: Vec<FetchFailure> = Vec::new();
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_BACKOFF * 2u32.pow(attempt - 1));
        }
        for source in sources {
            let url = format!("{source}/archive/{}.tar.xz", artifact.provider);
            match fetch_verified(&agent, &url, artifact) {
                Ok(bytes) => return Ok(bytes),
                Err(failure)
                    if !failures
                        .iter()
                        .any(|existing| existing.to_string() == failure.to_string()) =>
                {
                    failures.push(failure);
                }
                Err(_) => {}
            }
        }
    }
    Err(exhausted_download_error(artifact, failures))
}

fn exhausted_download_error(artifact: &ManagedArtifact, failures: Vec<FetchFailure>) -> TexeError {
    let details = failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n  ");
    let message = format!(
        "could not download runtime component {} from the pinned snapshot sources:\n  {}",
        artifact.provider, details
    );
    if !failures.is_empty() && failures.iter().all(FetchFailure::is_network) {
        TexeError::Network {
            message,
            source: Box::new(DownloadFailures(failures)),
        }
    } else {
        TexeError::Toolchain(message)
    }
}

/// Read at most one byte more than the recipe says the container holds. Read
/// to end instead and a stalled or hostile mirror can exhaust memory long
/// before the size check gets a chance to reject what it sent.
///
/// Failures come back as finished lines naming the source they describe, since
/// the caller reports them together.
fn fetch_verified(
    agent: &ureq::Agent,
    url: &str,
    artifact: &ManagedArtifact,
) -> Result<Vec<u8>, FetchFailure> {
    let response = agent
        .get(url)
        .call()
        .map_err(|source| FetchFailure::Request {
            url: url.to_string(),
            source,
        })?;
    let capacity = usize::try_from(artifact.size).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    response
        .into_body()
        .into_reader()
        .take(artifact.size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| FetchFailure::Body {
            url: url.to_string(),
            source,
        })?;
    verify_archive_bytes(&bytes, artifact).map_err(|source| FetchFailure::Verification {
        url: url.to_string(),
        source: Box::new(source),
    })?;
    Ok(bytes)
}

fn verify_archive(path: &Path, artifact: &ManagedArtifact) -> Result<(), TexeError> {
    let metadata = fs::metadata(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() != artifact.size {
        return Err(TexeError::Toolchain(format!(
            "runtime component {} has size {}, expected {}",
            artifact.provider,
            metadata.len(),
            artifact.size
        )));
    }
    let file = fs::File::open(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file).take(artifact.size.saturating_add(1));
    let mut hasher = Sha512::new();
    let mut observed = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        observed = observed.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    if observed != artifact.size || hex::encode(hasher.finalize()) != artifact.sha512 {
        return Err(TexeError::Toolchain(format!(
            "runtime component {} failed SHA-512 verification",
            artifact.provider
        )));
    }
    Ok(())
}

fn verify_archive_bytes(bytes: &[u8], artifact: &ManagedArtifact) -> Result<(), TexeError> {
    if u64::try_from(bytes.len()).ok() != Some(artifact.size) {
        return Err(TexeError::Toolchain(format!(
            "runtime component {} has size {}, expected {}",
            artifact.provider,
            bytes.len(),
            artifact.size
        )));
    }
    let actual = hex::encode(Sha512::digest(bytes));
    if actual != artifact.sha512 {
        return Err(TexeError::Toolchain(format!(
            "runtime component {} failed SHA-512 verification",
            artifact.provider
        )));
    }
    Ok(())
}

pub(super) fn extract_archive(path: &Path, destination: &Path) -> Result<(), TexeError> {
    let file = fs::File::open(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let compressed_size = file
        .metadata()
        .map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if compressed_size > MAX_ARCHIVE_COMPRESSED_BYTES {
        return Err(TexeError::Toolchain(format!(
            "runtime archive {} exceeds the {} MiB compressed-size limit",
            path.display(),
            MAX_ARCHIVE_COMPRESSED_BYTES / (1024 * 1024)
        )));
    }
    let stream = Stream::new_stream_decoder(MAX_XZ_DECODER_MEMORY_BYTES, 0).map_err(|error| {
        TexeError::Toolchain(format!(
            "could not initialize the XZ decoder for {}: {error}",
            path.display()
        ))
    })?;
    let compressed = BufReader::new(file.take(MAX_ARCHIVE_COMPRESSED_BYTES.saturating_add(1)));
    let decoder = XzDecoder::new_stream(compressed, stream);
    let expanded = BoundedReader::new(decoder, MAX_ARCHIVE_EXPANDED_BYTES);
    let mut archive = tar::Archive::new(expanded);
    let entries = archive.entries().map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(TexeError::Toolchain(format!(
                "runtime archive contains more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        let mut entry = entry.map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let relative = entry.path().map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(TexeError::Toolchain(format!(
                "runtime archive contains unsafe path: {}",
                relative.display()
            )));
        }
        let output = destination.join(relative.as_ref());
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|source| TexeError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mode = entry.header().mode().ok();
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output)
            .map_err(|source| TexeError::Io {
                path: output.clone(),
                source,
            })?;
        std::io::copy(&mut entry, &mut file).map_err(|source| TexeError::Io {
            path: output.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| TexeError::Io {
            path: output.clone(),
            source,
        })?;
        #[cfg(unix)]
        apply_archive_mode(&output, mode)?;
        #[cfg(not(unix))]
        apply_archive_mode(&output, mode);
    }
    let mut expanded = archive.into_inner();
    io::copy(&mut expanded, &mut io::sink()).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

struct BoundedReader<R> {
    inner: R,
    remaining: u64,
    limit: u64,
}

impl<R> BoundedReader<R> {
    const fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
            limit,
        }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!(
                    "expanded archive exceeds configured limit of {} MiB",
                    self.limit / (1024 * 1024)
                ))),
            };
        }
        let allowed = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining = self
            .remaining
            .saturating_sub(u64::try_from(read).expect("a single read count always fits in u64"));
        Ok(read)
    }
}

pub(super) fn write_new_file(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> Result<(), TexeError> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    apply_archive_mode(path, mode)?;
    #[cfg(not(unix))]
    apply_archive_mode(path, mode);
    Ok(())
}

#[cfg(unix)]
fn apply_archive_mode(path: &Path, mode: Option<u32>) -> Result<(), TexeError> {
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
            TexeError::Io {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_archive_mode(_path: &Path, _mode: Option<u32>) {}

pub(super) fn remove_staging(path: &Path) -> Result<(), TexeError> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if !name.starts_with(".runtime-")
        || !Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
    {
        return Err(TexeError::Toolchain(format!(
            "refusing to remove unexpected staging path: {}",
            path.display()
        )));
    }
    fs::remove_dir_all(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(super) fn remove_component_staging(path: &Path) -> Result<(), TexeError> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if !name.starts_with(".biber-")
        || !Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
    {
        return Err(TexeError::Toolchain(format!(
            "refusing to remove unexpected component staging path: {}",
            path.display()
        )));
    }
    fs::remove_dir_all(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::fs;
    use std::io::{Cursor, Read as _};

    use xz2::write::XzEncoder;

    use crate::TexeError;
    use crate::toolchain::artifact::{
        BoundedReader, FetchFailure, exhausted_download_error, extract_archive,
    };
    use crate::toolchain::catalog::ManagedArtifact;
    use crate::ux::ErrorCategory;

    #[test]
    fn expanded_archive_reader_rejects_bytes_over_its_limit() {
        let mut reader = BoundedReader::new(Cursor::new(b"four!"), 4);
        let mut bytes = Vec::new();
        let error = reader.read_to_end(&mut bytes).expect_err("over limit");
        assert!(error.to_string().contains("configured limit"));
        assert_eq!(bytes, b"four");
    }

    #[test]
    fn expanded_archive_reader_accepts_exact_limit() {
        let mut reader = BoundedReader::new(Cursor::new(b"four"), 4);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).expect("within limit");
        assert_eq!(bytes, b"four");
    }

    #[test]
    fn streamed_xz_archive_extracts_without_an_expanded_buffer() {
        let scratch = tempfile::tempdir().expect("temporary directory");
        let archive_path = scratch.path().join("runtime.tar.xz");
        let output = scratch.path().join("output");
        let encoder = XzEncoder::new(fs::File::create(&archive_path).expect("archive file"), 6);
        let mut archive = tar::Builder::new(encoder);
        let contents = b"runtime";
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_mtime(0);
        header.set_size(u64::try_from(contents.len()).expect("content size"));
        header.set_cksum();
        archive
            .append_data(&mut header, "bin/tool", contents.as_slice())
            .expect("archive entry");
        archive
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish XZ");

        extract_archive(&archive_path, &output).expect("extract archive");
        assert_eq!(
            fs::read(output.join("bin/tool")).expect("extracted file"),
            contents
        );
    }

    #[test]
    fn exhausted_downloads_distinguish_network_and_verification_failures() {
        let artifact = ManagedArtifact {
            provider: "runtime.test".to_string(),
            sha512: "digest".to_string(),
            size: 1,
        };
        let network = exhausted_download_error(
            &artifact,
            vec![FetchFailure::Body {
                url: "https://example.invalid/runtime.tar.xz".to_string(),
                source: std::io::Error::other("connection reset"),
            }],
        );
        assert_eq!(network.category(), ErrorCategory::Network);
        assert!(network.source().is_some());

        let integrity = exhausted_download_error(
            &artifact,
            vec![FetchFailure::Verification {
                url: "https://example.invalid/runtime.tar.xz".to_string(),
                source: Box::new(TexeError::Toolchain(
                    "failed SHA-512 verification".to_string(),
                )),
            }],
        );
        assert_eq!(integrity.category(), ErrorCategory::Tool);
    }
}
