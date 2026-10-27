use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::config::ToolchainConfig;
use crate::toolchain::catalog;
use crate::toolchain::component::biber_artifact_identities;
use crate::toolchain::{
    ResolvedToolchain, ToolchainIdentity, ToolchainProvider, VerificationPolicy,
};

#[derive(Debug, Default)]
pub struct SystemToolchainProvider;

impl ToolchainProvider for SystemToolchainProvider {
    fn resolve(
        &self,
        project_root: &Path,
        request: &ToolchainConfig,
        verification: VerificationPolicy,
        offline: bool,
    ) -> Result<ResolvedToolchain, TexeError> {
        if request.provider != "system" {
            return Err(TexeError::Toolchain(format!(
                "system provider cannot resolve provider {}",
                request.provider
            )));
        }
        if request.adapter != "kpathsea" {
            return Err(TexeError::Toolchain(format!(
                "engine adapter {} is not installed; this release provides `kpathsea`",
                request.adapter
            )));
        }
        let mut engine_executable = resolve_executable(project_root, &request.engine)?;
        let mut kpsewhich_executable = resolve_executable(project_root, &request.kpsewhich)?;
        if crate::build::process::protection_requested() {
            engine_executable = protected_executable(project_root, &engine_executable, &[])?;
            kpsewhich_executable = protected_executable(project_root, &kpsewhich_executable, &[])?;
        }
        let (texmf_dist, configuration_roots) = query_directories(&kpsewhich_executable)?;

        let mut roots = Vec::new();
        for root in [texmf_dist.join("fonts"), texmf_dist.join("web2c")] {
            if root.is_dir() {
                roots.push(root);
            }
        }
        roots.extend(configuration_roots);
        roots.extend(system_font_roots());
        let mut seen = BTreeSet::new();
        roots.retain(|root| seen.insert(root.clone()));

        let version = executable_version(&engine_executable)?;
        let mut hasher = Sha256::new();
        hasher.update(request.engine.as_bytes());
        hasher.update(engine_executable.as_os_str().as_encoded_bytes());
        hasher.update(version.as_bytes());
        let biber_selection = catalog::select_platform("stable")?;
        let biber = &biber_selection.platform.biber;
        hasher.update(biber.provider.as_bytes());
        hasher.update(biber.sha512.as_bytes());
        hasher.update(biber_selection.snapshot.biber.component_recipe.as_bytes());
        let target = format!("{}-{}", env::consts::ARCH, env::consts::OS);
        Ok(ResolvedToolchain {
            provider: request.provider.clone(),
            engine: request.engine.clone(),
            engine_executable,
            kpsewhich_executable,
            texmf_dist,
            engine_roots: roots,
            identity: ToolchainIdentity {
                provider: "system".to_string(),
                engine: request.engine.clone(),
                channel: "system".to_string(),
                target,
                fingerprint: hex::encode(hasher.finalize()),
                registry_url: None,
                registry_metadata_digest: None,
                artifacts: biber_artifact_identities(biber_selection),
            },
            managed: None,
            verification,
            offline,
        })
    }
}

fn system_font_roots() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/etc/fonts"),
        PathBuf::from("/usr/local/etc/fonts"),
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/Library/Fonts"),
        PathBuf::from("/System/Library/Fonts"),
    ];
    if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
        let home = PathBuf::from(home);
        candidates.extend([
            home.join(".fonts"),
            home.join(".config/fontconfig"),
            home.join(".local/share/fonts"),
        ]);
    }
    if let Some(config) = env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        candidates.push(PathBuf::from(config).join("fontconfig"));
    }
    if let Some(data) = env::var_os("XDG_DATA_HOME").filter(|path| !path.is_empty()) {
        candidates.push(PathBuf::from(data).join("fonts"));
    }
    if let Some(windows) = env::var_os("WINDIR").filter(|path| !path.is_empty()) {
        candidates.push(PathBuf::from(windows).join("Fonts"));
    }
    candidates.retain(|path| path.is_dir());
    candidates
}

pub fn resolve_executable(project_root: &Path, command: &str) -> Result<PathBuf, TexeError> {
    let configured = Path::new(command);
    if configured.components().count() > 1 || configured.is_absolute() {
        let candidate = if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            project_root.join(configured)
        };
        return candidate
            .is_file()
            .then_some(candidate)
            .ok_or_else(|| TexeError::ToolNotFound(command.to_string()));
    }
    let path = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path) {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return Ok(candidate);
        }
        #[cfg(windows)]
        {
            let executable = directory.join(format!("{command}.exe"));
            if executable.is_file() {
                return Ok(executable);
            }
        }
    }
    Err(TexeError::ToolNotFound(command.to_string()))
}

