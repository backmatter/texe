use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::TexeError;
use crate::atomic;
use crate::config::ProjectManifest;
use crate::integrations::IntegrationReport;

const LEGACY_WORKSPACE_PATH: &str = ".texe/editor/texe.code-workspace";
const PROJECT_SETTINGS_PATH: &str = ".vscode/settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectSettingsOutcome {
    Created,
    Replaced,
    Preserved,
}

pub(crate) fn configure(
    root: &Path,
    replace_project_settings: bool,
) -> Result<ProjectSettingsOutcome, TexeError> {
    let manifest = ProjectManifest::load(&root.join("texe.toml"))?;
    let settings = desired_settings(&manifest);
    remove_legacy_workspace(root)?;

    let project_settings = project_settings_path(root);
    let project_settings_existed = path_exists(&project_settings)?;
    let project_settings_outcome = if project_settings_existed && !replace_project_settings {
        ProjectSettingsOutcome::Preserved
    } else {
        let directory = project_settings
            .parent()
            .expect("project settings path has a parent");
        ensure_directory(directory, "VS Code settings")?;
        validate_settings_target(&project_settings)?;
        let mut bytes = serde_json::to_vec_pretty(&settings).map_err(|source| TexeError::Json {
            path: project_settings.clone(),
            source,
        })?;
        bytes.push(b'\n');
        atomic::write(&project_settings, &bytes)?;
        if project_settings_existed {
            ProjectSettingsOutcome::Replaced
        } else {
            ProjectSettingsOutcome::Created
        }
    };

    Ok(project_settings_outcome)
}

pub(crate) fn remove(root: &Path) -> Result<IntegrationReport, TexeError> {
    let removed = remove_legacy_workspace(root)?;
    Ok(IntegrationReport {
        messages: vec![if removed {
            "removed texe's legacy generated VS Code workspace; project settings were untouched"
                .to_string()
        } else {
            "texe no longer creates a separate VS Code workspace; project settings were untouched"
                .to_string()
        }],
    })
}

fn legacy_workspace_path(root: &Path) -> PathBuf {
    root.join(LEGACY_WORKSPACE_PATH)
}

pub(crate) fn project_settings_path(root: &Path) -> PathBuf {
    root.join(PROJECT_SETTINGS_PATH)
}

pub(crate) fn project_settings_exist(root: &Path) -> Result<bool, TexeError> {
    path_exists(&project_settings_path(root))
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

fn remove_legacy_workspace(root: &Path) -> Result<bool, TexeError> {
    let path = legacy_workspace_path(root);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(TexeError::Io { path, source });
        }
    };
    let private_root = root.join(".texe");
    let directory = path.parent().expect("legacy workspace path has a parent");
    if !validate_directory(&private_root, "remove", "legacy VS Code workspace")?
        || !validate_directory(directory, "remove", "legacy VS Code workspace")?
        || !metadata.is_file()
        || metadata.file_type().is_symlink()
    {
        return Err(TexeError::Build(format!(
            "refusing to remove legacy VS Code workspace through non-file {}",
            path.display()
        )));
    }
    fs::remove_file(&path).map_err(|source| TexeError::Io {
        path: path.clone(),
        source,
    })?;
    remove_if_empty(directory)?;
    Ok(true)
}

