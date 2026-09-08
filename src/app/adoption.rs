use std::fs;
use std::path::{Path, PathBuf};

use crate::app::{build_command, output::print_json};
use crate::cli::Command;
use crate::config::{InitRequest, ProjectManifest, configure_init, init_project};
use crate::{TexeError, integrations, ux};

pub(super) fn run(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Adopt {
        path,
        entry,
        engine,
        check,
        yes,
        no_build,
        no_editor,
        replace_conflicts,
    } = command
    else {
        unreachable!()
    };
    let root = fs::canonicalize(&path).map_err(|source| TexeError::Io { path, source })?;
    let existing = root.join("texe.toml").exists();
    if existing && (entry.is_some() || engine.is_some()) {
        return Err(TexeError::Manifest(
            "this project already has texe.toml; edit its entry or engine there".into(),
        ));
    }
    let manifest = adoption_manifest(&root, existing, entry, engine, yes, check, presentation)?;
    let (blockers, warnings, local_classes, external_tools, conflicts) =
        preflight(&root, &manifest, no_editor)?;
    let report = serde_json::json!({ "schema": "texe.adoption-report/v1", "project": root,
        "entry": manifest.project.entry, "engine": manifest.toolchain.engine, "provider": manifest.toolchain.provider,
        "existing_manifest": existing, "blockers": blockers, "warnings": warnings,
        "local_classes": local_classes, "external_tools": external_tools, "settings_conflicts": conflicts, "check": check });
    if check {
        if presentation.json {
            return print_json(&report);
        }
        if !presentation.quiet {
            print_preflight(&report);
        }
        return Ok(());
    }
    if !presentation.quiet {
        eprintln!(
            "Existing paper: {} · {} ({})",
            manifest.project.entry.display(),
            manifest.toolchain.engine,
            manifest.toolchain.provider
        );
        for warning in &warnings {
            eprintln!("{warning}");
        }
    }
    if !blockers.is_empty() {
        return Err(TexeError::Build(blockers.join("\n")));
    }
    let replace = review_conflicts(
        &root,
        &manifest,
        &conflicts,
        replace_conflicts,
        yes,
        presentation,
    )?;
    if !yes && !presentation.json && ux::TerminalCapabilities::detect().can_prompt() && !ux::prompt(cliclack::confirm("Configure this paper for texe? Builds automatically install missing TeX tools and packages.").initial_value(true).interact())? { return Ok(()); }
    if !existing {
        init_project(&root, &manifest.project.entry, &manifest.toolchain.engine)?;
    }
    if !no_editor {
        integrations::configure_vscode(&root, replace)?;
    }
    let build_result = if no_build {
        Ok(())
    } else {
        build_command::run_build(
            Some(&root),
            crate::build::BuildOptions::default(),
            false,
            ux::Presentation {
                json: false,
                quiet: presentation.quiet || presentation.json,
                verbose: presentation.verbose,
            },
            true,
        )
    };
    if !no_editor {
        integrations::record_editor_error(&root, build_result.as_ref().err())?;
        for message in integrations::setup_vscode(&root, true, false)?.messages {
            if !presentation.quiet {
                eprintln!("{message}");
            }
        }
    }
    if build_result.is_err() && !presentation.quiet && !presentation.json {
        eprintln!(
            "Project setup is complete. Fix the build error in your source, then save or use texe: Build and View to retry. Existing sources and the previous PDF were preserved."
        );
    }
    build_result?;
    if presentation.json {
        print_json(&report)?;
    }
    Ok(())
}

fn review_conflicts(
    root: &Path,
    manifest: &ProjectManifest,
    conflicts: &serde_json::Value,
    replace_conflicts: bool,
    yes: bool,
    presentation: ux::Presentation,
) -> Result<bool, TexeError> {
    let has_conflicts = conflicts.as_array().is_some_and(|items| !items.is_empty());
    let mut replace = replace_conflicts;
    if has_conflicts && !replace {
        if !yes && !presentation.json && ux::TerminalCapabilities::detect().can_prompt() {
            let preview = integrations::preview_vscode_manifest(root, manifest)?;
            eprintln!(
                "Proposed editor settings:\n{}",
                preview["settings"].as_str().unwrap_or_default()
            );
            replace = ux::prompt(
                cliclack::confirm(format!(
                    "Apply these editor changes? Conflicting settings: {conflicts}"
                ))
                .initial_value(false)
                .interact(),
            )?;
        }
        if !replace {
            return Err(TexeError::Build("editor settings need review; run texe adopt interactively, or use --replace-conflicts to accept the preflight changes; --no-editor keeps your editor setup".into()));
        }
    }
    Ok(replace)
}

fn print_preflight(report: &serde_json::Value) {
    let blockers = report["blockers"].as_array().expect("blockers");
    let conflicts = report["settings_conflicts"].as_array().expect("conflicts");
    println!(
        "{}",
        if blockers.is_empty() && conflicts.is_empty() {
            "Ready for the first build"
        } else {
            "Needs attention before adoption"
        }
    );
    println!(
        "Source: {} · Engine: {} ({})",
        report["entry"].as_str().unwrap_or_default(),
        report["engine"].as_str().unwrap_or_default(),
        report["provider"].as_str().unwrap_or_default()
    );
    for blocker in blockers {
        println!("Needs attention: {}", blocker.as_str().unwrap_or_default());
    }
    for warning in report["warnings"].as_array().expect("warnings") {
        println!("Note: {}", warning.as_str().unwrap_or_default());
    }
    if !conflicts.is_empty() {
        println!(
            "Editor settings to review: {}",
            report["settings_conflicts"]
        );
        println!(
            "Next: run texe adopt interactively to review the changes, or use --replace-conflicts to accept them."
        );
    } else if blockers.is_empty() {
        println!("Next: run texe adopt in the paper folder to configure it and build.");
    }
}