/// Version and Kpathsea probes run before the compiler sandbox is available.
/// Installed tools are trusted; a paper must not supply code for these probes.
pub(crate) fn protected_executable(
    project_root: &Path,
    executable: &Path,
    source_roots: &[PathBuf],
) -> Result<PathBuf, TexeError> {
    let resolved = executable.canonicalize().map_err(|source| TexeError::Io {
        path: executable.to_owned(),
        source,
    })?;
    let lexical = if executable.is_absolute() {
        executable.to_owned()
    } else {
        env::current_dir()
            .map_err(|source| TexeError::Io {
                path: PathBuf::from("."),
                source,
            })?
            .join(executable)
    };
    let project = project_root
        .canonicalize()
        .map_err(|source| TexeError::Io {
            path: project_root.to_owned(),
            source,
        })?;
    for root in std::iter::once(&project).chain(source_roots) {
        if lexical.starts_with(root) || resolved.starts_with(root) {
            return Err(TexeError::Toolchain(
                "Protected builds cannot execute build or lookup tools stored in the paper or \
                 its declared sources. Select installed TeX and package tools outside those folders."
                    .into(),
            ));
        }
    }
    Ok(resolved)
}

pub(crate) fn validate_protected_tools(
    project_root: &Path,
    manifest: &crate::config::ProjectManifest,
) -> Result<(), TexeError> {
    if !crate::build::process::protection_requested() {
        return Ok(());
    }
    // Validate both before either probe is started.
    for command in [&manifest.toolchain.engine, &manifest.toolchain.kpsewhich] {
        let executable = resolve_executable(project_root, command)?;
        protected_manifest_executable(project_root, manifest, &executable)?;
    }
    Ok(())
}

pub(crate) fn protected_manifest_executable(
    project_root: &Path,
    manifest: &crate::config::ProjectManifest,
    executable: &Path,
) -> Result<PathBuf, TexeError> {
    if !crate::build::process::protection_requested() {
        return Ok(executable.to_owned());
    }
    let sources = std::iter::once(&manifest.project.entry)
        .chain(manifest.inputs.roots.iter())
        .chain(manifest.bibliography.roots.iter())
        .map(|path| {
            let path = project_root.join(path);
            path.canonicalize()
                .map_err(|source| TexeError::Io { path, source })
        })
        .collect::<Result<Vec<_>, _>>()?;
    protected_executable(project_root, executable, &sources)
}

