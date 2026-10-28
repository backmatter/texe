//! Parent-side reads of compiler output must have the same confinement as the
//! compiler. The compiler can create links even when it cannot follow them.
use std::fs;
use std::io::Read as _;
use std::path::{Component, Path};

use crate::TexeError;

pub(crate) const MAX_METADATA: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_ARTIFACT: u64 = 128 * 1024 * 1024;

/// Check the PDF's completion markers and cross-reference location before it
/// replaces the last build. This is deliberately not a full PDF parser.
pub(crate) fn valid_pdf(bytes: &[u8]) -> bool {
    fn whitespace(byte: u8) -> bool {
        matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
    }
    fn number(input: &[u8]) -> Option<(&[u8], &[u8])> {
        let count = input
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        (count > 0).then(|| input.split_at(count))
    }
    fn trim_start(input: &[u8]) -> &[u8] {
        let count = input.iter().take_while(|&&byte| whitespace(byte)).count();
        &input[count..]
    }
    if !bytes.starts_with(b"%PDF-") {
        return false;
    }
    let end = bytes
        .iter()
        .rposition(|&byte| !whitespace(byte))
        .map_or(0, |position| position + 1);
    let complete = &bytes[..end];
    if !complete.ends_with(b"%%EOF") {
        return false;
    }
    let tail_start = complete.len().saturating_sub(64 * 1024);
    let Some(marker) = complete[tail_start..]
        .windows(b"startxref".len())
        .rposition(|part| part == b"startxref")
    else {
        return false;
    };
    let after = &complete[tail_start + marker + b"startxref".len()..];
    if !after.first().is_some_and(|&byte| whitespace(byte)) {
        return false;
    }
    let after = trim_start(after);
    let Some((offset, rest)) = number(after) else {
        return false;
    };
    if !rest.first().is_some_and(|&byte| whitespace(byte)) || trim_start(rest) != b"%%EOF" {
        return false;
    }
    let Some(offset) = std::str::from_utf8(offset)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return false;
    };
    if offset >= tail_start + marker {
        return false;
    }
    let target = &bytes[offset..];
    if target.starts_with(b"xref") {
        return target.get(4).is_some_and(|&byte| whitespace(byte));
    }
    // PDF 1.5+ permits a cross-reference stream instead of a table. Its offset
    // points at the stream's indirect-object header.
    let Some((_, rest)) = number(target) else {
        return false;
    };
    if !rest.first().is_some_and(|&byte| whitespace(byte)) {
        return false;
    }
    let Some((_, rest)) = number(trim_start(rest)) else {
        return false;
    };
    if !rest.first().is_some_and(|&byte| whitespace(byte)) {
        return false;
    }
    let rest = trim_start(rest);
    if !rest.starts_with(b"obj")
        || !rest
            .get(3)
            .is_some_and(|&byte| whitespace(byte) || byte == b'<')
    {
        return false;
    }
    let dictionary = trim_start(&rest[3..]);
    if !dictionary.starts_with(b"<<") {
        return false;
    }
    let header = &dictionary[..dictionary.len().min(64 * 1024)];
    let Some(stream) = header.windows(6).position(|part| part == b"stream") else {
        return false;
    };
    let compact: Vec<_> = header[..stream]
        .iter()
        .copied()
        .filter(|&byte| !whitespace(byte))
        .collect();
    compact.windows(10).any(|part| part == b"/Type/XRef")
}

fn invalid(path: &Path) -> TexeError {
    TexeError::Build(format!(
        "Compiler output must be a bounded regular file in its private directory: {}. Sources and the last PDF have been kept.",
        path.display()
    ))
}

fn io(path: &Path, source: std::io::Error) -> TexeError {
    TexeError::Io {
        path: path.into(),
        source,
    }
}

