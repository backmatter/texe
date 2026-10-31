//! Building with Shipout.
//!
//! Shipout is a compiler in the way rustc is: given a document and its package
//! tree it runs the passes, bibliography, and index stages it needs and reports
//! what it read as a `pqty.trace/v1` document. So texe drives it once rather
//! than sequencing passes, and only reconciles the trace with the package lock.
//!
//! `texe watch` keeps one `shipout serve` process, whose warm engine sessions
//! resume from the checkpoint before an edit instead of starting over.
#![allow(clippy::cast_possible_truncation)]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::TexeError;
use crate::build::errors::engine_failure;
use crate::build::filesystem::{find_artifact, job_stem};
use crate::build::process::{
    is_portable_build, raw_output, source_date_environment, source_read_roots,
};
use crate::build::warnings::remove_if_file;
use crate::build::{BuildContext, BuildState, EngineRun, MAX_CONVERGENCE_ROUNDS};
use crate::package::Convergence;
use crate::progress::PhaseKind;

static KEEP_ENGINE_WARM: AtomicBool = AtomicBool::new(false);
static SERVER: Mutex<Option<Server>> = Mutex::new(None);
/// What the last full build established, for edits that cannot affect it.
static PREPARED: Mutex<Option<PreparedBuild>> = Mutex::new(None);

/// Packages are declared in the preamble, so while it is unchanged the locked
/// environment is too and an edit can go straight to the warm engine.
#[derive(Clone)]
pub(super) struct PreparedBuild {
    preamble: Vec<u8>,
    environment_fingerprint: String,
    project_lock: Vec<u8>,
}

/// The entry file up to `\\begin{document}`.
fn preamble(entry: &Path) -> Option<Vec<u8>> {
    let source = std::fs::read(entry).ok()?;
    let marker = b"\\begin{document}";
    let end = source
        .windows(marker.len())
        .position(|window| window == marker)?;
    Some(source[..end].to_vec())
}

