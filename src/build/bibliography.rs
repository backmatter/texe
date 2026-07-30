use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::build::auxiliary::{self, CachedOutput, OutputCache};
use crate::build::process::{
    managed_path, raw_bundled_biber_output, raw_engine_output, raw_output, search_path_from,
};
use crate::config::ProjectManifest;
use crate::progress::{PhaseKind, Progress};
use crate::toolchain::{
    ResolvedToolchain, ensure_bundled_biber, ensure_managed_biber, resolve_executable,
};

#[derive(Debug, Default)]
pub(super) struct BibliographyState {
    processed: BTreeMap<PathBuf, String>,
    cache: OutputCache,
    runs: usize,
}

impl BibliographyState {
    pub(super) fn from_cache(cache: BTreeMap<String, CachedOutput>) -> Self {
        Self {
            cache: OutputCache::from_entries(cache),
            ..Self::default()
        }
    }

    pub(super) fn runs(&self) -> usize {
        self.runs
    }

    pub(super) fn cache_entries(&self) -> BTreeMap<String, CachedOutput> {
        self.cache.retained_entries()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BibliographyProcessor {
    Bibtex,
    Biber,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BibliographyControl {
    path: PathBuf,
    processor: BibliographyProcessor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BibliographyExecutable {
    path: PathBuf,
    library_dir: Option<PathBuf>,
    par_cache: Option<PathBuf>,
}

pub(super) struct BibliographyContext<'a> {
    pub(super) project_root: &'a Path,
    pub(super) manifest: &'a ProjectManifest,
    pub(super) toolchain: &'a ResolvedToolchain,
    pub(super) texmf: &'a Path,
    pub(super) output_dir: &'a Path,
    pub(super) recorder_path: &'a Path,
    pub(super) progress: &'a Progress,
}

pub(super) fn process_pending(
    context: &BibliographyContext<'_>,
    state: &mut BibliographyState,
) -> Result<bool, TexeError> {
    let controls = bibliography_controls(
        context.project_root,
        context.output_dir,
        context.recorder_path,
    )?;
    let mut ran = false;
    for control in controls {
        let digest = persistent_control_digest(&control, context)?;
        if state.processed.get(&control.path) == Some(&digest) {
            continue;
        }
        let output = control.path.with_extension("bbl");
        let persistent_cache = persistent_cache_supported(&control, context.manifest);
        let cache_key = persistent_cache
            .then(|| auxiliary::relative_path(context.output_dir, &control.path))
            .flatten();
        if state
            .cache
            .restore(cache_key.as_deref(), &digest, context.output_dir, &output)
        {
            state.processed.insert(control.path, digest);
            continue;
        }
        let name = match control.processor {
            BibliographyProcessor::Bibtex => "BibTeX",
            BibliographyProcessor::Biber => "Biber",
        };
        let stem = control
            .path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("document");
        context.progress.phase(
            PhaseKind::Bibliography,
            format!("running {name} for {stem}"),
            || {
                run_bibliography_processor(
                    context.project_root,
                    context.manifest,
                    context.toolchain,
                    context.texmf,
                    context.output_dir,
                    &control,
                )
            },
        )?;
        state
            .cache
            .record(cache_key, &digest, context.output_dir, &output);
        state.processed.insert(control.path, digest);
        state.runs += 1;
        ran = true;
    }
    Ok(ran)
}

fn bibliography_controls(
    project_root: &Path,
    output_dir: &Path,
    recorder_path: &Path,
) -> Result<Vec<BibliographyControl>, TexeError> {
    let mut candidates = BTreeSet::new();
    if let Ok(recorder) = fs::read_to_string(recorder_path) {
        for line in recorder.lines() {
            let Some(path) = line.strip_prefix("OUTPUT ") else {
                continue;
            };
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                project_root.join(path)
            };
            if path.starts_with(output_dir)
                && matches!(
                    path.extension().and_then(OsStr::to_str),
                    Some("aux" | "bcf")
                )
                && path.is_file()
            {
                candidates.insert(path);
            }
        }
    }
    if candidates.is_empty() {
        collect_bibliography_control_files(output_dir, output_dir, &mut candidates)?;
    }

    let biber_stems = candidates
        .iter()
        .filter(|path| path.extension() == Some(OsStr::new("bcf")))
        .filter_map(|path| path.with_extension("").file_name().map(OsStr::to_os_string))
        .collect::<BTreeSet<_>>();
    let mut controls = Vec::new();
    for path in candidates {
        match path.extension().and_then(OsStr::to_str) {
            Some("bcf") => controls.push(BibliographyControl {
                path,
                processor: BibliographyProcessor::Biber,
            }),
            Some("aux")
                if path
                    .file_stem()
                    .is_none_or(|stem| !biber_stems.contains(stem))
                    && bibtex_control_file(&path)? =>
            {
                controls.push(BibliographyControl {
                    path,
                    processor: BibliographyProcessor::Bibtex,
                });
            }
            _ => {}
        }
    }
    controls.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(controls)
}

fn collect_bibliography_control_files(
    root: &Path,
    directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), TexeError> {
    for entry in fs::read_dir(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| TexeError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_bibliography_control_files(root, &path, paths)?;
        } else if file_type.is_file()
            && path.starts_with(root)
            && matches!(
                path.extension().and_then(OsStr::to_str),
                Some("aux" | "bcf")
            )
        {
            paths.insert(path);
        }
    }
    Ok(())
}

fn bibtex_control_file(path: &Path) -> Result<bool, TexeError> {
    let bytes = fs::read(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(bytes
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(br"\bibdata{"))
        && bytes
            .split(|byte| *byte == b'\n')
            .any(|line| line.starts_with(br"\bibstyle{")))
}

fn bibliography_control_digest(
    control: &BibliographyControl,
    output_dir: &Path,
) -> Result<String, TexeError> {
    let mut hasher = Sha256::new();
    match control.processor {
        BibliographyProcessor::Biber => {
            let bytes = fs::read(&control.path).map_err(|source| TexeError::Io {
                path: control.path.clone(),
                source,
            })?;
            hasher.update(bytes);
        }
        BibliographyProcessor::Bibtex => {
            let root = fs::canonicalize(output_dir).map_err(|source| TexeError::Io {
                path: output_dir.to_path_buf(),
                source,
            })?;
            hash_bibtex_auxiliary(&control.path, &root, &mut BTreeSet::new(), &mut hasher)?;
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn persistent_control_digest(
    control: &BibliographyControl,
    context: &BibliographyContext<'_>,
) -> Result<String, TexeError> {
    let mut hasher = Sha256::new();
    hasher.update(bibliography_control_digest(control, context.output_dir)?.as_bytes());
    if context.toolchain.managed.is_none()
        && control.processor == BibliographyProcessor::Bibtex
        && context.manifest.bibliography.bibtex == "bibtex"
    {
        let executable = resolve_executable(context.project_root, "bibtex")?;
        hasher.update(auxiliary::required_file_digest(&executable)?.as_bytes());
    }
    for path in project_bibliography_files(context.project_root, context.manifest)? {
        let relative = path.strip_prefix(context.project_root).unwrap_or(&path);
        hasher.update(relative.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        hasher.update(fs::read(&path).map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?);
        hasher.update(b"\0");
    }
    Ok(hex::encode(hasher.finalize()))
}

fn persistent_cache_supported(control: &BibliographyControl, manifest: &ProjectManifest) -> bool {
    match control.processor {
        BibliographyProcessor::Bibtex => manifest.bibliography.bibtex == "bibtex",
        BibliographyProcessor::Biber => manifest.bibliography.biber == "biber",
    }
}

fn project_bibliography_files(
    project_root: &Path,
    manifest: &ProjectManifest,
) -> Result<BTreeSet<PathBuf>, TexeError> {
    let excluded = [
        project_root.join(&manifest.project.build_dir),
        project_root.join(&manifest.packages.texmf),
    ];
    let mut files = BTreeSet::new();
    let mut pending = vec![project_root.to_path_buf()];
    pending.extend(
        manifest
            .bibliography
            .roots
            .iter()
            .map(|root| project_root.join(root)),
    );
    let mut visited = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        if excluded
            .iter()
            .any(|excluded| directory == *excluded || directory.starts_with(excluded))
            || !visited.insert(directory.clone())
        {
            continue;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(TexeError::Io {
                    path: directory,
                    source,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| TexeError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && matches!(
                    path.extension().and_then(OsStr::to_str),
                    Some("bib" | "bst")
                )
            {
                files.insert(path);
            }
        }
    }
    Ok(files)
}

fn hash_bibtex_auxiliary(
    path: &Path,
    output_root: &Path,
    visited: &mut BTreeSet<PathBuf>,
    hasher: &mut Sha256,
) -> Result<(), TexeError> {
    let canonical = fs::canonicalize(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.starts_with(output_root) || !visited.insert(canonical.clone()) {
        return Ok(());
    }
    let bytes = fs::read(&canonical).map_err(|source| TexeError::Io {
        path: canonical.clone(),
        source,
    })?;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.starts_with(br"\citation{")
            || line.starts_with(br"\bibdata{")
            || line.starts_with(br"\bibstyle{")
        {
            hasher.update(line);
            hasher.update(b"\n");
        } else if let Some(include) = bibtex_auxiliary_include(line) {
            hasher.update(line);
            hasher.update(b"\n");
            let child = canonical.parent().unwrap_or(output_root).join(include);
            if child.is_file() {
                hash_bibtex_auxiliary(&child, output_root, visited, hasher)?;
            }
        }
    }
    Ok(())
}

fn bibtex_auxiliary_include(line: &[u8]) -> Option<&str> {
    let value = line
        .strip_prefix(br"\@input{")?
        .strip_suffix(b"}")
        .and_then(|value| std::str::from_utf8(value).ok())?;
    (!value.is_empty()).then_some(value)
}

fn run_bibliography_processor(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    texmf: &Path,
    output_dir: &Path,
    control: &BibliographyControl,
) -> Result<(), TexeError> {
    let (name, configured) = match control.processor {
        BibliographyProcessor::Bibtex => ("BibTeX", manifest.bibliography.bibtex.as_str()),
        BibliographyProcessor::Biber => ("Biber", manifest.bibliography.biber.as_str()),
    };
    let executable =
        resolve_bibliography_executable(project_root, toolchain, control.processor, configured)?;
    let directory = control.path.parent().ok_or_else(|| {
        TexeError::Build(format!(
            "bibliography control file has no parent: {}",
            control.path.display()
        ))
    })?;
    let stem = control
        .path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            TexeError::Build(format!(
                "bibliography control file must have a UTF-8 stem: {}",
                control.path.display()
            ))
        })?;
    let bibtex_proxy = if control.processor == BibliographyProcessor::Bibtex {
        Some(BibtexControlProxy::create(&control.path, output_dir)?)
    } else {
        None
    };
    let processor_stem = bibtex_proxy
        .as_ref()
        .map_or(stem, |proxy| proxy.stem.as_str());
    let arguments = [OsString::from(processor_stem)];
    let mut environment =
        bibliography_environment(project_root, manifest, texmf, toolchain, directory);
    if let Some(managed) = &toolchain.managed
        && let Some((_, path)) = environment
            .iter_mut()
            .find(|(name, _)| name == OsStr::new("PATH"))
    {
        *path = managed_bibliography_path(&executable.path, &managed.binary_dir)?;
    }
    if let Some(library_dir) = &executable.library_dir
        && library_dir.is_dir()
    {
        environment.push((
            OsString::from("LD_LIBRARY_PATH"),
            library_dir.as_os_str().to_os_string(),
        ));
    }
    if let Some(cache) = &executable.par_cache {
        fs::create_dir_all(cache).map_err(|source| TexeError::Io {
            path: cache.clone(),
            source,
        })?;
        environment.push((
            OsString::from("PAR_GLOBAL_TEMP"),
            cache.as_os_str().to_os_string(),
        ));
    }
    let output = if executable.par_cache.is_some() {
        raw_bundled_biber_output(&executable.path, &arguments, directory, &environment)?
    } else if toolchain.managed.is_some() {
        raw_engine_output(&executable.path, &arguments, directory, &environment)?
    } else {
        raw_output(&executable.path, &arguments, directory, &environment)?
    };
    let processor_log_path = bibtex_proxy.as_ref().map_or_else(
        || control.path.with_extension("blg"),
        BibtexControlProxy::blg,
    );
    let processor_log = fs::read_to_string(&processor_log_path).unwrap_or_default();
    if output.status.success() {
        if let Some(proxy) = &bibtex_proxy {
            proxy.publish(control)?;
        }
        return Ok(());
    }
    let detail = if processor_log.trim().is_empty() {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    } else {
        processor_log
    };
    Err(TexeError::Build(format!(
        "{name} failed for {} with status {}:\n{}",
        control.path.display(),
        output.status,
        detail.trim()
    )))
}

/// BibTeX interprets a resource beginning with `./` relative to its process
/// directory and bypasses `BIBINPUTS`/`BSTINPUTS`. texe must run it beside the
/// generated `.aux` so the `.bbl` lands in the private output tree, while
/// upstream projects commonly assume BibTeX runs at the project root.
///
/// Feed BibTeX a private, flattened control file which strips only that
/// redundant leading marker. All other paths and citation order stay intact,
/// and the configured confined search roots still decide what can be read.
struct BibtexControlProxy {
    path: PathBuf,
    stem: String,
}

impl BibtexControlProxy {
    fn create(control: &Path, output_root: &Path) -> Result<Self, TexeError> {
        let mut contents = br"\relax
"
        .to_vec();
        flatten_bibtex_auxiliary(control, output_root, &mut BTreeSet::new(), &mut contents)?;
        let directory = control.parent().ok_or_else(|| {
            TexeError::Build(format!(
                "bibliography control file has no parent: {}",
                control.display()
            ))
        })?;
        for nonce in 0..100u32 {
            let mut hasher = Sha256::new();
            hasher.update(control.as_os_str().as_encoded_bytes());
            hasher.update(std::process::id().to_le_bytes());
            hasher.update(nonce.to_le_bytes());
            let suffix = &hex::encode(hasher.finalize())[..16];
            let stem = format!("texe-bibtex-{suffix}");
            let path = directory.join(format!("{stem}.aux"));
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(&contents)
                        .and_then(|()| file.sync_all())
                        .map_err(|source| TexeError::Io {
                            path: path.clone(),
                            source,
                        })?;
                    return Ok(Self { path, stem });
                }
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(TexeError::Io {
                        path: path.clone(),
                        source,
                    });
                }
            }
        }
        Err(TexeError::Build(format!(
            "could not reserve a private BibTeX control beside {}",
            control.display()
        )))
    }

    fn bbl(&self) -> PathBuf {
        self.path.with_extension("bbl")
    }

    fn blg(&self) -> PathBuf {
        self.path.with_extension("blg")
    }

    fn publish(&self, control: &BibliographyControl) -> Result<(), TexeError> {
        let bbl = self.bbl();
        let bytes = fs::read(&bbl).map_err(|source| TexeError::Io { path: bbl, source })?;
        crate::atomic::write(&control.path.with_extension("bbl"), &bytes)?;
        let blg = self.blg();
        if let Ok(bytes) = fs::read(&blg) {
            crate::atomic::write(&control.path.with_extension("blg"), &bytes)?;
        }
        Ok(())
    }
}

impl Drop for BibtexControlProxy {
    fn drop(&mut self) {
        for path in [&self.path, &self.bbl(), &self.blg()] {
            let _ = fs::remove_file(path);
        }
    }
}

fn flatten_bibtex_auxiliary(
    path: &Path,
    output_root: &Path,
    visited: &mut BTreeSet<PathBuf>,
    output: &mut Vec<u8>,
) -> Result<(), TexeError> {
    let canonical = fs::canonicalize(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let root = fs::canonicalize(output_root).map_err(|source| TexeError::Io {
        path: output_root.to_path_buf(),
        source,
    })?;
    if !canonical.starts_with(&root) || !visited.insert(canonical.clone()) {
        return Ok(());
    }
    let bytes = fs::read(&canonical).map_err(|source| TexeError::Io {
        path: canonical.clone(),
        source,
    })?;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.starts_with(br"\citation{") {
            output.extend_from_slice(line);
            output.push(b'\n');
        } else if line.starts_with(br"\bibdata{") || line.starts_with(br"\bibstyle{") {
            output.extend(normalize_bibtex_project_relative_paths(line));
            output.push(b'\n');
        } else if let Some(include) = bibtex_auxiliary_include(line) {
            let child = canonical.parent().unwrap_or(&root).join(include);
            if child.is_file() {
                flatten_bibtex_auxiliary(&child, &root, visited, output)?;
            }
        }
    }
    Ok(())
}

fn normalize_bibtex_project_relative_paths(line: &[u8]) -> Vec<u8> {
    let Some(open) = line.iter().position(|byte| *byte == b'{') else {
        return line.to_vec();
    };
    let Some(body) = line.get(open + 1..line.len().saturating_sub(1)) else {
        return line.to_vec();
    };
    if !line.ends_with(b"}") {
        return line.to_vec();
    }
    let mut normalized = line[..=open].to_vec();
    for (index, value) in body.split(|byte| *byte == b',').enumerate() {
        if index > 0 {
            normalized.push(b',');
        }
        let whitespace = value
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .unwrap_or(value.len());
        normalized.extend_from_slice(&value[..whitespace]);
        let mut path = &value[whitespace..];
        while path.starts_with(b"./") {
            path = &path[2..];
        }
        normalized.extend_from_slice(path);
    }
    normalized.push(b'}');
    normalized
}

fn resolve_bibliography_executable(
    project_root: &Path,
    toolchain: &ResolvedToolchain,
    processor: BibliographyProcessor,
    configured: &str,
) -> Result<BibliographyExecutable, TexeError> {
    if processor == BibliographyProcessor::Biber && configured == "biber" {
        let biber = if let Some(managed) = &toolchain.managed {
            ensure_managed_biber(managed)?
        } else {
            ensure_bundled_biber(toolchain.verification, toolchain.offline)?
        };
        return Ok(BibliographyExecutable {
            path: biber.executable,
            library_dir: Some(biber.library_dir),
            par_cache: Some(biber.cache_dir),
        });
    }
    if let Some(managed) = &toolchain.managed
        && processor == BibliographyProcessor::Bibtex
        && configured == "bibtex"
    {
        return Ok(BibliographyExecutable {
            path: managed.binary_dir.join("bibtex"),
            library_dir: None,
            par_cache: None,
        });
    }
    Ok(BibliographyExecutable {
        path: resolve_executable(project_root, configured)?,
        library_dir: None,
        par_cache: None,
    })
}

fn bibliography_environment(
    project_root: &Path,
    manifest: &ProjectManifest,
    texmf: &Path,
    toolchain: &ResolvedToolchain,
    working_directory: &Path,
) -> Vec<(OsString, OsString)> {
    let configured_roots = manifest
        .bibliography
        .roots
        .iter()
        .map(|root| project_root.join(root))
        .collect::<Vec<_>>();
    let package_bibtex = texmf.join("bibtex/bib");
    let package_biblatex = texmf.join("biblatex/bib");
    let package_styles = texmf.join("bibtex/bst");
    let mut bibliography_roots = vec![project_root];
    bibliography_roots.extend(configured_roots.iter().map(PathBuf::as_path));
    bibliography_roots.extend([package_bibtex.as_path(), package_biblatex.as_path()]);
    let mut style_roots = vec![project_root];
    style_roots.extend(configured_roots.iter().map(PathBuf::as_path));
    style_roots.push(package_styles.as_path());
    let bibliography_inputs = search_path_from(&bibliography_roots, working_directory);
    let style_inputs = search_path_from(&style_roots, working_directory);
    let mut environment = vec![
        (OsString::from("BIBINPUTS"), bibliography_inputs),
        (OsString::from("BSTINPUTS"), style_inputs),
    ];
    if let Some(managed) = &toolchain.managed {
        environment.push((OsString::from("PATH"), managed_path(&managed.binary_dir)));
    }
    environment
}

/// Keep the selected processor discoverable alongside the managed TeX
/// runtime. PAR's macOS universal launcher re-executes an architecture-specific
/// image by command name, then uses that name to reopen its packed archive.
fn managed_bibliography_path(
    executable: &Path,
    runtime_binary_dir: &Path,
) -> Result<OsString, TexeError> {
    let executable_dir = executable.parent().ok_or_else(|| {
        TexeError::Toolchain(format!(
            "managed bibliography executable has no parent directory: {}",
            executable.display()
        ))
    })?;
    if executable_dir == runtime_binary_dir {
        return Ok(managed_path(runtime_binary_dir));
    }
    std::env::join_paths([executable_dir, runtime_binary_dir]).map_err(|error| {
        TexeError::Toolchain(format!(
            "managed bibliography path cannot be represented in PATH: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::ProjectManifest;
    use crate::build::bibliography::{
        BibliographyContext, BibliographyControl, BibliographyProcessor, BibtexControlProxy,
        bibliography_control_digest, bibliography_controls, bibliography_environment,
        managed_bibliography_path, normalize_bibtex_project_relative_paths,
        persistent_cache_supported, persistent_control_digest,
    };
    use crate::progress::{Progress, ProgressLayout};
    use crate::toolchain::ResolvedToolchain;

    #[test]
    fn detects_bibtex_controls_and_prefers_biber_for_the_same_job() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output_dir = directory.path().join(".texe/build/output");
        fs::create_dir_all(&output_dir).expect("output directory");
        let main_aux = output_dir.join("main.aux");
        let main_bcf = output_dir.join("main.bcf");
        let chapter_aux = output_dir.join("chapter.aux");
        fs::write(
            &main_aux,
            br"\relax
\bibstyle{plain}
\bibdata{references}
",
        )
        .expect("main aux");
        fs::write(&main_bcf, b"<bcf:controlfile />").expect("main bcf");
        fs::write(
            &chapter_aux,
            br"\relax
\bibdata{chapter-references}
\bibstyle{alpha}
",
        )
        .expect("chapter aux");
        let recorder = output_dir.join("main.fls");
        fs::write(
            &recorder,
            "OUTPUT .texe/build/output/main.aux\n\
             OUTPUT .texe/build/output/main.bcf\n\
             OUTPUT .texe/build/output/chapter.aux\n",
        )
        .expect("recorder");

        let controls =
            bibliography_controls(directory.path(), &output_dir, &recorder).expect("controls");

        assert_eq!(
            controls,
            vec![
                BibliographyControl {
                    path: chapter_aux,
                    processor: BibliographyProcessor::Bibtex,
                },
                BibliographyControl {
                    path: main_bcf,
                    processor: BibliographyProcessor::Biber,
                },
            ]
        );
    }

    fn system_toolchain() -> ResolvedToolchain {
        ResolvedToolchain {
            provider: "system".to_string(),
            engine: "pdflatex".to_string(),
            engine_executable: PathBuf::from("/usr/bin/pdflatex"),
            kpsewhich_executable: PathBuf::from("/usr/bin/kpsewhich"),
            texmf_dist: PathBuf::from("/usr/share/texmf-dist"),
            engine_roots: Vec::new(),
            identity: crate::toolchain::ToolchainIdentity {
                provider: "system".to_string(),
                engine: "pdflatex".to_string(),
                channel: "system".to_string(),
                target: "test".to_string(),
                fingerprint: "test".to_string(),
                registry_url: None,
                registry_metadata_digest: None,
                artifacts: Vec::new(),
            },
            managed: None,
            verification: crate::toolchain::VerificationPolicy::Interval,
            offline: false,
        }
    }

    #[test]
    fn bibliography_search_paths_are_frozen_to_project_and_package_tree() {
        let toolchain = system_toolchain();
        let manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
provider = "system"
engine = "pdflatex"
"#,
        )
        .expect("manifest");
        let environment = bibliography_environment(
            Path::new("/tmp/project"),
            &manifest,
            Path::new("/tmp/project/.texe/texmf"),
            &toolchain,
            Path::new("/tmp/project/.texe/build/output"),
        );
        for (name, value) in environment {
            if matches!(name.to_str(), Some("BIBINPUTS" | "BSTINPUTS")) {
                let value = value.to_string_lossy();
                assert!(value.contains("/tmp/project//"));
                assert!(!value.ends_with(if cfg!(windows) { ';' } else { ':' }));
            }
        }
    }

    #[test]
    fn bibliography_search_paths_include_declared_project_roots() {
        let toolchain = system_toolchain();
        let manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
provider = "system"
engine = "pdflatex"
[bibliography]
roots = ["vendor/natbib"]
"#,
        )
        .expect("manifest");
        let environment = bibliography_environment(
            Path::new("/tmp/project"),
            &manifest,
            Path::new("/tmp/project/.texe/texmf"),
            &toolchain,
            Path::new("/tmp/project/.texe/build/output"),
        )
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        assert!(
            environment
                .get(OsStr::new("BSTINPUTS"))
                .is_some_and(|value| value
                    .to_string_lossy()
                    .contains("/tmp/project/vendor/natbib//"))
        );
    }

    #[test]
    fn managed_biber_stays_discoverable_without_host_tools() {
        let value = managed_bibliography_path(
            Path::new("/data/texe/components/biber/bin/biber"),
            Path::new("/data/texe/runtime/bin"),
        )
        .expect("managed path");
        assert_eq!(
            std::env::split_paths(&value).collect::<Vec<_>>(),
            [
                PathBuf::from("/data/texe/components/biber/bin"),
                PathBuf::from("/data/texe/runtime/bin"),
            ]
        );
    }

    #[test]
    fn bibtex_proxy_flattens_auxiliaries_and_normalizes_explicit_project_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join(".texe/build/output");
        let chapter_dir = output.join("chapters");
        fs::create_dir_all(&chapter_dir).expect("output directories");
        let main = output.join("main.aux");
        fs::write(
            &main,
            br"\relax
\citation{root}
\@input{chapters/one.aux}
\bibstyle{././bib/natbib-oup}
\bibdata{./bib/references, ./bib/more}
\bibcite{old}{1}
",
        )
        .expect("main auxiliary");
        fs::write(
            chapter_dir.join("one.aux"),
            br"\citation{chapter}
\bibcite{chapter}{2}
",
        )
        .expect("chapter auxiliary");

        let proxy = BibtexControlProxy::create(&main, &output).expect("BibTeX proxy");

        assert_eq!(
            fs::read(&proxy.path).expect("proxy contents"),
            br"\relax
\citation{root}
\citation{chapter}
\bibstyle{bib/natbib-oup}
\bibdata{bib/references, bib/more}
"
        );
        assert!(proxy.path.starts_with(&output));
    }

    #[test]
    fn bibtex_path_normalization_leaves_non_explicit_paths_unchanged() {
        assert_eq!(
            normalize_bibtex_project_relative_paths(br"\bibdata{references, bib/more}"),
            br"\bibdata{references, bib/more}"
        );
    }

    #[test]
    fn bibtex_digest_tracks_inputs_but_ignores_generated_citation_labels() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let aux = directory.path().join("main.aux");
        fs::write(
            &aux,
            b"\\citation{first}\n\\bibstyle{plain}\n\\bibdata{references}\n",
        )
        .expect("initial aux");
        let control = BibliographyControl {
            path: aux.clone(),
            processor: BibliographyProcessor::Bibtex,
        };
        let initial =
            bibliography_control_digest(&control, directory.path()).expect("initial digest");

        fs::write(
            &aux,
            b"\\citation{first}\n\\bibstyle{plain}\n\\bibdata{references}\n\
              \\bibcite{first}{1}\n",
        )
        .expect("aux with generated label");
        let generated =
            bibliography_control_digest(&control, directory.path()).expect("generated digest");
        assert_eq!(initial, generated);

        fs::write(
            &aux,
            b"\\citation{second}\n\\bibstyle{plain}\n\\bibdata{references}\n",
        )
        .expect("changed citation");
        let changed =
            bibliography_control_digest(&control, directory.path()).expect("changed digest");
        assert_ne!(initial, changed);
    }

