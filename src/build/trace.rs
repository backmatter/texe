use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(windows)]
use std::path::Component;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::build::process::checked_output;
use crate::build::warnings::ErrorContext as _;
use crate::package::PqtyClient;
use crate::toolchain::ResolvedToolchain;

const TRACE_SCHEMA: &str = "pqty.trace/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InputTrace {
    schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    producer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment_fingerprint: Option<String>,
    inputs: Vec<TraceInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TraceInput {
    requested: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved: Option<String>,
    scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TraceEnvironment {
    fingerprint: String,
}

pub(super) struct TraceRequest<'a> {
    pub(super) project_root: &'a Path,
    pub(super) tools: &'a PqtyClient,
    pub(super) toolchain: &'a ResolvedToolchain,
    pub(super) environment_path: &'a Path,
    pub(super) texmf: &'a Path,
    pub(super) output_dir: &'a Path,
    pub(super) log_path: &'a Path,
    pub(super) recorder_path: &'a Path,
    pub(super) managed_format_root: Option<&'a Path>,
    pub(super) discovery: bool,
    pub(super) trace_path: &'a Path,
}

pub(super) fn create(request: &TraceRequest<'_>) -> Result<(), TexeError> {
    let mut trace = if request.recorder_path.is_file() {
        let mut arguments = vec![
            OsString::from("--fls"),
            request.recorder_path.as_os_str().to_os_string(),
            OsString::from("--environment"),
            request.environment_path.as_os_str().to_os_string(),
        ];
        let mut roots = vec![
            ClassificationRoot::new("--project-root", request.project_root)?,
            ClassificationRoot::new("--output-root", request.output_dir)?,
            ClassificationRoot::new("--package-root", request.texmf)?,
        ];
        if request.discovery && request.toolchain.managed.is_none() {
            roots.push(ClassificationRoot::new(
                "--package-root",
                &request.toolchain.texmf_dist,
            )?);
        }
        for root in &request.toolchain.engine_roots {
            roots.push(ClassificationRoot::new("--engine-root", root)?);
        }
        if let Some(format_root) = request.managed_format_root {
            roots.push(ClassificationRoot::new("--engine-root", format_root)?);
        }
        add_recorder_root_aliases(&mut roots, request.recorder_path, request.project_root);
        push_classification_roots(&mut arguments, &roots);
        arguments.push(OsString::from("--output"));
        arguments.push(request.trace_path.as_os_str().to_os_string());
        checked_output(
            &request.tools.trace_adapter,
            &arguments,
            request.project_root,
            &[],
        )
        .map_err(|error| error.context("could not adapt engine recorder trace"))?;
        read_trace(request.trace_path)?
    } else {
        let environment = read_environment(request.environment_path)?;
        InputTrace {
            schema: TRACE_SCHEMA.to_string(),
            producer: Some(format!("texe/{}", env!("CARGO_PKG_VERSION"))),
            environment_fingerprint: Some(environment.fingerprint),
            inputs: Vec::new(),
        }
    };

    let log = fs::read_to_string(request.log_path).unwrap_or_default();
    let existing = trace
        .inputs
        .iter()
        .map(|input| input.requested.clone())
        .collect::<BTreeSet<_>>();
    for (requested, kind) in missing_package_inputs(&log, request.texmf) {
        if !existing.contains(&requested) {
            trace.inputs.push(TraceInput {
                requested,
                resolved: None,
                scope: "package".to_string(),
                kind: Some(kind),
            });
        }
    }
    trace.inputs.sort_by(|left, right| {
        (&left.scope, &left.requested, &left.resolved).cmp(&(
            &right.scope,
            &right.requested,
            &right.resolved,
        ))
    });
    trace.inputs.dedup();
    let mut bytes = serde_json::to_vec_pretty(&trace).map_err(|source| TexeError::Json {
        path: request.trace_path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    atomic_write(request.trace_path, &bytes)
}

struct ClassificationRoot {
    flag: &'static str,
    #[cfg(windows)]
    canonical: PathBuf,
    aliases: Vec<PathBuf>,
}

impl ClassificationRoot {
    fn new(flag: &'static str, root: &Path) -> Result<Self, TexeError> {
        let canonical = canonical_root(root)?;
        let mut aliases = if flag == "--project-root" {
            vec![canonical.clone()]
        } else {
            vec![root.to_path_buf()]
        };
        if canonical != root && flag != "--project-root" {
            aliases.push(canonical.clone());
        }
        Ok(Self {
            flag,
            #[cfg(windows)]
            canonical,
            aliases,
        })
    }
}

fn push_classification_roots(arguments: &mut Vec<OsString>, roots: &[ClassificationRoot]) {
    for root in roots {
        for alias in &root.aliases {
            arguments.push(OsString::from(root.flag));
            arguments.push(alias.as_os_str().to_os_string());
        }
    }
}

#[cfg(not(windows))]
fn add_recorder_root_aliases(
    _roots: &mut [ClassificationRoot],
    _recorder_path: &Path,
    _working_directory: &Path,
) {
}

/// TeX Live on Windows can write an 8.3 spelling such as `RUNNER~1` to its
/// recorder even when it opened the file through the corresponding long path.
/// pqty-fls deliberately classifies paths lexically, so teach it only aliases
/// whose filesystem identity we can prove.
#[cfg(windows)]
fn add_recorder_root_aliases(
    roots: &mut [ClassificationRoot],
    recorder_path: &Path,
    working_directory: &Path,
) {
    let Ok(contents) = fs::read_to_string(recorder_path) else {
        return;
    };
    let mut working_directory = absolute_lexical(working_directory, working_directory);
    let mut seen = BTreeSet::new();

    propagate_root_aliases(roots);
    for line in contents.lines() {
        if let Some(path) = line.strip_prefix("PWD ") {
            if !path.is_empty() {
                working_directory = absolute_lexical(Path::new(path), &working_directory);
                add_working_directory_aliases(roots, &working_directory);
            }
            continue;
        }
        let Some(path) = line.strip_prefix("INPUT ") else {
            continue;
        };
        let recorded = absolute_lexical(Path::new(path), &working_directory);
        if !seen.insert(recorded.clone())
            || roots
                .iter()
                .any(|root| root.aliases.iter().any(|alias| recorded.starts_with(alias)))
        {
            continue;
        }
        let Ok(canonical_recorded) = fs::canonicalize(&recorded) else {
            continue;
        };
        for root in roots.iter_mut() {
            let Some(alias) =
                equivalent_root_alias(&recorded, &canonical_recorded, &root.canonical)
            else {
                continue;
            };
            if !root.aliases.contains(&alias)
                && fs::canonicalize(&alias).is_ok_and(|path| path == root.canonical)
            {
                root.aliases.push(alias);
            }
        }
        propagate_root_aliases(roots);
    }
}

#[cfg(windows)]
fn add_working_directory_aliases(roots: &mut [ClassificationRoot], recorded: &Path) {
    let Some(project) = roots.first() else {
        return;
    };
    if !recorder_spelling_matches(recorded, &project.canonical) {
        return;
    }
    let project = project.canonical.clone();
    for (index, root) in roots.iter_mut().enumerate() {
        let Ok(relative) = root.canonical.strip_prefix(&project) else {
            continue;
        };
        let alias = recorded.join(relative);
        if index == 0 {
            root.aliases.clear();
            root.aliases.push(alias);
        } else if !root.aliases.contains(&alias) {
            root.aliases.push(alias);
        }
    }
}

/// The Windows recorder can replace a non-ASCII character that is absent from
/// the active code page with `?`. Accept that spelling only for the exact
/// working directory texe supplied to the child process.
#[cfg(windows)]
fn recorder_spelling_matches(recorded: &Path, expected: &Path) -> bool {
    let Some(recorded) = portable_windows_path(recorded) else {
        return false;
    };
    let Some(expected) = portable_windows_path(expected) else {
        return false;
    };
    let mut expected = expected.chars();
    recorded.chars().all(|observed| {
        expected.next().is_some_and(|actual| {
            (observed == '?' && !actual.is_ascii())
                || observed == actual
                || (observed.is_ascii()
                    && actual.is_ascii()
                    && observed.eq_ignore_ascii_case(&actual))
        })
    }) && expected.next().is_none()
}

#[cfg(windows)]
fn portable_windows_path(path: &Path) -> Option<String> {
    let path = path.to_str()?;
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    };
    Some(path.replace('\\', "/"))
}

#[cfg(windows)]
fn propagate_root_aliases(roots: &mut [ClassificationRoot]) {
    let mut candidates = Vec::new();
    for parent in roots.iter() {
        for (index, child) in roots.iter().enumerate() {
            let Ok(relative) = child.canonical.strip_prefix(&parent.canonical) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                continue;
            }
            candidates.extend(
                parent
                    .aliases
                    .iter()
                    .map(|alias| (index, alias.join(relative))),
            );
        }
    }
    for (index, alias) in candidates {
        let root = &mut roots[index];
        if !root.aliases.contains(&alias)
            && fs::canonicalize(&alias).is_ok_and(|path| path == root.canonical)
        {
            root.aliases.push(alias);
        }
    }
}

