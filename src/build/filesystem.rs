use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::build::process::search_path_from;

/// TeX writes `\include{chapters/one}` auxiliaries beside the relative include
/// path under `-output-directory`. Engines do not create those parent
/// directories, so mirror the project's source directory shape into each
/// private build directory before invoking one.
pub(super) fn mirror_project_directories(
    project_root: &Path,
    output_dir: &Path,
    excluded: &[&Path],
) -> Result<(), TexeError> {
    let mut pending = vec![project_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| TexeError::Io {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| TexeError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            if excluded
                .iter()
                .any(|excluded| path == *excluded || path.starts_with(excluded))
            {
                continue;
            }
            let file_type = entry.file_type().map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            if !file_type.is_dir() {
                continue;
            }
            let relative = path
                .strip_prefix(project_root)
                .expect("directory came from the project root");
            let mirrored = output_dir.join(relative);
            fs::create_dir_all(&mirrored).map_err(|source| TexeError::Io {
                path: mirrored,
                source,
            })?;
            pending.push(path);
        }
    }
    Ok(())
}

pub(super) fn auxiliary_snapshot(
    directory: &Path,
) -> Result<BTreeMap<PathBuf, Vec<u8>>, TexeError> {
    let mut snapshot = BTreeMap::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current).map_err(|source| TexeError::Io {
            path: current.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| TexeError::Io {
                path: current.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() || !is_auxiliary(&path) {
                continue;
            }
            let bytes = fs::read(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            let relative = path
                .strip_prefix(directory)
                .expect("auxiliary came from the output directory")
                .to_path_buf();
            snapshot.insert(relative, bytes);
        }
    }
    Ok(snapshot)
}

fn is_auxiliary(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some(
            "aux"
                | "toc"
                | "out"
                | "lof"
                | "lot"
                | "bbl"
                | "bcf"
                | "idx"
                | "ind"
                | "ilg"
                | "glo"
                | "glg"
                | "gls"
                | "acn"
                | "alg"
                | "acr"
                | "nav"
                | "snm"
        )
    ) || path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.ends_with(".run.xml"))
}

pub(super) fn find_artifact(output_dir: &Path, entry: &Path) -> Option<PathBuf> {
    let stem = entry.file_stem()?;
    ["pdf", "dvi"]
        .into_iter()
        .map(|extension| output_dir.join(stem).with_extension(extension))
        .find(|path| path.is_file())
}

pub(super) fn job_stem(entry: &Path) -> Result<String, TexeError> {
    entry
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|stem| !stem.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            TexeError::Manifest(format!(
                "project entry must have a UTF-8 file stem: {}",
                entry.display()
            ))
        })
}

pub(super) fn package_search_path_value(
    working_directory: &Path,
    output_dir: &Path,
    texmf: &Path,
    input_roots: &[PathBuf],
    discovery: bool,
) -> OsString {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let generated_root = output_dir.join(".texe-generated");
    let mut roots = vec![generated_root.as_path(), output_dir, Path::new(".")];
    roots.extend(input_roots.iter().map(PathBuf::as_path));
    roots.push(texmf);
    let mut value = search_path_from(&roots, working_directory)
        .to_string_lossy()
        .into_owned();
    if discovery {
        value.push(separator);
    }
    OsString::from(value)
}