    #[test]
    fn persistent_bibliography_digest_tracks_database_contents() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let output = root.join(".texe/build/output");
        fs::create_dir_all(&output).expect("output");
        let control = BibliographyControl {
            path: output.join("main.bcf"),
            processor: BibliographyProcessor::Biber,
        };
        fs::write(&control.path, b"<bcf:controlfile />").expect("control");
        fs::write(root.join("references.bib"), b"@book{first,}").expect("database");
        let manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
provider = "system"
engine = "pdflatex"
"#,
        )
        .expect("manifest");
        let toolchain = system_toolchain();
        let progress = Progress::new(
            output.join("timings.json"),
            "pdflatex",
            false,
            5,
            false,
            false,
            ProgressLayout::Standalone,
        );
        let texmf = root.join(".texe/texmf");
        let recorder = output.join("main.fls");
        let context = BibliographyContext {
            project_root: root,
            manifest: &manifest,
            toolchain: &toolchain,
            texmf: &texmf,
            output_dir: &output,
            recorder_path: &recorder,
            progress: &progress,
        };
        let initial = persistent_control_digest(&control, &context).expect("initial digest");

        fs::write(root.join("references.bib"), b"@book{second,}").expect("changed database");
        let changed = persistent_control_digest(&control, &context).expect("changed digest");

        assert_ne!(initial, changed);
    }

    #[test]
    fn persistent_cache_requires_the_default_bibliography_processor() {
        let mut manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
        )
        .expect("manifest");
        let bibtex = BibliographyControl {
            path: PathBuf::from("main.aux"),
            processor: BibliographyProcessor::Bibtex,
        };
        let biber = BibliographyControl {
            path: PathBuf::from("main.bcf"),
            processor: BibliographyProcessor::Biber,
        };

        assert!(persistent_cache_supported(&bibtex, &manifest));
        assert!(persistent_cache_supported(&biber, &manifest));

        manifest.bibliography.bibtex = "./tools/bibtex".to_string();
        manifest.bibliography.biber = "./tools/biber".to_string();
        assert!(!persistent_cache_supported(&bibtex, &manifest));
        assert!(!persistent_cache_supported(&biber, &manifest));
    }
}
