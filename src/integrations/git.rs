use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::TexeError;
use crate::atomic;
use crate::config::ProjectManifest;
use crate::integrations::IntegrationReport;

const GITIGNORE_START: &str = "# >>> texe generated files";
const GITIGNORE_END: &str = "# <<< texe generated files";

pub(crate) fn setup_git(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<IntegrationReport, TexeError> {
    let mut report = IntegrationReport::default();
    let repository = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match repository {
        Ok(status) if status.success() => {
            report
                .messages
                .push("using the Git repository that already contains this paper".to_string());
        }
        Ok(_) => {
            let status = Command::new("git")
                .arg("init")
                .arg("--quiet")
                .current_dir(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .status()
                .map_err(|source| TexeError::Spawn {
                    tool: PathBuf::from("git"),
                    source,
                });
            match status {
                Ok(status) if status.success() => report
                    .messages
                    .push("initialized Git without staging files or creating a commit".to_string()),
                Ok(status) => {
                    report.messages.push(format!(
                        "Git could not be initialized (status {status}); the paper is still ready"
                    ));
                    return Ok(report);
                }
                Err(TexeError::Spawn { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    report.messages.push(
                        "Git is not installed; skipped version-control setup without changing the paper"
                            .to_string(),
                    );
                    return Ok(report);
                }
                Err(error) => return Err(error),
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            report.messages.push(
                "Git is not installed; skipped version-control setup without changing the paper"
                    .to_string(),
            );
            return Ok(report);
        }
        Err(source) => {
            return Err(TexeError::Spawn {
                tool: PathBuf::from("git"),
                source,
            });
        }
    }

    merge_gitignore(root, manifest)?;
    report
        .messages
        .push("added only texe build outputs to .gitignore".to_string());
    Ok(report)
}

fn merge_gitignore(root: &Path, manifest: &ProjectManifest) -> Result<(), TexeError> {
    let path = root.join(".gitignore");
    let original = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(TexeError::Io {
                path: path.clone(),
                source,
            });
        }
    };
    let without_owned = remove_owned_block(&original, GITIGNORE_START, GITIGNORE_END);
    let stem = manifest
        .project
        .entry
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("main");
    let mut merged = without_owned.trim_end_matches('\n').to_string();
    if !merged.is_empty() {
        merged.push_str("\n\n");
    }
    merged.push_str(GITIGNORE_START);
    merged.push('\n');
    write!(
        merged,
        "/{}\n/{}\n/{}\n/{}.pdf\n/{}.synctex.gz\n",
        slash_path(&manifest.project.build_dir),
        slash_path(&manifest.packages.texmf),
        slash_path(&manifest.packages.lock),
        stem,
        stem
    )
    .expect("writing to a String cannot fail");
    merged.push_str(GITIGNORE_END);
    merged.push('\n');
    atomic::write(&path, merged.as_bytes())
}

fn remove_owned_block(contents: &str, start: &str, end: &str) -> String {
    let Some(block_start) = contents.find(start) else {
        return contents.to_string();
    };
    let Some(relative_end) = contents[block_start..].find(end) else {
        return contents.to_string();
    };
    let block_end = block_start + relative_end + end.len();
    let after_newline = contents[block_end..]
        .strip_prefix("\r\n")
        .or_else(|| contents[block_end..].strip_prefix('\n'))
        .map_or(block_end, |suffix| contents.len() - suffix.len());
    format!("{}{}", &contents[..block_start], &contents[after_newline..])
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::config::ProjectManifest;
    use crate::integrations::git::merge_gitignore;

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
        .expect("manifest")
    }

    #[test]
    fn gitignore_keeps_user_content_and_only_names_derived_outputs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(directory.path().join(".gitignore"), "*.bak\n").expect("gitignore");
        merge_gitignore(directory.path(), &manifest()).expect("merge");
        let contents =
            fs::read_to_string(directory.path().join(".gitignore")).expect("gitignore contents");
        assert!(contents.starts_with("*.bak\n"));
        assert!(contents.contains("/.texe/build"));
        assert!(contents.contains("/.texe/state/pqty.lock"));
        assert!(contents.contains("/main.pdf"));
        assert!(!contents.contains("texe.lock"));
        assert!(!contents.contains("*.bib"));
    }
}
