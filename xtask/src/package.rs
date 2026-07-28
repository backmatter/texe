use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::Compression;
use flate2::GzBuilder;
use zip::write::SimpleFileOptions;

use crate::command::{
    cargo, copy_file, directory_bytes, executable_name, hash_file, repo_root, require,
    require_tools, run,
};
use crate::{Result, message, pqty};

const BINARIES: &[&str] = &["texe", "pqty", "pqty-fls"];

pub(crate) fn suite(selected_output: Option<&Path>) -> Result<()> {
    let repo = repo_root()?;
    let pqty_repo = pqty::checkout()?;
    pqty::verify(Some(&pqty_repo))?;
    let (target, kind) = release_target()?;
    let default_output = repo
        .join("dist")
        .join(format!("texe-{target}.{}", kind.extension()));
    let output = selected_output.unwrap_or(&default_output);

    run(cargo()
        .current_dir(&pqty_repo)
        .args(["build", "--release", "--locked", "--workspace"]))?;
    run(cargo()
        .current_dir(&repo)
        .args(["build", "--release", "--locked", "--package", "texe"]))?;
    let texe_target = cargo_target(&repo)?;
    let pqty_target = cargo_target(&pqty_repo)?;

    let scratch = crate::command::ScratchDir::new("package")?;
    let bundle = scratch.path().join(format!("texe-{target}"));
    let bin = bundle.join("bin");
    fs::create_dir_all(&bin)?;
    for binary in BINARIES {
        let source_root = if *binary == "texe" {
            &texe_target
        } else {
            &pqty_target
        };
        copy_file(
            &source_root.join("release").join(executable_name(binary)),
            &bin.join(executable_name(binary)),
        )?;
    }
    copy_file(&repo.join("README.md"), &bundle.join("README.md"))?;
    copy_file(&repo.join("LICENSE"), &bundle.join("LICENSE"))?;
    copy_file(
        &repo.join("assets/pdfjs/LICENSE"),
        &bundle.join("PDFJS-LICENSE"),
    )?;
    let sums = BINARIES
        .iter()
        .map(|binary| {
            let name = executable_name(binary);
            let path = bin.join(&name);
            Ok(format!(
                "{}  bin/{}\n",
                hash_file(&path)?,
                name.to_string_lossy()
            ))
        })
        .collect::<Result<String>>()?;
    fs::write(bundle.join("SHA256SUMS"), sums)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = output.with_extension(format!("{}.tmp", kind.extension()));
    match kind {
        ArchiveKind::TarGz => write_tar_gz(&bundle, &temporary)?,
        ArchiveKind::Zip => write_zip(&bundle, &temporary)?,
    }
    fs::rename(&temporary, output)?;
    println!("wrote {}", output.display());
    Ok(())
}

