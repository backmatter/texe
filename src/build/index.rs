//! MakeIndex-backed index and glossary workflows.
//!
//! LaTeX's ordinary index writes `.idx`. The `glossaries` package records each
//! glossary's input/log/output extensions in `\@newglossary` lines in the
//! auxiliary file and writes a job-specific `.ist`. Both reduce to one pinned
//! `MakeIndex` invocation, so texe does not need the host-side Perl
//! `makeglossaries` wrapper.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::build::process::{managed_path, raw_engine_output, raw_output, search_path_from};
use crate::config::ProjectManifest;
use crate::progress::{PhaseKind, Progress};
use crate::toolchain::{ResolvedToolchain, resolve_executable};

#[derive(Debug, Default)]
pub(super) struct IndexState {
    processed: BTreeMap<PathBuf, String>,
    runs: usize,
}

impl IndexState {
    pub(super) fn runs(&self) -> usize {
        self.runs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexControl {
    input: PathBuf,
    output: PathBuf,
    log: PathBuf,
    style: Option<PathBuf>,
}

pub(super) fn process_pending(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    texmf: &Path,
    output_dir: &Path,
    state: &mut IndexState,
    progress: &Progress,
) -> Result<bool, TexeError> {
    let mut ran = false;
    for control in index_controls(output_dir)? {
        let digest = control_digest(&control)?;
        if state.processed.get(&control.input) == Some(&digest) {
            continue;
        }
        if !nonempty_file(&control.input) {
            let removed = remove_if_file(&control.output)? | remove_if_file(&control.log)?;
            state.processed.insert(control.input, digest);
            ran |= removed;
            continue;
        }
        let label = control
            .input
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("document");
        progress.phase(
            PhaseKind::Index,
            format!("running MakeIndex for {label}"),
            || {
                run_makeindex(
                    project_root,
                    manifest,
                    toolchain,
                    texmf,
                    output_dir,
                    &control,
                )
            },
        )?;
        state.processed.insert(control.input, digest);
        state.runs += 1;
        ran = true;
    }
    Ok(ran)
}

fn index_controls(output_dir: &Path) -> Result<Vec<IndexControl>, TexeError> {
    let mut auxiliary = Vec::new();
    collect_files_with_extension(output_dir, output_dir, "aux", &mut auxiliary)?;
    let mut controls = BTreeMap::new();
    let mut glossary_inputs = BTreeSet::new();

    for path in auxiliary {
        let bytes = fs::read(&path).map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        for line in bytes.split(|byte| *byte == b'\n') {
            let Some(fields) = glossary_fields(line) else {
                continue;
            };
            let Some(stem) = path.file_stem() else {
                continue;
            };
            let base = path.with_file_name(stem);
            let input = base.with_extension(fields.input);
            if !input.is_file() {
                continue;
            }
            glossary_inputs.insert(input.clone());
            controls.insert(
                input.clone(),
                IndexControl {
                    input,
                    output: base.with_extension(fields.output),
                    log: base.with_extension(fields.log),
                    style: Some(base.with_extension("ist")),
                },
            );
        }
    }

    let mut indexes = Vec::new();
    collect_files_with_extension(output_dir, output_dir, "idx", &mut indexes)?;
    for input in indexes {
        if glossary_inputs.contains(&input) {
            continue;
        }
        controls.insert(
            input.clone(),
            IndexControl {
                output: input.with_extension("ind"),
                log: input.with_extension("ilg"),
                input,
                style: None,
            },
        );
    }
    Ok(controls.into_values().collect())
}

struct GlossaryFields<'a> {
    log: &'a str,
    output: &'a str,
    input: &'a str,
}

fn glossary_fields(line: &[u8]) -> Option<GlossaryFields<'_>> {
    let start = find_bytes(line, br"\@newglossary")? + br"\@newglossary".len();
    let groups = brace_groups(&line[start..]);
    let fields = if groups.len() >= 4 {
        &groups[groups.len() - 3..]
    } else {
        return None;
    };
    let log = safe_extension(fields[0])?;
    let output = safe_extension(fields[1])?;
    let input = safe_extension(fields[2])?;
    Some(GlossaryFields { log, output, input })
}

fn brace_groups(bytes: &[u8]) -> Vec<&str> {
    let mut groups = Vec::new();
    let mut offset = 0;
    while let Some(open) = bytes[offset..].iter().position(|byte| *byte == b'{') {
        let start = offset + open + 1;
        let Some(close) = bytes[start..].iter().position(|byte| *byte == b'}') else {
            break;
        };
        let end = start + close;
        if let Ok(value) = std::str::from_utf8(&bytes[start..end]) {
            groups.push(value);
        }
        offset = end + 1;
    }
    groups
}

fn safe_extension(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then_some(value)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn nonempty_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.len() > 0)
}

