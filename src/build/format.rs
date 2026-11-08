use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::build::process::{
    engine_input_environment, engine_path_from, is_portable_build, managed_path, portable_output,
    raw_engine_output, search_path_from, source_date_environment,
};
use crate::package::PackageEnvironment;
use crate::progress::{PhaseKind, Progress};
use crate::toolchain::ResolvedToolchain;

/// Formats are cached per toolchain and package environment, so their embedded
/// timestamp cannot come from a project's source-date lock.
const FORMAT_SOURCE_DATE_EPOCH: &str = "0";

#[derive(Debug, Clone)]
pub(super) struct ManagedFormat {
    pub(super) root: PathBuf,
    pub(super) formats: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct ManagedFormatRecipe {
    name: &'static str,
    ini: &'static str,
    translate_file: Option<&'static str>,
    language_lua: bool,
    extended_mode: bool,
}

struct FormatBuild<'a> {
    project_root: &'a Path,
    toolchain: &'a ResolvedToolchain,
    texmf: &'a Path,
    package_environment: &'a PackageEnvironment,
    recipe: &'static ManagedFormatRecipe,
    target: PathBuf,
    staging: PathBuf,
    formats: PathBuf,
    config: PathBuf,
}

const PDFLATEX_FORMAT: ManagedFormatRecipe = ManagedFormatRecipe {
    name: "pdflatex",
    ini: "pdflatex.ini",
    translate_file: Some("cp227.tcx"),
    language_lua: false,
    extended_mode: true,
};

const LUALATEX_FORMAT: ManagedFormatRecipe = ManagedFormatRecipe {
    name: "lualatex",
    ini: "lualatex.ini",
    translate_file: None,
    language_lua: true,
    extended_mode: false,
};

const XELATEX_FORMAT: ManagedFormatRecipe = ManagedFormatRecipe {
    name: "xelatex",
    ini: "xelatex.ini",
    translate_file: None,
    language_lua: false,
    extended_mode: true,
};

fn format_recipe(engine: &str) -> Result<&'static ManagedFormatRecipe, TexeError> {
    match engine {
        "pdflatex" => Ok(&PDFLATEX_FORMAT),
        "lualatex" => Ok(&LUALATEX_FORMAT),
        "xelatex" => Ok(&XELATEX_FORMAT),
        _ => Err(TexeError::Build(format!(
            "engine `{engine}` has no locked format recipe"
        ))),
    }
}

pub(super) fn name(engine: &str) -> Result<&'static str, TexeError> {
    Ok(format_recipe(engine)?.name)
}

pub(super) fn ensure(
    project_root: &Path,
    toolchain: &ResolvedToolchain,
    texmf: &Path,
    package_environment: &PackageEnvironment,
    build_root: &Path,
    packages_remote: bool,
    progress: &Progress,
) -> Result<Option<ManagedFormat>, TexeError> {
    // Shipout carries a sealed format and implements its own auxiliary stages.
    if toolchain.engine == "shipout" {
        return Ok(None);
    }
    let managed = toolchain.managed.as_ref();
    if managed.is_none() && !packages_remote {
        return Ok(None);
    }
    let recipe = format_recipe(&toolchain.engine)?;
    let environment_key = environment_cache_key(&package_environment.fingerprint)?;
    let format_cache = if is_portable_build(project_root, build_root) {
        // A document must never write executable format state shared with other
        // papers. Project-private formats also retire with their environment.
        build_root.join("formats")
    } else {
        managed.map_or_else(
            || build_root.join("formats"),
            |runtime| runtime.format_cache.clone(),
        )
    };
    // The user-selected runtime cache can have OS aliases such as macOS /var.
    // Resolve that trusted cache root before adding compiler-controlled paths;
    // generated paths below it still reject links through the bounded reader.
    let format_cache = if !is_portable_build(project_root, build_root) && managed.is_some() {
        fs::create_dir_all(&format_cache).map_err(|source| TexeError::Io {
            path: format_cache.clone(),
            source,
        })?;
        fs::canonicalize(&format_cache).map_err(|source| TexeError::Io {
            path: format_cache,
            source,
        })?
    } else {
        format_cache
    };
    let target = format_cache.join(format_cache_key(
        &toolchain.identity.fingerprint,
        environment_key,
        recipe.name,
    ));
    let format = ManagedFormat {
        root: target.clone(),
        formats: target.join("formats"),
    };
    let format_file = format.formats.join(format!("{}.fmt", recipe.name));
    if is_portable_build(project_root, build_root) {
        validate_private_format_paths(project_root, &target, &format.formats)?;
        if fs::symlink_metadata(&format_file)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(TexeError::Build(
                "A portable format cache must not alias a source file".into(),
            ));
        }
    }
    if format_file.is_file() {
        if is_portable_build(project_root, build_root) {
            super::generated::read(
                project_root,
                &format.formats,
                &format_file,
                super::generated::MAX_ARTIFACT,
            )?;
        }
        return Ok(Some(format));
    }

    let build = prepare_format_build(
        project_root,
        toolchain,
        texmf,
        package_environment,
        recipe,
        target,
    )?;
    let result = progress.phase(
        PhaseKind::Format,
        format!("generating locked {} format", recipe.name),
        || generate_format(&build),
    );
    if result.is_err() && build.staging.exists() {
        let _ = remove_format_staging(&build.staging, recipe.name);
    }
    result?;
    Ok(Some(format))
}

