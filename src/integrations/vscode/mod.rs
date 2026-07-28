use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::TexeError;
use crate::config::ProjectManifest;
use crate::integrations::IntegrationReport;

mod bridge;
mod settings;

pub(crate) fn setup_vscode(root: &Path, open: bool) -> Result<IntegrationReport, TexeError> {
    let mut report = IntegrationReport::default();
    settings::configure(root)?;
    report.messages.push(
        "generated a project-local VS Code workspace that builds with texe and shows the PDF in an editor tab"
            .to_string(),
    );
    report.messages.push(
        "VS Code may open this new folder in Restricted Mode; choose Trust to enable build-on-save and LaTeX Workshop"
            .to_string(),
    );

    let code_available = match Command::new("code")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            report.messages.push(format!(
                "the VS Code command returned {status}; the workspace was generated, but VS Code was not opened"
            ));
            false
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            report.messages.push(
                "the VS Code command is unavailable; the workspace was generated, but VS Code was not opened"
                    .to_string(),
            );
            false
        }
        Err(source) => {
            return Err(TexeError::Spawn {
                tool: PathBuf::from("code"),
                source,
            });
        }
    };
    if code_available {
        ensure_latex_workshop(&mut report);
        ensure_layout_companion(&mut report);
    }
    if open {
        report.messages.extend(open_vscode(root)?.messages);
    }
    Ok(report)
}

fn ensure_latex_workshop(report: &mut IntegrationReport) {
    if installed_extension_version("James-Yu.latex-workshop").is_some() {
        report
            .messages
            .push("kept the installed LaTeX Workshop extension unchanged".to_string());
        return;
    }
    match Command::new("code")
        .args(["--install-extension", "James-Yu.latex-workshop"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => report
            .messages
            .push("installed the LaTeX Workshop extension".to_string()),
        Ok(status) => report.messages.push(format!(
            "VS Code extension installation returned {status}; install LaTeX Workshop from Extensions"
        )),
        Err(source) => report.messages.push(format!(
            "could not install LaTeX Workshop ({source}); install it from Extensions"
        )),
    }
}

fn ensure_layout_companion(report: &mut IntegrationReport) {
    let layout_version = installed_extension_version("backmatter.texe-paper-layout");
    if layout_version.as_deref() == Some(env!("CARGO_PKG_VERSION")) {
        report
            .messages
            .push("texe's side-by-side paper layout is available in VS Code".to_string());
        return;
    }
    let action = if layout_version.is_some() {
        "updated"
    } else {
        "installed"
    };
    let extension = match bridge::path() {
        Ok(extension) => extension,
        Err(error) => {
            report.messages.push(format!(
                "could not prepare texe's VS Code paper layout ({error}); the source and PDF will still open as tabs"
            ));
            return;
        }
    };
    match Command::new("code")
        .arg("--install-extension")
        .arg(&extension)
        // This is texe's own versioned companion, not the third-party LaTeX
        // Workshop extension.
        .arg("--force")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => report.messages.push(format!(
            "{action} texe's side-by-side paper layout for VS Code"
        )),
        Ok(status) => report.messages.push(format!(
            "could not install texe's VS Code paper layout ({status}); the source and PDF will still open as tabs"
        )),
        Err(source) => report.messages.push(format!(
            "could not install texe's VS Code paper layout ({source}); the source and PDF will still open as tabs"
        )),
    }
}

fn installed_extension_version(identifier: &str) -> Option<String> {
    Command::new("code")
        .args(["--list-extensions", "--show-versions"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| extension_version_from_list(&output.stdout, identifier))
}

fn extension_version_from_list(output: &[u8], identifier: &str) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .find_map(|installed| {
            let installed = installed.trim();
            let (name, version) = installed.rsplit_once('@').unwrap_or((installed, ""));
            name.eq_ignore_ascii_case(identifier)
                .then(|| version.to_string())
        })
}

pub(crate) fn open_vscode(root: &Path) -> Result<IntegrationReport, TexeError> {
    let manifest = ProjectManifest::load(&root.join("texe.toml"))?;
    let targets = open_targets(root, &manifest);
    let source = &targets[0];
    let pdf = targets.get(1);
    let workspace = settings::workspace_path(root);
    let project = if workspace.is_file() {
        workspace.as_path()
    } else {
        root
    };
    let mut report = IntegrationReport::default();
    match Command::new("code")
        .arg(project)
        .args(&targets)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => {
            let message = pdf.map_or_else(
                || format!("opened {} in VS Code", source.display()),
                |pdf| {
                    format!(
                        "opened {} and {} in VS Code",
                        source.display(),
                        pdf.display()
                    )
                },
            );
            report.messages.push(message);
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => report
            .messages
            .push("the VS Code command is unavailable; open the paper folder manually".to_string()),
        Err(source) => {
            return Err(TexeError::Spawn {
                tool: PathBuf::from("code"),
                source,
            });
        }
    }
    Ok(report)
}

pub(crate) fn remove_vscode(root: &Path) -> Result<IntegrationReport, TexeError> {
    settings::remove(root)
}

fn open_targets(root: &Path, manifest: &ProjectManifest) -> Vec<PathBuf> {
    let source = root.join(&manifest.project.entry);
    let stem = manifest
        .project
        .entry
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("main"));
    let pdf = root.join(stem).with_extension("pdf");
    let mut targets = vec![source];
    if pdf.is_file() {
        // LaTeX Workshop registers its internal viewer as VS Code's default
        // custom editor for PDF files. Listing the PDF last makes it the
        // visible tab while leaving the source ready to edit.
        targets.push(pdf);
    }
    targets
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::config::ProjectManifest;
    use crate::integrations::vscode::{extension_version_from_list, open_targets};

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
    fn opens_an_existing_pdf_after_the_source() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(directory.path().join("main.pdf"), b"%PDF").expect("pdf");

        let targets = open_targets(directory.path(), &manifest());

        assert_eq!(
            targets,
            [
                directory.path().join("main.tex"),
                directory.path().join("main.pdf")
            ]
        );
    }

    #[test]
    fn only_opens_the_source_before_the_first_build() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let targets = open_targets(directory.path(), &manifest());

        assert_eq!(targets, [directory.path().join("main.tex")]);
    }

    #[test]
    fn existing_extensions_are_detected_without_changing_their_version() {
        let output = b"publisher.other@1.2.3\njames-yu.latex-workshop@10.9.0\n";
        assert_eq!(
            extension_version_from_list(output, "James-Yu.latex-workshop").as_deref(),
            Some("10.9.0")
        );
    }
}
