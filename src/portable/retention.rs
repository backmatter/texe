//! Retire only owned, inactive project environments under the project lock.
use super::{active, directory, error, parse, text, validate};
use crate::{TexeError, atomic};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const OWNER: &str = ".texe-environment.json";
const STATE: &str = ".texe/environment-retention.json";

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retention {
    project_id: String,
    previous: Option<String>,
    last_built: Option<String>,
}

fn state(root: &Path, project: &str) -> Result<Retention, TexeError> {
    let path = root.join(STATE);
    match fs::symlink_metadata(&path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Retention {
            project_id: project.into(),
            ..Retention::default()
        }),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            let value: Retention = serde_json::from_value(parse(&path)?)
                .map_err(|_| error("Invalid environment retention record"))?;
            if value.project_id != project {
                return Err(error("Environment retention identity changed"));
            }
            Ok(value)
        }
        _ => Err(error(
            "Environment retention requires a regular private file",
        )),
    }
}

fn save(root: &Path, value: &Retention) -> Result<(), TexeError> {
    atomic::write(
        &root.join(STATE),
        &serde_json::to_vec(value).map_err(|_| error("Could not save environment retention"))?,
    )
}

/// A failed new preparation owns no source and can remove its private tree.
pub(super) struct Candidate {
    root: PathBuf,
    value: Value,
    committed: bool,
}

impl Candidate {
    pub(super) fn new(root: &Path, value: &Value, newly_created: bool) -> Result<Self, TexeError> {
        if newly_created {
            atomic::write(
                &root.join(directory(value)?).join(OWNER),
                &serde_json::to_vec(value).map_err(|_| error("Invalid environment owner"))?,
            )?;
        }
        Ok(Self {
            root: root.into(),
            value: value.clone(),
            committed: false,
        })
    }

    pub(super) fn commit(&mut self, previous: Option<&Value>) {
        self.committed = true;
        let result = (|| {
            let mut record = state(&self.root, text(&self.value, "projectId")?)?;
            if let Some(previous) = previous
                && previous["id"] != self.value["id"]
            {
                record.previous = Some(text(previous, "id")?.into());
            }
            save(&self.root, &record)?;
            prune(&self.root, &self.value, &record)
        })();
        if let Err(error) = result {
            eprintln!("texe: could not retire inactive build state: {error}");
        }
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let protected = active(&self.root).ok().flatten();
        // A damaged active pointer makes ownership uncertain; preserve caches.
        if active(&self.root).is_err() {
            return;
        }
        if protected
            .as_ref()
            .is_some_and(|value| value["id"] == self.value["id"])
        {
            return;
        }
        if self.root.join(STATE).exists() {
            let Ok(record) = state(&self.root, self.value["projectId"].as_str().unwrap_or(""))
            else {
                return;
            };
            let identity = self.value["id"].as_str().unwrap_or("");
            if record.previous.as_deref() == Some(identity)
                || record.last_built.as_deref() == Some(identity)
            {
                return;
            }
        }
        let _ = remove_owned(&self.root, &self.value);
    }
}

pub(crate) fn built(root: &Path, identity: &str) {
    let result = (|| {
        let Some(current) = active(root)? else {
            return Ok(());
        };
        if text(&current, "id")? != identity {
            return Ok(());
        }
        let mut record = state(root, text(&current, "projectId")?)?;
        record.last_built = Some(identity.into());
        save(root, &record)?;
        prune(root, &current, &record)
    })();
    if let Err(error) = result {
        eprintln!("texe: could not retire inactive build state: {error}");
    }
}

