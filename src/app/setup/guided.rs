use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::app::build_command::run_build_in_flow;
use crate::app::setup::init::{InitCommand, InitIntegrations, quoted_path, run_init};
use crate::build::BuildOptions;
use crate::cli::TemplateChoice;
use crate::integrations;
use crate::{TexeError, ux};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidedTemplate {
    Basic,
    Empty,
}

impl fmt::Display for GuidedTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Basic => {
                "Basic scientific paper (Recommended) — sections, examples, and bibliography"
            }
            Self::Empty => "Empty document — only title, author, and a blank document",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupReview {
    CreateAndBuild,
    CreateOnly,
    Back,
    Cancel,
}

impl fmt::Display for SetupReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CreateAndBuild => "Create paper and build its first PDF (Recommended)",
            Self::CreateOnly => "Create files without downloading or building",
            Self::Back => "Go back and change answers",
            Self::Cancel => "Cancel without creating files",
        })
    }
}

#[derive(Debug, Clone)]
struct GuidedSetup {
    name: String,
    title: String,
    author: String,
    template: GuidedTemplate,
    git: bool,
    vscode: bool,
}

pub(super) fn run_guided_setup(presentation: ux::Presentation) -> Result<(), TexeError> {
    let current = std::env::current_dir().map_err(|source| TexeError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    ux::prompt(cliclack::intro(format!(
        "texe · Create a paper in {}",
        current.display()
    )))?;

    loop {
        let setup = prompt_guided_setup(&current)?;
        match prompt_setup_review(&current, &setup)? {
            SetupReview::Back => {
                ux::prompt(cliclack::log::remark("Let’s update those choices."))?;
            }
            SetupReview::Cancel => {
                ux::prompt(cliclack::outro_cancel(
                    "Setup cancelled. Nothing was changed.",
                ))?;
                return Ok(());
            }
            action @ (SetupReview::CreateAndBuild | SetupReview::CreateOnly) => {
                return create_guided_project(setup, action, presentation);
            }
        }
    }
}

fn prompt_guided_setup(current: &Path) -> Result<GuidedSetup, TexeError> {
    let validation_root = current.to_path_buf();
    let name: String = ux::prompt(
        cliclack::input("Project folder")
            .default_input("my-paper")
            .validate(move |answer: &String| validate_project_name(answer, &validation_root))
            .interact(),
    )?;
    let title: String = ux::prompt(
        cliclack::input("Paper title")
            .default_input(&title_from_project_name(&name))
            .interact(),
    )?;
    let author: String = ux::prompt(
        cliclack::input("Author (optional — leave blank to add later)")
            .required(false)
            .interact(),
    )?;
    let template = ux::prompt(
        cliclack::select("Starting document")
            .item(
                GuidedTemplate::Basic,
                "Basic scientific paper",
                "Recommended",
            )
            .item(
                GuidedTemplate::Empty,
                "Empty document",
                "minimal starting point",
            )
            .initial_value(GuidedTemplate::Basic)
            .interact(),
    )?;
    let git = ux::prompt(
        cliclack::confirm("Initialize Git version history? (optional)")
            .initial_value(false)
            .interact(),
    )?;
    let vscode_default = command_available("code");
    let vscode_prompt = if vscode_default {
        "Set up VS Code and install missing LaTeX extensions? (Recommended)"
    } else {
        "Create a VS Code workspace? (`code` command not found)"
    };
    let vscode = ux::prompt(
        cliclack::confirm(vscode_prompt)
            .initial_value(vscode_default)
            .interact(),
    )?;
    Ok(GuidedSetup {
        name,
        title,
        author,
        template,
        git,
        vscode,
    })
}

fn prompt_setup_review(current: &Path, setup: &GuidedSetup) -> Result<SetupReview, TexeError> {
    show_setup_review(current, setup)?;
    ux::prompt(
        cliclack::select("Ready?")
            .item(
                SetupReview::CreateAndBuild,
                "Create paper and first PDF",
                "Recommended",
            )
            .item(
                SetupReview::CreateOnly,
                "Create files only",
                "skip downloads and build",
            )
            .item(SetupReview::Back, "Change my answers", "")
            .item(SetupReview::Cancel, "Cancel", "nothing will be created")
            .initial_value(SetupReview::CreateAndBuild)
            .interact(),
    )
}

fn create_guided_project(
    setup: GuidedSetup,
    action: SetupReview,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let path = PathBuf::from(&setup.name);
    let create = cliclack::spinner().with_template("{msg} · {elapsed_precise}");
    create.start("Creating project files");
    let result = run_init(InitCommand {
        path: path.clone(),
        entry: Some(PathBuf::from("main.tex")),
        engine: Some("pdflatex".to_string()),
        title: Some(setup.title),
        author: Some(setup.author),
        template: Some(match setup.template {
            GuidedTemplate::Basic => TemplateChoice::Basic,
            GuidedTemplate::Empty => TemplateChoice::Empty,
        }),
        yes: true,
        integrations: InitIntegrations {
            git: setup.git,
            vscode: setup.vscode,
            open_vscode: false,
        },
        presentation: ux::Presentation {
            quiet: true,
            ..presentation
        },
    });
    let integration_messages = match result {
        Ok(messages) => messages,
        Err(error) => {
            create.clear();
            ux::prompt(cliclack::log::error("Could not create the paper"))?;
            return Err(error);
        }
    };
    create.clear();
    ux::prompt(cliclack::log::step("Created project files"))?;
    for message in integration_messages.iter().filter(|message| {
        [
            "could not",
            "unavailable",
            "not installed",
            "returned",
            "skipped",
        ]
        .iter()
        .any(|needle| message.to_ascii_lowercase().contains(needle))
    }) {
        ux::prompt(cliclack::log::warning(message))?;
    }
    if matches!(action, SetupReview::CreateAndBuild) {
        run_build_in_flow(
            Some(&path),
            BuildOptions::default(),
            false,
            presentation,
            true,
            true,
        )?;
    }
    if setup.vscode {
        for message in integrations::open_vscode(&path)?.messages {
            if message.contains("unavailable") {
                ux::prompt(cliclack::log::warning(message))?;
            } else {
                ux::prompt(cliclack::log::step(message))?;
            }
        }
    }
    finish_guided_setup(&path, action, setup.vscode)
}

fn finish_guided_setup(path: &Path, action: SetupReview, vscode: bool) -> Result<(), TexeError> {
    let source = path.join("main.tex");
    let built = matches!(action, SetupReview::CreateAndBuild);
    let message = if built && vscode {
        let pdf = path.join("main.pdf");
        format!(
            "Source  {}\nPDF     {}\n\nVS Code is opening the source left and PDF right.\nChoose Trust if prompted; saving main.tex rebuilds\nand refreshes the PDF.",
            source.display(),
            pdf.display()
        )
    } else if built {
        let pdf = path.join("main.pdf");
        format!(
            "Source  {}\nPDF     {}\n\nNext: texe watch --view --project {}",
            source.display(),
            pdf.display(),
            quoted_path(path)
        )
    } else {
        format!(
            "Source  {}\n\nNext: texe build --project {}",
            source.display(),
            quoted_path(path)
        )
    };
    ux::prompt(cliclack::outro_note(
        if built {
            "Paper ready"
        } else {
            "Paper created"
        },
        message,
    ))
}

fn validate_project_name(name: &str, current: &Path) -> Result<(), String> {
    let path = Path::new(name);
    if name.trim().is_empty() {
        return Err("enter a project folder name".to_string());
    }
    if path.is_absolute()
        || path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(
            "use one folder name without `/`, `\\`, `.` or `..`; texe creates it here".to_string(),
        );
    }
    let destination = current.join(path);
    if destination.exists() {
        return Err(format!(
            "{} already exists; choose a new name, or run `texe init` inside that folder",
            destination.display()
        ));
    }
    Ok(())
}