// One content-addressed directory retains the complete identity without
// multiplying path length with nested hashes that Windows TeX cannot open.
fn format_cache_key(toolchain: &str, environment: &str, recipe: &str) -> String {
    let mut hash = Sha256::new();
    for value in ["locked-languages-v2", toolchain, environment, recipe] {
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

fn environment_cache_key(fingerprint: &str) -> Result<&str, TexeError> {
    let key = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(TexeError::Build(format!(
            "pqty emitted an unsafe environment fingerprint: {fingerprint}"
        )));
    }
    Ok(key)
}

fn prepare_format_build<'a>(
    project_root: &'a Path,
    toolchain: &'a ResolvedToolchain,
    texmf: &'a Path,
    package_environment: &'a PackageEnvironment,
    recipe: &'static ManagedFormatRecipe,
    target: PathBuf,
) -> Result<FormatBuild<'a>, TexeError> {
    let parent = target.parent().ok_or_else(|| {
        TexeError::Build(format!(
            "format cache path has no parent: {}",
            target.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|source| TexeError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = format_staging_path(&target, recipe.name);
    if target.starts_with(project_root.join(".texe/environments"))
        || super::process::protection_requested()
    {
        crate::portable::private_directory(
            project_root,
            parent.strip_prefix(project_root).map_err(|_| {
                TexeError::Build("Private format cache is outside the project".into())
            })?,
        )?;
        if fs::symlink_metadata(&staging).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(TexeError::Build(
                "Private format staging must not alias source directories".into(),
            ));
        }
    }
    if staging.exists() {
        remove_format_staging(&staging, recipe.name)?;
    }
    let formats = staging.join("formats");
    let config = staging.join("config/tex/generic/config");
    for directory in [
        &formats,
        &config,
        &staging.join("var"),
        &staging.join("user-config"),
    ] {
        fs::create_dir_all(directory).map_err(|source| TexeError::Io {
            path: (*directory).clone(),
            source,
        })?;
    }
    Ok(FormatBuild {
        project_root,
        toolchain,
        texmf,
        package_environment,
        recipe,
        target,
        staging,
        formats,
        config,
    })
}

fn validate_private_format_paths(
    root: &Path,
    target: &Path,
    formats: &Path,
) -> Result<(), TexeError> {
    let parent = target
        .parent()
        .ok_or_else(|| TexeError::Build("Invalid format cache directory".into()))?;
    crate::portable::private_directory(
        root,
        parent
            .strip_prefix(root)
            .map_err(|_| TexeError::Build("Private format cache is outside the project".into()))?,
    )?;
    for path in [target, formats] {
        match fs::symlink_metadata(path) {
            Ok(_) => crate::portable::private_directory(
                root,
                path.strip_prefix(root).map_err(|_| {
                    TexeError::Build("Private format cache is outside the project".into())
                })?,
            )?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(TexeError::Io {
                    path: path.into(),
                    source,
                });
            }
        }
    }
    Ok(())
}

fn generate_format(build: &FormatBuild<'_>) -> Result<(), TexeError> {
    prepare_format_configuration(build)?;
    if let Some((format, inputs)) = reusable_format(build) {
        let target = build.formats.join(format!("{}.fmt", build.recipe.name));
        fs::copy(&format, &target).map_err(|source| TexeError::Io {
            path: target.clone(),
            source,
        })?;
        write_format_inputs(&build.staging, &inputs)?;
        return publish_generated_format(build);
    }
    let arguments = format_arguments(build);
    let environment = format_environment(build);
    let output = if build
        .target
        .starts_with(build.project_root.join(".texe/environments"))
        || super::process::protection_requested()
    {
        portable_output(
            build.project_root,
            build.toolchain,
            &build.toolchain.engine_executable,
            &arguments,
            build.project_root,
            &environment,
            std::slice::from_ref(&build.staging),
            &[],
            &[],
        )?
    } else {
        raw_engine_output(
            &build.toolchain.engine_executable,
            &arguments,
            build.project_root,
            &environment,
        )?
    };
    validate_generated_format(build, &output)?;
    let inputs = recorded_format_inputs(build);
    if let Some(inputs) = &inputs {
        write_format_inputs(&build.staging, inputs)?;
    }
    publish_generated_format(build)?;
    if let Some(inputs) = &inputs {
        share_format(build, inputs)?;
    }
    Ok(())
}

/// Format reuse by recorded inputs.
///
/// A format is a pure function of the engine, its recipe, fixed arguments and
/// environment, and the bytes initex reads. Every generated format records
/// those inputs from `-recorder` (`inputs.json`): each file relative to the
/// tree it came from (format staging, the locked package tree or the managed
/// runtime) with its SHA-256. A format whose recorded inputs are all
/// byte-identical in the current build is reused instead of running initex
/// again, so a package environment change that does not touch the format (a
/// font provider added during convergence) keeps it, and a fresh build
/// directory with the same packages copies it from the shared store.
///
/// A format that read anything outside those three trees (for example a
/// project file through a `.` search path) records no inputs and is never
/// reused or shared. A shared format is only taken after the consuming build
/// hashed its own copy of every recorded input, so a paper that altered its
/// package tree can at most produce a format no other paper accepts.
const FORMAT_INPUTS_SCHEMA: &str = "texe.format-inputs/v1";
const FORMAT_INPUTS: &str = "inputs.json";
/// Shared formats live in the managed runtime's format cache under this name.
const SHARED_FORMATS: &str = "by-inputs";
/// Candidate formats examined per lookup, newest first.
const MAX_FORMAT_CANDIDATES: usize = 64;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
enum FormatInputRoot {
    Staging,
    Packages,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatInput {
    root: FormatInputRoot,
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatInputs {
    schema: String,
    toolchain: String,
    recipe: String,
    inputs: Vec<FormatInput>,
}

fn format_input_roots(build: &FormatBuild<'_>) -> Vec<(FormatInputRoot, PathBuf)> {
    let mut roots = vec![
        (FormatInputRoot::Staging, build.staging.clone()),
        (FormatInputRoot::Packages, build.texmf.to_path_buf()),
    ];
    if let Some(runtime) = &build.toolchain.managed {
        roots.push((FormatInputRoot::Runtime, runtime.root.clone()));
    }
    roots
}

/// The inputs a recorder file lists, relative to their trees, or `None` when
/// any input lies outside `roots` (or cannot be placed lexically).
fn recorded_inputs(
    recorder: &str,
    roots: &[(FormatInputRoot, PathBuf)],
) -> Option<Vec<(FormatInputRoot, String)>> {
    let working_directory = recorder
        .lines()
        .find_map(|line| line.strip_prefix("PWD "))
        .map(PathBuf::from);
    // Absolute and lexically normal, or `None` (a relative path without a
    // working directory, or one climbing with `..`).
    let place = |recorded: &str| -> Option<PathBuf> {
        let path = Path::new(recorded);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            working_directory.as_ref()?.join(path)
        };
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => return None,
                other => normalized.push(other),
            }
        }
        Some(normalized)
    };
    // Files initex wrote itself (and may read back, as `texsys.aux`) are
    // outputs, not inputs.
    let outputs = recorder
        .lines()
        .filter_map(|line| line.strip_prefix("OUTPUT "))
        .filter_map(place)
        .collect::<std::collections::BTreeSet<_>>();
    let mut inputs = std::collections::BTreeSet::new();
    for line in recorder.lines() {
        let Some(input) = line.strip_prefix("INPUT ") else {
            continue;
        };
        // TeX opens the null device for empty streams; it holds no bytes.
        if input == "/dev/null" {
            continue;
        }
        let normalized = place(input)?;
        if outputs.contains(&normalized) {
            continue;
        }
        let (root, relative) = roots.iter().find_map(|(kind, root)| {
            normalized
                .strip_prefix(root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(|relative| (*kind, relative))
        })?;
        let relative = relative
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()?
            .join("/");
        inputs.insert((root, relative));
    }
    (!inputs.is_empty()).then(|| inputs.into_iter().collect())
}

/// SHA-256 of a regular (non-symlink) file of at most `MAX_ARTIFACT` bytes.
/// Hard links are fine here: only the bytes matter.
fn input_digest(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > super::generated::MAX_ARTIFACT {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(path)
        .ok()?
        .take(super::generated::MAX_ARTIFACT + 1);
    let mut buffer = vec![0; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    (total <= super::generated::MAX_ARTIFACT).then(|| hex::encode(hasher.finalize()))
}

fn recorded_format_inputs(build: &FormatBuild<'_>) -> Option<FormatInputs> {
    let recorder_text = super::generated::text_local(
        &build.formats.join(format!("{}.fls", build.recipe.name)),
        super::generated::MAX_METADATA,
    )
    .ok()?;
    let roots = format_input_roots(build);
    let listed = recorded_inputs(&recorder_text, &roots)?;
    let mut inputs = Vec::with_capacity(listed.len());
    for (root, path) in listed {
        let base = &roots
            .iter()
            .find(|(kind, _)| *kind == root)
            .expect("known root")
            .1;
        let sha256 = input_digest(&base.join(&path))?;
        inputs.push(FormatInput { root, path, sha256 });
    }
    Some(FormatInputs {
        schema: FORMAT_INPUTS_SCHEMA.to_string(),
        toolchain: build.toolchain.identity.fingerprint.clone(),
        recipe: build.recipe.name.to_string(),
        inputs,
    })
}

fn write_format_inputs(root: &Path, inputs: &FormatInputs) -> Result<(), TexeError> {
    let bytes = serde_json::to_vec_pretty(inputs).map_err(|source| TexeError::Json {
        path: root.join(FORMAT_INPUTS),
        source,
    })?;
    atomic_write(&root.join(FORMAT_INPUTS), &bytes)
}

fn read_format_inputs(root: &Path) -> Option<FormatInputs> {
    let text =
        super::generated::text_local(&root.join(FORMAT_INPUTS), super::generated::MAX_METADATA)
            .ok()?;
    serde_json::from_str::<FormatInputs>(&text)
        .ok()
        .filter(|inputs| inputs.schema == FORMAT_INPUTS_SCHEMA)
}

/// Whether every recorded input is byte-identical in this build's trees.
fn inputs_match(
    inputs: &FormatInputs,
    build: &FormatBuild<'_>,
    roots: &[(FormatInputRoot, PathBuf)],
    digests: &mut BTreeMap<(FormatInputRoot, String), Option<String>>,
) -> bool {
    inputs.toolchain == build.toolchain.identity.fingerprint
        && inputs.recipe == build.recipe.name
        && !inputs.inputs.is_empty()
        && inputs.inputs.iter().all(|input| {
            let Some(base) = roots
                .iter()
                .find(|(kind, _)| *kind == input.root)
                .map(|(_, base)| base)
            else {
                return false;
            };
            if input
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return false;
            }
            digests
                .entry((input.root, input.path.clone()))
                .or_insert_with(|| input_digest(&base.join(&input.path)))
                .as_deref()
                == Some(input.sha256.as_str())
        })
}

fn shared_format_store(build: &FormatBuild<'_>) -> Option<PathBuf> {
    build
        .toolchain
        .managed
        .as_ref()
        .map(|runtime| runtime.format_cache.join(SHARED_FORMATS))
}

/// A format generated earlier from exactly this build's inputs: a sibling in
/// the same format cache, or one in the shared store.
fn reusable_format(build: &FormatBuild<'_>) -> Option<(PathBuf, FormatInputs)> {
    let mut candidates = Vec::new();
    for directory in build
        .target
        .parent()
        .into_iter()
        .chain(shared_format_store(build).as_deref())
    {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == build.staging
                || entry.file_name().to_string_lossy().starts_with('.')
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok();
            candidates.push((modified, path));
        }
    }
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let roots = format_input_roots(build);
    let mut digests = BTreeMap::new();
    candidates
        .into_iter()
        .take(MAX_FORMAT_CANDIDATES)
        .find_map(|(_, candidate)| {
            let inputs = read_format_inputs(&candidate)?;
            let format = candidate
                .join("formats")
                .join(format!("{}.fmt", build.recipe.name));
            (fs::symlink_metadata(&format).is_ok_and(|metadata| metadata.is_file())
                && inputs_match(&inputs, build, &roots, &mut digests))
            .then_some((format, inputs))
        })
}

/// Offer a freshly generated format to other build directories through the
/// shared store, named by its inputs. Only this parent writes there.
fn share_format(build: &FormatBuild<'_>, inputs: &FormatInputs) -> Result<(), TexeError> {
    let Some(store) = shared_format_store(build) else {
        return Ok(());
    };
    // A format already in the runtime cache is found there as a sibling.
    if store
        .parent()
        .is_some_and(|cache| build.target.starts_with(cache))
    {
        return Ok(());
    }
    let serialized = serde_json::to_vec(inputs).map_err(|source| TexeError::Json {
        path: store.clone(),
        source,
    })?;
    let name = hex::encode(Sha256::digest(&serialized));
    let destination = store.join(&name);
    if destination.exists() {
        return Ok(());
    }
    let staging = store.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let formats = staging.join("formats");
        fs::create_dir_all(&formats).map_err(|source| TexeError::Io {
            path: formats.clone(),
            source,
        })?;
        let file = format!("{}.fmt", build.recipe.name);
        fs::copy(
            build.target.join("formats").join(&file),
            formats.join(&file),
        )
        .map_err(|source| TexeError::Io {
            path: formats.join(&file),
            source,
        })?;
        write_format_inputs(&staging, inputs)?;
        match fs::rename(&staging, &destination) {
            Ok(()) => Ok(()),
            Err(_) if destination.is_dir() => Ok(()),
            Err(source) => Err(TexeError::Io {
                path: destination.clone(),
                source,
            }),
        }
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn prepare_format_configuration(build: &FormatBuild<'_>) -> Result<(), TexeError> {
    let ini = locked_format_ini(build.texmf, build.recipe);
    if !ini.is_file() {
        return Err(TexeError::Build(format!(
            "locked format bootstrap file is absent from pqty.lock: {}",
            ini.display()
        )));
    }
    write_language_configuration(
        build.texmf,
        &build.config,
        &build.package_environment.languages,
        build.recipe.language_lua,
    )?;
    if build.recipe.language_lua {
        write_luaotfload_config(&build.config.join("luaotfload.conf"))?;
    }
    write_pdftex_map(
        build.texmf,
        // TeX Live's updmap default (pdftexDownloadBase14 = true): the base
        // fonts come from ps2pk35.map, which embeds the URW outlines. The
        // pdftex35.map alternative leaves Symbol and ZapfDingbats unembedded
        // with no outline to read, so pdfTeX stops at the first `\ding`
        // ("builtin glyph names is empty"), as in arXiv 2312.00752.
        &[FOUNDATIONAL_PDFTEX_MAP],
        &build.package_environment.font_maps,
        &build
            .staging
            .join("user-config/fonts/map/pdftex/updmap/pdftex.map"),
    )
}

fn format_arguments(build: &FormatBuild<'_>) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("-ini"),
        OsString::from("-recorder"),
        OsString::from("-interaction=nonstopmode"),
        OsString::from("-halt-on-error"),
        OsString::from(format!("-jobname={}", build.recipe.name)),
        OsString::from(format!("-progname={}", build.recipe.name)),
    ];
    if build.recipe.extended_mode {
        arguments.push(OsString::from("-etex"));
    }
    if let Some(translate_file) = build.recipe.translate_file {
        let runtime_texmf = build.toolchain.managed.as_ref().map_or_else(
            || build.toolchain.texmf_dist.clone(),
            |runtime| {
                // The managed runtime owns only engine bootstrap data; LaTeX
                // macros still come from the locked pqty tree.
                runtime.root.join("texmf-dist")
            },
        );
        arguments.push(prefixed_engine_path(
            "-translate-file=",
            &runtime_texmf.join("web2c").join(translate_file),
            build.project_root,
        ));
    }
    arguments.extend([
        prefixed_engine_path("-output-directory=", &build.formats, build.project_root),
        format_ini_argument(build.texmf, build.recipe, build.project_root),
    ]);
    arguments
}

fn locked_format_ini(texmf: &Path, recipe: &ManagedFormatRecipe) -> PathBuf {
    texmf.join("tex/latex/tex-ini-files").join(recipe.ini)
}

fn format_ini_argument(
    texmf: &Path,
    recipe: &ManagedFormatRecipe,
    working_directory: &Path,
) -> OsString {
    engine_path_from(&locked_format_ini(texmf, recipe), working_directory)
}

fn prefixed_engine_path(prefix: &str, path: &Path, working_directory: &Path) -> OsString {
    let mut argument = OsString::from(prefix);
    argument.push(engine_path_from(path, working_directory));
    argument
}

fn format_environment(build: &FormatBuild<'_>) -> Vec<(OsString, OsString)> {
    let managed = build.toolchain.managed.as_ref();
    let staging_config = build.staging.join("config");
    let managed_texmf_dist = managed.map(|runtime| runtime.root.join("texmf-dist"));
    let mut tex_search_roots = vec![
        build.staging.as_path(),
        staging_config.as_path(),
        build.texmf,
    ];
    if let Some(texmf_dist) = &managed_texmf_dist {
        tex_search_roots.push(texmf_dist);
    }
    let system_var = managed.map_or_else(
        || build.staging.join("var"),
        |runtime| runtime.root.join("texmf-var"),
    );
    let system_config = managed.map_or_else(
        || build.staging.join("user-config"),
        |runtime| runtime.root.join("texmf-config"),
    );
    let mut font_map_roots = vec![
        build.staging.join("user-config/fonts/map"),
        build.texmf.join("fonts/map"),
    ];
    if let Some(texmf_dist) = &managed_texmf_dist {
        font_map_roots.push(texmf_dist.join("fonts/map"));
    }
    let font_map_root_refs = font_map_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let search = search_path_from(&tex_search_roots, build.project_root);
    let mut environment = vec![
        (
            OsString::from("TEXMFHOME"),
            engine_path_from(build.texmf, build.project_root),
        ),
        (
            OsString::from("TEXMFVAR"),
            engine_path_from(&build.staging.join("var"), build.project_root),
        ),
        (
            OsString::from("TEXMFCACHE"),
            engine_path_from(&build.staging.join("var"), build.project_root),
        ),
        (
            OsString::from("TEXMFCONFIG"),
            engine_path_from(&build.staging.join("user-config"), build.project_root),
        ),
        (
            OsString::from("TEXMFSYSVAR"),
            engine_path_from(&system_var, build.project_root),
        ),
        (
            OsString::from("TEXMFSYSCONFIG"),
            engine_path_from(&system_config, build.project_root),
        ),
        (
            OsString::from("TEXFORMATS"),
            engine_path_from(&build.formats, build.project_root),
        ),
        (
            OsString::from("TEXFONTMAPS"),
            search_path_from(&font_map_root_refs, build.project_root),
        ),
    ];
    environment.extend(engine_input_environment(build.recipe.name, search));
    if let Some(managed) = managed {
        environment.extend([
            (
                OsString::from("OSFONTDIR"),
                engine_path_from(&build.texmf.join("fonts"), build.project_root),
            ),
            (OsString::from("PATH"), managed_path(&managed.binary_dir)),
        ]);
    }
    environment.extend(source_date_environment(FORMAT_SOURCE_DATE_EPOCH));
    environment
}

fn validate_generated_format(
    build: &FormatBuild<'_>,
    output: &std::process::Output,
) -> Result<(), TexeError> {
    let format = build.formats.join(format!("{}.fmt", build.recipe.name));
    if output.status.success()
        && super::generated::read_local(&format, super::generated::MAX_ARTIFACT).is_ok()
    {
        return Ok(());
    }
    let log = super::generated::text_local(
        &build.formats.join(format!("{}.log", build.recipe.name)),
        super::generated::MAX_METADATA,
    )
    .unwrap_or_default();
    let detail = if log.is_empty() {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    } else {
        log.lines()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    };
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        format!("engine exited with {}", output.status)
    } else {
        detail.to_string()
    };
    Err(TexeError::Build(format!(
        "could not generate locked {} format:\n{detail}",
        build.recipe.name
    )))
}

