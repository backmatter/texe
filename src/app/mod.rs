use clap::Parser;

use crate::build::BuildOptions;
use crate::clean::CleanOptions;
use crate::cli::{Cli, Command};
use crate::integrations;
use crate::{TexeError, ux};

mod build_command;
mod doctor;
mod maintenance;
mod output;
mod project;
mod setup;
mod watch_command;

use build_command::run_build;
use doctor::run_doctor;
use maintenance::{run_clean, run_clean_dry_run, run_storage};
use output::print_json;
pub(crate) use output::{human_bytes, human_count};
use project::load_project;
use setup::{InitCommand, InitIntegrations, run_bare, run_init};
use watch_command::run_watch;

pub fn main_entry() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let presentation_hint = ux::Presentation {
        json: arguments.iter().any(|argument| argument == "--json"),
        quiet: arguments
            .iter()
            .any(|argument| argument == "--quiet" || argument == "-q"),
        verbose: arguments
            .iter()
            .any(|argument| argument == "--verbose" || argument == "-v"),
    };
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return;
        }
        Err(error) => {
            let error = TexeError::Usage(error.to_string());
            ux::present_error(&error, presentation_hint);
            std::process::exit(i32::from(error.category().exit_code()));
        }
    };
    let presentation = ux::Presentation {
        json: cli.json,
        quiet: cli.quiet,
        verbose: cli.verbose,
    };
    if let Err(error) = run(cli) {
        ux::present_error(&error, presentation);
        std::process::exit(i32::from(error.category().exit_code()));
    }
}

fn run(cli: Cli) -> Result<(), TexeError> {
    let presentation = ux::Presentation {
        json: cli.json,
        quiet: cli.quiet,
        verbose: cli.verbose,
    };
    match cli.command {
        Some(command) => run_command(command, presentation),
        None => run_bare(presentation),
    }
}

fn run_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    match command {
        command @ Command::Init { .. } => run_init_command(command, presentation),
        command @ Command::Doctor { .. } => run_doctor_command(command, presentation),
        command @ Command::Clean { .. } => run_clean_command(command, presentation),
        command @ Command::Build { .. } => run_build_command(command, presentation),
        command @ Command::Watch { .. } => run_watch_command(command, presentation),
        command @ Command::Editor { .. } => run_editor_command(command, presentation),
        Command::Storage { project } => run_storage(project.as_deref(), presentation),
    }
}

fn run_init_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Init {
        path,
        entry,
        engine,
        title,
        author,
        template,
        yes,
        git,
        vscode,
    } = command
    else {
        unreachable!("init handler received another command");
    };
    run_init(InitCommand {
        path,
        entry,
        engine,
        title,
        author,
        template,
        yes,
        integrations: InitIntegrations {
            git,
            vscode,
            open_vscode: true,
        },
        presentation,
    })
    .map(|_| ())
}

fn run_editor_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Editor { project, remove } = command else {
        unreachable!("editor handler received another command");
    };
    let context = load_project(project.as_deref())?;
    let report = if remove {
        integrations::remove_vscode(&context.root)?
    } else {
        integrations::setup_vscode(
            &context.root,
            true,
            !presentation.json && !presentation.quiet,
        )?
    };
    if presentation.json {
        print_json(&serde_json::json!({
            "schema": "texe.editor-report/v1",
            "removed": remove,
            "messages": report.messages,
        }))
    } else {
        if !presentation.quiet {
            for message in report.messages {
                println!("{message}");
            }
        }
        Ok(())
    }
}

fn run_doctor_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Doctor {
        project,
        verify_toolchain,
        offline,
        yes,
    } = command
    else {
        unreachable!("doctor handler received another command");
    };
    run_doctor(
        project.as_deref(),
        verify_toolchain,
        offline,
        yes,
        presentation,
    )
}

fn run_clean_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Clean {
        project,
        caches,
        all,
        dry_run,
    } = command
    else {
        unreachable!("clean handler received another command");
    };
    let options = CleanOptions { caches, all };
    if dry_run {
        run_clean_dry_run(project.as_deref(), options, presentation)
    } else {
        run_clean(project.as_deref(), options, presentation)
    }
}

fn run_build_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Build {
        project,
        frozen,
        force,
        verify_toolchain,
        offline,
        yes,
    } = command
    else {
        unreachable!("build handler received another command");
    };
    run_build(
        project.as_deref(),
        BuildOptions {
            frozen,
            force,
            offline,
        },
        verify_toolchain,
        presentation,
        yes,
    )
}