/// Reject aliases in every existing directory component before parent writes.
pub(crate) fn validate_parent(root: &Path, path: &Path) -> Result<(), TexeError> {
    let relative = path.strip_prefix(root).map_err(|_| invalid(path))?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(invalid(path));
    }
    let mut current = root.to_owned();
    let metadata = fs::symlink_metadata(&current).map_err(|source| io(&current, source))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(path));
    }
    for part in relative.parent().unwrap_or(Path::new("")).components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => return Err(io(&current, source)),
            Ok(_) => return Err(invalid(path)),
        }
    }
    Ok(())
}

pub(crate) fn read(
    project: &Path,
    expected_root: &Path,
    path: &Path,
    limit: u64,
) -> Result<Vec<u8>, TexeError> {
    let relative = path
        .strip_prefix(expected_root)
        .map_err(|_| invalid(path))?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(invalid(path));
    }
    validate_parent(project, path)?;
    #[cfg(target_os = "linux")]
    let file = {
        use rustix::fs::{Mode, OFlags, ResolveFlags, open, openat2};
        let directory = open(
            project,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|source| io(project, source.into()))?;
        let relative = path.strip_prefix(project).map_err(|_| invalid(path))?;
        let descriptor = openat2(
            &directory,
            relative,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|source| io(path, source.into()))?;
        fs::File::from(descriptor)
    };
    #[cfg(not(target_os = "linux"))]
    let file = {
        let metadata = fs::symlink_metadata(path).map_err(|source| io(path, source))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(invalid(path));
        }
        fs::File::open(path).map_err(|source| io(path, source))?
    };
    let metadata = file.metadata().map_err(|source| io(path, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(invalid(path));
        }
    }
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid(path));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| io(path, source))?;
    if bytes.len() as u64 > limit {
        return Err(invalid(path));
    }
    Ok(bytes)
}

/// Resolve the OS temporary base, then reject aliases below the trusted anchor.
pub(crate) fn read_local(path: &Path, limit: u64) -> Result<Vec<u8>, TexeError> {
    let absolute = std::path::absolute(path).map_err(|source| io(path, source))?;
    if let Some((anchor, physical)) = temporary_path(&absolute)? {
        return read(&anchor, &anchor, &physical, limit);
    }
    let anchor = absolute.ancestors().last().ok_or_else(|| invalid(path))?;
    read(anchor, anchor, &absolute, limit)
}

fn temporary_path(
    path: &Path,
) -> Result<Option<(std::path::PathBuf, std::path::PathBuf)>, TexeError> {
    let temporary = std::path::absolute(std::env::temp_dir()).map_err(|source| io(path, source))?;
    let Ok(relative) = path.strip_prefix(&temporary) else {
        return Ok(None);
    };
    let anchor = fs::canonicalize(&temporary).map_err(|source| io(&temporary, source))?;
    let physical = anchor.join(relative);
    Ok(Some((anchor, physical)))
}

pub(crate) fn text_local(path: &Path, limit: u64) -> Result<String, TexeError> {
    String::from_utf8(read_local(path, limit)?).map_err(|_| invalid(path))
}

pub(crate) fn optional(
    project: &Path,
    expected_root: &Path,
    path: &Path,
    limit: u64,
) -> Result<Option<Vec<u8>>, TexeError> {
    match read(project, expected_root, path, limit) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(TexeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(failure) => Err(failure),
    }
}

#[cfg(target_os = "linux")]
fn parent_directory(
    project: &Path,
    path: &Path,
    create: bool,
) -> Result<(rustix::fd::OwnedFd, std::ffi::OsString), TexeError> {
    use rustix::fs::{Mode, OFlags, ResolveFlags, fsync, mkdirat, open, openat2};
    let relative = path.strip_prefix(project).map_err(|_| invalid(path))?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(invalid(path));
    }
    let mut directory = open(
        project,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| io(project, source.into()))?;
    for part in relative.parent().unwrap_or(Path::new("")).components() {
        if create {
            match mkdirat(&directory, part.as_os_str(), Mode::from_raw_mode(0o755)) {
                Ok(()) => fsync(&directory).map_err(|source| io(path, source.into()))?,
                Err(rustix::io::Errno::EXIST) => {}
                Err(source) => return Err(io(path, source.into())),
            }
        }
        directory = openat2(
            &directory,
            part.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|source| io(path, source.into()))?;
    }
    Ok((
        directory,
        relative
            .file_name()
            .ok_or_else(|| invalid(path))?
            .to_owned(),
    ))
}