fn prune(root: &Path, current: &Value, record: &Retention) -> Result<(), TexeError> {
    let mut keep = BTreeSet::from([text(current, "id")?.to_owned()]);
    keep.extend(record.previous.iter().cloned());
    keep.extend(record.last_built.iter().cloned());
    let parent = root.join(".texe/environments");
    super::private_directory(root, Path::new(".texe/environments"))?;
    for entry in fs::read_dir(&parent).map_err(|source| TexeError::Io {
        path: parent,
        source,
    })? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let owner = path.join(OWNER);
        let Ok(metadata) = fs::symlink_metadata(&owner) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(value) = parse(&owner) else {
            continue;
        };
        if validate(&value).is_err() || value["projectId"] != current["projectId"] {
            continue;
        }
        if directory(&value).is_err() || root.join(directory(&value)?) != path {
            continue;
        }
        let Ok(id) = text(&value, "id") else {
            continue;
        };
        if keep.contains(id) {
            continue;
        }
        // Explicit source lookup roots take priority over cache reclamation.
        let manifest = super::manifest_for_project(root, current)?;
        if std::iter::once(&manifest.project.entry)
            .chain(manifest.inputs.roots.iter())
            .chain(manifest.bibliography.roots.iter())
            .any(|source| root.join(source).starts_with(&path))
        {
            continue;
        }
        remove_owned(root, &value)?;
    }
    Ok(())
}