fn run_watch_command(command: Command, presentation: ux::Presentation) -> Result<(), TexeError> {
    let Command::Watch {
        project,
        frozen,
        verify_toolchain,
        offline,
        yes,
        poll_ms,
        view,
    } = command
    else {
        unreachable!("watch handler received another command");
    };
    run_watch(
        project.as_deref(),
        BuildOptions {
            frozen,
            force: false,
            offline,
        },
        verify_toolchain,
        presentation,
        poll_ms,
        yes,
        view,
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use clap::Parser as _;

    use crate::TexeError;
    use crate::app::doctor::{DoctorReport, doctor_output};
    use crate::app::project::verification_policy;
    use crate::app::watch_command::{
        watch_build_failed_event, watch_build_started_event, watch_build_succeeded_event,
        watch_started_event, watch_stopped_event,
    };
    use crate::build;
    use crate::cli::{Cli, Command};
    use crate::toolchain::VerificationPolicy;

    #[test]
    fn clap_parses_scripted_init_choices() {
        let cli = Cli::try_parse_from([
            "texe",
            "init",
            "paper",
            "--entry",
            "src/main.tex",
            "--engine",
            "xelatex",
            "--yes",
            "--git",
            "--vscode",
        ])
        .expect("init arguments should parse");
        let Some(Command::Init {
            path,
            entry,
            engine,
            yes,
            git,
            vscode,
            ..
        }) = cli.command
        else {
            panic!("expected init command");
        };
        assert_eq!(path, Path::new("paper"));
        assert_eq!(entry.as_deref(), Some(Path::new("src/main.tex")));
        assert_eq!(engine.as_deref(), Some("xelatex"));
        assert!(yes);
        assert!(git);
        assert!(vscode);
    }

    #[test]
    fn clap_parses_build_cache_and_verification_flags() {
        let cli =
            Cli::try_parse_from(["texe", "build", "--frozen", "--force", "--verify-toolchain"])
                .expect("build arguments should parse");
        let Some(Command::Build {
            frozen,
            force,
            verify_toolchain,
            ..
        }) = cli.command
        else {
            panic!("expected build command");
        };
        assert!(frozen);
        assert!(force);
        assert!(verify_toolchain);

        let cli = Cli::try_parse_from(["texe", "build"]).expect("bare build should parse");
        let Some(Command::Build {
            frozen,
            force,
            verify_toolchain,
            ..
        }) = cli.command
        else {
            panic!("expected build command");
        };
        assert!(!frozen);
        assert!(!force);
        assert!(!verify_toolchain);
        assert_eq!(verification_policy(false), VerificationPolicy::Interval);
        assert_eq!(verification_policy(true), VerificationPolicy::Deep);
    }

    #[test]
    fn clap_rejects_unknown_init_flags() {
        let error = Cli::try_parse_from(["texe", "init", "--interactive-engine"])
            .expect_err("unknown flags should fail");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn watch_events_form_a_compact_versioned_json_lines_stream() {
        let report = build::BuildReport {
            schema: "texe.build-report/v1".to_string(),
            engine: "pdflatex".to_string(),
            artifact: PathBuf::from("/paper/main.pdf"),
            engine_passes: 1,
            bibliography_runs: 0,
            index_runs: 0,
            convergence_rounds: 0,
            environment_fingerprint: "sha256:test".to_string(),
            cached: false,
            duration_millis: 120,
            warning_count: 0,
            warnings: Vec::new(),
        };
        let events = [
            watch_started_event(
                Path::new("/paper"),
                Path::new("/paper/main.pdf"),
                None,
                false,
            ),
            watch_build_started_event(1, &[]),
            watch_build_succeeded_event(1, &report),
            watch_build_failed_event(2, &TexeError::Build("example failure".to_string()), true),
            watch_stopped_event(),
        ];
        let stream = events
            .iter()
            .map(|event| serde_json::to_string(event).expect("watch event JSON"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = stream
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("one JSON event"))
            .collect::<Vec<_>>();

        assert_eq!(parsed.len(), 5);
        assert!(
            parsed
                .iter()
                .all(|event| event["schema"] == "texe.watch-event/v1")
        );
        assert_eq!(parsed[1]["event"], "build-started");
        assert_eq!(parsed[2]["report"]["schema"], "texe.build-report/v1");
        assert_eq!(parsed[3]["error"]["schema"], "texe.error/v1");
        assert_eq!(parsed[3]["watching"], true);
        assert_eq!(parsed[4]["event"], "watch-stopped");
    }

    #[test]
    fn default_doctor_output_keeps_implementation_paths_hidden() {
        let report = DoctorReport {
            schema: "texe.doctor-report/v1".to_string(),
            project_root: PathBuf::from("<PROJECT>"),
            manifest: PathBuf::from("<PROJECT>/texe.toml"),
            provider: "managed".to_string(),
            adapter: "kpathsea".to_string(),
            engine: "pdflatex".to_string(),
            engine_executable: PathBuf::from("<CACHE>/pdftex"),
            engine_version: "pdfTeX test".to_string(),
            kpsewhich_executable: PathBuf::from("<CACHE>/kpsewhich"),
            package_manager: PathBuf::from("<BIN>/pqty"),
            trace_adapter: PathBuf::from("<BIN>/pqty-fls"),
            texmf_dist: PathBuf::from("<CACHE>/texmf-dist"),
            engine_roots: vec![PathBuf::from("<CACHE>/texmf-dist")],
            toolchain_verification: VerificationPolicy::Interval,
        };
        let expected = include_str!("../../tests/fixtures/transcripts/doctor-default.txt")
            .lines()
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(doctor_output(&report, false).join("\n"), expected);
        let verbose = doctor_output(&report, true).join("\n");
        assert!(verbose.contains("package manager: <BIN>/pqty"));
        assert!(verbose.contains("runtime: <CACHE>/texmf-dist"));
    }
}
