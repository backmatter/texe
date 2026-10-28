mod bibliography;
mod format;
pub(crate) mod generated;
mod index;
pub(crate) mod process;
mod synctex;
mod trace;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Instant;

use serde::Serialize;

use crate::TexeError;
use crate::config::ProjectManifest;
use crate::guard::BuildGuard;
use crate::lockfile::{project_lock_bytes, restore_package_lock};
use crate::package::{
    Convergence, EnsureLockRequest, PackageEnvironment, PqtyClient, ReconcileRequest,
};
use crate::progress::{PhaseKind, Progress};
use crate::state;
use crate::toolchain::ResolvedToolchain;
use bibliography::BibliographyState;
use format::ManagedFormat;
use index::IndexState;

const MAX_CONVERGENCE_ROUNDS: usize = 16;

#[derive(Debug, Clone, Copy)]
struct AuxiliaryPassBudget {
    completed: usize,
    maximum: usize,
}

impl AuxiliaryPassBudget {
    const fn new(maximum: usize) -> Self {
        Self {
            completed: 0,
            maximum,
        }
    }

    const fn next(self) -> Option<usize> {
        if self.completed < self.maximum {
            Some(self.completed + 1)
        } else {
            None
        }
    }

    fn record_stable_environment_pass(&mut self) {
        self.completed += 1;
    }
}

#[allow(clippy::struct_excessive_bools)] // These are independent public CLI flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// Keep researcher sources read-only throughout compiler execution.
    pub protected: bool,
    /// Require the existing package lock and forbid convergence.
    pub frozen: bool,
    /// Rebuild even when nothing a rebuild would read has changed.
    pub force: bool,
    /// Forbid network access and require every runtime/package cache entry.
    pub offline: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub schema: String,
    pub engine: String,
    pub artifact: PathBuf,
    pub engine_passes: usize,
    pub bibliography_runs: usize,
    pub index_runs: usize,
    pub convergence_rounds: usize,
    pub environment_fingerprint: String,
    /// True when the published artifact was already current and no engine,
    /// package, or bibliography work ran.
    pub cached: bool,
    pub duration_millis: u64,
    pub warning_count: usize,
    pub warnings: Vec<BuildWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildWarning {
    pub kind: String,
    pub message: String,
}

struct BuildContext<'a> {
    project_root: &'a Path,
    manifest: &'a ProjectManifest,
    toolchain: &'a ResolvedToolchain,
    tools: &'a PqtyClient,
    entry: PathBuf,
    lock: PathBuf,
    texmf: PathBuf,
    build_root: PathBuf,
    discovery_dir: PathBuf,
    output_dir: PathBuf,
    environment_path: PathBuf,
    state_path: PathBuf,
    auxiliary_cache_path: PathBuf,
    timestamp: BuildTimestamp,
    progress: Progress,
}

struct BuildState {
    environment: PackageEnvironment,
    managed_format: Option<ManagedFormat>,
    convergence_rounds: usize,
    required_runtime_providers: BTreeSet<String>,
    engine_passes: usize,
    bibliography: BibliographyState,
    index: IndexState,
    auxiliary_cache_key: String,
}

#[derive(Debug)]
struct EngineRun {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    log_path: PathBuf,
    fls_path: PathBuf,
}