pub(crate) fn deb(suite_bin: &Path, output: &Path, version: &str) -> Result<()> {
    require(suite_bin.is_dir(), "--suite-bin must be a directory")?;
    for binary in BINARIES {
        require(
            suite_bin.join(executable_name(binary)).is_file(),
            format!("suite is missing {binary}"),
        )?;
    }

    require(
        (std::env::consts::OS, std::env::consts::ARCH) == ("linux", "x86_64"),
        format!(
            "Debian packages support Linux x86-64, not {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    )?;
    require_tools(&["dpkg-deb"])?;

    let repo = repo_root()?;
    let scratch = crate::command::ScratchDir::new("deb")?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let root = scratch.path().join("root");
    let control = root.join("DEBIAN/control");
    let bin = root.join("usr/bin");
    let doc = root.join("usr/share/doc/texe");
    fs::create_dir_all(&bin)?;
    fs::create_dir_all(&doc)?;
    for binary in BINARIES {
        copy_executable(&suite_bin.join(binary), &bin.join(binary))?;
    }
    copy_documents(&repo, &doc)?;
    let size = directory_bytes(&root.join("usr"))?.div_ceil(1024);
    let template = fs::read_to_string(repo.join("packaging/linux/control.in"))?;
    fs::create_dir_all(control.parent().expect("control parent"))?;
    fs::write(
        &control,
        template
            .replace("@VERSION@", version)
            .replace("@INSTALLED_SIZE@", &size.to_string()),
    )?;
    run(Command::new("dpkg-deb").args([
        "--root-owner-group",
        "--build",
        root.to_str()
            .ok_or_else(|| message("non-UTF-8 package root"))?,
        output
            .to_str()
            .ok_or_else(|| message("non-UTF-8 package output"))?,
    ]))?;
    println!("wrote {}", output.display());
    Ok(())
}

fn cargo_target(repo: &Path) -> Result<PathBuf> {
    let text = crate::command::capture(cargo().current_dir(repo).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]))?;
    let metadata: serde_json::Value = serde_json::from_str(&text)?;
    metadata["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| message("cargo metadata did not report target_directory"))
}

fn copy_documents(repo: &Path, destination: &Path) -> Result<()> {
    copy_file(&repo.join("README.md"), &destination.join("README.md"))?;
    copy_file(&repo.join("LICENSE"), &destination.join("LICENSE"))?;
    copy_file(
        &repo.join("assets/pdfjs/LICENSE"),
        &destination.join("PDFJS-LICENSE"),
    )
}

fn copy_executable(source: &Path, destination: &Path) -> Result<()> {
    copy_file(source, destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn write_tar_gz(bundle: &Path, output: &Path) -> Result<()> {
    let file = File::create(output)?;
    let encoder = GzBuilder::new().mtime(0).write(file, Compression::best());
    let mut archive = tar::Builder::new(encoder);
    append_tar_tree(
        &mut archive,
        bundle,
        bundle.parent().expect("bundle parent"),
    )?;
    archive.into_inner()?.finish()?;
    Ok(())
}

fn append_tar_tree<W: Write>(
    archive: &mut tar::Builder<W>,
    path: &Path,
    base: &Path,
) -> Result<()> {
    let mut entries = fs::read_dir(path)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let source = entry.path();
        let relative = source.strip_prefix(base)?;
        if entry.file_type()?.is_dir() {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_mtime(0);
            header.set_cksum();
            archive.append_data(&mut header, relative, std::io::empty())?;
            append_tar_tree(archive, &source, base)?;
        } else {
            let mut file = File::open(&source)?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata_in_mode(&fs::metadata(&source)?, tar::HeaderMode::Deterministic);
            header.set_uid(0);
            header.set_gid(0);
            let executable = relative
                .components()
                .nth(1)
                .is_some_and(|component| component.as_os_str() == "bin");
            header.set_mode(if executable { 0o755 } else { 0o644 });
            header.set_mtime(0);
            header.set_cksum();
            archive.append_data(&mut header, relative, &mut file)?;
        }
    }
    Ok(())
}

fn write_zip(bundle: &Path, output: &Path) -> Result<()> {
    let file = File::create(output)?;
    let mut archive = zip::ZipWriter::new(file);
    let base = bundle.parent().expect("bundle parent");
    append_zip_tree(&mut archive, bundle, base)?;
    archive.finish()?;
    Ok(())
}

fn append_zip_tree<W: Write + std::io::Seek>(
    archive: &mut zip::ZipWriter<W>,
    path: &Path,
    base: &Path,
) -> Result<()> {
    let mut entries = fs::read_dir(path)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let source = entry.path();
        let relative = source
            .strip_prefix(base)?
            .to_string_lossy()
            .replace('\\', "/");
        if entry.file_type()?.is_dir() {
            archive.add_directory(
                format!("{relative}/"),
                SimpleFileOptions::default().unix_permissions(0o755),
            )?;
            append_zip_tree(archive, &source, base)?;
        } else {
            let executable = relative.contains("/bin/");
            archive.start_file(
                relative,
                SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(if executable { 0o755 } else { 0o644 }),
            )?;
            let mut file = File::open(source)?;
            std::io::copy(&mut file, archive)?;
        }
    }
    Ok(())
}

fn release_target() -> Result<(&'static str, ArchiveKind)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok(("x86_64-linux", ArchiveKind::TarGz)),
        ("macos", "aarch64") => Ok(("aarch64-macos", ArchiveKind::TarGz)),
        ("windows", "x86_64") => Ok(("x86_64-windows", ArchiveKind::Zip)),
        (os, arch) => Err(message(format!("unsupported release host {os} {arch}"))),
    }
}

#[derive(Clone, Copy)]
enum ArchiveKind {
    TarGz,
    Zip,
}

impl ArchiveKind {
    const fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::append_tar_tree;

    #[test]
    fn tar_headers_exclude_host_ownership_and_timestamps() {
        let scratch = tempfile::tempdir().expect("temporary directory");
        let bundle = scratch.path().join("texe-test");
        let bin = bundle.join("bin");
        fs::create_dir_all(&bin).expect("bundle directories");
        fs::write(bin.join("texe"), b"binary").expect("bundle file");

        let mut builder = tar::Builder::new(Vec::new());
        append_tar_tree(&mut builder, &bundle, scratch.path()).expect("append bundle");
        let bytes = builder.into_inner().expect("finish archive");
        let mut archive = tar::Archive::new(bytes.as_slice());
        let headers = archive
            .entries()
            .expect("archive entries")
            .map(|entry| entry.expect("archive entry").header().clone())
            .collect::<Vec<_>>();

        assert!(!headers.is_empty());
        for header in headers {
            assert_eq!(header.uid().expect("uid"), 0);
            assert_eq!(header.gid().expect("gid"), 0);
            assert_eq!(header.mtime().expect("mtime"), 0);
        }
    }
}
