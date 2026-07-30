use std::path::{Path, PathBuf};

use crate::app::output::print_json;
use crate::cli::TemplateChoice;
use crate::config::{
    InitRequest, ProjectManifest, StarterDocument, StarterTemplate, configure_init,
    init_project_with_starter,
};
use crate::integrations;
use crate::{TexeError, ux};

pub(crate) struct InitCommand {
    pub(crate) path: PathBuf,
    pub(crate) entry: Option<PathBuf>,
    pub(crate) engine: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) template: Option<TemplateChoice>,
    pub(crate) yes: bool,
    pub(crate) integrations: InitIntegrations,
    pub(crate) presentation: ux::Presentation,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InitIntegrations {
    pub(crate) git: bool,
    pub(crate) vscode: bool,
    pub(crate) open_vscode: bool,
}

pub(crate) fn run_init(command: InitCommand) -> Result<Vec<String>, TexeError> {
    let InitCommand {
        path,
        entry,
        engine,
        title,
        author,
        template,
        yes,
        integrations: requested_integrations,
        presentation,
    } = command;
    let path = path.as_path();
    let terminals = ux::TerminalCapabilities::detect();
    let request = InitRequest {
        entry,
        engine,
        interactive: !yes && !presentation.json && terminals.can_prompt(),
        accept_defaults: yes,
    };
    let settings = configure_init(path, &request)?;
    let starter = StarterDocument {
        title: title.unwrap_or_else(|| default_paper_title(path)),
        author: author.unwrap_or_default(),
        template: template.map_or_else(StarterTemplate::default, Into::into),
    };
    let outcome = init_project_with_starter(path, &settings.entry, &settings.engine, &starter)?;
    let manifest = ProjectManifest::load(&outcome.manifest)?;
    let integration_messages = configure_integrations(
        path,
        &manifest,
        requested_integrations,
        !yes && !presentation.json && !presentation.quiet,
    );
    if presentation.json {
        print_json(&serde_json::json!({
            "schema": "texe.init-report/v1",
            "manifest": outcome.manifest,
            "created_files": outcome.created_files,
            "engine": settings.engine,
            "git": requested_integrations.git,
            "vscode": requested_integrations.vscode,
            "integration_messages": &integration_messages,
        }))?;
        return Ok(integration_messages);
    }
    if presentation.quiet {
        return Ok(integration_messages);
    }
    println!("initialized {}", outcome.manifest.display());
    for created in &outcome.created_files {
        println!("created {}", path.join(created).display());
    }
    println!("engine: {}", settings.engine);
    for message in &integration_messages {
        println!("{message}");
    }
    println!();
    if path == Path::new(".") {
        println!("next: run `texe build`");
    } else {
        println!("next: run `texe build --project {}`", quoted_path(path));
    }
    Ok(integration_messages)
}

fn configure_integrations(
    path: &Path,
    manifest: &ProjectManifest,
    requested: InitIntegrations,
    allow_prompts: bool,
) -> Vec<String> {
    let mut messages = Vec::new();
    if requested.git {
        match integrations::setup_git(path, manifest) {
            Ok(report) => messages.extend(report.messages),
            Err(error) => messages.push(format!(
                "Git setup could not be completed ({error}); the paper is still ready"
            )),
        }
    }
    if requested.vscode {
        match integrations::setup_vscode(path, requested.open_vscode, allow_prompts) {
            Ok(report) => messages.extend(report.messages),
            Err(error) => messages.push(format!(
                "VS Code setup could not be completed ({error}); the paper is still ready and can be opened manually"
            )),
        }
    }
    messages
}

fn default_paper_title(path: &Path) -> String {
    let name = if path == Path::new(".") {
        std::env::current_dir()
            .ok()
            .and_then(|directory| directory.file_name().map(ToOwned::to_owned))
    } else {
        path.file_name().map(ToOwned::to_owned)
    };
    name.and_then(|name| name.to_str().map(str::to_string))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Untitled Paper".to_string())
}

pub(crate) fn quoted_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}
