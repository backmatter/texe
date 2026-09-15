use std::path::Path;

use crate::{TexeError, config::ProjectManifest};

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ownership {
    original: Option<String>,
    pub(super) installed: String,
    edits: Vec<Edit>,
    #[serde(default)]
    exclusions: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    key: String,
    before: Option<String>,
    after: String,
}

fn parse(text: &str) -> Result<toml_edit::DocumentMut, TexeError> {
    text.parse()
        .map_err(|error| TexeError::Build(format!("invalid tex-ls.toml: {error}")))
}

pub(super) fn plan(
    root: &Path,
    manifest: &ProjectManifest,
    ownership: Option<Ownership>,
) -> Result<(Ownership, String, Vec<String>), TexeError> {
    let original = super::settings::read_optional(&root.join("tex-ls.toml"))?;
    let mut document = parse(original.as_deref().unwrap_or(""))?;
    let mut ownership = ownership.unwrap_or_else(|| Ownership {
        original,
        ..Ownership::default()
    });
    update_exclusions(&mut document, manifest, &mut ownership.exclusions)?;
    if !document.contains_key("build") {
        document["build"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let build = document["build"]
        .as_table_like_mut()
        .ok_or_else(|| TexeError::Build("tex-ls.toml build must be a table".into()))?;
    let desired = [
        ("root", super::settings::slash_path(&manifest.project.entry)),
        (
            "aux-dir",
            super::settings::slash_path(&manifest.project.build_dir.join("output")),
        ),
        ("pdf-dir", ".".into()),
        (
            "job-name",
            manifest
                .project
                .entry
                .file_stem()
                .expect("validated entry")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    let mut conflicts = Vec::new();
    for (key, after) in desired {
        let current = build.get(key);
        if current.and_then(toml_edit::Item::as_str) == Some(after.as_str()) {
            continue;
        }
        if current.is_some_and(|item| !item.is_value()) {
            return Err(TexeError::Build(format!(
                "tex-ls.toml build.{key} must be a value"
            )));
        }
        let before = current.map(ToString::to_string);
        let owned = ownership.edits.iter_mut().find(|edit| edit.key == key);
        if current.is_some()
            && owned.as_ref().is_none_or(|edit| {
                current.and_then(toml_edit::Item::as_str) != Some(edit.after.as_str())
            })
        {
            conflicts.push(format!("tex-ls.toml / build / {key}"));
        }
        // Reuse the value's decoration, including a trailing user comment.
        let mut value = toml_edit::Value::from(after.clone());
        if let Some(old) = current.and_then(toml_edit::Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        build.insert(key, toml_edit::Item::Value(value));
        if let Some(edit) = owned {
            edit.after = after;
        } else {
            ownership.edits.push(Edit {
                key: key.into(),
                before,
                after,
            });
        }
    }
    let proposed = document.to_string();
    ownership.installed.clone_from(&proposed);
    Ok((ownership, proposed, conflicts))
}

pub(super) fn restore(root: &Path, ownership: &Ownership) -> Result<Option<String>, TexeError> {
    let Some(current) = super::settings::read_optional(&root.join("tex-ls.toml"))? else {
        return Ok(None);
    };
    if current == ownership.installed {
        return Ok(ownership.original.clone());
    }
    let mut document = parse(&current)?;
    if let Some(build) = document
        .get_mut("build")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        for edit in &ownership.edits {
            if build.get(&edit.key).and_then(toml_edit::Item::as_str) != Some(edit.after.as_str()) {
                continue;
            }
            if let Some(before) = &edit.before {
                let mut old = parse(&format!("value = {before}"))?;
                build.insert(&edit.key, old.remove("value").expect("saved value"));
            } else {
                build.remove(&edit.key);
            }
        }
    }
    if let Some(array) = document
        .get_mut("extend-exclude")
        .and_then(toml_edit::Item::as_array_mut)
    {
        for index in (0..array.len()).rev() {
            if array
                .get(index)
                .and_then(toml_edit::Value::as_str)
                .is_some_and(|value| ownership.exclusions.iter().any(|owned| owned == value))
            {
                array.remove(index);
            }
        }
        if array.is_empty()
            && !parse(ownership.original.as_deref().unwrap_or(""))?.contains_key("extend-exclude")
        {
            document.remove("extend-exclude");
        }
    }
    Ok(Some(document.to_string()))
}

// Add only generated directories; keep user patterns and their decoration.
fn update_exclusions(
    document: &mut toml_edit::DocumentMut,
    manifest: &ProjectManifest,
    owned: &mut Vec<String>,
) -> Result<(), TexeError> {
    let mut desired = Vec::new();
    for path in [&manifest.project.build_dir, &manifest.packages.texmf] {
        let mut pattern = String::from("/");
        for character in super::settings::slash_path(path).chars() {
            if "*?[]!#\\".contains(character) {
                pattern.push('\\');
            }
            pattern.push(character);
        }
        pattern.push('/');
        if !desired.contains(&pattern) {
            desired.push(pattern);
        }
    }
    if !document.contains_key("extend-exclude") {
        document["extend-exclude"] = toml_edit::value(toml_edit::Array::new());
    }
    let array = document
        .get_mut("extend-exclude")
        .and_then(toml_edit::Item::as_array_mut)
        .filter(|array| array.iter().all(|value| value.as_str().is_some()))
        .ok_or_else(|| {
            TexeError::Build("tex-ls.toml extend-exclude must be an array of strings".into())
        })?;
    for index in (0..array.len()).rev() {
        let value = array
            .get(index)
            .and_then(toml_edit::Value::as_str)
            .expect("validated string");
        if owned.iter().any(|pattern| pattern == value)
            && !desired.iter().any(|pattern| pattern == value)
        {
            array.remove(index);
        }
    }
    owned.retain(|pattern| desired.contains(pattern));
    for pattern in desired {
        if !array.iter().any(|value| value.as_str() == Some(&pattern)) {
            array.push(pattern.clone());
            if !owned.contains(&pattern) {
                owned.push(pattern);
            }
        }
    }
    Ok(())
}
