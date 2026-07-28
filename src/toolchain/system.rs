use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        let engine_executable = resolve_executable(project_root, &request.engine)?;
        let kpsewhich_executable = resolve_executable(project_root, &request.kpsewhich)?;
        let texmf_dist = query_directory(&kpsewhich_executable, "TEXMFDIST")?;

        let mut roots = Vec::new();
        for root in [texmf_dist.join("fonts"), texmf_dist.join("web2c")] {
            if root.is_dir() {
                roots.push(root);
            }
        }
        for variable in ["TEXMFSYSVAR", "TEXMFSYSCONFIG", "TEXMFVAR", "TEXMFCONFIG"] {
            if let Ok(root) = query_directory(&kpsewhich_executable, variable) {
                roots.push(root);
            }
        }
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

fn query_directory(kpsewhich: &Path, variable: &str) -> Result<PathBuf, TexeError> {
    let output = Command::new(kpsewhich)
        .arg(format!("-var-value={variable}"))
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
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = PathBuf::from(&value);
    if value.is_empty() || !path.is_dir() {
        return Err(TexeError::Toolchain(format!(
            "{variable} from {} is not a directory: {value}",
            kpsewhich.display()
        )));
    }
    Ok(path)
}

pub fn executable_version(path: &Path) -> Result<String, TexeError> {
    let output = Command::new(path)
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