/// Resolve only derived paths in memory, then reject source aliases before the
/// protected caller creates a project lock, package tree or output directory.
pub(crate) fn protected_manifest(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<ProjectManifest, TexeError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(TexeError::Build(
            "Protected local builds currently require Linux x86-64 with Landlock V3. Sources and the last PDF have been kept.".into(),
        ));
    }
    let mut effective = manifest.clone();
    if !effective.project.build_dir.starts_with(".texe") {
        effective.project.build_dir = ".texe/protected-build".into();
    }
    if !effective.packages.lock.starts_with(".texe") {
        effective.packages.lock = ".texe/protected-state/pqty.lock".into();
    }
    if !effective.packages.texmf.starts_with(".texe") {
        effective.packages.texmf = ".texe/protected-texmf".into();
    }
    effective.validate()?;
    let portable = crate::portable::active(root)?.is_some();
    let reserved = [
        ".texe/recovery",
        ".texe/editor",
        ".texe/project",
        ".texe/runtime",
        ".texe/descriptors",
        ".texe/environment.json",
        ".texe/published.json",
        ".texe/environment-retention.json",
    ];
    for path in [
        &effective.project.build_dir,
        &effective.packages.texmf,
        &effective.packages.lock,
    ]
    .into_iter()
    .chain(effective.packages.store.iter())
    {
        if reserved
            .iter()
            .any(|private| path.starts_with(private) || Path::new(private).starts_with(path))
            || (!portable && path.starts_with(".texe/environments"))
        {
            return Err(TexeError::Build(
                "Build paths cannot overlap recovery or editor metadata. Sources and the last PDF have been kept.".into(),
            ));
        }
    }
    let derived = [
        root.join(&effective.project.build_dir),
        root.join(&effective.packages.texmf),
        root.join(&effective.packages.lock),
        root.join(".texe/project"),
    ];
    for (position, path) in derived.iter().enumerate() {
        if position == 2 {
            generated::optional(root, root, path, generated::MAX_METADATA)?;
        } else {
            generated::validate_parent(root, &path.join(".protected-path-check"))?;
        }
    }
    for source in std::iter::once(&effective.project.entry)
        .chain(effective.inputs.roots.iter())
        .chain(effective.bibliography.roots.iter())
    {
        let lexical = root.join(source);
        let resolved = lexical.canonicalize().map_err(|source| TexeError::Io {
            path: lexical.clone(),
            source,
        })?;
        if derived
            .iter()
            .any(|private| lexical.starts_with(private) || resolved.starts_with(private))
        {
            return Err(TexeError::Build(
                "Research source paths cannot use private build state. Sources and the last PDF have been kept.".into(),
            ));
        }
    }
    Ok(effective)
}

#[allow(clippy::too_many_lines)]
pub fn build_project(
    project_root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    tools: &PqtyClient,
    options: BuildOptions,
    progress: Progress,
) -> Result<BuildReport, TexeError> {
    let _protection = process::ProtectionScope::enter(options.protected);
    let effective;
    let manifest = if options.protected {
        effective = protected_manifest(project_root, manifest)?;
        &effective
    } else {
        manifest
    };
    let portable = crate::portable::active(project_root)?;
    if let Some(descriptor) = &portable {
        if crate::portable::manifest_for_project(project_root, descriptor)? != *manifest {
            return Err(TexeError::Build(
                "The build environment changed; retry this build".into(),
            ));
        }
        if matches!(descriptor["engine"].as_str(), Some("shipout" | "tectonic")) {
            return crate::portable::native::build(
                project_root,
                manifest,
                toolchain,
                tools,
                descriptor,
                options,
                &progress,
                true,
            );
        }
    }
    let options = BuildOptions {
        frozen: options.frozen || portable.is_some(),
        ..options
    };
    let started = Instant::now();
    let context = BuildContext::new(project_root, manifest, toolchain, tools, progress)?;
    // Held for the whole build, including the fast path: the check reads the
    // published outputs another build could be replacing.
    let _guard = BuildGuard::acquire(&context.build_root)?;
    announce_configuration_warnings(manifest, &context.progress);
    let unmanaged_commands = manifest.uses_unmanaged_commands();
    let cacheable = manifest.toolchain.provider == "managed"
        && !manifest.toolchain.shell_escape
        && !unmanaged_commands
        && (!options.protected || process::source_read_roots(project_root, manifest)?.is_empty());
    if cacheable
        && !options.force
        && let Some(mut report) = context.current_build()
    {
        if let Some(environment) = &portable {
            report.environment_fingerprint = environment["id"].as_str().unwrap().into();
        }
        return Ok(report);
    }
    context.progress.announce_rebuild();
    // A save during compilation must not associate its new bytes with a PDF
    // the engine already rendered from the earlier source.
    let protected_publication = portable.is_some() || options.protected;
    let rendered_sources = if protected_publication {
        Some(capture_render_sources(project_root, manifest)?)
    } else if cacheable {
        state::source_fingerprint(project_root, manifest)
            .ok()
            .flatten()
    } else {
        None
    };
    let mut state = context.prepare(options.frozen)?;
    if !options.frozen && context.discovery_required(&state) {
        context.discover_packages(&mut state)?;
    }
    let internal_artifact = context.frozen_build(&mut state, options.frozen)?;
    let warnings = collect_warnings(
        &context
            .output_dir
            .join(format!("{}.log", job_stem(&manifest.project.entry)?)),
    );
    let project_lock =
        project_lock_bytes(&context.lock, &toolchain.identity, context.timestamp.locked)?;
    let publish = || {
        if protected_publication {
            artifact::publish_artifact_at(
                project_root,
                &internal_artifact,
                &project_lock,
                &context.build_root.join("texe.lock"),
            )
        } else {
            publish_artifact(project_root, &internal_artifact, &project_lock)
        }
    };
    let published = if protected_publication {
        match publish_if_sources_unchanged(
            project_root,
            manifest,
            rendered_sources.as_deref().unwrap(),
            publish,
        ) {
            Ok(published) => published,
            Err(error) => {
                let _ =
                    generated::remove(project_root, &context.build_root.join(state::STATE_NAME));
                return Err(error);
            }
        }
    } else {
        publish()?
    };
    if let Err(error) = context.record_build(
        &published,
        &state.environment.fingerprint,
        rendered_sources.as_deref(),
    ) {
        eprintln!("texe: warning: could not save build cache: {error}");
    }
    let auxiliary_cache = auxiliary::AuxiliaryCache::from_entries(
        &state.auxiliary_cache_key,
        state.bibliography.cache_entries(),
        state.index.cache_entries(),
    );
    if let Err(error) = auxiliary::write(&context.auxiliary_cache_path, &auxiliary_cache) {
        eprintln!("texe: warning: could not save auxiliary build cache: {error}");
    }
    let report = BuildReport {
        schema: "texe.build-report/v1".to_string(),
        engine: toolchain.engine.clone(),
        artifact: published.artifact,
        engine_passes: state.engine_passes,
        convergence_rounds: state.convergence_rounds,
        environment_fingerprint: portable
            .as_ref()
            .map_or(state.environment.fingerprint, |v| {
                v["id"].as_str().unwrap().into()
            }),
        bibliography_runs: state.bibliography.runs(),
        index_runs: state.index.runs(),
        cached: false,
        duration_millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        warning_count: warnings.len(),
        warnings,
    };
    context.progress.finish(
        report.engine_passes,
        report.bibliography_runs,
        report.index_runs,
        report.convergence_rounds,
        &report.environment_fingerprint,
    );
    Ok(report)
}