fn title_from_project_name(name: &str) -> String {
    let words = name
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>();
    if words.is_empty() {
        "Untitled Paper".to_string()
    } else {
        words.join(" ")
    }
}

fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn show_setup_review(current: &Path, setup: &GuidedSetup) -> Result<(), TexeError> {
    let template = match setup.template {
        GuidedTemplate::Basic => "Basic scientific paper",
        GuidedTemplate::Empty => "Empty document",
    };
    let author = if setup.author.trim().is_empty() {
        "add later"
    } else {
        &setup.author
    };
    let mut extras = Vec::new();
    if setup.git {
        extras.push("Git");
    }
    if setup.vscode {
        extras.push("VS Code + missing LaTeX extensions");
    }
    let extras = if extras.is_empty() {
        "None".to_string()
    } else {
        extras.join(" · ")
    };
    ux::prompt(cliclack::note(
        "Review",
        format!(
            "Folder    {}\nTitle     {}\nAuthor    {}\nDocument  {} · {}\nExtras    {}\n\nFirst PDF  Downloads required, checksummed components\nto texe’s local cache. Paper source and metadata stay local.\nNothing has been changed yet.",
            current.join(&setup.name).display(),
            setup.title,
            author,
            template,
            "pdfLaTeX",
            extras,
        ),
    ))
}
