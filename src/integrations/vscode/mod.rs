use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::TexeError;
use crate::config::ProjectManifest;
use crate::integrations::IntegrationReport;
use crate::ux;

mod language_server;
mod settings;

// Rust only infers .exe on Windows; VS Code ships a code.cmd launcher.
fn code_command() -> Command {
    crate::process::command(if cfg!(windows) { "code.cmd" } else { "code" })
}

pub(crate) fn setup_vscode(
    root: &Path,
    open: bool,
    allow_settings_prompt: bool,
) -> Result<IntegrationReport, TexeError> {
    let mut report = IntegrationReport::default();
    let preview = settings::preview(root)?;
    let conflicts = preview["conflicts"].as_array().expect("conflicts");
    let replace_project_settings = !conflicts.is_empty()
        && allow_settings_prompt
        && ux::TerminalCapabilities::detect().can_prompt()
        && ux::prompt(
            cliclack::confirm(format!("Replace these conflicting settings? {conflicts:?}"))
                .initial_value(false)
                .interact(),
        )?;
    let settings_outcome = settings::configure(root, replace_project_settings)?;
    match settings_outcome {
        settings::ProjectSettingsOutcome::Created => report
            .messages
            .push("created .vscode/settings.json with texe's editor defaults".to_string()),
        settings::ProjectSettingsOutcome::Replaced => report.messages.push(
            "merged texe settings into .vscode/settings.json; recorded reversible changes"
                .to_string(),
        ),
        settings::ProjectSettingsOutcome::Preserved => report
            .messages
            .push("texe settings are already current".to_string()),
    }
    if settings_outcome != settings::ProjectSettingsOutcome::Preserved {
        report.messages.push(
            "VS Code will build with texe and show the project-root PDF in an editor tab"
                .to_string(),
        );
    }
    report.messages.push(
        "VS Code will open the project folder directly; texe does not create a separate workspace"
            .to_string(),
    );
    report.messages.push(
        "VS Code may open this new folder in Restricted Mode; choose Trust to enable Citet language tools and builds"
            .to_string(),
    );

    let code_available = match code_command()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            report.messages.push(format!(
                "the VS Code command returned {status}; setup finished, but VS Code was not opened"
            ));
            false
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            report.messages.push(
                "the VS Code command is unavailable; setup finished, but VS Code was not opened"
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
        ensure_citet(&mut report);
    }
    if open && code_available {
        report.messages.extend(open_vscode(root)?.messages);
    }
    Ok(report)
}

fn ensure_citet(report: &mut IntegrationReport) {
    match code_command()
         .args([
            "--install-extension", "backmatter.citet",
            "--install-extension", "backmatter.tex-ls", "--force",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            report.messages.push("installed the latest published Citet and tex-ls extensions from the VS Code Marketplace".into());
        }
        result => report.messages.push(format!(
            "could not install the latest Citet extension ({result:?}); install backmatter.citet from Extensions or use `texe build` in a terminal"
        )),
    }
}

pub(crate) fn open_vscode(root: &Path) -> Result<IntegrationReport, TexeError> {
    let manifest = settings::project_manifest(root)?;
    let targets = open_targets(root, &manifest);
    let source = &targets[0];
    let pdf = targets.get(1);
    let mut report = IntegrationReport::default();
    match code_command()
        .arg(root)
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
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("main.tex"));
    let pdf = root.join(stem).with_extension("pdf");
    let mut targets = vec![source];
    if pdf.is_file() {
        // Citet opens the project PDF beside the source after activation.
        targets.push(pdf);
    }
    targets
}

pub(crate) fn preview_vscode(root: &Path) -> Result<serde_json::Value, TexeError> {
    settings::preview(root)
}
pub(crate) fn configure_vscode(root: &Path, replace: bool) -> Result<IntegrationReport, TexeError> {
    settings::configure(root, replace)?;
    Ok(IntegrationReport {
        messages: vec!["updated texe's VS Code settings".into()],
    })
}

pub(crate) fn configure_vscode_owned(root: &Path) -> Result<IntegrationReport, TexeError> {
    settings::configure_owned(root)?;
    Ok(IntegrationReport {
        messages: vec!["updated texe-owned VS Code settings; preserved user overrides".into()],
    })
}

pub(crate) fn preview_vscode_manifest(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<serde_json::Value, TexeError> {
    settings::preview_manifest(root, manifest)
}

pub(crate) fn record_editor_error(root: &Path, error: Option<&TexeError>) -> Result<(), TexeError> {
    settings::record_editor_error(root, error)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::config::ProjectManifest;
    use crate::integrations::vscode::open_targets;

    #[test]
    fn launches_vscode_from_a_directory_with_spaces() {
        let scratch = tempfile::tempdir().expect("temporary directory");
        let bin = scratch.path().join("VS Code/bin");
        fs::create_dir_all(&bin).expect("launcher directory");
        let launcher = bin.join(if cfg!(windows) { "code.cmd" } else { "code" });
        fs::write(
            &launcher,
            if cfg!(windows) {
                "@echo off\r\necho native-launcher\r\n"
            } else {
                "#!/bin/sh\necho native-launcher\n"
            },
        )
        .expect("launcher");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755))
                .expect("executable launcher");
        }
        let output = super::code_command()
            .env("PATH", &bin)
            .arg("--version")
            .output()
            .expect("launch VS Code shim");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "native-launcher"
        );
    }

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
}