fn remove_owned(root: &Path, value: &Value) -> Result<(), TexeError> {
    validate(value)?;
    super::private_directory(root, Path::new(".texe/environments"))?;
    let path = root.join(directory(value)?);
    let metadata = fs::symlink_metadata(&path).map_err(|source| TexeError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    if protects_source(root, &path, value)? {
        return Ok(());
    }
    let marker = path.join(OWNER);
    let Ok(metadata) = fs::symlink_metadata(&marker) else {
        return Ok(());
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || parse(&marker)? != *value {
        return Ok(());
    }
    // Unknown files may be user work. Never infer ownership from a hash name.
    for entry in fs::read_dir(&path).map_err(|source| TexeError::Io {
        path: path.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        if !matches!(
            entry.file_name().to_str(),
            Some(OWNER | "build" | "texmf" | "pqty.lock")
        ) {
            return Ok(());
        }
    }
    fs::remove_dir_all(&path).map_err(|source| TexeError::Io { path, source })
}

fn protects_source(root: &Path, directory: &Path, value: &Value) -> Result<bool, TexeError> {
    let mut manifests = vec![super::manifest_for_project(root, value)?];
    if let Some(current) = active(root)? {
        manifests.push(super::manifest_for_project(root, &current)?);
    }
    let local = root.join(crate::config::MANIFEST_NAME);
    if local.exists() {
        manifests.push(crate::config::ProjectManifest::load(&local)?);
    }
    let candidate = fs::canonicalize(directory).map_err(|source| TexeError::Io {
        path: directory.into(),
        source,
    })?;
    for manifest in manifests {
        for source in std::iter::once(&manifest.project.entry)
            .chain(manifest.inputs.roots.iter())
            .chain(manifest.bibliography.roots.iter())
        {
            let path = root.join(source);
            if path.starts_with(directory) {
                return Ok(true);
            }
            match fs::canonicalize(&path) {
                Ok(path) if path.starts_with(&candidate) => return Ok(true),
                Ok(_) => {}
                // An unresolved explicit source root makes deletion uncertain.
                Err(_) => return Ok(true),
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(epoch: u64) -> Value {
        let mut profile = serde_json::json!({"schema":"texe.hosted-profile/v1", "runtimeRecipe":"texlive-2026-07-26"});
        profile["id"] = format!(
            "sha256:{}",
            super::super::digest(&serde_json::to_vec(&profile).unwrap())
        )
        .into();
        let mut value = serde_json::json!({"schema":"texe.environment/v1", "projectId":"retention-project",
            "profile":profile, "packages":{"schema":"pqty.lock/v1", "stage":"exact"},
            "entry":"main.tex", "engine":"pdflatex", "source_date_epoch":epoch});
        value["id"] = format!(
            "sha256:{}",
            super::super::digest(&serde_json::to_vec(&value).unwrap())
        )
        .into();
        value
    }

    fn prepared(root: &Path, value: &Value) -> Candidate {
        if !root.join("main.tex").exists() {
            fs::write(root.join("main.tex"), b"research source").unwrap();
        }
        super::super::private_directory(root, &directory(value).unwrap()).unwrap();
        let result = Candidate::new(root, value, true).unwrap();
        fs::create_dir(root.join(directory(value).unwrap()).join("build")).unwrap();
        fs::write(
            root.join(directory(value).unwrap()).join("build/last.pdf"),
            b"derived PDF",
        )
        .unwrap();
        result
    }

    fn activate(root: &Path, candidate: &mut Candidate, previous: Option<&Value>) {
        atomic::write(
            &root.join(super::super::ACTIVE),
            &serde_json::to_vec(&candidate.value).unwrap(),
        )
        .unwrap();
        candidate.commit(previous);
    }

    #[test]
    fn rotation_keeps_active_previous_and_last_successful_build() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("main.tex"), b"research source").unwrap();
        fs::write(root.join("main.pdf"), b"last good PDF").unwrap();
        let (a, b, c, d) = (value(1), value(2), value(3), value(4));
        let mut ca = prepared(root, &a);
        activate(root, &mut ca, None);
        built(root, a["id"].as_str().unwrap());
        let mut cb = prepared(root, &b);
        activate(root, &mut cb, Some(&a));
        let mut cc = prepared(root, &c);
        activate(root, &mut cc, Some(&b));
        let mut cd = prepared(root, &d);
        activate(root, &mut cd, Some(&c));
        assert!(root.join(directory(&a).unwrap()).exists());
        assert!(!root.join(directory(&b).unwrap()).exists());
        assert!(root.join(directory(&c).unwrap()).exists());
        built(root, d["id"].as_str().unwrap());
        assert!(!root.join(directory(&a).unwrap()).exists());
        assert!(root.join(directory(&c).unwrap()).exists());
        assert!(root.join(directory(&d).unwrap()).exists());
        assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"research source");
        assert_eq!(fs::read(root.join("main.pdf")).unwrap(), b"last good PDF");
    }

    #[test]
    fn failed_new_preparation_removes_only_its_owned_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let a = value(1);
        let b = value(2);
        let mut ca = prepared(root, &a);
        activate(root, &mut ca, None);
        drop(prepared(root, &b));
        assert!(!root.join(directory(&b).unwrap()).exists());
        assert!(root.join(directory(&a).unwrap()).exists());
        assert_eq!(active(root).unwrap().unwrap(), a);
    }

    #[test]
    fn unknown_files_and_unmarked_directories_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let a = value(1);
        let mut candidate = prepared(root, &a);
        fs::write(
            root.join(directory(&a).unwrap()).join("research.tex"),
            b"user work",
        )
        .unwrap();
        candidate.committed = true;
        remove_owned(root, &a).unwrap();
        assert_eq!(
            fs::read(root.join(directory(&a).unwrap()).join("research.tex")).unwrap(),
            b"user work"
        );
        fs::remove_file(root.join(directory(&a).unwrap()).join(OWNER)).unwrap();
        remove_owned(root, &a).unwrap();
        assert!(root.join(directory(&a).unwrap()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_environment_is_not_followed_or_removed() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let a = value(1);
        let root = temp.path();
        super::super::private_directory(root, Path::new(".texe/environments")).unwrap();
        fs::write(outside.path().join("source.tex"), b"outside research").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join(directory(&a).unwrap())).unwrap();
        remove_owned(root, &a).unwrap();
        assert_eq!(
            fs::read(outside.path().join("source.tex")).unwrap(),
            b"outside research"
        );
        assert!(
            fs::symlink_metadata(root.join(directory(&a).unwrap()))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_candidate_preserves_a_declared_source_root_alias() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let a = value(1);
        let candidate = prepared(root, &a);
        let owned = root.join(directory(&a).unwrap());
        fs::write(owned.join("build/research.tex"), b"declared source work").unwrap();
        std::os::unix::fs::symlink(owned.join("build"), root.join("styles")).unwrap();
        let mut manifest = super::super::manifest(&a).unwrap();
        manifest.inputs.roots = vec!["styles".into()];
        fs::write(
            root.join(crate::config::MANIFEST_NAME),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        drop(candidate);
        assert_eq!(
            fs::read(root.join("styles/research.tex")).unwrap(),
            b"declared source work"
        );
    }
}
