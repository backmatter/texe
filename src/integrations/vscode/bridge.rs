use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::TexeError;

const PACKAGE: &[u8] = include_bytes!("../../../assets/vscode-extension/package.json");
const MAIN: &[u8] = include_bytes!("../../../assets/vscode-extension/extension.js");
const README: &[u8] = include_bytes!("../../../assets/vscode-extension/README.md");
const LICENSE: &[u8] = include_bytes!("../../../assets/vscode-extension/LICENSE");
const MANIFEST: &[u8] = include_bytes!("../../../assets/vscode-extension/extension.vsixmanifest");
const CONTENT_TYPES: &[u8] = include_bytes!("../../../assets/vscode-extension/[Content_Types].xml");

pub(crate) fn path() -> Result<PathBuf, TexeError> {
    write(&crate::toolchain::texe_data_home()?.join("editor"))
}

pub(crate) fn matches_installed(directory: &Path) -> Result<bool, TexeError> {
    for (name, expected) in [
        ("extension.js", MAIN),
        ("README.md", README),
        ("LICENSE.txt", LICENSE),
    ] {
        if fs::read(directory.join(name)).ok().as_deref() != Some(expected) {
            return Ok(false);
        }
    }

    let installed_path = directory.join("package.json");
    let Ok(installed_bytes) = fs::read(&installed_path) else {
        return Ok(false);
    };
    let Ok(mut installed) = serde_json::from_slice::<serde_json::Value>(&installed_bytes) else {
        return Ok(false);
    };
    if let Some(object) = installed.as_object_mut() {
        // VS Code adds installation bookkeeping that is not part of the VSIX.
        object.remove("__metadata");
    }
    let expected = serde_json::from_slice::<serde_json::Value>(&with_current_version(PACKAGE)?)
        .map_err(|source| TexeError::Json {
            path: installed_path,
            source,
        })?;
    Ok(installed == expected)
}

fn write(directory: &Path) -> Result<PathBuf, TexeError> {
    let package = with_current_version(PACKAGE)?;
    let manifest = with_current_version(MANIFEST)?;
    let files: [(&str, &[u8]); 6] = [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("extension.vsixmanifest", &manifest),
        ("extension/package.json", &package),
        ("extension/extension.js", MAIN),
        ("extension/README.md", README),
        ("extension/LICENSE.txt", LICENSE),
    ];
    let mut digest = Sha256::new();
    for (_, bytes) in files {
        digest.update(bytes);
    }
    let digest = hex::encode(digest.finalize());
    fs::create_dir_all(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let target = directory.join(format!(
        "texe-paper-layout-{}-{}.vsix",
        env!("CARGO_PKG_VERSION"),
        &digest[..12]
    ));
    if target.is_file() {
        return Ok(target);
    }

    let temporary = directory.join(format!(
        ".texe-paper-layout-{}-{}.part",
        std::process::id(),
        &digest[..12]
    ));
    let file = fs::File::create(&temporary).map_err(|source| TexeError::Io {
        path: temporary.clone(),
        source,
    })?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, bytes) in files {
        archive
            .start_file(name, options)
            .map_err(|error| archive_error(&temporary, error))?;
        archive.write_all(bytes).map_err(|source| TexeError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    archive
        .finish()
        .map_err(|error| archive_error(&temporary, error))?;
    match fs::rename(&temporary, &target) {
        Ok(()) => {}
        Err(_) if target.is_file() => {
            let _ = fs::remove_file(&temporary);
        }
        Err(source) => {
            return Err(TexeError::Io {
                path: target,
                source,
            });
        }
    }
    Ok(target)
}

fn with_current_version(template: &[u8]) -> Result<Vec<u8>, TexeError> {
    const TOKEN: &str = "@VERSION@";
    let template = std::str::from_utf8(template).map_err(|error| {
        TexeError::Build(format!("VS Code extension template is not UTF-8: {error}"))
    })?;
    if template.matches(TOKEN).count() != 1 {
        return Err(TexeError::Build(
            "VS Code extension template must contain exactly one @VERSION@ token".to_string(),
        ));
    }
    Ok(template
        .replace(TOKEN, env!("CARGO_PKG_VERSION"))
        .into_bytes())
}

fn archive_error(path: &Path, error: zip::result::ZipError) -> TexeError {
    TexeError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read as _;

    use crate::integrations::vscode::bridge::{
        LICENSE, MAIN, PACKAGE, README, matches_installed, with_current_version, write,
    };

    #[test]
    fn bundled_bridge_is_a_complete_versioned_vsix() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = write(directory.path()).expect("VSIX");
        let mut archive =
            zip::ZipArchive::new(fs::File::open(path).expect("open VSIX")).expect("valid VSIX");

        for name in [
            "[Content_Types].xml",
            "extension.vsixmanifest",
            "extension/package.json",
            "extension/extension.js",
            "extension/README.md",
            "extension/LICENSE.txt",
        ] {
            assert!(archive.by_name(name).is_ok(), "missing {name}");
        }
        let mut package = String::new();
        archive
            .by_name("extension/package.json")
            .expect("package")
            .read_to_string(&mut package)
            .expect("read package");
        let package: serde_json::Value = serde_json::from_str(&package).expect("package JSON");
        assert_eq!(package["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(package["publisher"], "backmatter");
        assert!(
            package["activationEvents"]
                .as_array()
                .expect("activation events")
                .iter()
                .any(|event| event == "onCommand:texe.openPaper")
        );
        assert!(
            package["contributes"]["commands"]
                .as_array()
                .expect("commands")
                .iter()
                .any(|command| command["command"] == "texe.openPaper")
        );
        let mut manifest = String::new();
        archive
            .by_name("extension.vsixmanifest")
            .expect("manifest")
            .read_to_string(&mut manifest)
            .expect("read manifest");
        assert!(manifest.contains(&format!(
            "Version=\"{}\" Publisher=\"backmatter\"",
            env!("CARGO_PKG_VERSION")
        )));
    }

    #[test]
    fn installed_companion_is_compared_by_contents_not_only_version() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(directory.path().join("extension.js"), MAIN).expect("extension source");
        fs::write(directory.path().join("README.md"), README).expect("readme");
        fs::write(directory.path().join("LICENSE.txt"), LICENSE).expect("license");
        let mut package: serde_json::Value =
            serde_json::from_slice(&with_current_version(PACKAGE).expect("versioned package"))
                .expect("package JSON");
        package["__metadata"] = serde_json::json!({
            "installedTimestamp": 1,
        });
        fs::write(
            directory.path().join("package.json"),
            serde_json::to_vec_pretty(&package).expect("package bytes"),
        )
        .expect("package");

        assert!(matches_installed(directory.path()).expect("matching companion"));

        fs::write(directory.path().join("extension.js"), b"stale").expect("stale extension");
        assert!(!matches_installed(directory.path()).expect("stale companion"));
    }
}
