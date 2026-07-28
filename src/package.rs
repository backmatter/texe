use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::build::process::{checked_output, raw_output_streaming};
use crate::config::ProjectManifest;
use crate::progress::Progress;
use crate::toolchain::{ResolvedToolchain, locked_format_bootstrap_providers, resolve_executable};

const CAPABILITIES_SCHEMA: &str = "pqty.capabilities/v1";
const LOCK_SCHEMA: &str = "pqty.lock/v1";
const ENVIRONMENT_SCHEMA: &str = "pqty.env/v1";
const TRACE_SCHEMA: &str = "pqty.trace/v1";
const TRACE_REPORT_SCHEMA: &str = "pqty.trace-report/v1";
const CONVERGENCE_REPORT_SCHEMA: &str = "pqty.convergence-report/v1";
const PROGRESS_SCHEMA: &str = "pqty.progress/v1";

#[derive(Debug, Clone)]
pub struct PqtyClient {
    pub manager: PathBuf,
    pub trace_adapter: PathBuf,
    fingerprint: String,
    offline: bool,
    cache_home: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    schema: String,
    version: String,
    lock_schema: String,
    environment_schema: String,
    trace_schema: String,
    trace_report_schema: String,
    convergence_report_schema: String,
    progress_schema: String,
}

impl Capabilities {
    fn validate(&self, manager: &Path) -> Result<(), TexeError> {
        if self.schema == CAPABILITIES_SCHEMA
            && self.lock_schema == LOCK_SCHEMA
            && self.environment_schema == ENVIRONMENT_SCHEMA
            && self.trace_schema == TRACE_SCHEMA
            && self.trace_report_schema == TRACE_REPORT_SCHEMA
            && self.convergence_report_schema == CONVERGENCE_REPORT_SCHEMA
            && self.progress_schema == PROGRESS_SCHEMA
            && !self.version.trim().is_empty()
        {
            return Ok(());
        }
        Err(TexeError::Build(format!(
            "incompatible pqty protocol from {}: expected {CAPABILITIES_SCHEMA} with \
             {LOCK_SCHEMA}, {ENVIRONMENT_SCHEMA}, {TRACE_SCHEMA}, \
             {TRACE_REPORT_SCHEMA}, {CONVERGENCE_REPORT_SCHEMA}, and {PROGRESS_SCHEMA}",
            manager.display()
        )))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PackageEnvironment {
    pub(crate) schema: String,
    pub(crate) fingerprint: String,
    #[serde(default)]
    pub(crate) font_maps: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Convergence {
    Stable,
    Changed,
}

pub(crate) struct EnsureLockRequest<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) manifest: &'a ProjectManifest,
    pub(crate) toolchain: &'a ResolvedToolchain,
    pub(crate) entry: &'a Path,
    pub(crate) lock: &'a Path,
    pub(crate) frozen: bool,
    pub(crate) progress: &'a Progress,
}

pub(crate) struct ReconcileRequest<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) manifest: &'a ProjectManifest,
    pub(crate) toolchain: &'a ResolvedToolchain,
    pub(crate) lock: &'a Path,
    pub(crate) trace: &'a Path,
    pub(crate) frozen: bool,
    pub(crate) progress: &'a Progress,
}

#[derive(Debug, Deserialize)]
struct ConvergenceReport {
    schema: String,
    status: String,
    #[serde(default)]
    unresolved: Vec<serde_json::Value>,
}

impl PqtyClient {
    pub fn resolve(
        project_root: &Path,
        manifest: &ProjectManifest,
        offline: bool,
    ) -> Result<Self, TexeError> {
        let require_bundled =
            manifest.toolchain.provider == "managed" && !manifest.uses_unmanaged_commands();
        let manager =
            resolve_suite_executable(project_root, &manifest.packages.manager, require_bundled)?;
        let trace_adapter = resolve_suite_executable(
            project_root,
            &manifest.packages.trace_adapter,
            require_bundled,
        )?;
        let capabilities = Self::inspect_capabilities(&manager, project_root)?;
        capabilities.validate(&manager)?;
        let client = Self {
            fingerprint: command_suite_fingerprint(&manager, &trace_adapter, &capabilities)?,
            manager,
            trace_adapter,
            offline,
            cache_home: crate::toolchain::texe_data_home()?,
        };
        Ok(client)
    }

