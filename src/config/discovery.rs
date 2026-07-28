use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::config::validation::validate_relative_path;
use crate::config::{DEFAULT_ENGINE, DEFAULT_ENTRY, InitRequest, InitSettings, MANIFEST_NAME};

/// Find the nearest `texe.toml` at or above `start`.
///
/// # Errors
///
/// Returns an error when no project manifest is found.
pub fn discover_manifest(start: &Path) -> Result<PathBuf, TexeError> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    for directory in start.ancestors() {
        let candidate = directory.join(MANIFEST_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(TexeError::Manifest(format!(
        "could not find {MANIFEST_NAME} from {}",
        start.display()
    )))
}

pub fn resolve_manifest(argument: Option<&Path>) -> Result<PathBuf, TexeError> {
    match argument {
        Some(path) if path.is_file() => Ok(path.to_path_buf()),
        Some(path) => discover_manifest(path),
        None => {
            let current = std::env::current_dir().map_err(|source| TexeError::Io {
                path: PathBuf::from("."),
                source,
            })?;
            discover_manifest(&current)
        }
    }
}

/// Detect project entry points and resolve the choices needed by `texe init`.
///
/// Explicit request values always win. Interactive requests ask only when a
/// choice remains; non-interactive requests reject ambiguous entry points
/// unless `accept_defaults` is set.
///
/// # Errors
///
/// Returns an error for unsafe paths, unreadable directories, ambiguous
/// non-interactive entry points, or invalid prompt input.
pub fn configure_init(directory: &Path, request: &InitRequest) -> Result<InitSettings, TexeError> {
    configure_init_with(directory, request, &mut cliclack_select)
}

pub(super) fn configure_init_with<F>(
    directory: &Path,
    request: &InitRequest,
    select: &mut F,
) -> Result<InitSettings, TexeError>
where
    F: FnMut(&str, &[String], usize) -> Result<usize, TexeError>,
{
    let entry = match request.entry.as_ref() {
        Some(entry) => {
            validate_relative_path("entry", entry)?;
            entry.clone()
        }
        None => select_entry(directory, request, select)?,
    };
    let hinted_engine = detect_engine_hint(&directory.join(&entry));
    let engine = match request.engine.as_deref() {
        Some(engine) => validate_engine(engine)?,
        None => hinted_engine.unwrap_or_else(|| DEFAULT_ENGINE.to_string()),
    };
    Ok(InitSettings { entry, engine })
}

fn cliclack_select(message: &str, options: &[String], default: usize) -> Result<usize, TexeError> {
    let mut prompt = cliclack::select(message);
    for (index, option) in options.iter().enumerate() {
        prompt = prompt.item(index, option, "");
    }
    crate::ux::prompt(
        prompt
            .initial_value(default.min(options.len().saturating_sub(1)))
            .interact(),
    )
}

fn select_entry<F>(
    directory: &Path,
    request: &InitRequest,
    select: &mut F,
) -> Result<PathBuf, TexeError>
where
    F: FnMut(&str, &[String], usize) -> Result<usize, TexeError>,
{
    let candidates = discover_entries(directory)?;
    match candidates.as_slice() {
        [] => Ok(PathBuf::from(DEFAULT_ENTRY)),
        [entry] => Ok(entry.clone()),
        entries => {
            let default = preferred_entry(entries);
            if request.interactive {
                let options = entries
                    .iter()
                    .map(|entry| entry.display().to_string())
                    .collect::<Vec<_>>();
                let selected = select("LaTeX entry", &options, default)?;
                Ok(entries[selected].clone())
            } else if request.accept_defaults {
                Ok(entries[default].clone())
            } else {
                Err(TexeError::Manifest(format!(
                    "found multiple LaTeX entry points ({}); pass --entry <path> or run \
                     `texe init` interactively",
                    entries
                        .iter()
                        .map(|entry| entry.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
        }
    }
}

fn discover_entries(directory: &Path) -> Result<Vec<PathBuf>, TexeError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !directory.is_dir() {
        return Err(TexeError::Manifest(format!(
            "initialization path is not a directory: {}",
            directory.display()
        )));
    }
    let mut candidates = Vec::new();
    collect_tex_files(directory, directory, &mut candidates)?;
    candidates.sort();

    let roots = candidates
        .iter()
        .filter(|entry| is_document_root(&directory.join(entry)))
        .cloned()
        .collect::<Vec<_>>();
    if roots.is_empty() {
        Ok(candidates)
    } else {
        Ok(roots)
    }
}

fn collect_tex_files(
    root: &Path,
    directory: &Path,
    candidates: &mut Vec<PathBuf>,
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
        if file_type.is_dir() {
            let name = entry.file_name();
            if !matches!(
                name.to_str(),
                Some(".git" | ".texe" | "node_modules" | "target")
            ) {
                collect_tex_files(root, &path, candidates)?;
            }
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
        {
            let relative = path.strip_prefix(root).map_err(|_| {
                TexeError::Manifest(format!(
                    "detected entry escaped the project directory: {}",
                    path.display()
                ))
            })?;
            candidates.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn is_document_root(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|source| source.contains("\\documentclass"))
}

fn preferred_entry(entries: &[PathBuf]) -> usize {
    entries
        .iter()
        .position(|entry| entry == Path::new(DEFAULT_ENTRY))
        .or_else(|| {
            entries.iter().position(|entry| {
                entry
                    .file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case(DEFAULT_ENTRY))
            })
        })
        .unwrap_or(0)
}

fn detect_engine_hint(path: &Path) -> Option<String> {
    let source = fs::read_to_string(path).ok()?;
    source.lines().take(50).find_map(|line| {
        let line = line.trim();
        if !line.starts_with('%') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.to_ascii_lowercase();
        if !key.contains("tex") || !key.contains("program") {
            return None;
        }
        normalize_engine(value)
    })
}

fn normalize_engine(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "pdflatex" | "pdftex" => Some("pdflatex".to_string()),
        "xelatex" | "xetex" => Some("xelatex".to_string()),
        "lualatex" | "luatex" | "luahbtex" => Some("lualatex".to_string()),
        _ => None,
    }
}

pub(super) fn validate_engine(engine: &str) -> Result<String, TexeError> {
    let engine = engine.trim();
    if engine.is_empty() {
        Err(TexeError::Manifest("engine cannot be empty".to_string()))
    } else {
        Ok(engine.to_string())
    }
}