pub(crate) fn announce_configuration_warnings(manifest: &ProjectManifest, progress: &Progress) {
    if manifest.toolchain.shell_escape {
        progress.warning(
            "shell escape is enabled; external commands and their inputs are not pinned by \
             texe.lock",
        );
    }
    if manifest.uses_unmanaged_commands() {
        progress.warning(
            "unmanaged command overrides are enabled; project or host commands may execute and \
             the no-op build cache is disabled",
        );
    }
}

mod artifact;
mod auxiliary;
mod engine;
mod environment;
mod errors;
mod filesystem;
mod warnings;

use artifact::{BuildTimestamp, PublishedBuild, publish_artifact, resolve_build_timestamp};
use errors::{engine_failure, package_error_with_engine_context};
use filesystem::{auxiliary_snapshot, find_artifact, job_stem, mirror_project_directories};
use warnings::collect_warnings;

impl<'a> BuildContext<'a> {
    fn new(
        project_root: &'a Path,
        manifest: &'a ProjectManifest,
        toolchain: &'a ResolvedToolchain,
        tools: &'a PqtyClient,
        progress: Progress,
    ) -> Result<Self, TexeError> {
        let entry = project_root.join(&manifest.project.entry);
        if !entry.is_file() {
            return Err(TexeError::Build(format!(
                "entry file does not exist: {}",
                entry.display()
            )));
        }
        let build_root = project_root.join(&manifest.project.build_dir);
        let texmf = project_root.join(&manifest.packages.texmf);
        let discovery_dir = build_root.join("discovery");
        let output_dir = build_root.join("output");
        if process::is_portable_build(project_root, &build_root) {
            for directory in [&discovery_dir, &output_dir, &texmf] {
                crate::portable::private_directory(
                    project_root,
                    directory.strip_prefix(project_root).map_err(|_| {
                        TexeError::Build("Private build state is outside the project".into())
                    })?,
                )?;
            }
        }
        for directory in [&discovery_dir, &output_dir] {
            fs::create_dir_all(directory).map_err(|source| TexeError::Io {
                path: directory.clone(),
                source,
            })?;
            mirror_project_directories(project_root, directory, &[&build_root, &texmf])?;
        }
        Ok(Self {
            project_root,
            manifest,
            toolchain,
            tools,
            entry,
            lock: project_root.join(&manifest.packages.lock),
            texmf,
            environment_path: build_root.join("pqty.env.json"),
            state_path: build_root.join(state::STATE_NAME),
            auxiliary_cache_path: build_root.join(auxiliary::CACHE_NAME),
            build_root,
            discovery_dir,
            output_dir,
            timestamp: resolve_build_timestamp(project_root),
            progress,
        })
    }