fn publish_generated_format(build: &FormatBuild<'_>) -> Result<(), TexeError> {
    match fs::rename(&build.staging, &build.target) {
        Ok(()) => Ok(()),
        Err(_)
            if build
                .target
                .join("formats")
                .join(format!("{}.fmt", build.recipe.name))
                .is_file() =>
        {
            remove_format_staging(&build.staging, build.recipe.name)
        }
        Err(source) => Err(TexeError::Io {
            path: build.target.clone(),
            source,
        }),
    }
}

fn write_language_configuration(
    texmf: &Path,
    config: &Path,
    languages: &[crate::package::LockedLanguage],
    language_lua: bool,
) -> Result<(), TexeError> {
    let read_base = |name: &str| {
        let path = texmf.join("tex/generic/config").join(name);
        super::generated::text_local(&path, super::generated::MAX_METADATA)
    };
    let mut dat = read_base("language.us")?;
    let mut def = read_base("language.us.def")?;
    let mut lua = if language_lua {
        read_base("language.us.lua")?
    } else {
        String::new()
    };
    let mut names = dat
        .lines()
        .filter_map(|line| line.split('%').next()?.split_whitespace().next())
        .map(|name| name.trim_start_matches('=').to_string())
        .collect::<std::collections::BTreeSet<_>>();
    dat.push('\n');
    def.push('\n');
    for language in languages {
        let declaration = &language.declaration;
        let name = &declaration.name;
        let valid_name = |name: &str| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        };
        if !valid_name(name) || declaration.synonyms.iter().any(|name| !valid_name(name)) {
            return Err(TexeError::Build(format!(
                "Invalid locked language name from {}",
                language.provider
            )));
        }
        // English remains language zero, with Knuth's original patterns and
        // aliases. Never redefine an initial language through package metadata.
        if names.contains(name) {
            continue;
        }
        for path in std::iter::once(&declaration.loader)
            .chain(&declaration.patterns)
            .chain(&declaration.exceptions)
        {
            if path.is_empty()
                || path.split('/').any(|part| matches!(part, "" | "." | ".."))
                || !path.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'+')
                })
                || !texmf.join(path).is_file()
            {
                return Err(TexeError::Build(format!(
                    "Invalid or absent locked language file: {path}"
                )));
            }
        }
        let loader = &declaration.loader;
        let left = declaration.left_hyphen_min;
        let right = declaration.right_hyphen_min;
        writeln!(dat, "{name} {loader}").expect("write to String");
        writeln!(
            def,
            "\\addlanguage {{{name}}}{{{loader}}}{{}}{{{left}}}{{{right}}}"
        )
        .expect("write to String");
        names.insert(name.clone());
        let mut synonyms = Vec::new();
        for synonym in &declaration.synonyms {
            if names.insert(synonym.clone()) {
                writeln!(dat, "={synonym}").expect("write to String");
                writeln!(
                    def,
                    "\\addlanguage {{{synonym}}}{{{loader}}}{{}}{{{left}}}{{{right}}}"
                )
                .expect("write to String");
                synonyms.push(synonym.clone());
            }
        }
        if language_lua {
            let quote = |value: &str| serde_json::to_string(value).expect("serialize string");
            write!(lua, "[{}]={{loader={},lefthyphenmin={left},righthyphenmin={right},patterns={},hyphenation={},synonyms={{{}}}",
                quote(name), quote(loader), quote(&declaration.patterns.join(",")), quote(&declaration.exceptions.join(",")),
                synonyms.iter().map(|s| quote(s)).collect::<Vec<_>>().join(",")).expect("write to String");
            if let Some(special) = declaration
                .attributes
                .iter()
                .find(|attribute| attribute.name == "luaspecial")
            {
                write!(lua, ",special={}", quote(&special.value)).expect("write to String");
            }
            lua.push_str("},\n");
        }
    }
    def.push_str("\\uselanguage {USenglish}\n");
    atomic_write(&config.join("language.dat"), dat.as_bytes())?;
    atomic_write(&config.join("language.def"), def.as_bytes())?;
    if language_lua {
        lua.push_str("}\n");
        atomic_write(&config.join("language.dat.lua"), lua.as_bytes())?;
    }
    Ok(())
}