fn ensure_directory(path: &Path, subject: &str) -> Result<(), TexeError> {
    if validate_directory(path, "write", subject)? {
        return Ok(());
    }
    fs::create_dir(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn validate_directory(path: &Path, operation: &str, subject: &str) -> Result<bool, TexeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(TexeError::Build(format!(
            "refusing to {operation} {subject} through non-directory {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(TexeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validate_settings_target(path: &Path) -> Result<(), TexeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(TexeError::Build(format!(
            "refusing to replace VS Code settings through non-file {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(TexeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn path_exists(path: &Path) -> Result<bool, TexeError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
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
            "latex-workshop.latex.autoBuild.onSave.files.ignore",
            serde_json::json!([]),
        ),
        (
            "latex-workshop.latex.build.enableMagicComments",
            serde_json::Value::Bool(false),
        ),
        (
            "latex-workshop.latex.jobname",
            serde_json::Value::String(stem.to_string()),
        ),
        (
            "latex-workshop.latex.outDir",
            serde_json::Value::String("%WORKSPACE_FOLDER%".to_string()),
        ),
        (
            "latex-workshop.latex.search.rootFiles.include",
            serde_json::json!([slash_path(&manifest.project.entry)]),
        ),
        (
            "latex-workshop.latex.search.rootFiles.exclude",
            serde_json::json!(["**/.texe/**"]),
        ),
        (
            "latex-workshop.latex.rootFile.useSubFile",
            serde_json::Value::Bool(false),
        ),
        (
            "latex-workshop.latex.rootFile.doNotPrompt",
            serde_json::Value::Bool(true),
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
            "latex-workshop.latex.extraExts",
            serde_json::json!([".tikz"]),
        ),
        (
            "files.associations",
            serde_json::json!({
                "*.tikz": "latex",
            }),
        ),
        (
            "files.watcherExclude",
            serde_json::json!({
                "**/.texe/**": true,
            }),
        ),
        (
            "search.exclude",
            serde_json::json!({
                "**/.texe": true,
            }),
        ),
        ("texe.editor.enabled", serde_json::Value::Bool(true)),
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

    use crate::integrations::vscode::settings::{
        ProjectSettingsOutcome, configure, legacy_workspace_path, project_settings_path, remove,
    };

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
    fn setup_creates_project_settings_when_the_file_is_missing() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "sources/main.tex");

        let outcome = configure(directory.path(), false).expect("setup");

        assert_eq!(outcome, ProjectSettingsOutcome::Created);
        let settings: serde_json::Value = serde_json::from_slice(
            &fs::read(project_settings_path(directory.path())).expect("project settings"),
        )
        .expect("valid project settings");
        assert_eq!(
            settings["latex-workshop.latex.external.build.command"],
            "texe"
        );
        assert_eq!(
            settings["latex-workshop.latex.extraExts"],
            serde_json::json!([".tikz"])
        );
        assert_eq!(
            settings["latex-workshop.latex.search.rootFiles.include"],
            serde_json::json!(["sources/main.tex"])
        );
        assert_eq!(
            settings["latex-workshop.latex.search.rootFiles.exclude"],
            serde_json::json!(["**/.texe/**"])
        );
        assert_eq!(
            settings["latex-workshop.latex.outDir"],
            "%WORKSPACE_FOLDER%"
        );
        assert_eq!(settings["latex-workshop.latex.jobname"], "main");
        assert_eq!(settings["latex-workshop.latex.rootFile.useSubFile"], false);
        assert_eq!(settings["latex-workshop.latex.rootFile.doNotPrompt"], true);
        assert_eq!(settings["files.associations"]["*.tikz"], "latex");
        assert_eq!(settings["files.watcherExclude"]["**/.texe/**"], true);
        assert_eq!(settings["search.exclude"]["**/.texe"], true);
        assert_eq!(settings["texe.editor.enabled"], true);
        assert_eq!(
            settings["texe.editor.openPaper"]["source"],
            "sources/main.tex"
        );
        assert!(!legacy_workspace_path(directory.path()).exists());
    }

    #[test]
    fn setup_preserves_existing_project_settings_without_replacement_permission() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "sources/main.tex");
        let vscode = directory.path().join(".vscode");
        fs::create_dir_all(&vscode).expect("vscode");
        let settings = b"{\n  // user setting\n  \"editor.wordWrap\": \"on\",\n}\n";
        fs::write(vscode.join("settings.json"), settings).expect("settings");

        let outcome = configure(directory.path(), false).expect("setup");

        assert_eq!(outcome, ProjectSettingsOutcome::Preserved);
        assert_eq!(
            fs::read(vscode.join("settings.json")).expect("unchanged settings"),
            settings
        );
        assert!(!legacy_workspace_path(directory.path()).exists());
    }

    #[test]
    fn setup_replaces_the_entire_project_settings_file_with_permission() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "main.tex");
        let vscode = directory.path().join(".vscode");
        fs::create_dir_all(&vscode).expect("vscode");
        fs::write(
            vscode.join("settings.json"),
            b"{\"editor.wordWrap\":\"on\"}\n",
        )
        .expect("settings");

        let outcome = configure(directory.path(), true).expect("setup");

        assert_eq!(outcome, ProjectSettingsOutcome::Replaced);
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(vscode.join("settings.json")).expect("settings"))
                .expect("valid project settings");
        assert!(settings.get("editor.wordWrap").is_none());
        assert_eq!(
            settings["latex-workshop.latex.external.build.command"],
            "texe"
        );
    }

    #[test]
    fn reconfiguration_preserves_project_settings_without_a_fallback_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "main.tex");
        configure(directory.path(), false).expect("first setup");
        let project_settings =
            fs::read(project_settings_path(directory.path())).expect("project settings");

        write_manifest(directory.path(), "revised.tex");
        let outcome = configure(directory.path(), false).expect("updated setup");

        assert_eq!(outcome, ProjectSettingsOutcome::Preserved);
        assert_eq!(
            fs::read(project_settings_path(directory.path())).expect("project settings"),
            project_settings
        );
        assert!(!legacy_workspace_path(directory.path()).exists());
    }

    #[test]
    fn setup_and_removal_delete_only_the_legacy_generated_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        write_manifest(directory.path(), "main.tex");
        let vscode = directory.path().join(".vscode");
        fs::create_dir_all(&vscode).expect("vscode");
        let settings = b"{\"editor.tabSize\":2}\n";
        fs::write(vscode.join("settings.json"), settings).expect("settings");
        let legacy_workspace = legacy_workspace_path(directory.path());
        fs::create_dir_all(legacy_workspace.parent().expect("legacy workspace parent"))
            .expect("legacy workspace directory");
        fs::write(&legacy_workspace, b"legacy").expect("legacy workspace");
        configure(directory.path(), false).expect("setup");

        assert!(!legacy_workspace.exists());
        fs::create_dir_all(legacy_workspace.parent().expect("legacy workspace parent"))
            .expect("legacy workspace directory");
        fs::write(&legacy_workspace, b"legacy").expect("legacy workspace");
        remove(directory.path()).expect("remove");
        assert!(!legacy_workspace.exists());
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

        assert!(configure(directory.path(), false).is_err());
        assert!(remove(directory.path()).is_err());
        assert_eq!(
            fs::read(outside.path().join("texe.code-workspace")).expect("outside file"),
            b"outside"
        );
    }
}
