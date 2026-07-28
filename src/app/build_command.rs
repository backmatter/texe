use std::path::{Path, PathBuf};

use crate::app::doctor::confirm_first_download;
use crate::app::output::{human_count, print_json};
use crate::app::project::{load_project, resolve_components, verification_policy};
use crate::build::{self, BuildOptions, build_project};
use crate::progress::{self, PhaseKind, Progress, ProgressLayout};
use crate::{TexeError, state, ux};

pub(super) fn run_build(
    project: Option<&Path>,
    options: BuildOptions,
    verify_toolchain: bool,
    presentation: ux::Presentation,
    accept_downloads: bool,
) -> Result<(), TexeError> {
    run_build_in_flow(
        project,
        options,
        verify_toolchain,
        presentation,
        accept_downloads,
        false,
    )
}

pub(super) struct BuildOutcome {
    pub(super) entry: PathBuf,
    pub(super) report: build::BuildReport,
    pub(super) progress: Progress,
}

pub(super) fn run_build_in_flow(
    project: Option<&Path>,
    options: BuildOptions,
    verify_toolchain: bool,
    presentation: ux::Presentation,
    accept_downloads: bool,
    embedded: bool,
) -> Result<(), TexeError> {
    let outcome = execute_build(
        project,
        options,
        verify_toolchain,
        presentation,
        accept_downloads,
        embedded,
    )?;
    present_build_report(
        &outcome.entry,
        &outcome.report,
        &outcome.progress,
        presentation,
    )
}

pub(super) fn execute_build(
    project: Option<&Path>,
    options: BuildOptions,
    verify_toolchain: bool,
    presentation: ux::Presentation,
    accept_downloads: bool,
    embedded: bool,
) -> Result<BuildOutcome, TexeError> {
    let context = load_project(project)?;
    let build_root = context.root.join(&context.manifest.project.build_dir);
    confirm_first_download(
        &context,
        &build_root,
        options.offline,
        accept_downloads,
        true,
        presentation,
    )?;
    let progress = Progress::new(
        build_root.join("timings.json"),
        &context.manifest.toolchain.engine,
        options.frozen,
        context.manifest.toolchain.max_passes,
        !presentation.quiet,
        presentation.verbose,
        if embedded {
            ProgressLayout::Embedded
        } else {
            ProgressLayout::Standalone
        },
    );
    progress.begin(state::read(&build_root.join(state::STATE_NAME)).is_some());
    let build_result = (|| {
        let provider = &context.manifest.toolchain.provider;
        let (toolchain, tools) = progress.phase(
            PhaseKind::Toolchain,
            format!("preparing {provider} toolchain and build tools"),
            || {
                resolve_components(
                    &context,
                    verification_policy(verify_toolchain),
                    options.offline,
                )
            },
        )?;
        build_project(
            &context.root,
            &context.manifest,
            &toolchain,
            &tools,
            options,
            progress.clone(),
        )
    })();
    let report = match build_result {
        Ok(report) => {
            if report.cached {
                progress.complete(&format!(
                    "PDF is up to date · {}",
                    report.artifact.display()
                ));
            } else {
                progress.complete(&format!(
                    "Built {} in {} · {}",
                    report.artifact.display(),
                    progress::format_duration(std::time::Duration::from_millis(
                        report.duration_millis
                    )),
                    warning_summary(report.warning_count)
                ));
            }
            report
        }
        Err(error) => {
            progress.fail("Build stopped");
            return Err(error);
        }
    };
    Ok(BuildOutcome {
        entry: context.manifest.project.entry,
        report,
        progress,
    })
}

pub(super) fn present_build_report(
    entry: &Path,
    report: &build::BuildReport,
    progress: &Progress,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    if presentation.json {
        return print_json(report);
    }
    if presentation.quiet {
        return Ok(());
    }
    if progress.is_live() {
        for warning in report.warnings.iter().take(3) {
            ux::prompt(cliclack::log::warning(&warning.message))?;
        }
        if report.warning_count > 3 {
            ux::prompt(cliclack::log::warning(format!(
                "{} more; use texe build --verbose to see all",
                human_count(report.warning_count - 3, "warning", "warnings")
            )))?;
        }
        return Ok(());
    }
    if report.cached {
        println!("checked {}", entry.display());
        println!(
            "{} is up to date; no project files changed",
            report.artifact.display()
        );
    } else {
        println!(
            "built {} in {}",
            report.artifact.display(),
            progress::format_duration(std::time::Duration::from_millis(report.duration_millis))
        );
        println!(
            "{} engine pass{}, {} bibliography run{}, {} index run{}, {} package convergence round{}",
            report.engine_passes,
            if report.engine_passes == 1 { "" } else { "es" },
            report.bibliography_runs,
            if report.bibliography_runs == 1 {
                ""
            } else {
                "s"
            },
            report.index_runs,
            if report.index_runs == 1 { "" } else { "s" },
            report.convergence_rounds,
            if report.convergence_rounds == 1 {
                ""
            } else {
                "s"
            },
        );
        if report.warning_count == 0 {
            println!("no warnings");
        } else {
            for warning in report.warnings.iter().take(3) {
                println!("warning: {}", warning.message);
            }
            if report.warning_count > 3 {
                println!(
                    "{} more; run `texe build --verbose` to show all",
                    human_count(report.warning_count - 3, "warning", "warnings")
                );
            } else {
                println!(
                    "{}",
                    human_count(report.warning_count, "warning", "warnings")
                );
            }
        }
        if presentation.verbose {
            println!("package environment: {}", report.environment_fingerprint);
            for warning in report.warnings.iter().skip(3) {
                println!("warning: {}", warning.message);
            }
        }
    }
    Ok(())
}

fn warning_summary(count: usize) -> String {
    match count {
        0 => "no warnings".to_string(),
        1 => "1 warning".to_string(),
        count => format!("{count} warnings"),
    }
}