fn write_luaotfload_config(target: &Path) -> Result<(), TexeError> {
    // LuaLaTeX normally indexes both TeX and operating-system fonts. A managed
    // lock describes only the former, so make that boundary explicit instead
    // of allowing luaotfload to read /etc/fonts or a user's font directories.
    atomic_write(
        target,
        b"; Generated by texe: managed LuaLaTeX sees only locked TEXMF fonts.\n\
          [db]\n\
          location-precedence = texmf\n",
    )
}

/// The base-font map TeX Live's updmap puts first in pdftex.map by default.
const FOUNDATIONAL_PDFTEX_MAP: &str = "ps2pk35.map";

fn write_pdftex_map(
    texmf: &Path,
    foundational_maps: &[&str],
    map_names: &[String],
    target: &Path,
) -> Result<(), TexeError> {
    let mut bytes = b"% Generated by texe from locked TeX Live map declarations.\n".to_vec();
    for map_name in foundational_maps {
        append_font_map(texmf, map_name, &mut bytes)?;
    }
    for map_name in map_names {
        append_font_map(texmf, map_name, &mut bytes)?;
    }
    atomic_write(target, &bytes)
}

fn append_font_map(texmf: &Path, map_name: &str, bytes: &mut Vec<u8>) -> Result<(), TexeError> {
    let map = find_unique_font_map(&texmf.join("fonts/map"), map_name)?;
    bytes.extend_from_slice(format!("\n% {map_name}\n").as_bytes());
    let fragment = fs::read(&map).map_err(|source| TexeError::Io {
        path: map.clone(),
        source,
    })?;
    bytes.extend_from_slice(&fragment);
    if !fragment.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(())
}

