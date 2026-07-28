use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};

use crate::{Result, message};

pub(crate) struct ScratchDir {
    directory: Option<tempfile::TempDir>,
    keep: bool,
}

impl ScratchDir {
    pub(crate) fn new(label: &str) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix(&format!("texe-{label}-"))
            .tempdir()?;
        Ok(Self {
            directory: Some(directory),
            keep: std::env::var_os("KEEP_WORK").is_some_and(|value| value == "1"),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.as_ref().expect("scratch directory").path()
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let Some(directory) = self.directory.take() else {
            return;
        };
        if self.keep {
            let path = directory.keep();
            eprintln!("xtask: kept diagnostic workspace at {}", path.display());
        } else {
            let path = directory.path().to_path_buf();
            if let Err(error) = directory.close() {
                eprintln!(
                    "xtask: could not remove scratch directory {}: {error}",
                    path.display()
                );
            }
        }
    }
}

pub(crate) fn repo_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| message("xtask manifest has no repository parent"))
}

pub(crate) fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")))
}

pub(crate) fn run(command: &mut Command) -> Result<()> {
    let rendered = format!("{command:?}");
    let status = command.status()?;
    require_success(status, &rendered)
}

pub(crate) fn output(command: &mut Command) -> Result<Output> {
    let rendered = format!("{command:?}");
    let output = command.output()?;
    require_success(output.status, &rendered)?;
    Ok(output)
}

pub(crate) fn capture(command: &mut Command) -> Result<String> {
    String::from_utf8(output(command)?.stdout).map_err(Into::into)
}

pub(crate) fn read_text(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?)
}

pub(crate) fn append(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    let mut file = fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(bytes.as_ref())?;
    Ok(())
}

pub(crate) fn nonempty(path: &Path) -> Result<()> {
    if !path.is_file() || fs::metadata(path)?.len() == 0 {
        return Err(message(format!(
            "expected a nonempty file: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn require_contains(path: &Path, needle: &str) -> Result<()> {
    if !read_text(path)?.contains(needle) {
        return Err(message(format!(
            "{} does not contain {needle:?}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn require_absent(path: &Path, needle: &str) -> Result<()> {
    if read_text(path)?.contains(needle) {
        return Err(message(format!(
            "{} unexpectedly contains {needle:?}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn require(condition: bool, detail: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message(detail))
    }
}

pub(crate) fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    Ok(())
}

pub(crate) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

pub(crate) fn directory_bytes(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    use sha2::{Digest as _, Sha256};
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn executable_name(name: &str) -> OsString {
    let mut binary = OsString::from(name);
    binary.push(std::env::consts::EXE_SUFFIX);
    binary
}

pub(crate) fn on_path(tool: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    executable_extensions().iter().any(|extension| {
        std::env::split_paths(&path)
            .any(|directory| directory.join(format!("{tool}{extension}")).is_file())
    })
}

pub(crate) fn require_tools(tools: &[&str]) -> Result<()> {
    for tool in tools {
        require(on_path(tool), format!("required tool is missing: {tool}"))?;
    }
    Ok(())
}

pub(crate) fn clean_environment(command: &mut Command, root: &Path, bin: &Path) {
    let inherited = ["SystemRoot", "WINDIR", "COMSPEC"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect::<Vec<_>>();
    command.env_clear();
    command.envs(inherited);
    command
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("LOCALAPPDATA", root.join("data/local"))
        .env("APPDATA", root.join("data/roaming"))
        .env("TEXE_HOME", root.join("data/texe"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("TMPDIR", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .env("TMP", root.join("tmp"))
        .env("PATH", bin);
}

fn require_success(status: ExitStatus, rendered: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(message(format!("{rendered} exited with {status}")))
    }
}

fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        std::env::var_os("PATHEXT").map_or_else(
            || vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()],
            |value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .map(str::to_ascii_lowercase)
                    .collect()
            },
        )
    } else {
        vec![String::new()]
    }
}