    /// Report the previous build when nothing a rebuild would read has moved
    /// since it finished. `texe build` sits in an editing loop, where a build
    /// with no edits behind it should not repeat a package scan, a
    /// materialization, and two engine passes to arrive at the same bytes.
    fn current_build(&self) -> Option<BuildReport> {
        let recorded = state::read(&self.state_path)?;
        let fingerprint = state::build_fingerprint(
            self.project_root,
            self.manifest,
            &self.toolchain.identity,
            self.tools.fingerprint(),
            self.timestamp.effective,
        )
        .ok()
        .flatten()?;
        if recorded.fingerprint != fingerprint {
            return None;
        }
        let artifact = state::verify_outputs(self.project_root, &recorded.outputs)?;
        if artifact.file_stem() != self.manifest.project.entry.file_stem() {
            return None;
        }
        Some(BuildReport {
            schema: "texe.build-report/v1".to_string(),
            engine: self.toolchain.engine.clone(),
            artifact,
            engine_passes: 0,
            bibliography_runs: 0,
            index_runs: 0,
            convergence_rounds: 0,
            environment_fingerprint: recorded.environment_fingerprint,
            cached: true,
            duration_millis: 0,
            warning_count: 0,
            warnings: Vec::new(),
        })
    }

    /// Record what this build read and produced, so the next one can decide in
    /// a few file reads whether it has anything to do. Written after the
    /// composite lock, which the fingerprint covers.
    fn record_build(
        &self,
        published: &PublishedBuild,
        environment_fingerprint: &str,
        rendered_sources: Option<&str>,
    ) -> Result<(), TexeError> {
        record_if_sources_unchanged(
            self.project_root,
            self.manifest,
            &self.state_path,
            rendered_sources,
            || self.record_unchanged_build(published, environment_fingerprint),
        )
    }

    fn record_unchanged_build(
        &self,
        published: &PublishedBuild,
        environment_fingerprint: &str,
    ) -> Result<(), TexeError> {
        let Some(fingerprint) = state::build_fingerprint(
            self.project_root,
            self.manifest,
            &self.toolchain.identity,
            self.tools.fingerprint(),
            self.timestamp.effective,
        )?
        else {
            return Ok(());
        };
        state::write(
            &self.state_path,
            &fingerprint,
            environment_fingerprint,
            self.project_root,
            &published.paths(),
        )
    }

    fn prepare(&self, frozen: bool) -> Result<BuildState, TexeError> {
        if let Some(descriptor) = crate::portable::active(self.project_root)? {
            crate::portable::restore_package_lock_from_descriptor(
                self.project_root,
                self.manifest,
                &descriptor,
            )?;
        } else {
            restore_package_lock(
                self.project_root,
                &self.lock,
                &self.toolchain.identity,
                frozen,
            )?;
        }
        if frozen {
            self.tools.ensure_lock(&EnsureLockRequest {
                project_root: self.project_root,
                manifest: self.manifest,
                toolchain: self.toolchain,
                entry: &self.entry,
                lock: &self.lock,
                frozen: true,
                progress: &self.progress,
            })?;
        } else {
            let label = if self.lock.is_file() {
                "refreshing package lock"
            } else {
                "resolving package lock"
            };
            self.progress.phase(PhaseKind::Package, label, || {
                self.tools.ensure_lock(&EnsureLockRequest {
                    project_root: self.project_root,
                    manifest: self.manifest,
                    toolchain: self.toolchain,
                    entry: &self.entry,
                    lock: &self.lock,
                    frozen: false,
                    progress: &self.progress,
                })
            })?;
        }
        self.progress.phase(
            PhaseKind::Package,
            "fetching/materializing package environment",
            || {
                self.tools.install(
                    self.project_root,
                    self.manifest,
                    self.toolchain,
                    &self.lock,
                    &self.texmf,
                    &self.progress,
                )
            },
        )?;
        let environment =
            self.tools
                .environment(self.project_root, &self.lock, &self.environment_path)?;
        let managed_format = format::ensure(
            self.project_root,
            self.toolchain,
            &self.texmf,
            &environment,
            &self.build_root,
            self.manifest.packages.remote,
            &self.progress,
        )?;
        let auxiliary_cache_key =
            auxiliary::cache_key(self.manifest, self.toolchain, &environment.fingerprint)?;
        let auxiliary_cache = auxiliary::read(&self.auxiliary_cache_path, &auxiliary_cache_key);
        Ok(BuildState {
            environment,
            managed_format,
            convergence_rounds: 0,
            required_runtime_providers: BTreeSet::new(),
            engine_passes: 0,
            bibliography: BibliographyState::from_cache(auxiliary_cache.bibliography),
            index: IndexState::from_cache(auxiliary_cache.index),
            auxiliary_cache_key,
        })
    }