#[cfg(windows)]
fn equivalent_root_alias(
    recorded: &Path,
    canonical_recorded: &Path,
    canonical_root: &Path,
) -> Option<PathBuf> {
    let relative = canonical_recorded.strip_prefix(canonical_root).ok()?;
    let mut alias = recorded.to_path_buf();
    for _ in relative.components() {
        alias.pop().then_some(())?;
    }
    Some(alias)
}

#[cfg(windows)]
fn absolute_lexical(path: &Path, relative_to: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        relative_to.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn canonical_root(root: &Path) -> Result<PathBuf, TexeError> {
    fs::canonicalize(root).map_err(|source| TexeError::Io {
        path: root.to_path_buf(),
        source,
    })
}

pub(super) fn runtime_provider_requirements(log_path: &Path) -> Vec<String> {
    let log = fs::read_to_string(log_path).unwrap_or_default();
    let mut providers = BTreeSet::new();
    for line in logical_log_lines(&log) {
        if let Some(provider) = missing_pdftex_type1_provider(line.as_ref()) {
            insert_font_provider_requirements(&mut providers, provider);
        }
    }
    let missfont = log_path.with_file_name("missfont.log");
    for line in fs::read_to_string(missfont).unwrap_or_default().lines() {
        if let Some(provider) = missing_mktexpk_provider(line) {
            insert_font_provider_requirements(&mut providers, provider);
        }
    }
    providers.into_iter().map(ToString::to_string).collect()
}

fn insert_font_provider_requirements(
    providers: &mut BTreeSet<&'static str>,
    provider: &'static str,
) {
    providers.insert(provider);
    if provider == "cm-super" {
        // cm-super carries the scalable Type 1 outlines and maps; TeX Live's
        // separate ec provider carries the TFM metrics those outlines use.
        providers.insert("ec");
    }
}

fn read_trace(path: &Path) -> Result<InputTrace, TexeError> {
    let text = fs::read(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let trace: InputTrace = serde_json::from_slice(&text).map_err(|source| TexeError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    if trace.schema != TRACE_SCHEMA {
        return Err(TexeError::Build(format!(
            "trace adapter emitted schema {}; expected {TRACE_SCHEMA}",
            trace.schema
        )));
    }
    Ok(trace)
}

fn read_environment(path: &Path) -> Result<TraceEnvironment, TexeError> {
    let text = fs::read(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&text).map_err(|source| TexeError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn missing_package_inputs(log: &str, texmf: &Path) -> Vec<(String, String)> {
    let mut missing = BTreeSet::new();
    for line in logical_log_lines(log) {
        let line = line.as_ref();
        if let Some((requested, kind)) = missing_file_input(line) {
            missing.insert((requested, kind));
        }
        if let Some((requested, kind)) = missing_tex_input(line) {
            missing.insert((requested, kind));
        }
        if let Some(language) = missing_babel_language(line) {
            let requested = babel_language_definition(texmf, &language)
                .unwrap_or_else(|| format!("{language}.ldf"));
            missing.insert((requested, "tex".to_string()));
        }
        if let Some(requested) = missing_tikz_library(line) {
            missing.insert((requested, "tex".to_string()));
        }
        if let Some(requested) = missing_biblatex_style(line) {
            missing.insert((requested, "bibliography".to_string()));
        }
        if let Some(requested) = missing_font_metric(line) {
            missing.insert((requested, "font-metric".to_string()));
        }
        if let Some(requested) = missing_luatex_font_metric(line) {
            missing.insert((requested, "font-metric".to_string()));
        }
        if let Some((requested, kind)) = missing_fontspec_file(line) {
            missing.insert((requested, kind));
        }
        if let Some(requested) = missing_pdftex_type1_file(line) {
            missing.insert((requested, "type1-font".to_string()));
        }
        if let Some(requested) = missing_nfss_definition(line) {
            missing.insert((requested, "tex".to_string()));
        }
        if let Some(requested) = missing_lua_module(line) {
            missing.insert((requested, "data".to_string()));
        }
    }
    missing.into_iter().collect()
}

fn logical_log_lines(log: &str) -> Vec<Cow<'_, str>> {
    let physical = log.lines().collect::<Vec<_>>();
    let mut logical = Vec::with_capacity(physical.len());
    let mut start = 0;
    while start < physical.len() {
        let mut end = start;
        while end + 1 < physical.len() && physical[end].len() == 79 {
            end += 1;
        }
        if end == start {
            logical.push(Cow::Borrowed(physical[start]));
        } else {
            logical.push(Cow::Owned(physical[start..=end].concat()));
        }
        start = end + 1;
    }
    logical
}

fn missing_file_input(line: &str) -> Option<(String, String)> {
    let marker = ["File `", "file `"]
        .into_iter()
        .find(|marker| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find('\'')?;
    if !tail[end..].contains("not found") {
        return None;
    }
    let requested = tail[..end].trim();
    if !is_safe_registry_file_request(requested) {
        return None;
    }
    let extension = Path::new(requested)
        .extension()
        .and_then(OsStr::to_str)?
        .to_ascii_lowercase();
    let kind = match extension.as_str() {
        "sty" | "cls" | "def" | "fd" | "cfg" | "clo" | "ldf" => "tex",
        "tex" => "data",
        "bst" => "bibliography",
        _ => return None,
    };
    Some((requested.to_string(), kind.to_string()))
}

fn is_safe_registry_file_request(requested: &str) -> bool {
    !requested.is_empty()
        && !requested.starts_with('/')
        && !requested.contains('\\')
        && requested
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn missing_tex_input(line: &str) -> Option<(String, String)> {
    let requested = line
        .strip_prefix("! I can't find file `")?
        .strip_suffix("'.")?
        .trim();
    if requested.contains(['/', '\\', '#', '{', '}', ' '])
        || !requested
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    let extension = Path::new(requested)
        .extension()
        .and_then(OsStr::to_str)?
        .to_ascii_lowercase();
    let kind = match extension.as_str() {
        "sty" | "cls" | "def" | "fd" | "cfg" | "clo" | "ldf" => "tex",
        "bst" => "bibliography",
        _ => return None,
    };
    Some((requested.to_string(), kind.to_string()))
}

fn missing_fontspec_file(line: &str) -> Option<(String, String)> {
    let start = line.find("The font \"")? + "The font \"".len();
    let tail = &line[start..];
    let end = tail.find('"')?;
    if !tail[end..].contains("cannot be found") {
        return None;
    }
    let font = tail[..end].trim();
    if font.is_empty()
        || font.contains(['/', '\\', '#', '{', '}'])
        || !font
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b' '))
    {
        return None;
    }
    let extension = Path::new(font)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("otf") => Some((font.to_string(), "open-type-font".to_string())),
        Some("ttf") => Some((font.to_string(), "true-type-font".to_string())),
        Some(_) => None,
        None => Some((fontspec_regular_stem(font), "font-family".to_string())),
    }
}

fn fontspec_regular_stem(font: &str) -> String {
    let compact = font.replace(' ', "");
    if compact.contains('-') {
        return compact;
    }
    for suffix in ["BoldItalic", "Regular", "Italic", "Bold"] {
        if compact
            .to_ascii_lowercase()
            .ends_with(&suffix.to_ascii_lowercase())
        {
            let stem = &compact[..compact.len() - suffix.len()];
            return format!("{stem}-{suffix}");
        }
    }
    format!("{compact}-Regular")
}

fn missing_pdftex_type1_file(line: &str) -> Option<String> {
    let marker = "(file ";
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(')')?;
    // pdfTeX wraps this diagnostic after "Type" in ordinary 79-column logs.
    if !tail[end..].contains("cannot open Type") {
        return None;
    }
    let requested = tail[..end].trim();
    let extension = Path::new(requested)
        .extension()
        .and_then(OsStr::to_str)?
        .to_ascii_lowercase();
    (matches!(extension.as_str(), "pfb" | "pfa") && is_safe_registry_file_request(requested))
        .then(|| requested.to_string())
}

fn missing_nfss_definition(line: &str) -> Option<String> {
    let requested = line.strip_prefix("No file ")?.strip_suffix('.')?.trim();
    if !requested.to_ascii_lowercase().ends_with(".fd")
        || requested.contains(['/', '\\', '#', '{', '}', ' '])
        || !requested
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    Some(requested.to_ascii_lowercase())
}

fn missing_babel_language(line: &str) -> Option<String> {
    // TeX's `-file-line-error` mode prefixes package diagnostics with the
    // originating path and line number, while older/non-file-line logs begin
    // the same message with `!`. Match the stable Babel text in either form.
    let marker = "Package babel Error: Unknown option '";
    let tail = &line[line.find(marker)? + marker.len()..];
    let end = tail.find('\'')?;
    let language = tail[..end].trim();
    if language.is_empty()
        || language.contains(['/', '\\', '.', '#', '{', '}'])
        || !language
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(language.to_string())
}

/// Babel's user-facing language name is not always its definition filename:
/// for example `russian` is implemented by `russianb.ldf`. The already locked
/// Babel locale descriptor records that mapping, so use it instead of asking
/// pqty for a filename that TeX Live does not publish.
fn babel_language_definition(texmf: &Path, language: &str) -> Option<String> {
    let filename = format!("babel-{language}.tex");
    let mut pending = vec![texmf.join("tex/generic/babel/locale")];
    let mut candidates = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).ok()?.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if file_type.is_dir() {
                pending.push(path);
            } else if entry.file_name() == OsStr::new(&filename) && file_type.is_file() {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    let [descriptor] = candidates.as_slice() else {
        return None;
    };
    let descriptor = fs::read_to_string(descriptor).ok()?;
    let marker = "\\BabelDefinitionFile{0}{";
    let tail = &descriptor[descriptor.find(marker)? + marker.len()..];
    let end = tail.find('}')?;
    let module = tail[..end].trim();
    if module.is_empty()
        || !module
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(format!("{module}.ldf"))
}

fn missing_tikz_library(line: &str) -> Option<String> {
    let tail = line.strip_prefix("! Package tikz Error: I did not find the tikz library '")?;
    let end = tail.find('\'')?;
    let library = tail[..end].trim();
    if library.is_empty()
        || library.contains(['/', '\\', '.', '#', '{', '}', ' '])
        || !library
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(format!("tikzlibrary{library}.code.tex"))
}

fn missing_biblatex_style(line: &str) -> Option<String> {
    let tail = line.strip_prefix("! Package biblatex Error: Style '")?;
    let end = tail.find('\'')?;
    let style = tail[..end].trim();
    if !tail[end..].contains("not found")
        || style.is_empty()
        || style.contains(['/', '\\', '.', '#', '{', '}', ' '])
        || !style
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(format!("{style}.bbx"))
}

fn missing_font_metric(line: &str) -> Option<String> {
    let line = line.strip_prefix("! Font ")?;
    let (_, tail) = line.split_once('=')?;
    let metric = tail.split_whitespace().next()?.trim();
    if metric.eq_ignore_ascii_case("nullfont")
        || metric.eq_ignore_ascii_case("nullfont.tfm")
        || metric.is_empty()
        || metric.contains(['/', '\\', '#', '{', '}'])
        || !metric
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || !line.contains("not loadable: Metric (TFM) file")
    {
        return None;
    }
    if metric.to_ascii_lowercase().ends_with(".tfm") {
        Some(metric.to_string())
    } else {
        Some(format!("{metric}.tfm"))
    }
}

fn missing_luatex_font_metric(line: &str) -> Option<String> {
    let tail = line.strip_prefix("! Font ")?;
    let (_, metric_description) = tail.rsplit_once('=')?;
    let metric = metric_description.split_whitespace().next()?.trim();
    if metric.eq_ignore_ascii_case("nullfont")
        || metric.eq_ignore_ascii_case("nullfont.tfm")
        || !tail.contains("not loadable: metric data")
        || metric.is_empty()
        || !metric
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    if metric.to_ascii_lowercase().ends_with(".tfm") {
        Some(metric.to_string())
    } else {
        Some(format!("{metric}.tfm"))
    }
}

fn missing_pdftex_type1_provider(line: &str) -> Option<&'static str> {
    let pdftex = line.find("pdftex (file ")?;
    let line = &line[pdftex..];
    let start = line.find("(file ")? + "(file ".len();
    let tail = &line[start..];
    let end = tail.find(')')?;
    if !tail[end..].contains(": Font ") {
        return None;
    }
    let font = tail[..end].trim().to_ascii_lowercase();
    if font.starts_with("ec") || font.starts_with("tc") {
        // The managed runtime deliberately has no Metafont/mktexpk process.
        // cm-super supplies the Type 1 replacements and map fragments for the
        // EC/TC T1 and TS1 families; the caller pairs it with `ec` metrics.
        Some("cm-super")
    } else {
        None
    }
}

fn missing_mktexpk_provider(line: &str) -> Option<&'static str> {
    let command = line
        .strip_prefix("mktexpk ")
        .or_else(|| line.strip_prefix("mktextfm "))?;
    let font = command.split_whitespace().next_back()?.to_ascii_lowercase();
    if font.starts_with("ec") || font.starts_with("tc") {
        Some("cm-super")
    } else {
        None
    }
}

fn missing_lua_module(line: &str) -> Option<String> {
    let start = line.find("module '")? + "module '".len();
    let tail = &line[start..];
    let end = tail.find('\'')?;
    if !tail[end..].contains("not found") {
        return None;
    }
    let leaf = tail[..end].rsplit('.').next()?.trim();
    if leaf.is_empty() || leaf.contains(['/', '\\']) {
        return None;
    }
    Some(format!("{leaf}.lua"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    use crate::build::trace::canonical_root;
    #[cfg(windows)]
    use crate::build::trace::{equivalent_root_alias, recorder_spelling_matches};
    use crate::build::trace::{
        missing_babel_language, missing_font_metric, missing_fontspec_file, missing_package_inputs,
        missing_pdftex_type1_provider, runtime_provider_requirements,
    };

    #[cfg(unix)]
    #[test]
    fn classification_roots_include_supplied_and_canonical_aliases() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let physical = directory.path().join("physical");
        let alias = directory.path().join("alias");
        fs::create_dir(&physical).expect("physical directory");
        std::os::unix::fs::symlink(&physical, &alias).expect("directory alias");

        let mut arguments = Vec::new();
        let roots =
            [super::ClassificationRoot::new("--engine-root", &alias)
                .expect("classification roots")];
        super::push_classification_roots(&mut arguments, &roots);
        let canonical = physical.canonicalize().expect("physical root");
        assert_eq!(canonical_root(&alias).expect("canonical root"), canonical);
        assert_eq!(
            arguments,
            [
                std::ffi::OsString::from("--engine-root"),
                alias.into_os_string(),
                std::ffi::OsString::from("--engine-root"),
                canonical.clone().into_os_string(),
            ]
        );

        let project =
            [super::ClassificationRoot::new("--project-root", &physical).expect("project root")];
        let mut arguments = Vec::new();
        super::push_classification_roots(&mut arguments, &project);
        assert_eq!(
            arguments,
            [
                std::ffi::OsString::from("--project-root"),
                canonical.into_os_string(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn recorder_alias_preserves_the_windows_spelling_of_a_proven_root() {
        let recorded =
            Path::new(r"c:\Users\RUNNER~1\AppData\Local\texe\runtime\texmf-dist\web2c\texmf.cnf");
        let canonical_recorded = Path::new(
            r"\\?\C:\Users\runneradmin\AppData\Local\texe\runtime\texmf-dist\web2c\texmf.cnf",
        );
        let canonical_root = Path::new(r"\\?\C:\Users\runneradmin\AppData\Local\texe\runtime");
        assert_eq!(
            equivalent_root_alias(recorded, canonical_recorded, canonical_root),
            Some(Path::new(r"c:\Users\RUNNER~1\AppData\Local\texe\runtime").to_path_buf())
        );
    }

    #[cfg(windows)]
    #[test]
    fn recorder_working_directory_allows_only_non_ascii_substitution() {
        let expected = Path::new(r"\\?\C:\Users\Ada\Research Δ Results");
        assert!(recorder_spelling_matches(
            Path::new(r"c:\Users\Ada\Research ? Results"),
            expected
        ));
        assert!(!recorder_spelling_matches(
            Path::new(r"c:\Users\Ada\Research X Results"),
            expected
        ));
        assert!(!recorder_spelling_matches(
            Path::new(r"c:\Users\?da\Research Δ Results"),
            expected
        ));
    }

    #[test]
    fn parses_only_package_shaped_missing_files() {
        let log = r#"
! LaTeX Error: File `xcolor.sty' not found.
! LaTeX Error: File `article.cls' not found.
! LaTeX Error: File `ushyphex.tex' not found.
! LaTeX Error: File `../untrusted.tex' not found.
! Package fontenc Error: Encoding file `lgrenc.def' not found.
! Package babel Error: Unknown option 'english'.
! Package tikz Error: I did not find the tikz library 'tikzmark'. I looked for
! Package biblatex Error: Style 'ieee' not found.
! I can't find file `RoyalIn.fd'.
! Font TS1/cmr/m/n/10.95=tcrm1095 at 10.95pt not loadable: Metric (TFM) file no
! Font \=txsyc at 10.95pt not loadable: metric data not found or bad.
! Font \U/MnSymbolC/m/n/10.95=MnSymbolC10 at 10.95pt not loadable: metric data n
! Font \SOUL@tt=ectt1000 not loadable: metric data not found or bad.
! Font \T1/pbk/l/n/10=nullfont not loadable: Metric (TFM) file not found.
(fontspec)                The font "texgyretermes-regular" cannot be found;
(fontspec)                The font "Inconsolatazi4" cannot be found;
!pdfTeX error: pdftex (file putb8a.pfb): cannot open Type
 1 font file for reading
[\directlua]:1: module 'luaotfload-main' not found:
No file LGRcmr.fd.
"#;
        assert_eq!(
            missing_package_inputs(log, Path::new("/missing-texmf")),
            vec![
                (
                    "Inconsolatazi4-Regular".to_string(),
                    "font-family".to_string()
                ),
                ("MnSymbolC10.tfm".to_string(), "font-metric".to_string()),
                ("RoyalIn.fd".to_string(), "tex".to_string()),
                ("article.cls".to_string(), "tex".to_string()),
                ("ectt1000.tfm".to_string(), "font-metric".to_string()),
                ("english.ldf".to_string(), "tex".to_string()),
                ("ieee.bbx".to_string(), "bibliography".to_string()),
                ("lgrcmr.fd".to_string(), "tex".to_string()),
                ("lgrenc.def".to_string(), "tex".to_string()),
                ("luaotfload-main.lua".to_string(), "data".to_string()),
                ("putb8a.pfb".to_string(), "type1-font".to_string()),
                ("tcrm1095.tfm".to_string(), "font-metric".to_string()),
                (
                    "texgyretermes-regular".to_string(),
                    "font-family".to_string()
                ),
                (
                    "tikzlibrarytikzmark.code.tex".to_string(),
                    "tex".to_string()
                ),
                ("txsyc.tfm".to_string(), "font-metric".to_string()),
                ("ushyphex.tex".to_string(), "data".to_string()),
                ("xcolor.sty".to_string(), "tex".to_string())
            ]
        );
    }

    #[test]
    fn rejects_unsafe_babel_option_text() {
        assert_eq!(
            missing_babel_language("! Package babel Error: Unknown option '../english'."),
            None
        );
        assert_eq!(
            missing_babel_language("! Package babel Error: Unknown option 'english.ini'."),
            None
        );
        assert_eq!(
            missing_font_metric(
                "! Font T1/cmr/m/n/10=../../secret at 10pt not loadable: Metric (TFM) file"
            ),
            None
        );
        assert_eq!(
            missing_pdftex_type1_provider(
                "ux-runtime/bin/pdftex (file tcrm1000): Font tcrm1000 at 6"
            ),
            Some("cm-super")
        );
        assert_eq!(
            missing_pdftex_type1_provider(
                "!pdfTeX error: /runtime/pdftex (file cmr10): Font cmr10 at 600"
            ),
            None
        );
        assert_eq!(
            missing_fontspec_file("(fontspec) The font \"../../secret\" cannot be found;"),
            None
        );
    }

    #[test]
    fn detects_babel_language_in_file_line_error_output() {
        assert_eq!(
            missing_babel_language(
                "/project/.texe/texmf/tex/generic/babel/babel.sty:4330: \
                 Package babel Error: Unknown option 'english'."
            ),
            Some("english".to_string())
        );
    }

    #[test]
    fn detects_babel_language_when_tex_wraps_the_diagnostic_marker() {
        let log = format!(
            "{}Package b\nabel Error: Unknown option 'english'.",
            "x".repeat(70)
        );
        assert_eq!(
            missing_package_inputs(&log, Path::new("/missing-texmf")),
            vec![("english.ldf".to_string(), "tex".to_string())]
        );
    }

    #[test]
    fn babel_language_diagnostics_follow_the_locked_locale_mapping() {
        let directory = tempfile::tempdir().expect("temporary TEXMF");
        let locale = directory
            .path()
            .join("tex/generic/babel/locale/ru/babel-russian.tex");
        fs::create_dir_all(locale.parent().expect("locale parent")).expect("locale directory");
        fs::write(&locale, b"\\BabelDefinitionFile{0}{russianb}{}%\n").expect("locale descriptor");

        assert_eq!(
            missing_package_inputs(
                "! Package babel Error: Unknown option 'russian'.",
                directory.path(),
            ),
            vec![("russianb.ldf".to_string(), "tex".to_string())]
        );
    }

    #[test]
    fn detects_wrapped_pdftex_ec_font_failures() {
        assert_eq!(
            missing_pdftex_type1_provider(
                "ux-runtime/bin/pdftex (file ecrm3583): Font ecrm3583 at 600 not found"
            ),
            Some("cm-super")
        );
    }

    #[test]
    fn detects_ec_fonts_from_missfont_artifact() {
        let directory = tempfile::tempdir().expect("temporary engine output");
        let log = directory.path().join("document.log");
        fs::write(&log, b"ordinary TeX log\n").expect("engine log");
        fs::write(
            directory.path().join("missfont.log"),
            b"mktexpk --mfmode / --bdpi 600 --mag 1+0/600 --dpi 600 ecrm3583\n\
              mktextfm ecti1000\n",
        )
        .expect("missfont log");

        assert_eq!(
            runtime_provider_requirements(&log),
            vec!["cm-super".to_string(), "ec".to_string()]
        );
    }
}
