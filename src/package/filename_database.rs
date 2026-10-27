use std::{fs, path::Path};

use crate::TexeError;
use crate::build::generated;

/// Write a deterministic kpathsea database after package materialization.
pub(super) fn refresh(root: &Path) -> Result<(), TexeError> {
    match fs::symlink_metadata(root) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(TexeError::Io {
                path: root.into(),
                source,
            });
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(TexeError::Build(
                "The package environment must be a private directory".into(),
            ));
        }
    }
    let path = root.join("ls-R");
    let absolute = std::path::absolute(&path).map_err(|source| TexeError::Io {
        path: path.clone(),
        source,
    })?;
    let anchor = absolute
        .ancestors()
        .last()
        .expect("absolute filesystem root");
    generated::validate_parent(anchor, &absolute)?;
    let mut output =
        "% ls-R -- filename database for kpathsea; do not change this line.\n".to_string();
    visit(root, root, &mut output)?;
    if generated::read_local(&path, generated::MAX_METADATA)
        .ok()
        .as_deref()
        != Some(output.as_bytes())
    {
        generated::write(anchor, &absolute, output.as_bytes())?;
    }
    Ok(())
}

fn visit(root: &Path, directory: &Path, output: &mut String) -> Result<(), TexeError> {
    let read = fs::read_dir(directory).map_err(|source| TexeError::Io {
        path: directory.into(),
        source,
    })?;
    let mut entries = read
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| TexeError::Io {
            path: directory.into(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let relative = directory.strip_prefix(root).expect("TEXMF descendant");
    let relative = relative.to_string_lossy().replace('\\', "/");
    output.push_str("\n./");
    output.push_str(&relative);
    output.push_str(":\n");
    for entry in &entries {
        let name = entry.file_name();
        let name = name
            .to_str()
            .filter(|name| !name.contains(['\r', '\n']))
            .ok_or_else(|| {
                TexeError::Build(format!(
                    "invalid TEXMF filename: {}",
                    entry.path().display()
                ))
            })?;
        if name != "ls-R" {
            output.push_str(name);
            output.push('\n');
        }
    }
    for entry in entries {
        let kind = entry.file_type().map_err(|source| TexeError::Io {
            path: entry.path(),
            source,
        })?;
        // pqty links individual files. Do not follow directory links or cycles.
        if kind.is_dir() {
            visit(root, &entry.path(), output)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_tracks_additions_and_removals_without_rewriting_unchanged_content() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("tex/latex/example");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("example.sty"), "package").unwrap();
        refresh(root.path()).unwrap();
        let path = root.path().join("ls-R");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("./tex/latex/example:\nexample.sty\n")
        );
        refresh(root.path()).unwrap();
        assert_eq!(before, fs::metadata(&path).unwrap().modified().unwrap());
        fs::remove_file(package.join("example.sty")).unwrap();
        fs::write(package.join("new.sty"), "package").unwrap();
        refresh(root.path()).unwrap();
        let updated = fs::read_to_string(path).unwrap();
        assert!(!updated.contains("example.sty"));
        assert!(updated.contains("new.sty"));
    }

    #[cfg(unix)]
    #[test]
    fn package_database_root_alias_cannot_redirect_reads_or_writes() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let research = directory.path().join("research");
        fs::create_dir(&research).unwrap();
        fs::write(research.join("ls-R"), b"research notes").unwrap();
        let alias = directory.path().join("texmf");
        symlink(&research, &alias).unwrap();
        assert!(refresh(&alias).is_err());
        assert_eq!(fs::read(research.join("ls-R")).unwrap(), b"research notes");
    }

    #[cfg(unix)]
    #[test]
    fn package_database_replaces_derived_leaf_alias_without_changing_its_target() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("texmf");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("example.sty"), b"package").unwrap();
        let research = directory.path().join("research.tex");
        fs::write(&research, b"research notes").unwrap();
        symlink(&research, root.join("ls-R")).unwrap();
        refresh(&root).unwrap();
        assert_eq!(fs::read(&research).unwrap(), b"research notes");
        assert!(
            !fs::symlink_metadata(root.join("ls-R"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(root.join("ls-R"))
                .unwrap()
                .contains("example.sty")
        );
    }
}
