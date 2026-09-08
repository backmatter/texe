use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Ownership {
    original: Option<String>,
    installed: String,
    edits: Vec<OwnedEdit>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedEdit {
    path: Vec<String>,
    before: Option<serde_json::Value>,
    #[serde(default)]
    before_exists: bool,
    after: serde_json::Value,
}

fn read_optional(path: &Path) -> Result<Option<String>, TexeError> {
    validate_settings_target(path)?;
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 4 * 1024 * 1024) {
        return Err(TexeError::Build(format!(
            "VS Code settings or ownership record exceeds 4 MiB: {}",
            path.display()
        )));
    }
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(TexeError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn parse(text: &str) -> Result<jsonc_parser::cst::CstRootNode, TexeError> {
    let options = jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    let root = jsonc_parser::cst::CstRootNode::parse(text, &options)
        .map_err(|error| TexeError::Build(format!("invalid VS Code settings: {error}")))?;
    if root.object_value().is_none() {
        return Err(TexeError::Build(
            "VS Code settings must be a JSON object".into(),
        ));
    }
    unique_properties(&root.object_value().expect("object"))?;
    Ok(root)
}

fn unique_properties(object: &jsonc_parser::cst::CstObject) -> Result<(), TexeError> {
    let mut names = std::collections::BTreeSet::new();
    for property in object.properties() {
        let name = property
            .name()
            .and_then(|name| name.decoded_value().ok())
            .ok_or_else(|| TexeError::Build("invalid settings property".into()))?;
        if !names.insert(name.clone()) {
            return Err(TexeError::Build(format!(
                "duplicate VS Code settings property: {name}"
            )));
        }
        if let Some(child) = property.object_value() {
            unique_properties(&child)?;
        }
    }
    Ok(())
}

fn input(value: &serde_json::Value) -> jsonc_parser::cst::CstInputValue {
    use jsonc_parser::cst::CstInputValue as V;
    use serde_json::Value;
    match value {
        Value::Null => V::Null,
        Value::Bool(v) => V::Bool(*v),
        Value::Number(v) => V::Number(v.to_string()),
        Value::String(v) => V::String(v.clone()),
        Value::Array(v) => V::Array(v.iter().map(input).collect()),
        Value::Object(v) => V::Object(v.iter().map(|(k, v)| (k.clone(), input(v))).collect()),
    }
}

fn get(value: &serde_json::Value, path: &[String]) -> Option<serde_json::Value> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    Some(current.clone())
}

fn set(object: &jsonc_parser::cst::CstObject, path: &[String], value: Option<&serde_json::Value>) {
    if path.len() > 1 {
        if value.is_some() || object.object_value(&path[0]).is_some() {
            set(&object.object_value_or_set(&path[0]), &path[1..], value);
        }
    } else if let Some(value) = value {
        if let Some(prop) = object.get(&path[0]) {
            prop.set_value(input(value));
        } else {
            object.append(&path[0], input(value));
        }
    } else if let Some(prop) = object.get(&path[0]) {
        prop.remove();
    }
}

fn leaves(
    path: Vec<String>,
    value: &serde_json::Value,
    output: &mut Vec<(Vec<String>, serde_json::Value)>,
) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            let mut child = path.clone();
            child.push(key.clone());
            leaves(child, value, output);
        }
    } else {
        output.push((path, value.clone()));
    }
}

