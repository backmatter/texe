//! Preparation and activation of portable, hosted project environments.
pub(crate) mod native;
pub(crate) mod retention;
use crate::{
    TexeError, atomic,
    config::ProjectManifest,
    guard::BuildGuard,
    package::PqtyClient,
    progress::{Progress, ProgressLayout},
    toolchain::{ManagedToolchainProvider, ToolchainProvider, VerificationPolicy},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const ACTIVE: &str = ".texe/environment.json";
fn error(message: impl Into<String>) -> TexeError {
    TexeError::Build(message.into())
}
fn read(path: &Path) -> Result<Vec<u8>, TexeError> {
    fs::read(path).map_err(|source| TexeError::Io {
        path: path.to_owned(),
        source,
    })
}
fn parse(path: &Path) -> Result<Value, TexeError> {
    if fs::metadata(path)
        .map_err(|source| TexeError::Io {
            path: path.to_owned(),
            source,
        })?
        .len()
        > 64 * 1024 * 1024
    {
        return Err(error("Environment metadata is too large"));
    }
    let bytes = read(path)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(error("Environment metadata is too large"));
    }
    serde_json::from_slice(&bytes).map_err(|source| TexeError::Json {
        path: path.to_owned(),
        source,
    })
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, TexeError> {
    value[key]
        .as_str()
        .ok_or_else(|| error(format!("Invalid environment field {key}")))
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn validate_identity(value: &Value) -> Result<(), TexeError> {
    let id = text(value, "id")?;
    let mut payload = value.clone();
    payload
        .as_object_mut()
        .ok_or_else(|| error("Invalid environment object"))?
        .remove("id");
    if id
        != format!(
            "sha256:{}",
            digest(
                &serde_json::to_vec(&payload).map_err(|_| error("Invalid environment identity"))?
            )
        )
    {
        return Err(error("Environment metadata checksum mismatch"));
    }
    Ok(())
}
#[allow(clippy::case_sensitive_file_extension_comparisons)] // The environment schema requires the exact .tex suffix.
pub(crate) fn validate(value: &Value) -> Result<(), TexeError> {
    if value["schema"] != "texe.environment/v1"
        || value["profile"]["schema"] != "texe.hosted-profile/v1"
        || value["packages"]["schema"] != "pqty.lock/v1"
        || value["packages"]["stage"] != "exact"
        || value["source_date_epoch"].as_u64().is_none()
    {
        return Err(error("Unsupported portable project environment"));
    }
    validate_identity(value)?;
    validate_identity(&value["profile"])?;
    let project = text(value, "projectId")?;
    if project.is_empty()
        || project.len() > 128
        || !project
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(error("Invalid environment project identity"));
    }
    if let Some(bibliography) = value.get("managedBibliography")
        && text(bibliography, "content")?.len() > 2 * 1024 * 1024
    {
        return Err(error("Managed bibliography is too large"));
    }
    let entry = text(value, "entry")?;
    if !entry.ends_with(".tex")
        || entry.contains('\\')
        || entry.contains('\0')
        || entry.contains(':')
        || entry
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(error("Unsafe environment entry path"));
    }
    if !matches!(
        text(value, "engine")?,
        "pdflatex" | "xelatex" | "lualatex" | "shipout" | "tectonic"
    ) {
        return Err(error("Unsupported environment engine"));
    }
    Ok(())
}

/// Materialize managed citations beside build intermediates, keeping source bytes
/// and line numbers intact. The hosted source materializer follows the same rules.
pub(crate) fn entry_for_build(
    root: &Path,
    manifest: &ProjectManifest,
    output: &Path,
    descriptor: Option<&Value>,
) -> Result<PathBuf, TexeError> {
    let original = root.join(&manifest.project.entry);
    let Some(bibliography) = descriptor.and_then(|v| v.get("managedBibliography")) else {
        return Ok(original);
    };
    let source = fs::read_to_string(&original).map_err(|source| TexeError::Io {
        path: original.clone(),
        source,
    })?;
    let directory = output.join(".texe-generated");
    private_directory(
        root,
        directory
            .strip_prefix(root)
            .map_err(|_| error("Generated input is outside project"))?,
    )?;
    atomic::write(
        &directory.join("citet-citations.bib"),
        text(bibliography, "content")?.as_bytes(),
    )?;
    let modified = inject_managed_bibliography(&source);
    let entry = directory.join(
        manifest
            .project
            .entry
            .file_name()
            .ok_or_else(|| error("Invalid entry"))?,
    );
    atomic::write(&entry, modified.as_bytes())?;
    Ok(entry)
}