/// Create and rename through held directory descriptors. A compiler-created
/// ancestor link cannot redirect either staging or publication outside root.
pub(crate) fn write(project: &Path, path: &Path, bytes: &[u8]) -> Result<(), TexeError> {
    if project.parent().is_none() {
        let absolute = std::path::absolute(path).map_err(|source| io(path, source))?;
        if let Some((anchor, physical)) = temporary_path(&absolute)? {
            return write(&anchor, &physical, bytes);
        }
    }
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
        use std::io::Write as _;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let (directory, name) = parent_directory(project, path, true)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = format!(
            ".texe-parent-{}-{stamp}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let result = (|| {
            let descriptor = openat(
                &directory,
                temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o666),
            )
            .map_err(|source| io(path, source.into()))?;
            let mut file = fs::File::from(descriptor);
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|source| io(path, source))?;
            renameat(&directory, temporary.as_str(), &directory, &name)
                .map_err(|source| io(path, source.into()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = unlinkat(&directory, temporary.as_str(), AtFlags::empty());
        }
        result?;
        fs::File::from(directory)
            .sync_all()
            .map_err(|source| io(path, source))
    }
    #[cfg(not(target_os = "linux"))]
    {
        validate_parent(project, path)?;
        crate::atomic::write(path, bytes)
    }
}

