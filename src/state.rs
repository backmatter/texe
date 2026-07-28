//! The build fingerprint that lets an unchanged project skip a rebuild.
//!
//! `texe build` sits in an editing loop, where the common case is that nothing
//! has moved since the last successful build. The fingerprint covers every
//! input a rebuild would read — the effective manifest, the resolved toolchain
//! identity, the pinned build timestamp, the composite lock, and every project
//! file that lock names — so a match means the published artifact is already
//! the artifact a rebuild would produce.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::atomic;
use crate::config::ProjectManifest;
use crate::lockfile::LOCK_NAME;
use crate::toolchain::ToolchainIdentity;

pub const STATE_SCHEMA: &str = "texe.build-state/v1";
pub const STATE_NAME: &str = "build-state.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildState {
    pub schema: String,
    pub generated_with: String,
    pub fingerprint: String,
    pub environment_fingerprint: String,
    /// Files the build published, project-relative, with the digests they had
    /// when it finished. A hand-edited or deleted artifact fails this check
    /// and forces a rebuild.
    pub outputs: Vec<OutputDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputDigest {
    pub path: PathBuf,
    pub digest: String,
}

/// Read a previously recorded build state. An absent, unreadable, or
/// unsupported state file simply means the fast path is unavailable.
pub fn read(path: &Path) -> Option<BuildState> {
    let bytes = fs::read(path).ok()?;
    let state: BuildState = serde_json::from_slice(&bytes).ok()?;
    (state.schema == STATE_SCHEMA).then_some(state)
}

pub fn write(
    path: &Path,
    fingerprint: &str,
    environment_fingerprint: &str,
    project_root: &Path,
    outputs: &[PathBuf],
) -> Result<(), TexeError> {
    let state = BuildState {
        schema: STATE_SCHEMA.to_string(),
        generated_with: format!("texe/{}", env!("CARGO_PKG_VERSION")),
        fingerprint: fingerprint.to_string(),
        environment_fingerprint: environment_fingerprint.to_string(),
        outputs: outputs
            .iter()
            .map(|output| {
                let relative = output.strip_prefix(project_root).unwrap_or(output);
                let bytes = fs::read(output).map_err(|source| TexeError::Io {
                    path: output.clone(),
                    source,
                })?;
                Ok(OutputDigest {
                    path: relative.to_path_buf(),
                    digest: digest(&bytes),
                })
            })
            .collect::<Result<Vec<_>, TexeError>>()?,
    };
    let mut bytes = serde_json::to_vec_pretty(&state).map_err(|source| TexeError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    atomic::write(path, &bytes)
}

/// Digest every input a rebuild would read. Returns `None` when the project has
/// no composite lock yet, which is exactly when there is nothing to reuse.
pub fn build_fingerprint(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ToolchainIdentity,
    command_suite_fingerprint: &str,
    source_date_epoch: u64,
) -> Result<Option<String>, TexeError> {
    let Ok(lock_bytes) = fs::read(project_root.join(LOCK_NAME)) else {
        return Ok(None);
    };
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&lock_bytes) else {
        return Ok(None);
    };
    let mut hasher = Sha256::new();
    hasher.update(STATE_SCHEMA.as_bytes());
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(source_date_epoch.to_le_bytes());
    hasher.update(canonical_json(manifest, "texe.toml")?);
    hasher.update(canonical_json(toolchain, "toolchain identity")?);
    hasher.update(command_suite_fingerprint.as_bytes());
    hasher.update(&lock_bytes);
    for path in locked_project_files(&lock) {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        match read_project_file(project_root, &path)? {
            Some(bytes) => hasher.update(digest(&bytes)),
            None => hasher.update("absent"),
        }
        hasher.update(b"\0");
    }
    Ok(Some(digest_of(hasher)))
}

/// Confirm that every recorded output is still on disk with the bytes the
/// build left there, and return the primary artifact.
pub fn verify_outputs(project_root: &Path, outputs: &[OutputDigest]) -> Option<PathBuf> {
    let mut artifact = None;
    for output in outputs {
        let path = project_file(project_root, &output.path.to_string_lossy())?;
        let bytes = fs::read(&path).ok()?;
        if bytes.is_empty() || digest(&bytes) != output.digest {
            return None;
        }
        artifact.get_or_insert(path);
    }
    artifact
}