fn remove_if_file(path: &Path) -> Result<bool, TexeError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(TexeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn collect_files_with_extension(
    root: &Path,
    directory: &Path,
    extension: &str,
    paths: &mut Vec<PathBuf>,
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
            collect_files_with_extension(root, &path, extension, paths)?;
        } else if file_type.is_file()
            && path.starts_with(root)
            && path.extension() == Some(OsStr::new(extension))
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn control_digest(control: &IndexControl) -> Result<String, TexeError> {
    let mut hasher = Sha256::new();
    for path in [Some(&control.input), control.style.as_ref()]
        .into_iter()
        .flatten()
    {
        hasher.update(path.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        let bytes = fs::read(path).map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        hasher.update(bytes);
        hasher.update(b"\0");
    }
    Ok(hex::encode(hasher.finalize()))
}

fn run_makeindex(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    texmf: &Path,
    output_dir: &Path,
    control: &IndexControl,
) -> Result<(), TexeError> {
    let executable = resolve_makeindex(project_root, manifest, toolchain)?;
    let directory = control.input.parent().ok_or_else(|| {
        TexeError::Build(format!(
            "index control file has no parent: {}",
            control.input.display()
        ))
    })?;
    let local_name = |path: &Path| {
        path.file_name()
            .map(std::ffi::OsStr::to_os_string)
            .ok_or_else(|| {
                TexeError::Build(format!("index path has no filename: {}", path.display()))
            })
    };
    let mut arguments = Vec::new();
    if let Some(style) = &control.style {
        if !style.is_file() {
            return Err(TexeError::Build(format!(
                "glossary input {} has no generated MakeIndex style {}",
                control.input.display(),
                style.display()
            )));
        }
        arguments.extend([OsString::from("-s"), local_name(style)?]);
    }
    arguments.extend([
        OsString::from("-t"),
        local_name(&control.log)?,
        OsString::from("-o"),
        local_name(&control.output)?,
        local_name(&control.input)?,
    ]);
    let environment = index_environment(project_root, texmf, output_dir, toolchain, directory);
    let output = if toolchain.managed.is_some() {
        raw_engine_output(&executable, &arguments, directory, &environment)?
    } else {
        raw_output(&executable, &arguments, directory, &environment)?
    };
    if output.status.success() {
        return Ok(());
    }
    let processor_log = fs::read_to_string(&control.log).unwrap_or_default();
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
        "MakeIndex failed for {} with status {}:\n{}",
        control.input.display(),
        output.status,
        detail.trim()
    )))
}

fn resolve_makeindex(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
) -> Result<PathBuf, TexeError> {
    if let Some(managed) = &toolchain.managed
        && manifest.index.makeindex == "makeindex"
    {
        return Ok(managed.binary_dir.join("makeindex"));
    }
    resolve_executable(project_root, &manifest.index.makeindex)
}

fn index_environment(
    project_root: &Path,
    texmf: &Path,
    output_dir: &Path,
    toolchain: &ResolvedToolchain,
    working_directory: &Path,
) -> Vec<(OsString, OsString)> {
    let package_styles = texmf.join("makeindex");
    let mut roots = vec![project_root, output_dir, package_styles.as_path()];
    // Own the managed path for the duration of search-path construction.
    let managed_styles = toolchain
        .managed
        .as_ref()
        .map(|managed| managed.root.join("texmf-dist/makeindex"));
    if let Some(styles) = managed_styles.as_deref() {
        roots.push(styles);
    }
    let mut environment = vec![(
        OsString::from("INDEXSTYLE"),
        search_path_from(&roots, working_directory),
    )];
    if let Some(managed) = &toolchain.managed {
        environment.push((OsString::from("PATH"), managed_path(&managed.binary_dir)));
    }
    environment
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::build::index::{glossary_fields, index_controls};

    #[test]
    fn discovers_standard_indexes_and_all_declared_glossaries() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::write(root.join("main.idx"), br"\indexentry{alpha}{1}").expect("index");
        fs::write(root.join("main.glo"), br"\glossaryentry{term}{1}").expect("glossary");
        fs::write(root.join("main.acn"), br"\glossaryentry{api}{1}").expect("acronym");
        fs::write(root.join("main.ist"), b"actual '?'\n").expect("style");
        fs::write(
            root.join("main.aux"),
            br"\@newglossary{main}{glg}{gls}{glo}
\@newglossary[alg]{acronym}{alg}{acr}{acn}
",
        )
        .expect("auxiliary");

        let controls = index_controls(root).expect("controls");
        assert_eq!(controls.len(), 3);
        assert!(
            controls
                .iter()
                .any(|control| { control.input.ends_with("main.idx") && control.style.is_none() })
        );
        assert!(controls.iter().any(|control| {
            control.input.ends_with("main.glo")
                && control.output.ends_with("main.gls")
                && control.log.ends_with("main.glg")
        }));
        assert!(controls.iter().any(|control| {
            control.input.ends_with("main.acn")
                && control.output.ends_with("main.acr")
                && control.log.ends_with("main.alg")
        }));
    }

    #[test]
    fn rejects_unsafe_glossary_extensions() {
        assert!(glossary_fields(br"\@newglossary{main}{glg}{gls}{../glo}").is_none());
    }
}