pub(crate) fn remove(project: &Path, path: &Path) -> Result<(), TexeError> {
    if project.parent().is_none() {
        let absolute = std::path::absolute(path).map_err(|source| io(path, source))?;
        if let Some((anchor, physical)) = temporary_path(&absolute)? {
            return remove(&anchor, &physical);
        }
    }
    #[cfg(target_os = "linux")]
    let result = (|| {
        let (directory, name) = parent_directory(project, path, false)?;
        rustix::fs::unlinkat(&directory, &name, rustix::fs::AtFlags::empty())
            .and_then(|()| rustix::fs::fsync(&directory))
            .map_err(|source| io(path, source.into()))
    })();
    #[cfg(not(target_os = "linux"))]
    let result = validate_parent(project, path)
        .and_then(|()| fs::remove_file(path).map_err(|source| io(path, source)));
    match result {
        Err(TexeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(())
        }
        result => result,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn parent_reads_reject_symlinks_hardlinks_oversize_and_parent_aliases() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        let output = project.join(".texe/build/output");
        fs::create_dir_all(&output).unwrap();
        let source = fixture.path().join("source.tex");
        fs::write(&source, b"outside research sentinel").unwrap();
        let result = output.join("main.pdf");
        std::os::unix::fs::symlink(&source, &result).unwrap();
        assert!(read(&project, &output, &result, 1024).is_err());
        fs::remove_file(&result).unwrap();
        fs::hard_link(&source, &result).unwrap();
        assert!(read(&project, &output, &result, 1024).is_err());
        fs::remove_file(&result).unwrap();
        fs::File::create(&result).unwrap().set_len(1025).unwrap();
        assert!(read(&project, &output, &result, 1024).is_err());
        fs::remove_file(&result).unwrap();
        fs::remove_dir(&output).unwrap();
        std::os::unix::fs::symlink(fixture.path(), &output).unwrap();
        assert!(read(&project, &output, &output.join("source.tex"), 1024).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"outside research sentinel");
    }

    #[test]
    fn parent_writes_never_follow_directory_or_destination_links() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let research = fixture.path().join("research");
        fs::create_dir(&research).unwrap();
        let source = research.join("main.tex");
        fs::write(&source, b"research source").unwrap();
        std::os::unix::fs::symlink(&research, project.join("output")).unwrap();
        assert!(write(&project, &project.join("output/main.tex"), b"destroyed").is_err());
        assert_eq!(fs::read(&source).unwrap(), b"research source");
        let destination = project.join("result.txt");
        std::os::unix::fs::symlink(&source, &destination).unwrap();
        write(&project, &destination, b"private output").unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"research source");
        assert_eq!(fs::read(&destination).unwrap(), b"private output");
        assert!(
            !fs::symlink_metadata(destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn generated_special_files_are_rejected_without_waiting_for_a_writer() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("main.log");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &path,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .unwrap();
        assert!(read(fixture.path(), fixture.path(), &path, MAX_METADATA).is_err());
    }

    #[test]
    fn publication_rolls_back_the_current_destination_after_mutation_then_failure() {
        for operation in ["replace", "remove", "create"] {
            let fixture = tempfile::tempdir().unwrap();
            let project = fixture.path();
            let source = project.join("main.tex");
            let lock = project.join("texe.lock");
            let destination = project.join("main.pdf");
            fs::write(&source, b"research source").unwrap();
            fs::write(&lock, b"old lock").unwrap();
            if operation != "create" {
                fs::write(&destination, b"last good output").unwrap();
            }
            let new_output = (operation != "remove").then_some(b"new output".as_slice());
            let mut failed = false;
            let result = replace_files_impl(
                project,
                &[(&lock, Some(b"new lock")), (&destination, new_output)],
                |path, bytes| {
                    match bytes {
                        Some(bytes) => write(project, path, bytes)?,
                        None => remove(project, path)?,
                    }
                    if path == destination && !failed {
                        failed = true;
                        return Err(io(
                            path,
                            std::io::Error::other("injected directory fsync failure"),
                        ));
                    }
                    Ok(())
                },
            );
            assert!(result.is_err(), "{operation}");
            assert!(failed, "{operation}");
            assert_eq!(
                fs::read(&source).unwrap(),
                b"research source",
                "{operation}"
            );
            assert_eq!(fs::read(&lock).unwrap(), b"old lock", "{operation}");
            if operation == "create" {
                assert!(!destination.exists());
            } else {
                assert_eq!(
                    fs::read(&destination).unwrap(),
                    b"last good output",
                    "{operation}"
                );
            }
        }
    }
}

pub(crate) fn replace_files(
    project: &Path,
    files: &[(&Path, Option<&[u8]>)],
) -> Result<(), TexeError> {
    replace_files_impl(project, files, |path, bytes| match bytes {
        Some(bytes) => write(project, path, bytes),
        None => remove(project, path),
    })
}

fn replace_files_impl(
    project: &Path,
    files: &[(&Path, Option<&[u8]>)],
    mut replace: impl FnMut(&Path, Option<&[u8]>) -> Result<(), TexeError>,
) -> Result<(), TexeError> {
    let old = files
        .iter()
        .map(|(path, _)| optional(project, project, path, MAX_ARTIFACT))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, (path, bytes)) in files.iter().enumerate() {
        if let Err(failure) = replace(path, *bytes) {
            let mut errors = Vec::new();
            // A rename/unlink can succeed before its directory fsync fails.
            // Restore the failing destination too, even when it was new.
            for ((path, _), old) in files[..=index].iter().zip(&old[..=index]).rev() {
                if let Err(error) = replace(path, old.as_deref()) {
                    errors.push(error.to_string());
                }
            }
            return if errors.is_empty() {
                Err(failure)
            } else {
                Err(TexeError::Build(format!(
                    "{failure}; could not restore previous outputs: {}",
                    errors.join("; ")
                )))
            };
        }
    }
    Ok(())
}