fn find_unique_font_map(root: &Path, map_name: &str) -> Result<PathBuf, TexeError> {
    if Path::new(map_name).file_name().and_then(OsStr::to_str) != Some(map_name) {
        return Err(TexeError::Build(format!(
            "pqty emitted an invalid font-map name: {map_name}"
        )));
    }
    let mut matches = Vec::new();
    collect_named_files(root, map_name, &mut matches)?;
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(TexeError::Build(format!(
            "locked font-map fragment is missing from the pqty tree: {map_name}"
        ))),
        _ => Err(TexeError::Build(format!(
            "locked font-map fragment is ambiguous in the pqty tree: {map_name}"
        ))),
    }
}

fn collect_named_files(
    directory: &Path,
    name: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<(), TexeError> {
    let entries = fs::read_dir(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| TexeError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        // Recurse only into real directories, so a linked tree cannot loop, but
        // resolve the leaves: a symlinked or hardlinked package tree stores its
        // font maps as links into the pqty store, and `file_type` here does not
        // follow them.
        if file_type.is_dir() {
            collect_named_files(&path, name, matches)?;
        } else if entry.file_name() == OsStr::new(name) && path.is_file() {
            matches.push(path);
        }
    }
    Ok(())
}

fn format_staging_path(target: &Path, format_name: &str) -> PathBuf {
    target.with_file_name(format!(
        ".{}.{}.{}.tmp",
        format_name,
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ))
}

fn remove_format_staging(path: &Path, format_name: &str) -> Result<(), TexeError> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if !name.starts_with(&format!(".{format_name}."))
        || !Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
    {
        return Err(TexeError::Build(format!(
            "refusing to remove unexpected format staging path: {}",
            path.display()
        )));
    }
    fs::remove_dir_all(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn locked_languages_preserve_english_and_generate_all_engine_configurations() {
        let root = tempfile::tempdir().unwrap();
        let texmf = root.path().join("texmf");
        let base = texmf.join("tex/generic/config");
        let config = root.path().join("config");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(base.join("language.us"), "english hyphen.tex\n=american\n").unwrap();
        fs::write(
            base.join("language.us.def"),
            "\\addlanguage {USenglish}{hyphen}{}{2}{3}\n",
        )
        .unwrap();
        fs::write(
            base.join("language.us.lua"),
            "return { [\"english\"]={special=\"language0\"},\n",
        )
        .unwrap();
        for file in ["loadhyph-pl.tex", "hyph-pl.pat.txt", "hyph-pl.hyp.txt"] {
            fs::write(texmf.join(file), b"locked language bytes").unwrap();
        }
        let language = serde_json::from_value(
            serde_json::json!({"provider":"hyphen-polish", "declaration": {
                "name":"polish", "synonyms":["Polish"], "loader":"loadhyph-pl.tex",
                "patterns":["hyph-pl.pat.txt"], "exceptions":["hyph-pl.hyp.txt"],
                "left_hyphen_min":2, "right_hyphen_min":2,
                "attributes":[{"name":"luaspecial", "value":"disabled:fixture restriction"}]
            }}),
        )
        .unwrap();
        super::write_language_configuration(&texmf, &config, &[language], true).unwrap();
        let dat = fs::read_to_string(config.join("language.dat")).unwrap();
        assert_eq!(
            dat,
            "english hyphen.tex\n=american\n\npolish loadhyph-pl.tex\n=Polish\n"
        );
        let def = fs::read_to_string(config.join("language.def")).unwrap();
        assert!(def.starts_with("\\addlanguage {USenglish}{hyphen}{}{2}{3}"));
        assert!(def.contains("\\addlanguage {polish}{loadhyph-pl.tex}{}{2}{2}"));
        assert!(def.ends_with("\\uselanguage {USenglish}\n"));
        let lua = fs::read_to_string(config.join("language.dat.lua")).unwrap();
        assert!(lua.contains("patterns=\"hyph-pl.pat.txt\""));
        assert!(lua.contains("hyphenation=\"hyph-pl.hyp.txt\""));
        assert!(lua.contains("special=\"disabled:fixture restriction\""));
        assert!(lua.ends_with("}\n"));
    }

    #[test]
    fn format_cache_keys_are_bounded_and_separate_each_identity_component() {
        let inputs = [
            ("runtime-a", "packages-a", "pdflatex"),
            ("runtime-b", "packages-a", "pdflatex"),
            ("runtime-a", "packages-b", "pdflatex"),
            ("runtime-a", "packages-a", "lualatex"),
            ("a", "bc", "pdflatex"),
            ("ab", "c", "pdflatex"),
        ];
        let keys: std::collections::BTreeSet<_> = inputs
            .iter()
            .map(|(runtime, packages, recipe)| super::format_cache_key(runtime, packages, recipe))
            .collect();
        assert_eq!(keys.len(), inputs.len());
        assert!(
            keys.iter()
                .all(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        );
        let staging: std::collections::BTreeSet<_> = keys
            .iter()
            .map(|key| super::format_staging_path(Path::new(key), "pdflatex"))
            .collect();
        assert_eq!(staging.len(), keys.len());
    }
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use crate::build::format::{
        PDFLATEX_FORMAT, format_ini_argument, locked_format_ini, write_luaotfload_config,
        write_pdftex_map,
    };

    #[test]
    fn format_bootstrap_uses_the_exact_locked_ini() {
        let texmf = Path::new("/locked/texmf");
        assert_eq!(
            locked_format_ini(texmf, &PDFLATEX_FORMAT),
            texmf.join("tex/latex/tex-ini-files/pdflatex.ini")
        );
        let argument = format_ini_argument(texmf, &PDFLATEX_FORMAT, Path::new("/project"));
        let expected = OsString::from("/locked/texmf/tex/latex/tex-ini-files/pdflatex.ini");
        assert_eq!(argument, expected);
    }

    #[test]
    fn pdftex_map_places_foundational_aliases_before_package_maps() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let texmf = directory.path().join("texmf");
        let maps = texmf.join("fonts/map/dvips/example");
        fs::create_dir_all(&maps).expect("font map directory");
        fs::write(maps.join("ps2pk35.map"), b"pbkl8r URWBookman <ubkl8a.pfb\n")
            .expect("foundational map");
        fs::write(maps.join("example.map"), b"Example Example-Regular\n").expect("package map");
        let target = directory.path().join("pdftex.map");

        write_pdftex_map(
            &texmf,
            &[super::FOUNDATIONAL_PDFTEX_MAP],
            &["example.map".to_string()],
            &target,
        )
        .expect("combined map");

        let map = fs::read_to_string(target).expect("combined map text");
        assert!(map.contains("pbkl8r URWBookman"));
        assert!(map.contains("Example Example-Regular"));
        assert!(
            map.find("pbkl8r URWBookman").expect("foundational entry")
                < map.find("Example Example-Regular").expect("package entry")
        );
    }

    #[test]
    fn managed_luaotfload_config_disables_system_font_locations() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("luaotfload.conf");
        write_luaotfload_config(&path).expect("write configuration");
        let configuration = fs::read_to_string(path).expect("read configuration");
        assert!(configuration.contains("location-precedence = texmf"));
        assert!(!configuration.contains("location_precedence"));
        assert!(!configuration.contains("location-precedence = system"));
    }

    #[cfg(unix)]
    #[test]
    fn portable_format_creation_rejects_links_before_changing_research() {
        use crate::package::PackageEnvironment;
        use crate::progress::{Progress, ProgressLayout};
        use crate::toolchain::{ResolvedToolchain, ToolchainIdentity, VerificationPolicy};

        for alias in ["cache", "target", "formats", "format-file", "staging"] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("project");
            let research = fixture.path().join("research");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(&research).unwrap();
            let source = research.join("notes.tex");
            fs::write(&source, b"research source").unwrap();
            let build = root
                .join(".texe/environments")
                .join("a".repeat(64))
                .join("build");
            let cache = build.join("formats");
            let target = cache.join(super::format_cache_key(
                "test-toolchain",
                "test-packages",
                "pdflatex",
            ));
            let link = match alias {
                "cache" => cache,
                "target" => target.clone(),
                "formats" => target.join("formats"),
                "format-file" => target.join("formats/pdflatex.fmt"),
                "staging" => super::format_staging_path(&target, "pdflatex"),
                _ => unreachable!(),
            };
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(
                if alias == "format-file" {
                    &source
                } else {
                    &research
                },
                &link,
            )
            .unwrap();
            let toolchain = ResolvedToolchain {
                provider: "managed".into(),
                engine: "pdflatex".into(),
                engine_executable: root.join("engine-must-not-run"),
                kpsewhich_executable: root.join("kpsewhich-must-not-run"),
                texmf_dist: root.join("texmf"),
                engine_roots: Vec::new(),
                identity: ToolchainIdentity {
                    provider: "managed".into(),
                    engine: "pdflatex".into(),
                    channel: "test".into(),
                    target: "test".into(),
                    fingerprint: "test-toolchain".into(),
                    registry_url: None,
                    registry_metadata_digest: None,
                    artifacts: Vec::new(),
                },
                managed: None,
                verification: VerificationPolicy::Deep,
                offline: true,
            };
            let packages = PackageEnvironment {
                schema: "pqty.environment/v1".into(),
                fingerprint: "sha256:test-packages".into(),
                font_maps: Vec::new(),
                languages: Vec::new(),
            };
            let progress = Progress::new(
                root.join("timing.json"),
                "pdflatex",
                true,
                4,
                false,
                false,
                ProgressLayout::Standalone,
            );
            let error = super::ensure(
                &root,
                &toolchain,
                &root.join("texmf"),
                &packages,
                &build,
                true,
                &progress,
            )
            .unwrap_err();
            let message = error.to_string().to_lowercase();
            assert!(
                message.contains("private") || message.contains("portable"),
                "{alias}: {message}"
            );
            assert_eq!(fs::read(&source).unwrap(), b"research source", "{alias}");
            assert_eq!(fs::read_dir(&research).unwrap().count(), 1, "{alias}");
        }
    }

    #[test]
    fn recorded_format_inputs_are_placed_in_their_trees() {
        use super::{FormatInputRoot, recorded_inputs};
        let roots = [
            (
                FormatInputRoot::Staging,
                Path::new("/p/.texe/build/formats/.stage").into(),
            ),
            (
                FormatInputRoot::Packages,
                Path::new("/p/.texe/texmf").into(),
            ),
            (
                FormatInputRoot::Runtime,
                Path::new("/home/texe/toolchains/tl").into(),
            ),
        ];
        let recorder = "PWD /p\n\
            INPUT /home/texe/toolchains/tl/texmf-dist/web2c/texmf.cnf\n\
            INPUT /p/.texe/texmf/tex/latex/base/latex.ltx\n\
            INPUT /dev/null\n\
            OUTPUT /p/.texe/build/formats/.stage/formats/texsys.aux\n\
            INPUT /p/.texe/build/formats/.stage/formats/./texsys.aux\n\
            OUTPUT /p/.texe/build/formats/.stage/formats/pdflatex.log\n\
            INPUT /p/.texe/build/formats/.stage/config/tex/generic/config/language.dat\n\
            INPUT /p/.texe/texmf/tex/latex/base/latex.ltx\n";
        assert_eq!(
            recorded_inputs(recorder, &roots),
            Some(vec![
                (
                    FormatInputRoot::Staging,
                    "config/tex/generic/config/language.dat".into()
                ),
                (FormatInputRoot::Packages, "tex/latex/base/latex.ltx".into()),
                (
                    FormatInputRoot::Runtime,
                    "texmf-dist/web2c/texmf.cnf".into()
                ),
            ])
        );
    }

    #[test]
    fn formats_that_read_a_project_file_are_never_reused_or_shared() {
        use super::{FormatInputRoot, recorded_inputs};
        let roots = [
            (
                FormatInputRoot::Staging,
                Path::new("/p/.texe/build/formats/.stage").into(),
            ),
            (
                FormatInputRoot::Packages,
                Path::new("/p/.texe/texmf").into(),
            ),
            (
                FormatInputRoot::Runtime,
                Path::new("/home/texe/toolchains/tl").into(),
            ),
        ];
        let package = "INPUT /p/.texe/texmf/tex/latex/base/latex.ltx\n";
        for project_file in [
            // A `.` search path resolving a project file.
            "INPUT ./cmr10.tfm\n",
            "INPUT /p/cmr10.tfm\n",
            "INPUT /p/.texe/texmf/../../hyphen.tex\n",
            "INPUT /elsewhere/fonttext.cfg\n",
        ] {
            let recorder = format!("PWD /p\n{package}{project_file}");
            assert_eq!(recorded_inputs(&recorder, &roots), None, "{project_file}");
        }
        // Relative inputs cannot be placed without a working directory.
        assert_eq!(recorded_inputs("INPUT tex/latex.ltx\n", &roots), None);
    }
}
