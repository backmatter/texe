//! The Citet extension is built in citet/apps/vscode and exported as a standalone VSIX.
use std::fs;
use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::TexeError;

const VSIX: &[u8] = include_bytes!("../../../assets/citet-vscode.vsix");

pub(crate) fn path() -> Result<PathBuf, TexeError> {
    write(&crate::toolchain::texe_data_home()?.join("editor"))
}

pub(crate) fn matches_installed(directory: &Path) -> Result<bool, TexeError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(VSIX)).map_err(|error| archive_error(directory, error))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| archive_error(directory, error))?;
        let Some(name) = file.name().strip_prefix("extension/").map(str::to_owned) else {
            continue;
        };
        if file.is_dir() {
            continue;
        }
        let target = directory.join(&name);
        let Ok(installed) = fs::read(&target) else {
            return Ok(false);
        };
        let mut expected = Vec::new();
        file.read_to_end(&mut expected)
            .map_err(|source| TexeError::Io {
                path: target.clone(),
                source,
            })?;
        if name == "package.json" {
            let Ok(mut installed) = serde_json::from_slice::<serde_json::Value>(&installed) else {
                return Ok(false);
            };
            if let Some(object) = installed.as_object_mut() {
                object.remove("__metadata");
            }
            let expected: serde_json::Value =
                serde_json::from_slice(&expected).map_err(|source| TexeError::Json {
                    path: target,
                    source,
                })?;
            if installed != expected {
                return Ok(false);
            }
        } else if installed != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn write(directory: &Path) -> Result<PathBuf, TexeError> {
    let digest = hex::encode(Sha256::digest(VSIX));
    fs::create_dir_all(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let target = directory.join(format!("citet-{}.vsix", &digest[..12]));
    if fs::read(&target).ok().as_deref() != Some(VSIX) {
        crate::atomic::write(&target, VSIX)?;
    }
    Ok(target)
}

fn archive_error(path: &Path, error: zip::result::ZipError) -> TexeError {
    TexeError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_citet_is_self_contained_and_not_versioned_with_texe() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = write(directory.path()).expect("VSIX");
        let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        for name in [
            "extension/package.json",
            "extension/out/extension.js",
            "extension/out/pdf-viewer-webview.js",
            "extension/out/pdf.worker.min.mjs",
            "extension/syntaxes/latex.tmLanguage.json",
            "extension/THIRD-PARTY-NOTICES.txt",
        ] {
            assert!(archive.by_name(name).is_ok(), "missing {name}");
        }
        let package: serde_json::Value =
            serde_json::from_reader(archive.by_name("extension/package.json").unwrap()).unwrap();
        assert_eq!(package["name"], "citet");
        assert_eq!(package["publisher"], "citet");
        assert!(package.get("extensionDependencies").is_none());
        assert!(package.get("dependencies").is_none());
    }

    #[test]
    fn installed_extension_is_compared_by_all_bundled_contents() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut archive = zip::ZipArchive::new(Cursor::new(VSIX)).unwrap();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let Some(name) = file.name().strip_prefix("extension/").map(str::to_owned) else {
                continue;
            };
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::io::copy(&mut file, &mut fs::File::create(&path).unwrap()).unwrap();
        }
        assert!(matches_installed(directory.path()).unwrap());
        fs::write(directory.path().join("out/extension.js"), b"stale").unwrap();
        assert!(!matches_installed(directory.path()).unwrap());
    }
}
