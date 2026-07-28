use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::TexeError;
use crate::atomic;
use crate::config::ProjectManifest;
use crate::integrations::IntegrationReport;

const WORKSPACE_PATH: &str = ".texe/editor/texe.code-workspace";

pub(crate) fn configure(root: &Path) -> Result<(), TexeError> {
    let manifest = ProjectManifest::load(&root.join("texe.toml"))?;
    let path = workspace_path(root);
    let directory = path.parent().expect("workspace path has a parent");
    ensure_owned_directory(&root.join(".texe"))?;
    ensure_owned_directory(directory)?;

    let workspace = serde_json::json!({
        "folders": [
            {
                // The generated workspace lives in .texe/editor/.
                "path": "../..",
            }
        ],
        "settings": desired_settings(&manifest),
    });
    let mut bytes = serde_json::to_vec_pretty(&workspace).map_err(|source| TexeError::Json {
        path: path.clone(),
        source,
    })?;
    bytes.push(b'\n');
    atomic::write(&path, &bytes)
}

pub(crate) fn remove(root: &Path) -> Result<IntegrationReport, TexeError> {
    let path = workspace_path(root);
    let directory = path.parent().expect("workspace path has a parent");
    if !validate_owned_directory(&root.join(".texe"), "remove")?
        || !validate_owned_directory(directory, "remove")?
        || fs::symlink_metadata(&path)
            .is_err_and(|source| source.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(IntegrationReport {
            messages: vec!["no texe-owned VS Code workspace was found".to_string()],
        });
    }
    fs::remove_file(&path).map_err(|source| TexeError::Io {
        path: path.clone(),
        source,
    })?;
    remove_if_empty(directory)?;
    Ok(IntegrationReport {
        messages: vec![
            "removed texe's generated VS Code workspace; project settings were untouched"
                .to_string(),
        ],
    })
}

pub(crate) fn workspace_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_PATH)
}

fn remove_if_empty(path: &Path) -> Result<(), TexeError> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(TexeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_owned_directory(path: &Path) -> Result<(), TexeError> {
    if validate_owned_directory(path, "write")? {
        return Ok(());
    }
    fs::create_dir(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn validate_owned_directory(path: &Path, operation: &str) -> Result<bool, TexeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(TexeError::Build(format!(
            "refusing to {operation} a VS Code workspace through non-directory {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(TexeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn desired_settings(manifest: &ProjectManifest) -> BTreeMap<&'static str, serde_json::Value> {
    let stem = manifest
        .project
        .entry
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("main");
    let request = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    BTreeMap::from([
        (
            "latex-workshop.latex.external.build.command",
            serde_json::Value::String("texe".to_string()),
        ),
        (
            "latex-workshop.latex.external.build.args",
            serde_json::json!(["build"]),
        ),
        (
            "latex-workshop.latex.autoBuild.run",
            serde_json::Value::String("onSave".to_string()),
        ),
        (
            "latex-workshop.view.pdf.viewer",
            serde_json::Value::String("tab".to_string()),
        ),
        (
            "latex-workshop.view.pdf.tab.editorGroup",
            serde_json::Value::String("right".to_string()),
        ),
        (
            "latex-workshop.message.error.show",
            serde_json::Value::Bool(false),
        ),
        (
            "texe.editor.openPaper",
            serde_json::json!({
                "source": slash_path(&manifest.project.entry),
                "pdf": format!("{stem}.pdf"),
                "request": format!("{}-{request}", env!("CARGO_PKG_VERSION")),
            }),
        ),
    ])
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::integrations::vscode::settings::{configure, remove, workspace_path};

    fn write_manifest(root: &Path, entry: &str) {
        fs::write(
            root.join("texe.toml"),
            format!(
                "schema = \"texe.project/v1\"\n[project]\nentry = \"{entry}\"\n\
                 [toolchain]\nengine = \"pdflatex\"\n"
            ),
        )
        .expect("write manifest");
    }

    #[test]
    fn setup_uses_an_owned_workspace_without_touching_project_settings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "sources/main.tex");
        let vscode = directory.path().join(".vscode");
        fs::create_dir_all(&vscode).expect("vscode");
        let settings = b"{\n  // user setting\n  \"editor.wordWrap\": \"on\",\n}\n";
        fs::write(vscode.join("settings.json"), settings).expect("settings");

        configure(directory.path()).expect("setup");
        assert_eq!(
            fs::read(vscode.join("settings.json")).expect("unchanged settings"),
            settings
        );
        let workspace: serde_json::Value =
            serde_json::from_slice(&fs::read(workspace_path(directory.path())).expect("workspace"))
                .expect("valid workspace");
        assert_eq!(workspace["folders"][0]["path"], "../..");
        assert_eq!(
            workspace["settings"]["latex-workshop.latex.external.build.command"],
            "texe"
        );
        assert_eq!(
            workspace["settings"]["texe.editor.openPaper"]["source"],
            "sources/main.tex"
        );
        assert_eq!(
            workspace["settings"]["texe.editor.openPaper"]["pdf"],
            "main.pdf"
        );
    }

    #[test]
    fn reconfiguration_updates_only_the_generated_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "main.tex");
        configure(directory.path()).expect("first setup");

        write_manifest(directory.path(), "revised.tex");
        configure(directory.path()).expect("updated setup");
        let workspace: serde_json::Value =
            serde_json::from_slice(&fs::read(workspace_path(directory.path())).expect("workspace"))
                .expect("valid workspace");
        assert_eq!(
            workspace["settings"]["texe.editor.openPaper"]["source"],
            "revised.tex"
        );
        assert_eq!(
            workspace["settings"]["texe.editor.openPaper"]["pdf"],
            "revised.pdf"
        );
    }

    #[test]
    fn removal_deletes_only_the_generated_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "main.tex");
        let vscode = directory.path().join(".vscode");
        fs::create_dir_all(&vscode).expect("vscode");
        let settings = b"{\"editor.tabSize\":2}\n";
        fs::write(vscode.join("settings.json"), settings).expect("settings");
        configure(directory.path()).expect("setup");

        remove(directory.path()).expect("remove");
        assert!(!workspace_path(directory.path()).exists());
        assert_eq!(
            fs::read(vscode.join("settings.json")).expect("unchanged settings"),
            settings
        );
    }

    #[cfg(unix)]
    #[test]
    fn setup_and_removal_refuse_a_symlinked_editor_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let outside = tempfile::tempdir().expect("outside directory");
        write_manifest(directory.path(), "main.tex");
        fs::create_dir(directory.path().join(".texe")).expect("private root");
        symlink(outside.path(), directory.path().join(".texe/editor")).expect("editor symlink");
        fs::write(outside.path().join("texe.code-workspace"), b"outside").expect("outside file");

        assert!(configure(directory.path()).is_err());
        assert!(remove(directory.path()).is_err());
        assert_eq!(
            fs::read(outside.path().join("texe.code-workspace")).expect("outside file"),
            b"outside"
        );
    }
}