    /// Whether this build needs a dedicated discovery phase before the frozen
    /// passes.
    ///
    /// Discovery exists to find packages the engine only asks for at runtime,
    /// which the static scan behind `pqty lock` cannot see. It costs a full
    /// engine pass, and in an editing loop it usually finds nothing: a project
    /// whose materialized package environment is byte for byte the one that
    /// last built successfully here has already converged. Anything the current
    /// edit newly requires is still caught, because the frozen loop reconciles
    /// its own trace after every pass and restarts on a changed environment —
    /// including when the pass failed on the missing input.
    ///
    /// A new `\usepackage` moves the environment fingerprint during
    /// `prepare`, so the statically visible case takes discovery as before.
    fn discovery_required(&self, state: &BuildState) -> bool {
        state::read(&self.state_path).is_none_or(|recorded| {
            recorded.environment_fingerprint != state.environment.fingerprint
        })
    }

    fn discover_packages(&self, state: &mut BuildState) -> Result<(), TexeError> {
        for round in 1..=MAX_CONVERGENCE_ROUNDS {
            let run = self.progress.phase(
                PhaseKind::EngineDiscovery,
                format!("discovering runtime packages ({round}/{MAX_CONVERGENCE_ROUNDS})"),
                || {
                    self.run_engine(
                        &self.discovery_dir,
                        state.managed_format.as_ref(),
                        true,
                        false,
                    )
                },
            )?;
            state.engine_passes += 1;
            let trace_path = self.build_root.join("discovery.trace.json");
            trace::create(&trace::TraceRequest {
                project_root: self.project_root,
                tools: self.tools,
                toolchain: self.toolchain,
                environment_path: &self.environment_path,
                texmf: &self.texmf,
                output_dir: &self.discovery_dir,
                log_path: &run.log_path,
                recorder_path: &run.fls_path,
                managed_format_root: state
                    .managed_format
                    .as_ref()
                    .map(|format| format.root.as_path()),
                discovery: true,
                trace_path: &trace_path,
            })?;
            match self.reconcile_run(&run, &trace_path, false)? {
                Convergence::Stable => {
                    if self.require_runtime_providers(state, &run, false)? {
                        continue;
                    }
                    if run.status.success() {
                        return Ok(());
                    }
                    // Nonstop discovery can leave LuaTeX in an invalid
                    // recovery state after collecting several independent
                    // missing inputs. Once the package environment is stable,
                    // verify it from a fresh fail-fast invocation before
                    // treating that recovery-only crash as a document error.
                    let verified = self.progress.phase(
                        PhaseKind::EngineDiscovery,
                        "verifying converged package discovery",
                        || {
                            self.run_engine(
                                &self.discovery_dir,
                                state.managed_format.as_ref(),
                                true,
                                true,
                            )
                        },
                    )?;
                    state.engine_passes += 1;
                    trace::create(&trace::TraceRequest {
                        project_root: self.project_root,
                        tools: self.tools,
                        toolchain: self.toolchain,
                        environment_path: &self.environment_path,
                        texmf: &self.texmf,
                        output_dir: &self.discovery_dir,
                        log_path: &verified.log_path,
                        recorder_path: &verified.fls_path,
                        managed_format_root: state
                            .managed_format
                            .as_ref()
                            .map(|format| format.root.as_path()),
                        discovery: true,
                        trace_path: &trace_path,
                    })?;
                    match self.reconcile_run(&verified, &trace_path, false)? {
                        Convergence::Changed => {
                            state.convergence_rounds += 1;
                            self.refresh_environment(state)?;
                        }
                        Convergence::Stable => {
                            if self.require_runtime_providers(state, &verified, false)? {
                                continue;
                            }
                            if verified.status.success() {
                                return Ok(());
                            }
                            return Err(engine_failure(
                                self.project_root,
                                &self.manifest.project.entry,
                                self.toolchain,
                                &verified,
                            ));
                        }
                    }
                }
                Convergence::Changed => {
                    state.convergence_rounds += 1;
                    self.refresh_environment(state)?;
                }
            }
        }
        Err(TexeError::Build(format!(
            "package environment did not converge after {MAX_CONVERGENCE_ROUNDS} rounds"
        )))
    }