/// Every project file the package lock names: the scanned sources, plus the
/// bibliography databases, graphics, and inputs it resolved. Editing any of
/// them must produce a different PDF than the published one.
fn locked_project_files(lock: &serde_json::Value) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let Some(packages) = lock.get("packages") else {
        return paths;
    };
    if let Some(root) = packages.get("root").and_then(serde_json::Value::as_str) {
        paths.insert(root.to_string());
    }
    for source in array(packages, "sources") {
        if let Some(path) = source.get("path").and_then(serde_json::Value::as_str) {
            paths.insert(path.to_string());
        }
    }
    for field in ["inputs", "graphics", "bibliographies"] {
        for record in array(packages, field) {
            if let Some(path) = record
                .get("resolved_path")
                .and_then(serde_json::Value::as_str)
            {
                paths.insert(path.to_string());
            }
        }
    }
    paths
}

fn array<'a>(value: &'a serde_json::Value, field: &str) -> &'a [serde_json::Value] {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn read_project_file(project_root: &Path, path: &str) -> Result<Option<Vec<u8>>, TexeError> {
    let Some(path) = project_file(project_root, path) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    fs::read(&path)
        .map(Some)
        .map_err(|source| TexeError::Io { path, source })
}

/// Locked paths are project-relative by construction. One that is not stays
/// out of the fingerprint rather than reaching outside the project.
fn project_file(project_root: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(project_root.join(relative))
}

fn canonical_json<T: Serialize>(value: &T, what: &str) -> Result<Vec<u8>, TexeError> {
    serde_json::to_vec(value).map_err(|source| TexeError::Json {
        path: PathBuf::from(what),
        source,
    })
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    digest_of(hasher)
}