    fn inspect_capabilities(
        manager: &Path,
        project_root: &Path,
    ) -> Result<Capabilities, TexeError> {
        let output = checked_output(
            manager,
            &[
                OsString::from("--no-config"),
                OsString::from("capabilities"),
            ],
            project_root,
            &[],
        )
        .map_err(|error| error.context("could not inspect pqty capabilities"))?;
        serde_json::from_slice(&output.stdout).map_err(|source| TexeError::Json {
            path: PathBuf::from("<pqty capabilities>"),
            source,
        })
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn ensure_lock(&self, request: &EnsureLockRequest<'_>) -> Result<(), TexeError> {
        let project_root = request.project_root;
        let manifest = request.manifest;
        let toolchain = request.toolchain;
        let entry = request.entry;
        let lock = request.lock;
        let frozen = request.frozen;
        let progress = request.progress;
        if frozen {
            if lock.is_file() {
                return Ok(());
            }
            return Err(TexeError::Build(format!(
                "--frozen requires an existing lock: {}",
                lock.display()
            )));
        }
        if let Some(parent) = lock.parent() {
            fs::create_dir_all(parent).map_err(|source| TexeError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let input_roots = package_input_roots(manifest);
        let mut arguments = Self::project_arguments("lock", project_root, entry, &input_roots)?;
        arguments.extend([OsString::from("--output"), lock.as_os_str().to_os_string()]);
        if let Some(managed) = &toolchain.managed {
            arguments.push(OsString::from("--tlpdb-url"));
            arguments.push(OsString::from(&managed.registry_url));
            arguments.push(OsString::from("--tlpdb-sha256"));
            arguments.push(OsString::from(&managed.registry_metadata_sha256));
            for provider in &managed.bootstrap_providers {
                arguments.push(OsString::from("--require-provider"));
                arguments.push(OsString::from(provider));
            }
        } else if manifest.packages.remote {
            arguments.push(OsString::from("--remote"));
            arguments.push(OsString::from("latest"));
            for provider in locked_format_bootstrap_providers(&toolchain.engine)? {
                arguments.push(OsString::from("--require-provider"));
                arguments.push(OsString::from(provider.as_str()));
            }
        }
        append_registry_transport_argument(&mut arguments, toolchain);
        append_store_argument(&mut arguments, project_root, manifest);
        self.checked_output_with_progress(&arguments, project_root, progress)
            .map(|_| ())
            .map_err(|error| error.context("could not create pqty lock"))
    }

    fn project_arguments(
        command: &str,
        project_root: &Path,
        entry: &Path,
        input_roots: &[PathBuf],
    ) -> Result<Vec<OsString>, TexeError> {
        let entry = entry.strip_prefix(project_root).map_err(|_| {
            TexeError::Build(format!(
                "project entry {} is outside {}",
                entry.display(),
                project_root.display()
            ))
        })?;
        let mut arguments = vec![
            OsString::from("--no-config"),
            OsString::from("--progress"),
            OsString::from("json"),
            OsString::from("--project-root"),
            project_root.as_os_str().to_os_string(),
        ];
        for root in input_roots {
            arguments.push(OsString::from("--input-root"));
            arguments.push(root.as_os_str().to_os_string());
        }
        arguments.extend([OsString::from(command), entry.as_os_str().to_os_string()]);
        Ok(arguments)
    }

    pub(crate) fn install(
        &self,
        project_root: &Path,
        manifest: &ProjectManifest,
        toolchain: &ResolvedToolchain,
        lock: &Path,
        texmf: &Path,
        progress: &Progress,
    ) -> Result<(), TexeError> {
        let mut arguments = vec![
            OsString::from("--no-config"),
            OsString::from("--progress"),
            OsString::from("json"),
            OsString::from("install"),
            OsString::from("--lock"),
            lock.as_os_str().to_os_string(),
            OsString::from("--out"),
            texmf.as_os_str().to_os_string(),
            OsString::from("--link"),
            OsString::from(&manifest.packages.link),
        ];
        append_registry_transport_argument(&mut arguments, toolchain);
        append_store_argument(&mut arguments, project_root, manifest);
        self.checked_output_with_progress(&arguments, project_root, progress)
            .map(|_| ())
            .map_err(|error| error.context("could not install pqty environment"))
    }

    pub(crate) fn environment(
        &self,
        project_root: &Path,
        lock: &Path,
        output_path: &Path,
    ) -> Result<PackageEnvironment, TexeError> {
        let mut arguments = vec![
            OsString::from("--no-config"),
            OsString::from("env"),
            OsString::from("--lock"),
            lock.as_os_str().to_os_string(),
        ];
        self.append_offline(&mut arguments);
        let environment = self.process_environment();
        let output = checked_output(&self.manager, &arguments, project_root, &environment)
            .map_err(|error| error.context("could not inspect pqty environment"))?;
        let environment: PackageEnvironment =
            serde_json::from_slice(&output.stdout).map_err(|source| TexeError::Json {
                path: output_path.to_path_buf(),
                source,
            })?;
        if environment.schema != ENVIRONMENT_SCHEMA {
            return Err(TexeError::Build(format!(
                "pqty emitted environment schema {}; expected {ENVIRONMENT_SCHEMA}",
                environment.schema
            )));
        }
        atomic_write(output_path, &output.stdout)?;
        Ok(environment)
    }

    pub(crate) fn reconcile(
        &self,
        request: &ReconcileRequest<'_>,
    ) -> Result<Convergence, TexeError> {
        let project_root = request.project_root;
        let manifest = request.manifest;
        let toolchain = request.toolchain;
        let lock = request.lock;
        let trace = request.trace;
        let frozen = request.frozen;
        let progress = request.progress;
        if frozen {
            let mut arguments = vec![
                OsString::from("--no-config"),
                OsString::from("check-trace"),
                OsString::from("--lock"),
                lock.as_os_str().to_os_string(),
                OsString::from("--trace"),
                trace.as_os_str().to_os_string(),
            ];
            self.append_offline(&mut arguments);
            let environment = self.process_environment();
            checked_output(&self.manager, &arguments, project_root, &environment)
                .map(|_| Convergence::Stable)
                .map_err(|error| error.context("frozen package trace does not match pqty.lock"))
        } else {
            let mut arguments = vec![
                OsString::from("--no-config"),
                OsString::from("--progress"),
                OsString::from("json"),
                OsString::from("converge"),
                OsString::from("--lock"),
                lock.as_os_str().to_os_string(),
                OsString::from("--trace"),
                trace.as_os_str().to_os_string(),
            ];
            append_registry_transport_argument(&mut arguments, toolchain);
            append_store_argument(&mut arguments, project_root, manifest);
            let output = self.raw_output_with_progress(&arguments, project_root, progress)?;
            let report: ConvergenceReport =
                serde_json::from_slice(&output.stdout).map_err(|source| {
                    if output.status.success() {
                        TexeError::Json {
                            path: trace.to_path_buf(),
                            source,
                        }
                    } else {
                        TexeError::Process {
                            tool: self.manager.clone(),
                            status: output.status.code(),
                            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                        }
                    }
                })?;
            if report.schema != CONVERGENCE_REPORT_SCHEMA {
                return Err(TexeError::Build(format!(
                    "pqty emitted convergence schema {}",
                    report.schema
                )));
            }
            match report.status.as_str() {
                "stable" if output.status.success() => Ok(Convergence::Stable),
                "changed" if output.status.success() => Ok(Convergence::Changed),
                "unresolved" => Err(TexeError::Build(format!(
                    "pqty could not resolve {} runtime package input(s){}",
                    report.unresolved.len(),
                    unresolved_runtime_input_summary(&report.unresolved)
                ))),
                status => Err(TexeError::Process {
                    tool: self.manager.clone(),
                    status: output.status.code(),
                    stderr: format!(
                        "unexpected convergence status {status}; {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                }),
            }
        }
    }

    pub(crate) fn require_providers(
        &self,
        project_root: &Path,
        manifest: &ProjectManifest,
        toolchain: &ResolvedToolchain,
        lock: &Path,
        providers: &[String],
        progress: &Progress,
    ) -> Result<(), TexeError> {
        if providers.is_empty() {
            return Ok(());
        }
        let mut arguments = vec![
            OsString::from("--no-config"),
            OsString::from("--progress"),
            OsString::from("json"),
            OsString::from("require"),
            OsString::from("--lock"),
            lock.as_os_str().to_os_string(),
        ];
        for provider in providers {
            arguments.push(OsString::from("--provider"));
            arguments.push(OsString::from(provider));
        }
        append_registry_transport_argument(&mut arguments, toolchain);
        append_store_argument(&mut arguments, project_root, manifest);
        self.checked_output_with_progress(&arguments, project_root, progress)
            .map(|_| ())
            .map_err(|error| error.context("could not add runtime package providers"))
    }

    fn raw_output_with_progress(
        &self,
        arguments: &[OsString],
        project_root: &Path,
        progress: &Progress,
    ) -> Result<std::process::Output, TexeError> {
        let mut arguments = arguments.to_vec();
        self.append_offline(&mut arguments);
        let environment = self.process_environment();
        raw_output_streaming(
            &self.manager,
            &arguments,
            project_root,
            &environment,
            |line| progress.handle_pqty_line(line),
        )
    }

    fn checked_output_with_progress(
        &self,
        arguments: &[OsString],
        project_root: &Path,
        progress: &Progress,
    ) -> Result<std::process::Output, TexeError> {
        let output = self.raw_output_with_progress(arguments, project_root, progress)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(TexeError::Process {
                tool: self.manager.clone(),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    fn append_offline(&self, arguments: &mut Vec<OsString>) {
        if self.offline && !arguments.iter().any(|argument| argument == "--offline") {
            arguments.insert(0, OsString::from("--offline"));
        }
    }

    /// pqty follows the XDG cache convention on every supported platform.
    /// Giving the child process texe's data home makes its default `pqty/`
    /// directory part of the same owned, inspectable storage tree as managed
    /// runtimes and downloads.
    fn process_environment(&self) -> [(OsString, OsString); 1] {
        [(
            OsString::from("XDG_CACHE_HOME"),
            self.cache_home.as_os_str().to_os_string(),
        )]
    }
}

fn package_input_roots(manifest: &ProjectManifest) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in manifest
        .inputs
        .roots
        .iter()
        .chain(&manifest.bibliography.roots)
    {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    roots
}

fn unresolved_runtime_input_summary(unresolved: &[serde_json::Value]) -> String {
    let details = unresolved
        .iter()
        .take(8)
        .filter_map(|input| {
            let requested = input.get("requested")?.as_str()?;
            let reason = input
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            Some(format!("{requested} ({reason})"))
        })
        .collect::<Vec<_>>();
    if details.is_empty() {
        return String::new();
    }
    let remainder = unresolved.len().saturating_sub(details.len());
    format!(
        ": {}{}",
        details.join(", "),
        if remainder == 0 {
            String::new()
        } else {
            format!(", and {remainder} more")
        }
    )
}

fn resolve_suite_executable(
    project_root: &Path,
    command: &str,
    require_bundled: bool,
) -> Result<PathBuf, TexeError> {
    let configured = Path::new(command);
    if configured.components().count() == 1
        && !configured.is_absolute()
        && let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return Ok(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = directory.join(format!("{command}.exe"));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if require_bundled {
        return Err(TexeError::ToolNotFound(format!(
            "{command} beside the texe executable; install the complete texe command suite"
        )));
    }
    resolve_executable(project_root, command)
}

fn command_suite_fingerprint(
    manager: &Path,
    trace_adapter: &Path,
    capabilities: &Capabilities,
) -> Result<String, TexeError> {
    let mut hasher = Sha256::new();
    hasher.update(b"texe.command-suite-fingerprint/v1");
    hasher.update(
        serde_json::to_vec(capabilities).map_err(|source| TexeError::Json {
            path: PathBuf::from("<pqty capabilities>"),
            source,
        })?,
    );
    hash_executable(&mut hasher, manager)?;
    hash_executable(&mut hasher, trace_adapter)?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn hash_executable(hasher: &mut Sha256, path: &Path) -> Result<(), TexeError> {
    let identity_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    hasher.update(identity_path.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    let mut file = fs::File::open(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hasher.update(b"\0");
    Ok(())
}

fn append_store_argument(
    arguments: &mut Vec<OsString>,
    project_root: &Path,
    manifest: &ProjectManifest,
) {
    if let Some(store) = &manifest.packages.store {
        arguments.push(OsString::from("--store"));
        arguments.push(project_root.join(store).into_os_string());
    }
}

fn append_registry_transport_argument(
    arguments: &mut Vec<OsString>,
    toolchain: &ResolvedToolchain,
) {
    if toolchain
        .managed
        .as_ref()
        .is_some_and(|managed| managed.registry_url.starts_with("http://"))
    {
        arguments.push(OsString::from("--allow-insecure-registry"));
    }
}

trait ErrorContext {
    fn context(self, context: &str) -> TexeError;
}

impl ErrorContext for TexeError {
    fn context(self, context: &str) -> TexeError {
        match self {
            TexeError::Process {
                tool,
                status,
                stderr,
            } => TexeError::Process {
                tool,
                status,
                stderr: if stderr.is_empty() {
                    context.to_string()
                } else {
                    format!("{context}: {stderr}")
                },
            },
            error => TexeError::Build(format!("{context}: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use crate::ProjectManifest;
    use crate::package::{
        CAPABILITIES_SCHEMA, CONVERGENCE_REPORT_SCHEMA, Capabilities, ENVIRONMENT_SCHEMA,
        LOCK_SCHEMA, PROGRESS_SCHEMA, PqtyClient, TRACE_REPORT_SCHEMA, TRACE_SCHEMA,
        package_input_roots, unresolved_runtime_input_summary,
    };

    #[test]
    fn unresolved_runtime_inputs_name_the_actionable_requests() {
        let unresolved = vec![
            serde_json::json!({
                "requested": "Version.tex",
                "reason": "no-provider"
            }),
            serde_json::json!({
                "requested": "example.sty",
                "reason": "ambiguous-provider"
            }),
        ];

        assert_eq!(
            unresolved_runtime_input_summary(&unresolved),
            ": Version.tex (no-provider), example.sty (ambiguous-provider)"
        );
    }

    #[test]
    fn project_commands_are_isolated_and_project_root_relative() {
        let arguments = PqtyClient::project_arguments(
            "scan",
            Path::new("/work/project"),
            Path::new("/work/project/paper/main.tex"),
            &[PathBuf::from("vendor/natbib")],
        )
        .expect("project arguments");
        assert_eq!(
            arguments,
            [
                "--no-config",
                "--progress",
                "json",
                "--project-root",
                "/work/project",
                "--input-root",
                "vendor/natbib",
                "scan",
                "paper/main.tex"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn project_commands_reject_an_entry_outside_the_project() {
        assert!(
            PqtyClient::project_arguments(
                "lock",
                Path::new("/work/project"),
                Path::new("/work/other/main.tex"),
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn pqty_processes_use_the_texe_owned_cache_home() {
        let client = PqtyClient {
            manager: PathBuf::from("/suite/pqty"),
            trace_adapter: PathBuf::from("/suite/pqty-fls"),
            fingerprint: "sha256:test".to_string(),
            offline: false,
            cache_home: PathBuf::from("/managed/texe"),
        };

        assert_eq!(
            client.process_environment(),
            [(
                OsString::from("XDG_CACHE_HOME"),
                OsString::from("/managed/texe")
            )]
        );
    }

    #[test]
    fn package_input_roots_preserve_order_and_deduplicate_across_sections() {
        let manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"

[project]
entry = "main.tex"

[toolchain]
engine = "pdflatex"

[inputs]
roots = ["styles", "shared"]

[bibliography]
roots = ["shared", "bibliography"]
"#,
        )
        .expect("manifest");

        assert_eq!(
            package_input_roots(&manifest),
            [
                PathBuf::from("styles"),
                PathBuf::from("shared"),
                PathBuf::from("bibliography")
            ]
        );
    }

    #[test]
    fn capabilities_require_every_consumed_schema() {
        let compatible = Capabilities {
            schema: CAPABILITIES_SCHEMA.to_string(),
            version: "0.1.0".to_string(),
            lock_schema: LOCK_SCHEMA.to_string(),
            environment_schema: ENVIRONMENT_SCHEMA.to_string(),
            trace_schema: TRACE_SCHEMA.to_string(),
            trace_report_schema: TRACE_REPORT_SCHEMA.to_string(),
            convergence_report_schema: CONVERGENCE_REPORT_SCHEMA.to_string(),
            progress_schema: PROGRESS_SCHEMA.to_string(),
        };
        assert!(compatible.validate(Path::new("/suite/pqty")).is_ok());

        let incompatible = Capabilities {
            trace_schema: "pqty.trace/v2".to_string(),
            ..compatible
        };
        assert!(incompatible.validate(Path::new("/suite/pqty")).is_err());
    }
}