    /// Run engine passes until the auxiliary state stops moving.
    ///
    /// The comparison is seeded with what the previous build left in the output
    /// directory, so an edit that resolves to the same cross-references,
    /// citations, and table of contents — the common case in an editing loop —
    /// finishes in one pass instead of paying for a second one to discover that
    /// nothing moved. A first build, a changed reference, or a bibliography run
    /// still takes as many passes as it needs. Engine attempts that discover a
    /// changed package environment are bounded separately and do not consume
    /// this auxiliary-stabilization budget.
    fn frozen_build(&self, state: &mut BuildState, frozen: bool) -> Result<PathBuf, TexeError> {
        let mut previous_auxiliary = Some(auxiliary_snapshot(&self.output_dir)?);
        let mut artifact: Option<PathBuf> = None;
        let mut pass_budget = AuxiliaryPassBudget::new(self.manifest.toolchain.max_passes);
        loop {
            let Some(pass) = pass_budget.next() else {
                return Err(TexeError::Build(format!(
                    "{} auxiliary state did not stabilize after {} frozen passes{}",
                    self.toolchain.engine,
                    self.manifest.toolchain.max_passes,
                    artifact.as_ref().map_or_else(String::new, |path| format!(
                        " (latest artifact: {})",
                        path.display()
                    ))
                )));
            };
            let run = self.progress.phase(
                PhaseKind::EngineFinal,
                format!(
                    "frozen {} pass {pass}/{}",
                    self.toolchain.engine, self.manifest.toolchain.max_passes
                ),
                || self.run_engine(&self.output_dir, state.managed_format.as_ref(), false, true),
            )?;
            state.engine_passes += 1;
            let trace_path = self.build_root.join("frozen.trace.json");
            self.create_run_trace(state, &run, &trace_path, false)?;
            match self.reconcile_run(&run, &trace_path, frozen)? {
                Convergence::Stable => {
                    if self.require_runtime_providers(state, &run, frozen)? {
                        previous_auxiliary = None;
                        continue;
                    }
                }
                Convergence::Changed => {
                    state.convergence_rounds += 1;
                    if state.convergence_rounds > MAX_CONVERGENCE_ROUNDS {
                        return Err(TexeError::Build(format!(
                            "package environment did not converge after \
                             {MAX_CONVERGENCE_ROUNDS} rounds"
                        )));
                    }
                    self.refresh_environment(state)?;
                    // The environment the last pass ran against is gone, so
                    // nothing it wrote is evidence of stability.
                    previous_auxiliary = None;
                    continue;
                }
            }
            if !run.status.success() {
                return Err(engine_failure(
                    self.project_root,
                    &self.manifest.project.entry,
                    self.toolchain,
                    &run,
                ));
            }
            pass_budget.record_stable_environment_pass();
            artifact = find_artifact(&self.output_dir, &self.manifest.project.entry);
            if self.process_auxiliary_tools(state, &run)? {
                // A processor just rewrote derived state. Compare the next pass
                // against that, not against what preceded it.
                previous_auxiliary = Some(auxiliary_snapshot(&self.output_dir)?);
                continue;
            }
            let auxiliary = auxiliary_snapshot(&self.output_dir)?;
            if previous_auxiliary.as_ref() == Some(&auxiliary) {
                return artifact.ok_or_else(|| {
                    TexeError::Build(format!(
                        "{} succeeded but produced no PDF or DVI in {}",
                        self.toolchain.engine,
                        self.output_dir.display()
                    ))
                });
            }
            previous_auxiliary = Some(auxiliary);
        }
    }