fn adoption_manifest(
    root: &Path,
    existing: bool,
    entry: Option<PathBuf>,
    engine: Option<String>,
    yes: bool,
    check: bool,
    presentation: ux::Presentation,
) -> Result<ProjectManifest, TexeError> {
    let manifest = if existing {
        ProjectManifest::load(&root.join("texe.toml"))?
    } else {
        let settings = configure_init(
            root,
            &InitRequest {
                entry,
                engine,
                interactive: !yes
                    && !check
                    && !presentation.json
                    && ux::TerminalCapabilities::detect().can_prompt(),
                accept_defaults: false,
            },
        )?;
        let provider = if matches!(settings.engine.as_str(), "pdflatex" | "lualatex") {
            "managed"
        } else {
            "system"
        };
        let text = format!(
            "schema = \"texe.project/v1\"\n[project]\nentry = {}\n[toolchain]\nengine = {}\nprovider = \"{provider}\"\n",
            toml::Value::String(settings.entry.to_string_lossy().into()),
            toml::Value::String(settings.engine)
        );
        toml::from_str::<ProjectManifest>(&text)
            .map_err(|error| TexeError::Manifest(error.to_string()))?
    };
    Ok(manifest)
}

type Preflight = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<&'static str>,
    serde_json::Value,
);

fn preflight(
    root: &Path,
    manifest: &ProjectManifest,
    no_editor: bool,
) -> Result<Preflight, TexeError> {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let source_path = root.join(&manifest.project.entry);
    if !fs::canonicalize(&source_path).is_ok_and(|path| path.starts_with(root) && path.is_file()) {
        blockers.push("entry must be an existing source file inside the paper folder".to_string());
    }
    let commands = crate::config::source::project_commands(root, &manifest.project.entry);
    let has = |name: &str| commands.iter().any(|(command, _)| command == name);
    let minted = crate::config::source::has_package(&commands, "minted")
        || commands
            .iter()
            .any(|(name, argument)| name == "begin" && argument == "minted");
    let unicode = ["fontspec", "unicode-math"]
        .iter()
        .any(|package| crate::config::source::has_package(&commands, package));
    if unicode && manifest.toolchain.engine == "pdflatex" {
        blockers.push("fontspec/unicode-math requires LuaLaTeX or XeLaTeX; use --engine lualatex for a managed installation, or change the engine in an existing texe.toml".into());
    }
    if manifest.toolchain.provider == "system" {
        for tool in [&manifest.toolchain.engine, "kpsewhich"] {
            if !on_path(tool) {
                blockers.push(format!(
                    "system tool {tool} is missing from PATH; install your TeX distribution first"
                ));
            }
        }
    }
    let mut external_tools = Vec::new();
    for (marker, tool) in [
        ("minted", "latexminted / Python"),
        ("write18", "shell escape"),
        ("makeglossaries", "makeindex (glossary)"),
        ("makeindex", "makeindex"),
        ("addbibresource", "biber"),
    ] {
        if (marker == "minted" && minted) || (marker != "minted" && has(marker)) {
            external_tools.push(tool);
        }
    }
    if (minted || has("write18")) && !manifest.toolchain.shell_escape {
        blockers.push("the entry requests external processing; configure and verify that workflow in texe.toml before adoption (shell escape is disabled by default)".into());
    }
    if has("setmainfont") || has("setsansfont") {
        warnings.push(
            "system fonts are referenced; verify those fonts are installed on every machine".into(),
        );
    }
    for file in [".latexmkrc", "latexmkrc", "Makefile"] {
        if root.join(file).is_file() {
            warnings.push(format!("{file} exists; texe does not execute it. Review its engine flags and custom build steps."));
        }
    }
    let mut local_classes = fs::read_dir(root)
        .map_err(|source| TexeError::Io {
            path: root.to_owned(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "cls" || ext == "sty")
        })
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    local_classes.sort();
    warnings.push("static preflight follows literal local includes and class/package declarations within a bounded scan; dynamic TeX, fonts and package availability still need the first build".into());
    let preview = if no_editor {
        serde_json::json!({"conflicts": []})
    } else {
        integrations::preview_vscode_manifest(root, manifest)?
    };
    let conflicts = preview["conflicts"].clone();
    Ok((blockers, warnings, local_classes, external_tools, conflicts))
}

fn on_path(tool: &str) -> bool {
    if Path::new(tool).is_absolute() {
        return Path::new(tool).is_file();
    }
    std::env::var_os("PATH").is_some_and(|value| {
        std::env::split_paths(&value).any(|dir| {
            let candidate = dir.join(tool);
            candidate.is_file() || cfg!(windows) && candidate.with_extension("exe").is_file()
        })
    })
}

pub(super) fn guided(presentation: ux::Presentation) -> Result<(), TexeError> {
    let path: String = ux::prompt(
        cliclack::input("Existing paper folder")
            .default_input(".")
            .interact(),
    )?;
    run(
        Command::Adopt {
            path: PathBuf::from(path),
            entry: None,
            engine: None,
            check: false,
            yes: false,
            no_build: false,
            no_editor: false,
            replace_conflicts: false,
        },
        presentation,
    )
}