fn query_directories(kpsewhich: &Path) -> Result<(PathBuf, Vec<PathBuf>), TexeError> {
    // One Kpathsea initialization per build, rather than one per variable.
    // Newline delimiters preserve spaces and platform path-list separators.
    const QUERY: &str =
        "-expand-var=$TEXMFDIST\n$TEXMFSYSVAR\n$TEXMFSYSCONFIG\n$TEXMFVAR\n$TEXMFCONFIG";
    let output = crate::process::command(kpsewhich)
        .arg(QUERY)
        .output()
        .map_err(|source| TexeError::Spawn {
            tool: kpsewhich.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(TexeError::Process {
            tool: kpsewhich.to_path_buf(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    parse_directories(kpsewhich, &String::from_utf8_lossy(&output.stdout))
}

fn parse_directories(kpsewhich: &Path, output: &str) -> Result<(PathBuf, Vec<PathBuf>), TexeError> {
    let values = output.lines().map(str::trim).collect::<Vec<_>>();
    if values.len() != 5 {
        return Err(TexeError::Toolchain(format!(
            "{} returned an invalid directory query response",
            kpsewhich.display()
        )));
    }
    let distribution = PathBuf::from(values[0]);
    if values[0].is_empty() || !distribution.is_dir() {
        return Err(TexeError::Toolchain(format!(
            "TEXMFDIST from {} is not a directory: {}",
            kpsewhich.display(),
            values[0]
        )));
    }
    let roots = values[1..]
        .iter()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .collect();
    Ok((distribution, roots))
}

pub fn executable_version(path: &Path) -> Result<String, TexeError> {
    let output = crate::process::command(path)
        .arg(OsStr::new("--version"))
        .output()
        .map_err(|source| TexeError::Spawn {
            tool: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(TexeError::Process {
            tool: path.to_path_buf(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_directories;
    use std::path::Path;

    #[test]
    fn directory_query_preserves_spaces_and_ignores_missing_optional_roots() {
        let scratch = tempfile::tempdir().unwrap();
        let distribution = scratch.path().join("TeX distribution");
        let config = scratch.path().join("system config");
        std::fs::create_dir(&distribution).unwrap();
        std::fs::create_dir(&config).unwrap();
        let output = format!(
            "{}\r\n{}\r\n\r\n$TEXMFVAR\r\n{}\r\n",
            distribution.display(),
            config.display(),
            scratch.path().join("absent").display()
        );
        let (actual, roots) = parse_directories(Path::new("kpsewhich"), &output).unwrap();
        assert_eq!(actual, distribution);
        assert_eq!(roots, vec![config]);
    }

    #[test]
    fn directory_query_rejects_missing_distribution_and_malformed_output() {
        for output in ["\n\n\n\n\n", "one line\n", "$TEXMFDIST\n\n\n\n\n"] {
            assert!(parse_directories(Path::new("kpsewhich"), output).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::too_many_lines)]
    fn protected_tool_probes_reject_source_executables_before_running_either_probe() {
        use crate::toolchain::ToolchainProvider as _;
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("paper");
        let sources = scratch.path().join("shared-sources");
        let installed = scratch.path().join("installed");
        for directory in [&root, &sources, &installed] {
            std::fs::create_dir(directory).unwrap();
        }
        std::fs::write(root.join("main.tex"), "researcher source").unwrap();
        let marker = root.join("probe-executed");
        let source = root.join("main.tex");
        let script = format!(
            "#!/bin/sh\nprintf corrupted > '{}'\nprintf executed > '{}'\n",
            source.display(),
            marker.display(),
        );
        let paper_tool = root.join("custom-engine");
        let source_tool = sources.join("custom-engine");
        let installed_tool = installed.join("tex-engine");
        for path in [&paper_tool, &source_tool, &installed_tool] {
            std::fs::write(path, &script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        symlink(&sources, root.join("shared")).unwrap();
        symlink(&paper_tool, installed.join("paper-alias")).unwrap();
        symlink(&installed_tool, root.join("installed-alias")).unwrap();
        let mut manifest: crate::config::ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
provider = "system"
engine = "pdflatex"
[inputs]
roots = ["shared"]
"#,
        )
        .unwrap();
        let _scope = crate::build::process::ProtectionScope::enter(true);
        for rejected in [
            paper_tool.clone(),
            source_tool,
            installed.join("paper-alias"),
            root.join("installed-alias"),
        ] {
            for reject_engine in [true, false] {
                manifest.toolchain.engine = if reject_engine {
                    &rejected
                } else {
                    &installed_tool
                }
                .to_string_lossy()
                .into_owned();
                manifest.toolchain.kpsewhich = if reject_engine {
                    &installed_tool
                } else {
                    &rejected
                }
                .to_string_lossy()
                .into_owned();
                let error = super::validate_protected_tools(&root, &manifest).unwrap_err();
                assert!(error.to_string().contains("declared sources"));
                assert!(!marker.exists(), "neither probe may be started");
                assert_eq!(
                    std::fs::read_to_string(&source).unwrap(),
                    "researcher source"
                );
            }
        }
        manifest.toolchain.engine = paper_tool.to_string_lossy().into_owned();
        manifest.toolchain.kpsewhich = installed_tool.to_string_lossy().into_owned();
        assert!(
            super::SystemToolchainProvider
                .resolve(
                    &root,
                    &manifest.toolchain,
                    crate::toolchain::VerificationPolicy::Interval,
                    true,
                )
                .is_err()
        );
        assert!(
            !marker.exists(),
            "direct protected provider calls must also reject paper tools"
        );
        assert_eq!(
            super::protected_manifest_executable(&root, &manifest, &installed_tool).unwrap(),
            installed_tool
        );
        manifest.packages.manager = paper_tool.to_string_lossy().into_owned();
        manifest.packages.trace_adapter = installed_tool.to_string_lossy().into_owned();
        assert!(crate::package::PqtyClient::resolve(&root, &manifest, true).is_err());
        assert!(
            !marker.exists(),
            "the pqty capabilities probe must reject paper tools"
        );
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "researcher source"
        );
    }
}