    fn create_run_trace(
        &self,
        state: &BuildState,
        run: &EngineRun,
        trace_path: &Path,
        discovery: bool,
    ) -> Result<(), TexeError> {
        trace::create(&trace::TraceRequest {
            project_root: self.project_root,
            tools: self.tools,
            toolchain: self.toolchain,
            environment_path: &self.environment_path,
            texmf: &self.texmf,
            output_dir: if discovery {
                &self.discovery_dir
            } else {
                &self.output_dir
            },
            log_path: &run.log_path,
            recorder_path: &run.fls_path,
            managed_format_root: state
                .managed_format
                .as_ref()
                .map(|format| format.root.as_path()),
            discovery,
            trace_path,
        })
    }

    fn process_auxiliary_tools(
        &self,
        state: &mut BuildState,
        run: &EngineRun,
    ) -> Result<bool, TexeError> {
        let bibliography_ran = bibliography::process_pending(
            &bibliography::BibliographyContext {
                project_root: self.project_root,
                manifest: self.manifest,
                toolchain: self.toolchain,
                texmf: &self.texmf,
                output_dir: &self.output_dir,
                recorder_path: &run.fls_path,
                progress: &self.progress,
            },
            &mut state.bibliography,
        )?;
        let index_ran = index::process_pending(
            self.project_root,
            self.manifest,
            self.toolchain,
            &self.texmf,
            &self.output_dir,
            &mut state.index,
            &self.progress,
        )?;
        Ok(bibliography_ran || index_ran)
    }

    fn refresh_environment(&self, state: &mut BuildState) -> Result<(), TexeError> {
        self.progress.phase(
            PhaseKind::Package,
            "fetching/materializing updated package environment",
            || {
                self.tools.install(
                    self.project_root,
                    self.manifest,
                    self.toolchain,
                    &self.lock,
                    &self.texmf,
                    &self.progress,
                )
            },
        )?;
        state.environment =
            self.tools
                .environment(self.project_root, &self.lock, &self.environment_path)?;
        let auxiliary_cache_key = auxiliary::cache_key(
            self.manifest,
            self.toolchain,
            &state.environment.fingerprint,
        )?;
        if auxiliary_cache_key != state.auxiliary_cache_key {
            state.bibliography = BibliographyState::default();
            state.index = IndexState::default();
            state.auxiliary_cache_key = auxiliary_cache_key;
        }
        state.managed_format = format::ensure(
            self.project_root,
            self.toolchain,
            &self.texmf,
            &state.environment,
            &self.build_root,
            self.manifest.packages.remote,
            &self.progress,
        )?;
        Ok(())
    }

    fn require_runtime_providers(
        &self,
        state: &mut BuildState,
        run: &EngineRun,
        frozen: bool,
    ) -> Result<bool, TexeError> {
        let providers = trace::runtime_provider_requirements(&run.log_path)
            .into_iter()
            .filter(|provider| !state.required_runtime_providers.contains(provider))
            .collect::<Vec<_>>();
        if providers.is_empty() {
            return Ok(false);
        }
        if frozen {
            return Err(TexeError::Build(format!(
                "frozen build requires additional runtime provider(s): {}",
                providers.join(", ")
            )));
        }
        state.convergence_rounds += 1;
        if state.convergence_rounds > MAX_CONVERGENCE_ROUNDS {
            return Err(TexeError::Build(format!(
                "package environment did not converge after {MAX_CONVERGENCE_ROUNDS} rounds"
            )));
        }
        self.progress.phase(
            PhaseKind::Package,
            format!("adding runtime provider(s): {}", providers.join(", ")),
            || {
                self.tools.require_providers(
                    self.project_root,
                    self.manifest,
                    self.toolchain,
                    &self.lock,
                    &providers,
                    &self.progress,
                )
            },
        )?;
        state.required_runtime_providers.extend(providers);
        self.refresh_environment(state)?;
        Ok(true)
    }