/// Keep a Shipout server for the life of this process.
pub(crate) fn keep_engine_warm() {
    KEEP_ENGINE_WARM.store(true, Ordering::Relaxed);
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start(executable: &Path, environment: &[(OsString, OsString)]) -> Option<Self> {
        let mut child = Command::new(executable)
            .arg("serve")
            .envs(environment.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);
        Some(Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        })
    }

    fn compile(&mut self, arguments: &[String], cwd: &Path) -> Option<Value> {
        self.next_id += 1;
        let request = json!({
            "id": self.next_id,
            "method": "compile",
            "params": {"args": arguments, "cwd": cwd},
        });
        writeln!(self.stdin, "{request}").ok()?;
        self.stdin.flush().ok()?;
        let mut line = String::new();
        self.stdout.read_line(&mut line).ok()?;
        serde_json::from_str::<Value>(&line)
            .ok()?
            .get("result")
            .cloned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl PreparedBuild {
    pub(super) fn environment_fingerprint(&self) -> &str {
        &self.environment_fingerprint
    }

    pub(super) fn project_lock(&self) -> &[u8] {
        &self.project_lock
    }
}

impl BuildContext<'_> {
    /// Compile, reconcile packages with what the build read, and repeat while
    /// that changes the package environment.
    pub(super) fn shipout_build(
        &self,
        state: &mut BuildState,
        frozen: bool,
    ) -> Result<PathBuf, TexeError> {
        self.materialize_generated_inputs(&self.output_dir)?;
        let stem = job_stem(&self.manifest.project.entry)?;
        let output = self.output_dir.join(format!("{stem}.pdf"));
        if let Some(passes) = self.warm_compile()?
            && output.is_file()
        {
            state.engine_passes += passes;
            return find_artifact(&self.output_dir, &self.manifest.project.entry)
                .ok_or_else(|| TexeError::Build("Shipout produced no PDF".into()));
        }
        let trace_path = self.build_root.join("frozen.trace.json");
        for round in 1..=MAX_CONVERGENCE_ROUNDS {
            remove_if_file(&output)?;
            let run = self.progress.phase(
                PhaseKind::EngineFinal,
                format!("compiling with Shipout ({round}/{MAX_CONVERGENCE_ROUNDS})"),
                || self.compile_once(&trace_path),
            )?;
            state.engine_passes += reported_passes(&run.stdout);
            match self.reconcile_run(&run, &trace_path, frozen)? {
                Convergence::Changed => {
                    state.convergence_rounds += 1;
                    self.refresh_environment(state)?;
                }
                Convergence::Stable if run.status.success() => {
                    return find_artifact(&self.output_dir, &self.manifest.project.entry)
                        .ok_or_else(|| TexeError::Build("Shipout produced no PDF".into()));
                }
                Convergence::Stable => {
                    return Err(engine_failure(
                        self.project_root,
                        &self.manifest.project.entry,
                        self.toolchain,
                        &run,
                    ));
                }
            }
        }
        Err(TexeError::Build(format!(
            "package environment did not converge after {MAX_CONVERGENCE_ROUNDS} rounds"
        )))
    }

    /// Remember what a completed full build established.
    pub(super) fn record_prepared_build(&self, fingerprint: &str, project_lock: &[u8]) {
        if !KEEP_ENGINE_WARM.load(Ordering::Relaxed) {
            return;
        }
        let prepared = preamble(&self.entry).map(|preamble| PreparedBuild {
            preamble,
            environment_fingerprint: fingerprint.to_string(),
            project_lock: project_lock.to_vec(),
        });
        if let Ok(mut slot) = PREPARED.lock() {
            *slot = prepared;
        }
    }

    /// Compile through the warm server when nothing a package step would look
    /// at has changed. `None` means take the full path, which also reconciles
    /// any failure: a missing package needs a trace, not just a report.
    pub(super) fn warm_edit(&self) -> Result<Option<(PreparedBuild, usize)>, TexeError> {
        let prepared = PREPARED.lock().ok().and_then(|slot| slot.clone());
        let Some(prepared) = prepared else {
            return Ok(None);
        };
        if preamble(&self.entry).as_deref() != Some(prepared.preamble.as_slice()) {
            return Ok(None);
        }
        self.materialize_generated_inputs(&self.output_dir)?;
        Ok(self.warm_compile()?.map(|passes| (prepared, passes)))
    }

    /// Compile through the warm server, returning the passes it ran.
    fn warm_compile(&self) -> Result<Option<usize>, TexeError> {
        // A long-lived server cannot be confined to one build's policy, so a
        // protected build always takes the sandboxed path in `compile_once`.
        if !KEEP_ENGINE_WARM.load(Ordering::Relaxed) || self.confined() {
            return Ok(None);
        }
        let mut arguments = self.shipout_arguments()?;
        arguments.retain(|argument| argument != "--json");
        let environment = self.shipout_environment();
        let mut server = SERVER
            .lock()
            .map_err(|_| TexeError::Build("engine lock".into()))?;
        if server.is_none() {
            *server = Server::start(&self.toolchain.engine_executable, &environment);
        }
        let result = server
            .as_mut()
            .and_then(|server| server.compile(&arguments, self.project_root));
        let Some(result) = result else {
            *server = None;
            return Ok(None);
        };
        if result["status"] != "success" {
            return Ok(None);
        }
        Ok(Some(result["passes"].as_u64().unwrap_or(0) as usize))
    }

    fn compile_once(&self, trace_path: &Path) -> Result<EngineRun, TexeError> {
        let mut arguments = vec![OsString::from("compile")];
        arguments.extend(self.shipout_arguments()?.into_iter().map(OsString::from));
        arguments.push("--trace".into());
        arguments.push(trace_path.as_os_str().to_owned());
        let environment = self.shipout_environment();
        let tool = &self.toolchain.engine_executable;
        let output = if self.confined() {
            let (reads, executes) = self.sandbox_roots()?;
            let policy = crate::compiler_sandbox::Policy::new(
                self.project_root,
                std::slice::from_ref(&self.build_root),
                &reads,
                &executes,
            )?
            .with_shipout_arena(true);
            crate::compiler_sandbox::run(
                tool,
                &arguments,
                self.project_root,
                &environment,
                &policy,
            )?
        } else {
            raw_output(tool, &arguments, self.project_root, &environment)?
        };
        let stem = job_stem(&self.manifest.project.entry)?;
        Ok(EngineRun {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            log_path: self.output_dir.join(format!("{stem}.log")),
            fls_path: self.output_dir.join(format!("{stem}.fls")),
        })
    }

    fn confined(&self) -> bool {
        is_portable_build(self.project_root, &self.build_root)
    }

    /// Shipout carries its kernel and fonts next to its binary, so the sandbox
    /// has to let it read its own install, including through a symbolic link.
    fn sandbox_roots(&self) -> Result<(Vec<PathBuf>, Vec<PathBuf>), TexeError> {
        let tool = &self.toolchain.engine_executable;
        let installed = tool.canonicalize().map_err(|source| TexeError::Io {
            path: tool.clone(),
            source,
        })?;
        let bin = installed
            .parent()
            .ok_or_else(|| TexeError::Build("Shipout has no install directory".into()))?;
        let mut reads = source_read_roots(self.project_root, self.manifest)?;
        reads.extend(
            bin.parent()
                .unwrap_or(bin)
                .join("share/shipout")
                .canonicalize(),
        );
        reads.push(bin.to_owned());
        Ok((reads, vec![bin.to_owned()]))
    }

    fn shipout_environment(&self) -> Vec<(OsString, OsString)> {
        source_date_environment(&self.timestamp.effective.to_string()).to_vec()
    }

    fn shipout_arguments(&self) -> Result<Vec<String>, TexeError> {
        let text = |path: &Path| path.to_string_lossy().into_owned();
        let stem = job_stem(&self.manifest.project.entry)?;
        let mut arguments = vec![
            text(&self.manifest.project.entry),
            "--output".to_string(),
            text(&self.output_dir.join(format!("{stem}.pdf"))),
            "--out-dir".to_string(),
            text(&self.output_dir),
            "--texmf".to_string(),
            text(&self.texmf),
            "--synctex".to_string(),
            "--json".to_string(),
        ];
        let roots = std::iter::once(self.output_dir.join(".texe-generated"))
            .chain(
                self.manifest
                    .inputs
                    .roots
                    .iter()
                    .map(|root| self.project_root.join(root)),
            )
            .chain(
                self.manifest
                    .bibliography
                    .roots
                    .iter()
                    .map(|root| self.project_root.join(root)),
            );
        for root in roots {
            arguments.push("--input-root".to_string());
            arguments.push(text(&root));
        }
        Ok(arguments)
    }
}

fn reported_passes(stdout: &[u8]) -> usize {
    serde_json::from_slice::<Value>(stdout)
        .ok()
        .and_then(|result| result["passes"].as_u64())
        .map_or(0, |passes| passes as usize)
}