fn inject_managed_bibliography(source: &str) -> String {
    let has_biblatex = regex::Regex::new(r"\\usepackage(?:\s*\[[^\]]*\])?\s*\{[^}]*\bbiblatex\b[^}]*\}|\\(?:addbibresource|printbibliography)\b").unwrap();
    if has_biblatex.is_match(source) {
        return source.into();
    }
    let bibliography = regex::Regex::new(r"\\bibliography\s*\{([^}]*)\}").unwrap();
    let style = regex::Regex::new(r"\\bibliographystyle\s*\{[^}]+\}").unwrap();
    if let Some(found) = bibliography.captures(source) {
        let sources = found[1]
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
        if sources.contains(&"citet-citations") {
            return source.into();
        }
        let command = found.get(0).unwrap();
        let mut replacement = String::new();
        if !style.is_match(source) {
            replacement.push_str(r"\bibliographystyle{plain}");
        }
        write!(
            replacement,
            "\\bibliography{{{},citet-citations}}",
            sources.join(",")
        )
        .expect("write to String");
        // Retain any newlines in the old command so following source locations
        // continue to point to the researcher's original document.
        replacement
            .push_str(&"\n".repeat(command.as_str().bytes().filter(|b| *b == b'\n').count()));
        return format!(
            "{}{}{}",
            &source[..command.start()],
            replacement,
            &source[command.end()..]
        );
    }
    let injection = r"\bibliographystyle{plain}\bibliography{citet-citations}";
    if source.contains(r"\end{document}") {
        source.replacen(
            r"\end{document}",
            &format!("{injection}\\end{{document}}"),
            1,
        )
    } else {
        format!("{source}{injection}")
    }
}
pub(crate) fn active(root: &Path) -> Result<Option<Value>, TexeError> {
    let path = root.join(ACTIVE);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(TexeError::Io { path, source }),
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            Err(error("Environment metadata must be a regular file"))
        }
        Ok(_) => {
            let value = parse(&path)?;
            validate(&value)?;
            Ok(Some(value))
        }
    }
}
pub(crate) fn directory(value: &Value) -> Result<PathBuf, TexeError> {
    let id = text(value, "id")?
        .strip_prefix("sha256:")
        .ok_or_else(|| error("Invalid environment identity"))?;
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(error("Invalid environment identity"));
    }
    Ok(PathBuf::from(".texe/environments").join(id))
}
pub(crate) fn manifest(value: &Value) -> Result<ProjectManifest, TexeError> {
    let directory = directory(value)?;
    let path = directory.to_string_lossy().replace('\\', "/");
    let engine = text(value, "engine")?;
    let channel = text(&value["profile"], "runtimeRecipe")?;
    let provider = if matches!(engine, "shipout" | "tectonic") {
        "portable"
    } else {
        "managed"
    };
    let source = format!(
        "schema = \"texe.project/v1\"\n[project]\nentry = {}\nbuild_dir = \"{path}/build\"\n[toolchain]\nprovider = \"{provider}\"\nengine = \"{engine}\"\nchannel = {}\n[packages]\nlock = \"{path}/pqty.lock\"\ntexmf = \"{path}/texmf\"\nremote = true\n",
        serde_json::to_string(text(value, "entry")?).unwrap(),
        serde_json::to_string(channel).unwrap()
    );
    let result: ProjectManifest = toml::from_str(&source).map_err(|source| TexeError::Toml {
        path: PathBuf::from(ACTIVE),
        source,
    })?;
    result.validate()?;
    Ok(result)
}
/// Preserve local, declarative source lookup choices while the hosted
/// descriptor remains authoritative for the engine and derived state.
pub(crate) fn manifest_for_project(
    root: &Path,
    value: &Value,
) -> Result<ProjectManifest, TexeError> {
    let mut result = manifest(value)?;
    let path = root.join(crate::config::MANIFEST_NAME);
    match fs::symlink_metadata(&path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(TexeError::Io { path, source }),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            let local = ProjectManifest::load(&path)?;
            result.project.generated = local.project.generated;
            result.inputs.roots = local.inputs.roots;
            result.bibliography.roots = local.bibliography.roots;
            result.toolchain.max_passes = local.toolchain.max_passes;
            result.validate()?;
        }
        Ok(_) => return Err(error("Local project configuration must be a regular file")),
    }
    let writable = root.join(&result.project.build_dir);
    for source in std::iter::once(&result.project.entry)
        .chain(result.inputs.roots.iter())
        .chain(result.bibliography.roots.iter())
    {
        let path = root.join(source);
        if path.starts_with(&writable)
            || path
                .canonicalize()
                .is_ok_and(|resolved| resolved.starts_with(&writable))
        {
            return Err(error(
                "Research source paths cannot use the environment's private build directory",
            ));
        }
    }
    Ok(result)
}
pub(crate) fn requires_biber(descriptor: &Value) -> bool {
    descriptor["packages"]["closure"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["provider"] == "biblatex"))
}

