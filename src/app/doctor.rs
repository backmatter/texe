use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::app::output::{human_bytes, human_count, print_json};
use crate::app::project::{ProjectContext, load_project, resolve_components, verification_policy};
use crate::toolchain::{self, VerificationPolicy, executable_version};
use crate::{TexeError, state, ux};

#[derive(Debug, Serialize)]
pub(super) struct DoctorReport {
    pub(super) schema: String,
    pub(super) project_root: PathBuf,
    pub(super) manifest: PathBuf,
    pub(super) provider: String,
    pub(super) adapter: String,
    pub(super) engine: String,
    pub(super) engine_executable: PathBuf,
    pub(super) engine_version: String,
    pub(super) kpsewhich_executable: PathBuf,
    pub(super) package_manager: PathBuf,
    pub(super) trace_adapter: PathBuf,
    pub(super) texmf_dist: PathBuf,
    pub(super) engine_roots: Vec<PathBuf>,
    pub(super) toolchain_verification: VerificationPolicy,
}
pub(super) fn run_doctor(
    project: Option<&Path>,
    verify_toolchain: bool,
    offline: bool,
    accept_downloads: bool,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let verification = verification_policy(verify_toolchain);
    let context = load_project(project)?;
    let build_root = context.root.join(&context.manifest.project.build_dir);
    confirm_first_download(
        &context,
        &build_root,
        offline,
        accept_downloads,
        false,
        presentation,
    )?;
    let (toolchain, tools) = resolve_components(&context, verification, offline)?;
    let report = DoctorReport {
        schema: "texe.doctor-report/v1".to_string(),
        project_root: context.root,
        manifest: context.manifest_path,
        provider: toolchain.provider,
        adapter: context.manifest.toolchain.adapter,
        engine: toolchain.engine,
        engine_version: executable_version(&toolchain.engine_executable)?,
        engine_executable: toolchain.engine_executable,
        kpsewhich_executable: toolchain.kpsewhich_executable,
        package_manager: tools.manager,
        trace_adapter: tools.trace_adapter,
        texmf_dist: toolchain.texmf_dist,
        engine_roots: toolchain.engine_roots,
        toolchain_verification: verification,
    };
    if presentation.json {
        print_json(&report)?;
    } else if !presentation.quiet {
        for line in doctor_output(&report, presentation.verbose) {
            println!("{line}");
        }
    }
    Ok(())
}

pub(super) fn doctor_output(report: &DoctorReport, verbose: bool) -> Vec<String> {
    let mut lines = vec![
        "Paper environment ready".to_string(),
        format!("project: {}", report.project_root.display()),
        format!("engine: {}", customer_engine_name(&report.engine)),
    ];
    if verbose {
        lines.extend([
            format!("engine executable: {}", report.engine_executable.display()),
            format!("engine version: {}", report.engine_version),
            format!("provider: {}", report.provider),
            format!("adapter: {}", report.adapter),
            format!("package manager: {}", report.package_manager.display()),
            format!("trace adapter: {}", report.trace_adapter.display()),
            format!("runtime: {}", report.texmf_dist.display()),
            format!(
                "toolchain verification: {}",
                match report.toolchain_verification {
                    VerificationPolicy::Deep => "deep",
                    VerificationPolicy::Interval => "interval",
                }
            ),
            format!("manifest: {}", report.manifest.display()),
        ]);
        lines.extend(
            report
                .engine_roots
                .iter()
                .map(|root| format!("engine input root: {}", root.display())),
        );
    } else {
        lines.push("details: run `texe doctor --verbose`".to_string());
    }
    lines
}

fn customer_engine_name(engine: &str) -> &str {
    match engine {
        "pdflatex" => "pdfLaTeX",
        "lualatex" => "LuaLaTeX",
        "xelatex" => "XeLaTeX",
        engine => engine,
    }
}

pub(super) fn confirm_first_download(
    context: &ProjectContext,
    build_root: &Path,
    offline: bool,
    accepted: bool,
    include_project_packages: bool,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let first_build = state::read(&build_root.join(state::STATE_NAME)).is_none();
    if !first_build {
        return Ok(());
    }
    let preflight = toolchain::download_preflight(&context.manifest.toolchain)?;
    let might_download_packages = include_project_packages && context.manifest.packages.remote;
    let needs_runtime_download = preflight
        .as_ref()
        .is_some_and(|plan| plan.missing_components > 0);
    if !needs_runtime_download && !might_download_packages {
        return Ok(());
    }
    if offline {
        if needs_runtime_download {
            let plan = preflight.expect("managed preflight exists");
            return Err(TexeError::Toolchain(format!(
                "offline mode cannot prepare {}: {}, {}, are not cached",
                plan.engine,
                human_count(
                    plan.missing_components,
                    "runtime component",
                    "runtime components"
                ),
                human_bytes(plan.missing_bytes)
            )));
        }
        return Ok(());
    }
    if accepted {
        return Ok(());
    }
    let terminals = ux::TerminalCapabilities::detect();
    if !presentation.quiet && !presentation.json {
        let mut details = Vec::new();
        if let Some(plan) = &preflight {
            if plan.missing_components == 0 {
                details.push("LaTeX runtime  Already cached".to_string());
            } else {
                details.push(format!(
                    "LaTeX runtime  {}, about {}",
                    human_count(
                        plan.missing_components,
                        "checksummed component",
                        "checksummed components"
                    ),
                    human_bytes(plan.missing_bytes)
                ));
            }
        }
        if might_download_packages {
            details.push("Paper packages Only what this source requires".to_string());
            details.push("Destination    texe’s local managed cache".to_string());
        }
        details.push("Privacy        No paper source or metadata is sent".to_string());
        if terminals.can_prompt() {
            ux::prompt(cliclack::note("First build", details.join("\n")))?;
        } else {
            println!("First build download");
            for detail in details {
                println!("  {detail}");
            }
        }
    }
    if presentation.json || !terminals.can_prompt() {
        return Ok(());
    }
    let confirmed = ux::prompt(
        cliclack::confirm("Download the required build components? (Recommended)")
            .initial_value(true)
            .interact(),
    )?;
    if confirmed {
        Ok(())
    } else {
        ux::prompt(cliclack::outro_cancel(
            "Build cancelled. Nothing was downloaded.",
        ))?;
        Err(TexeError::Prompt("cancelled".to_string()))
    }
}