    fn reconcile_run(
        &self,
        run: &EngineRun,
        trace_path: &Path,
        frozen: bool,
    ) -> Result<Convergence, TexeError> {
        self.progress
            .phase(
                PhaseKind::Package,
                "reconciling runtime package trace",
                || {
                    self.tools.reconcile(&ReconcileRequest {
                        project_root: self.project_root,
                        manifest: self.manifest,
                        toolchain: self.toolchain,
                        lock: &self.lock,
                        trace: trace_path,
                        frozen,
                        progress: &self.progress,
                    })
                },
            )
            .map_err(|error| {
                package_error_with_engine_context(
                    error,
                    self.project_root,
                    &self.manifest.project.entry,
                    self.toolchain,
                    run,
                )
            })
    }
}

pub(crate) fn capture_render_sources(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<String, TexeError> {
    state::source_fingerprint(root, manifest)?.ok_or_else(|| TexeError::Build(
        "The saved paper exceeds the source verification limit. Sources and the last PDF have been kept.".into(),
    ))
}

pub(crate) fn verify_render_sources(
    root: &Path,
    manifest: &ProjectManifest,
    expected: &str,
) -> Result<(), TexeError> {
    match state::source_fingerprint(root, manifest) {
        Ok(Some(current)) if current == expected => Ok(()),
        _ => Err(TexeError::SourcesChanged),
    }
}

pub(crate) fn publish_if_sources_unchanged<T>(
    root: &Path,
    manifest: &ProjectManifest,
    expected: &str,
    publish: impl FnOnce() -> Result<T, TexeError>,
) -> Result<T, TexeError> {
    verify_render_sources(root, manifest, expected)?;
    publish()
}

fn record_if_sources_unchanged(
    root: &Path,
    manifest: &ProjectManifest,
    state_path: &Path,
    rendered_sources: Option<&str>,
    record: impl FnOnce() -> Result<(), TexeError>,
) -> Result<(), TexeError> {
    let current_sources = state::source_fingerprint(root, manifest).ok().flatten();
    if rendered_sources.is_none() || current_sources.as_deref() != rendered_sources {
        // Removing the previous record also prevents the queued retry from
        // reusing a prior fingerprint after an edit that happens to yield the
        // same PDF bytes. Failure to establish a snapshot is a safe cache miss.
        generated::remove(root, state_path)?;
        return Ok(());
    }
    let result = record();
    let current_sources = state::source_fingerprint(root, manifest).ok().flatten();
    if current_sources.as_deref() != rendered_sources {
        generated::remove(root, state_path)?;
    }
    result
}

#[cfg(test)]
mod tests;

pub(crate) fn materialize_portable_generated(
    output_dir: &Path,
    manifest: &ProjectManifest,
) -> Result<(), TexeError> {
    engine::write_generated_inputs(output_dir, &manifest.project.generated)
}

pub(crate) fn prepare_portable(
    root: &Path,
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    tools: &PqtyClient,
    descriptor: &serde_json::Value,
    progress: &Progress,
) -> Result<(), TexeError> {
    crate::portable::private_directory(root, &manifest.project.build_dir)?;
    crate::portable::private_directory(root, &manifest.packages.texmf)?;
    crate::portable::restore_package_lock_from_descriptor(root, manifest, descriptor)?;
    let lock = root.join(&manifest.packages.lock);
    let texmf = root.join(&manifest.packages.texmf);
    let build_root = root.join(&manifest.project.build_dir);
    std::fs::create_dir_all(&build_root).map_err(|source| TexeError::Io {
        path: build_root.clone(),
        source,
    })?;
    tools.install(root, manifest, toolchain, &lock, &texmf, progress)?;
    let environment = tools.environment(root, &lock, &build_root.join("pqty.env.json"))?;
    if environment.fingerprint != descriptor["packageFingerprint"].as_str().unwrap_or("") {
        return Err(TexeError::Build(
            "Portable environment fingerprint mismatch".into(),
        ));
    }
    if !matches!(toolchain.engine.as_str(), "shipout" | "tectonic") {
        format::ensure(
            root,
            toolchain,
            &texmf,
            &environment,
            &build_root,
            true,
            progress,
        )?;
    }
    Ok(())
}

pub(crate) fn publish_native(
    root: &Path,
    output: &Path,
    lock: &[u8],
    lock_path: &Path,
) -> Result<PathBuf, TexeError> {
    Ok(artifact::publish_artifact_at(root, output, lock, lock_path)?.artifact)
}