/// The descriptor owns this derived lock. Recreate it after clean, or repair
/// damaged cache bytes, without changing the project's selected environment.
pub(crate) fn restore_package_lock_from_descriptor(
    root: &Path,
    manifest: &ProjectManifest,
    descriptor: &Value,
) -> Result<(), TexeError> {
    let packages = &descriptor["packages"];
    if packages["schema"] != "pqty.lock/v1" || packages["stage"] != "exact" {
        return Err(error("The portable environment has no exact package lock"));
    }
    let path = root.join(&manifest.packages.lock);
    let parent = manifest
        .packages
        .lock
        .parent()
        .ok_or_else(|| error("Invalid portable package lock path"))?;
    private_directory(root, parent)?;
    if fs::symlink_metadata(&path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        && parse(&path).is_ok_and(|current| current == *packages)
    {
        return Ok(());
    }
    atomic::write(
        &path,
        &serde_json::to_vec(packages).map_err(|_| error("Invalid portable package lock"))?,
    )
}
pub(crate) fn private_directory(root: &Path, path: &Path) -> Result<(), TexeError> {
    let mut current = root.to_owned();
    for part in path.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(error(
                    "Environment preparation requires private regular directories",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current)
                .map_err(|source| TexeError::Io {
                    path: current.clone(),
                    source,
                })?,
            Err(source) => {
                return Err(TexeError::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}
pub(crate) fn prepare(
    root: &Path,
    descriptor: &Path,
    bundle: &Path,
    runtime: Option<&Path>,
    offline: bool,
) -> Result<Value, TexeError> {
    let root = root.canonicalize().map_err(|source| TexeError::Io {
        path: root.to_owned(),
        source,
    })?;
    private_directory(&root, Path::new(".texe/project"))?;
    let _guard = BuildGuard::acquire(&root.join(".texe/project"))?;
    let value = parse(descriptor)?;
    validate(&value)?;
    let expected_size = value["bundle"]["size"]
        .as_u64()
        .filter(|size| *size <= 512 * 1024 * 1024)
        .ok_or_else(|| error("Invalid environment bundle size"))?;
    let meta = fs::metadata(bundle).map_err(|source| TexeError::Io {
        path: bundle.to_owned(),
        source,
    })?;
    if meta.len() != expected_size {
        return Err(error("Environment bundle is incomplete"));
    }
    let bytes = read(bundle)?;
    if digest(&bytes) != text(&value["bundle"], "sha256")? {
        return Err(error("Environment bundle checksum mismatch"));
    }
    let manifest = manifest_for_project(&root, &value)?;
    let previous = active(&root)?;
    let candidate_path = directory(&value)?;
    let newly_created = !root.join(&candidate_path).exists();
    private_directory(&root, &candidate_path)?;
    let mut candidate = retention::Candidate::new(&root, &value, newly_created)?;
    let tools = PqtyClient::resolve(&root, &manifest, offline)?;
    let lock = root.join(&manifest.packages.lock);
    tools.import_bundle(&root, bundle, &lock)?;
    if parse(&lock)? != value["packages"] {
        return Err(error(
            "Environment package lock differs from the verified bundle",
        ));
    }
    let toolchain = if matches!(text(&value, "engine")?, "shipout" | "tectonic") {
        native::install(&value, runtime, VerificationPolicy::Interval)?;
        native::resolve(
            &root,
            &manifest,
            &value,
            VerificationPolicy::Interval,
            offline,
        )?
    } else {
        ManagedToolchainProvider.resolve(
            &root,
            &manifest.toolchain,
            VerificationPolicy::Interval,
            offline,
        )?
    };
    verify_engine(&value, &toolchain.engine_executable)?;
    let progress = Progress::new(
        crate::progress::history_path(&root)?,
        &manifest.toolchain.engine,
        true,
        manifest.toolchain.max_passes,
        false,
        false,
        ProgressLayout::Standalone,
    );
    crate::build::prepare_portable(&root, &manifest, &toolchain, &tools, &value, &progress)?;
    if requires_biber(&value) {
        if let Some(runtime) = &toolchain.managed {
            crate::toolchain::ensure_managed_biber(runtime)?;
        } else {
            native::prepared_biber(&value, toolchain.verification, offline)?;
        }
    }
    if matches!(text(&value, "engine")?, "shipout" | "tectonic") {
        native::build(
            &root,
            &manifest,
            &toolchain,
            &tools,
            &value,
            crate::build::BuildOptions {
                protected: true,
                frozen: true,
                force: true,
                offline: true,
            },
            &progress,
            false,
        )?;
    }
    // Last commit point: failures above never replace the previous environment.
    atomic::write(
        &root.join(ACTIVE),
        &serde_json::to_vec(&value).map_err(|_| error("Invalid environment metadata"))?,
    )?;
    candidate.commit(previous.as_ref());
    Ok(
        serde_json::json!({"schema":"texe.environment-report/v1", "id":value["id"], "ready":true, "engine":value["engine"]}),
    )
}
pub(crate) fn verify_engine(value: &Value, executable: &Path) -> Result<(), TexeError> {
    // Concurrent process startup can briefly retain a writer on a newly
    // extracted executable, even after the installing thread closed it.
    // Retry that specific OS state; other launch errors are immediately final.
    let mut retries = 0;
    let output = loop {
        match std::process::Command::new(executable)
            .arg("--version")
            .output()
        {
            Ok(output) => break output,
            Err(source)
                if source.kind() == std::io::ErrorKind::ExecutableFileBusy && retries < 4 =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10 << retries));
                retries += 1;
            }
            Err(source) => {
                return Err(TexeError::Io {
                    path: executable.to_owned(),
                    source,
                });
            }
        }
    };
    let version = String::from_utf8_lossy(&output.stdout);
    let expected = text(
        &value["profile"]["engines"][text(value, "engine")?],
        "version",
    )?;
    if !output.status.success() || version.lines().next().map(str::trim) != Some(expected.trim()) {
        return Err(error(
            "The local engine does not match the project's hosted engine. The last working environment has been kept.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        inject_managed_bibliography, manifest_for_project, restore_package_lock_from_descriptor,
    };
    use std::{fs, path::PathBuf};

    fn descriptor() -> serde_json::Value {
        let mut profile = serde_json::json!({
            "schema": "texe.hosted-profile/v1",
            "runtimeRecipe": "texlive-2026-07-26",
        });
        profile["id"] = format!(
            "sha256:{}",
            super::digest(&serde_json::to_vec(&profile).unwrap())
        )
        .into();
        let mut value = serde_json::json!({
            "schema": "texe.environment/v1", "projectId": "test-project",
            "engine": "pdflatex", "entry": "hosted/main.tex", "source_date_epoch": 1_700_000_000,
            "profile": profile,
            "packages": { "schema": "pqty.lock/v1", "stage": "exact", "closure": [] },
        });
        value["id"] = format!(
            "sha256:{}",
            super::digest(&serde_json::to_vec(&value).unwrap())
        )
        .into();
        value
    }

    #[test]
    fn portable_manifests_preserve_safe_local_roots_and_pass_limits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("texe.toml");
        let original = r#"schema = "texe.project/v1"
[project]
entry = "local.tex"
build_dir = ".texe/old-build"
[[project.generated]]
path = "generated.tex"
content = "locally generated"
[toolchain]
provider = "managed"
engine = "lualatex"
max_passes = 7
shell_escape = true
allow_unmanaged_commands = true
[inputs]
roots = ["styles", "figures"]
[bibliography]
roots = ["references"]
bibtex = "custom-bibtex"
[packages]
manager = "custom-pqty"
"#;
        fs::write(&path, original).unwrap();
        let manifest = manifest_for_project(directory.path(), &descriptor()).unwrap();
        assert_eq!(manifest.project.entry, PathBuf::from("hosted/main.tex"));
        assert_eq!(manifest.toolchain.engine, "pdflatex");
        assert_eq!(manifest.toolchain.max_passes, 7);
        assert_eq!(
            manifest.inputs.roots,
            vec![PathBuf::from("styles"), PathBuf::from("figures")]
        );
        assert_eq!(
            manifest.bibliography.roots,
            vec![PathBuf::from("references")]
        );
        assert!(manifest.project.build_dir.starts_with(".texe/environments"));
        assert!(manifest.packages.lock.starts_with(".texe/environments"));
        assert_eq!(manifest.project.generated.len(), 1);
        assert_eq!(
            manifest.project.generated[0].path,
            PathBuf::from("generated.tex")
        );
        assert_eq!(manifest.project.generated[0].content, "locally generated");
        assert!(!manifest.toolchain.shell_escape);
        assert!(!manifest.toolchain.allow_unmanaged_commands);
        assert_eq!(manifest.packages.manager, "pqty");
        assert_eq!(manifest.bibliography.bibtex, "bibtex");
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn unsafe_local_roots_are_rejected_without_rewriting_the_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("texe.toml");
        let original = "schema = \"texe.project/v1\"\n[project]\nentry = \"main.tex\"\n[toolchain]\nengine = \"pdflatex\"\n[inputs]\nroots = [\"../outside\"]\n";
        fs::write(&path, original).unwrap();
        assert!(manifest_for_project(directory.path(), &descriptor()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn declared_research_roots_cannot_enter_the_writable_portable_build() {
        let directory = tempfile::tempdir().unwrap();
        let value = descriptor();
        let build = super::manifest(&value).unwrap().project.build_dir;
        for section in ["inputs", "bibliography"] {
            let original = format!(
                "schema = \"texe.project/v1\"\n[project]\nentry = \"main.tex\"\n[toolchain]\nengine = \"pdflatex\"\n[{section}]\nroots = [{}]\n",
                serde_json::to_string(&build.join("research").to_string_lossy().replace('\\', "/"))
                    .unwrap()
            );
            let path = directory.path().join("texe.toml");
            fs::write(&path, &original).unwrap();
            let error = manifest_for_project(directory.path(), &value).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("Research source paths")
                    || message.contains("private .texe directory"),
                "{section}: {message}"
            );
            assert_eq!(fs::read_to_string(path).unwrap(), original);
            assert!(!directory.path().join(&build).exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_research_root_cannot_alias_the_writable_portable_build() {
        let directory = tempfile::tempdir().unwrap();
        let value = descriptor();
        let build = directory
            .path()
            .join(super::manifest(&value).unwrap().project.build_dir);
        fs::create_dir_all(&build).unwrap();
        fs::write(build.join("notes.tex"), b"research source").unwrap();
        std::os::unix::fs::symlink(&build, directory.path().join("research")).unwrap();
        let original = "schema = \"texe.project/v1\"\n[project]\nentry = \"main.tex\"\n[toolchain]\nengine = \"pdflatex\"\n[inputs]\nroots = [\"research\"]\n";
        let path = directory.path().join("texe.toml");
        fs::write(&path, original).unwrap();
        let error = manifest_for_project(directory.path(), &value).unwrap_err();
        assert!(error.to_string().contains("Research source paths"));
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        assert_eq!(
            fs::read(build.join("notes.tex")).unwrap(),
            b"research source"
        );
    }
    #[test]
    fn cleaning_a_portable_project_can_restore_its_exact_lock_offline() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let value = descriptor();
        let manifest = manifest_for_project(root, &value).unwrap();
        super::private_directory(root, &super::directory(&value).unwrap()).unwrap();
        fs::write(
            root.join(super::ACTIVE),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let lock = root.join(&manifest.packages.lock);
        restore_package_lock_from_descriptor(root, &manifest, &value).unwrap();
        fs::create_dir_all(root.join(&manifest.project.build_dir)).unwrap();
        fs::write(
            root.join(&manifest.project.build_dir).join("old.aux"),
            b"derived",
        )
        .unwrap();
        fs::write(root.join("main.tex"), b"research source").unwrap();
        fs::write(root.join("main.pdf"), b"last PDF").unwrap();
        let mut report = crate::clean::CleanReport {
            schema: "texe.clean-report/v1".into(),
            project: Vec::new(),
            caches: Vec::new(),
            freed_bytes: 0,
        };
        crate::clean::clean_project(root, &manifest, &mut report).unwrap();
        assert!(!lock.exists());
        let active = super::active(root)
            .unwrap()
            .expect("clean keeps the selected environment");
        restore_package_lock_from_descriptor(root, &manifest, &active).unwrap();
        assert_eq!(super::parse(&lock).unwrap(), value["packages"]);
        fs::write(&lock, b"damaged lock").unwrap();
        restore_package_lock_from_descriptor(root, &manifest, &active).unwrap();
        assert_eq!(super::parse(&lock).unwrap(), value["packages"]);
        assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"research source");
        assert_eq!(fs::read(root.join("main.pdf")).unwrap(), b"last PDF");
        assert_eq!(super::active(root).unwrap().unwrap(), value);
    }

    #[cfg(unix)]
    #[test]
    fn repairing_a_linked_derived_lock_keeps_the_target_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let value = descriptor();
        let manifest = manifest_for_project(root, &value).unwrap();
        restore_package_lock_from_descriptor(root, &manifest, &value).unwrap();
        let lock = root.join(&manifest.packages.lock);
        fs::remove_file(&lock).unwrap();
        let outside = root.join("research.tex");
        fs::write(&outside, b"research source").unwrap();
        std::os::unix::fs::symlink(&outside, &lock).unwrap();
        restore_package_lock_from_descriptor(root, &manifest, &value).unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"research source");
        assert!(
            !fs::symlink_metadata(&lock)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(super::parse(&lock).unwrap(), value["packages"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_briefly_busy_engine_is_verified_after_its_writer_closes() {
        use std::{io::Write as _, os::unix::fs::PermissionsExt as _};
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("engine");
        let mut writer = fs::File::create(&executable).unwrap();
        writer
            .write_all(b"#!/bin/sh\necho 'test engine version'\n")
            .unwrap();
        writer
            .set_permissions(fs::Permissions::from_mode(0o755))
            .unwrap();
        writer.sync_all().unwrap();
        let closing = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(40));
            drop(writer);
        });
        let mut value = descriptor();
        value["profile"]["engines"]["pdflatex"]["version"] = "test engine version".into();
        super::verify_engine(&value, &executable).expect("retry the transient busy state");
        closing.join().unwrap();
    }

    #[test]
    fn managed_citations_preserve_original_source_line_numbers() {
        let source = "\\documentclass{article}\n\\begin{document}\\cite{managed}\n\\bibliography{local}\n\\end{document}\n";
        let modified = inject_managed_bibliography(source);
        assert!(
            modified.contains("\\bibliographystyle{plain}\\bibliography{local,citet-citations}")
        );
        assert_eq!(modified.lines().count(), source.lines().count());
        assert_eq!(inject_managed_bibliography(&modified), modified);
        let source = "\\usepackage{biblatex}\n\\printbibliography";
        assert_eq!(inject_managed_bibliography(source), source);
    }
}