fn digest_of(hasher: Sha256) -> String {
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::ProjectManifest;
    use crate::lockfile::LOCK_NAME;
    use crate::state::{STATE_NAME, build_fingerprint, project_file, read, verify_outputs, write};
    use crate::toolchain::ToolchainIdentity;

    fn identity() -> ToolchainIdentity {
        ToolchainIdentity {
            provider: "managed".to_string(),
            engine: "pdflatex".to_string(),
            channel: "snapshot".to_string(),
            target: "x86_64-linux".to_string(),
            fingerprint: "abc".to_string(),
            registry_url: None,
            registry_metadata_digest: None,
            artifacts: Vec::new(),
        }
    }

    fn manifest() -> ProjectManifest {
        toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
        )
        .expect("manifest parses")
    }

    fn project() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::write(root.join("main.tex"), b"\\documentclass{article}").expect("entry");
        fs::write(root.join("references.bib"), b"@book{knuth,}").expect("bibliography");
        let lock = serde_json::json!({
            "schema": "texe.lock/v1",
            "generated_with": "texe/test",
            "toolchain": identity(),
            "packages": {
                "schema": "pqty.lock/v1",
                "root": "main.tex",
                "sources": [{"path": "main.tex", "digest": "sha256:0"}],
                "bibliographies": [
                    {"kind": "bibliography", "name": "references",
                     "resolved_path": "references.bib", "source": {}},
                    {"kind": "bibliography-style", "name": "plain",
                     "resolved_path": null, "source": {}}
                ],
            },
        });
        fs::write(
            root.join(LOCK_NAME),
            serde_json::to_vec_pretty(&lock).expect("lock"),
        )
        .expect("write lock");
        directory
    }

    fn fingerprint(root: &Path) -> String {
        build_fingerprint(
            root,
            &manifest(),
            &identity(),
            "sha256:command-suite",
            1_700_000_000,
        )
        .expect("fingerprint")
        .expect("locked project has a fingerprint")
    }

    #[test]
    fn an_unlocked_project_has_no_fingerprint() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            build_fingerprint(
                directory.path(),
                &manifest(),
                &identity(),
                "sha256:command-suite",
                0,
            )
            .expect("fingerprint"),
            None
        );
    }

    #[test]
    fn editing_any_locked_project_file_changes_the_fingerprint() {
        let directory = project();
        let root = directory.path();
        let initial = fingerprint(root);
        assert_eq!(initial, fingerprint(root));

        fs::write(root.join("main.tex"), b"\\documentclass{report}").expect("edit entry");
        let edited_entry = fingerprint(root);
        assert_ne!(initial, edited_entry);

        fs::write(root.join("main.tex"), b"\\documentclass{article}").expect("restore entry");
        assert_eq!(initial, fingerprint(root));

        fs::write(root.join("references.bib"), b"@book{lamport,}").expect("edit bibliography");
        assert_ne!(initial, fingerprint(root));
    }

    #[test]
    fn editing_a_nested_entry_changes_the_fingerprint() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::create_dir_all(root.join("paper")).expect("paper directory");
        fs::write(root.join("paper/main.tex"), b"first").expect("nested entry");
        let lock = serde_json::json!({
            "schema": "texe.lock/v1",
            "generated_with": "texe/test",
            "toolchain": identity(),
            "packages": {
                "schema": "pqty.lock/v1",
                "root": "paper/main.tex",
                "sources": [{"path": "paper/main.tex", "digest": "sha256:0"}],
            },
        });
        fs::write(
            root.join(LOCK_NAME),
            serde_json::to_vec_pretty(&lock).expect("lock"),
        )
        .expect("write lock");
        let mut manifest = manifest();
        manifest.project.entry = PathBuf::from("paper/main.tex");
        let initial = build_fingerprint(
            root,
            &manifest,
            &identity(),
            "sha256:command-suite",
            1_700_000_000,
        )
        .expect("fingerprint")
        .expect("locked project");

        fs::write(root.join("paper/main.tex"), b"second").expect("edit nested entry");
        let edited = build_fingerprint(
            root,
            &manifest,
            &identity(),
            "sha256:command-suite",
            1_700_000_000,
        )
        .expect("fingerprint")
        .expect("locked project");
        assert_ne!(initial, edited);
    }

    #[test]
    fn a_deleted_locked_file_changes_the_fingerprint() {
        let directory = project();
        let root = directory.path();
        let initial = fingerprint(root);
        fs::remove_file(root.join("references.bib")).expect("remove bibliography");
        assert_ne!(initial, fingerprint(root));
    }

    #[test]
    fn the_timestamp_toolchain_and_manifest_all_participate() {
        let directory = project();
        let root = directory.path();
        let initial = fingerprint(root);

        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &manifest(),
                &identity(),
                "sha256:command-suite",
                1_700_000_001,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );

        let mut other = identity();
        other.fingerprint = "def".to_string();
        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &manifest(),
                &other,
                "sha256:command-suite",
                1_700_000_000,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );

        let mut other_manifest = manifest();
        other_manifest.toolchain.max_passes += 1;
        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &other_manifest,
                &identity(),
                "sha256:command-suite",
                1_700_000_000,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );

        assert_ne!(
            initial,
            build_fingerprint(
                root,
                &manifest(),
                &identity(),
                "sha256:different-command-suite",
                1_700_000_000,
            )
            .expect("fingerprint")
            .expect("fingerprint")
        );
    }

    #[test]
    fn recorded_outputs_must_still_be_on_disk_unchanged() {
        let directory = project();
        let root = directory.path();
        let artifact = root.join("main.pdf");
        fs::write(&artifact, b"%PDF-test").expect("artifact");
        fs::write(root.join("main.synctex.gz"), b"sync").expect("synctex");
        let state_path = root.join(".texe/build").join(STATE_NAME);
        write(
            &state_path,
            "sha256:fingerprint",
            "sha256:environment",
            root,
            &[artifact.clone(), root.join("main.synctex.gz")],
        )
        .expect("state is recorded");

        let state = read(&state_path).expect("state is readable");
        assert_eq!(state.fingerprint, "sha256:fingerprint");
        assert_eq!(state.outputs[0].path, Path::new("main.pdf"));
        assert_eq!(verify_outputs(root, &state.outputs), Some(artifact.clone()));

        fs::write(&artifact, b"%PDF-edited").expect("edit artifact");
        assert_eq!(verify_outputs(root, &state.outputs), None);

        fs::remove_file(&artifact).expect("remove artifact");
        assert_eq!(verify_outputs(root, &state.outputs), None);
    }

    #[test]
    fn locked_paths_cannot_reach_outside_the_project() {
        assert_eq!(project_file(Path::new("/project"), "../secret"), None);
        assert_eq!(project_file(Path::new("/project"), "/etc/passwd"), None);
        assert_eq!(
            project_file(Path::new("/project"), "sections/one.tex"),
            Some(PathBuf::from("/project/sections/one.tex"))
        );
    }
}
