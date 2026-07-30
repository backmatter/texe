use std::fmt;

use crate::app::build_command::run_build;
use crate::app::doctor::run_doctor;
use crate::app::maintenance::run_storage;
use crate::app::output::print_json;
use crate::app::project::{ProjectContext, load_project};
use crate::app::setup::guided::run_guided_setup;
use crate::app::watch_command::run_watch;
use crate::build::BuildOptions;
use crate::integrations;
use crate::{TexeError, ux};

pub(crate) fn run_bare(presentation: ux::Presentation) -> Result<(), TexeError> {
    let terminals = ux::TerminalCapabilities::detect();
    if presentation.json || !terminals.can_prompt() {
        if presentation.json {
            return print_json(&serde_json::json!({
                "schema": "texe.bare-report/v1",
                "status": "command-required",
                "next": "texe init --yes"
            }));
        }
        if !presentation.quiet {
            println!("texe needs a command when input is redirected.");
            println!("next: run `texe init --yes` to create a paper without prompts");
            println!("help: run `texe --help` to see every command");
        }
        return Ok(());
    }
    match load_project(None) {
        Ok(context) => run_project_menu(&context, presentation),
        Err(TexeError::Manifest(message)) if message.contains("could not find texe.toml") => {
            run_guided_setup(presentation)
        }
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectMenu {
    WatchAndView,
    Build,
    OpenVscode,
    Doctor,
    Storage,
    Exit,
}

impl fmt::Display for ProjectMenu {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WatchAndView => "Watch, build, and view the PDF",
            Self::Build => "Build once",
            Self::OpenVscode => "Open the paper in VS Code",
            Self::Doctor => "Check project health",
            Self::Storage => "Manage storage",
            Self::Exit => "Exit",
        })
    }
}

fn run_project_menu(
    context: &ProjectContext,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let stem = context
        .manifest
        .project
        .entry
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("main");
    let pdf = context.root.join(format!("{stem}.pdf"));
    ux::prompt(cliclack::intro(format!(
        "texe · {}",
        context.root.display()
    )))?;
    ux::prompt(cliclack::log::remark(format!(
        "PDF: {}",
        if pdf.is_file() {
            pdf.display().to_string()
        } else {
            "not built yet".to_string()
        }
    )))?;
    let options = if pdf.is_file() {
        [
            ProjectMenu::WatchAndView,
            ProjectMenu::Build,
            ProjectMenu::OpenVscode,
            ProjectMenu::Doctor,
            ProjectMenu::Storage,
            ProjectMenu::Exit,
        ]
    } else {
        [
            ProjectMenu::Build,
            ProjectMenu::WatchAndView,
            ProjectMenu::OpenVscode,
            ProjectMenu::Doctor,
            ProjectMenu::Storage,
            ProjectMenu::Exit,
        ]
    };
    let mut menu = cliclack::select("What would you like to do?");
    for (index, option) in options.into_iter().enumerate() {
        menu = menu.item(
            option,
            option.to_string(),
            if index == 0 { "Recommended" } else { "" },
        );
    }
    match ux::prompt(menu.interact())? {
        ProjectMenu::WatchAndView => run_watch(
            Some(&context.root),
            BuildOptions::default(),
            false,
            presentation,
            250,
            false,
            true,
        ),
        ProjectMenu::Build => run_build(
            Some(&context.root),
            BuildOptions::default(),
            false,
            presentation,
            false,
        ),
        ProjectMenu::OpenVscode => {
            for message in integrations::setup_vscode(&context.root, true, true)?.messages {
                println!("{message}");
            }
            Ok(())
        }
        ProjectMenu::Doctor => run_doctor(Some(&context.root), false, false, false, presentation),
        ProjectMenu::Storage => run_storage(Some(&context.root), presentation),
        ProjectMenu::Exit => Ok(()),
    }
}