fn plan(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<(Ownership, String, Vec<String>), TexeError> {
    validate_directory(&root.join(".vscode"), "read", "VS Code settings")?;
    let original = read_optional(&project_settings_path(root))?;
    let tree = parse(original.as_deref().unwrap_or("{}\n"))?;
    let current = tree.to_serde_value().expect("object");
    let mut ownership: Ownership = match read_optional(&root.join(".vscode/texe-integration.json"))?
    {
        Some(text) => serde_json::from_str(&text).map_err(|source| TexeError::Json {
            path: root.join(".vscode/texe-integration.json"),
            source,
        })?,
        None => Ownership {
            original,
            ..Default::default()
        },
    };
    let mut desired = Vec::new();
    for (key, value) in desired_settings(manifest) {
        leaves(vec![key.to_string()], &value, &mut desired);
    }
    let mut conflicts = Vec::new();
    for (path, after) in desired {
        let before = get(&current, &path);
        if before.as_ref() == Some(&after) {
            continue;
        }
        let owned = ownership.edits.iter_mut().find(|edit| edit.path == path);
        if before.is_some()
            && owned
                .as_ref()
                .is_none_or(|edit| Some(&edit.after) != before.as_ref())
        {
            conflicts.push(path.join(" / "));
        }
        // A non-object ancestor is also a conflict; never silently discard it.
        for length in 1..path.len() {
            if get(&current, &path[..length]).is_some_and(|v| !v.is_object()) {
                return Err(TexeError::Build(format!(
                    "{} must be an object before texe can merge settings",
                    path[..length].join(" / ")
                )));
            }
        }
        set(&tree.object_value().expect("object"), &path, Some(&after));
        if let Some(edit) = owned {
            edit.after = after;
        } else {
            ownership.edits.push(OwnedEdit {
                path,
                before_exists: before.is_some(),
                before,
                after,
            });
        }
    }
    Ok((ownership, tree.to_string(), conflicts))
}

pub(crate) fn preview(root: &Path) -> Result<serde_json::Value, TexeError> {
    let manifest = ProjectManifest::load(&root.join("texe.toml"))?;
    preview_manifest(root, &manifest)
}

pub(crate) fn preview_manifest(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<serde_json::Value, TexeError> {
    let (_, proposed, conflicts) = plan(root, manifest)?;
    Ok(
        serde_json::json!({ "schema": "texe.editor-preview/v1", "conflicts": conflicts, "settings": proposed }),
    )
}

pub(crate) fn configure(
    root: &Path,
    replace_conflicts: bool,
) -> Result<ProjectSettingsOutcome, TexeError> {
    let manifest = ProjectManifest::load(&root.join("texe.toml"))?;
    let (mut ownership, proposed, conflicts) = plan(root, &manifest)?;
    if !conflicts.is_empty() && !replace_conflicts {
        return Err(TexeError::Build(format!(
            "VS Code settings conflict: {}; inspect with `texe editor --preview`, then use --replace-conflicts to accept",
            conflicts.join(", ")
        )));
    }
    let existed = project_settings_exist(root)?;
    if read_optional(&project_settings_path(root))?.as_deref() == Some(&proposed) {
        return Ok(ProjectSettingsOutcome::Preserved);
    }
    remove_legacy_workspace(root)?;
    ensure_directory(&root.join(".vscode"), "VS Code settings")?;
    ownership.installed.clone_from(&proposed);
    let ledger = root.join(".vscode/texe-integration.json");
    let bytes = serde_json::to_vec_pretty(&ownership).map_err(|source| TexeError::Json {
        path: ledger.clone(),
        source,
    })?;
    atomic::replace_files(&[
        (&project_settings_path(root), Some(proposed.as_bytes())),
        (&ledger, Some(&bytes)),
    ])?;
    Ok(if existed {
        ProjectSettingsOutcome::Replaced
    } else {
        ProjectSettingsOutcome::Created
    })
}

pub(crate) fn remove(root: &Path) -> Result<IntegrationReport, TexeError> {
    validate_directory(&root.join(".vscode"), "remove", "VS Code settings")?;
    remove_legacy_workspace(root)?;
    let ledger = root.join(".vscode/texe-integration.json");
    if let Some(text) = read_optional(&ledger)? {
        let ownership: Ownership =
            serde_json::from_str(&text).map_err(|source| TexeError::Json {
                path: ledger.clone(),
                source,
            })?;
        let path = project_settings_path(root);
        if let Some(current) = read_optional(&path)? {
            let restored = if current == ownership.installed {
                ownership.original
            } else {
                let tree = parse(&current)?;
                let value = tree.to_serde_value().expect("object");
                let original = parse(ownership.original.as_deref().unwrap_or("{}"))?
                    .to_serde_value()
                    .expect("object");
                let mut containers = std::collections::BTreeSet::new();
                for edit in ownership.edits {
                    for length in 1..edit.path.len() {
                        containers.insert(edit.path[..length].to_vec());
                    }
                    if !edit.path.is_empty()
                        && get(&value, &edit.path).as_ref() == Some(&edit.after)
                    {
                        let before = edit
                            .before
                            .or_else(|| edit.before_exists.then_some(serde_json::Value::Null));
                        set(
                            &tree.object_value().expect("object"),
                            &edit.path,
                            before.as_ref(),
                        );
                    }
                }
                for path in containers.into_iter().rev() {
                    if get(&original, &path).is_none()
                        && get(&tree.to_serde_value().expect("object"), &path)
                            .is_some_and(|v| v.as_object().is_some_and(serde_json::Map::is_empty))
                    {
                        // Keep comments added inside a formerly generated object.
                        let mut object = tree.object_value().expect("object");
                        for key in &path {
                            object = object.object_value(key).expect("existing object");
                        }
                        let text = object.to_string();
                        if !text.contains("//") && !text.contains("/*") {
                            set(&tree.object_value().expect("object"), &path, None);
                        }
                    }
                }
                Some(tree.to_string())
            };
            atomic::replace_files(&[
                (&path, restored.as_deref().map(str::as_bytes)),
                (&ledger, None),
            ])?;
        } else {
            atomic::replace_files(&[(&ledger, None)])?;
        }
    }
    Ok(IntegrationReport {
        messages: vec![
            "removed texe-owned VS Code settings; later user edits were preserved".into(),
        ],
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

    BTreeMap::from([
        (
            "latex-workshop.latex.external.build.command",
            serde_json::json!(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("texe"))),
        ),
        (
            "latex-workshop.latex.external.build.args",
            serde_json::json!(["editor-build", "--project", "%WORKSPACE_FOLDER%"]),
        ),
        (
            "latex-workshop.latex.autoBuild.run",
            serde_json::Value::String("never".to_string()),
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
        (
            "texe.executablePath",
            serde_json::json!(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("texe"))),
        ),
        ("texe.editor.enabled", serde_json::Value::Bool(true)),
        (
            "texe.editor.openPaper",
            serde_json::json!({
                "source": slash_path(&manifest.project.entry),
                "pdf": format!("{stem}.pdf"),
                "request": slash_path(&manifest.project.entry),
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

/// Pass the adoption failure to the first editor session for clickable Problems.
pub(super) fn record_editor_error(root: &Path, error: Option<&TexeError>) -> Result<(), TexeError> {
    let private = root.join(".texe");
    ensure_directory(&private, "editor state")?;
    let directory = private.join("editor");
    ensure_directory(&directory, "editor state")?;
    let path = directory.join("adoption-error.json");
    validate_settings_target(&path)?;
    if let Some(error) = error {
        let bytes =
            serde_json::to_vec(&crate::ux::ErrorEnvelope::from_error(error)).map_err(|source| {
                TexeError::Json {
                    path: path.clone(),
                    source,
                }
            })?;
        atomic::write(&path, &bytes)
    } else {
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(TexeError::Io { path, source }),
        }
    }
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
            serde_json::json!(std::env::current_exe().unwrap())
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
    fn merges_jsonc_and_restores_exact_original() {
        let directory = tempfile::tempdir().unwrap();
        write_manifest(directory.path(), "main.tex");
        fs::create_dir(directory.path().join(".vscode")).unwrap();
        let path = project_settings_path(directory.path());
        let original = "{\n // keep my comment\n \"editor.wordWrap\": \"on\",\n \"files.associations\": {\"*.custom\": \"latex\"},\n}\n";
        fs::write(&path, original).unwrap();
        configure(directory.path(), false).unwrap();
        let merged = fs::read_to_string(&path).unwrap();
        assert!(merged.contains("// keep my comment"));
        assert!(merged.contains("*.custom"));
        remove(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn preserves_explicit_null_when_other_settings_changed_after_setup() {
        let directory = tempfile::tempdir().unwrap();
        write_manifest(directory.path(), "main.tex");
        fs::create_dir(directory.path().join(".vscode")).unwrap();
        let path = project_settings_path(directory.path());
        fs::write(&path, r#"{"latex-workshop.view.pdf.viewer":null}"#).unwrap();
        configure(directory.path(), true).unwrap();
        let tree = super::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        tree.object_value()
            .unwrap()
            .append("editor.fontSize", 17.into());
        fs::write(&path, tree.to_string()).unwrap();
        remove(directory.path()).unwrap();
        let value = super::parse(&fs::read_to_string(path).unwrap())
            .unwrap()
            .to_serde_value()
            .unwrap();
        assert_eq!(
            value.get("latex-workshop.view.pdf.viewer"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(value["editor.fontSize"], 17);
        assert!(value.get("texe.editor.openPaper").is_none());
    }

    #[test]
    fn rejects_ambiguous_or_non_jsonc_settings() {
        for text in [
            r#"{"key":1,"key":2}"#,
            "{key:1}",
            r#"{"key":1 "other":2}"#,
            r#"{"key":0xFF}"#,
        ] {
            assert!(super::parse(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn conflicts_are_previewed_without_mutation() {
        let directory = tempfile::tempdir().unwrap();
        write_manifest(directory.path(), "main.tex");
        fs::create_dir(directory.path().join(".vscode")).unwrap();
        let path = project_settings_path(directory.path());
        let original = r#"{"latex-workshop.latex.autoBuild.run":"onSave","editor.wordWrap":"on"}"#;
        fs::write(&path, original).unwrap();
        let preview = super::preview(directory.path()).unwrap();
        assert_eq!(
            preview["conflicts"],
            serde_json::json!(["latex-workshop.latex.autoBuild.run"])
        );
        assert!(configure(directory.path(), false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(
            !directory
                .path()
                .join(".vscode/texe-integration.json")
                .exists()
        );
        configure(directory.path(), true).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("editor.wordWrap")
        );
        remove(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn reconfiguration_updates_paths_and_removal_preserves_later_edits() {
        let directory = tempfile::tempdir().unwrap();
        write_manifest(directory.path(), "main.tex");
        configure(directory.path(), false).unwrap();
        let path = project_settings_path(directory.path());
        write_manifest(directory.path(), "sources/paper.v2.tex");
        configure(directory.path(), false).unwrap();
        let tree = super::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            tree.to_serde_value().unwrap()["texe.editor.openPaper"]["pdf"],
            "paper.v2.pdf"
        );
        tree.object_value()
            .unwrap()
            .get("latex-workshop.view.pdf.viewer")
            .unwrap()
            .set_value("browser".into());
        tree.object_value()
            .unwrap()
            .append("editor.fontSize", 17.into());
        fs::write(&path, tree.to_string()).unwrap();
        remove(directory.path()).unwrap();
        let value = super::parse(&fs::read_to_string(path).unwrap())
            .unwrap()
            .to_serde_value()
            .unwrap();
        assert_eq!(value["editor.fontSize"], 17);
        assert_eq!(value["latex-workshop.view.pdf.viewer"], "browser");
        assert!(value.get("texe.editor.enabled").is_none());
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
