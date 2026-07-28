use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::config::{self, ProjectManifest};
use crate::package::PqtyClient;
use crate::toolchain::{
    ManagedToolchainProvider, ResolvedToolchain, SystemToolchainProvider, ToolchainProvider,
    VerificationPolicy,
};

pub(super) struct ProjectContext {
    pub(super) root: PathBuf,
    pub(super) manifest_path: PathBuf,
    pub(super) manifest: ProjectManifest,
}

pub(super) fn load_project(argument: Option<&Path>) -> Result<ProjectContext, TexeError> {
    let manifest_path = config::resolve_manifest(argument)?;
    let manifest_path = fs::canonicalize(&manifest_path).map_err(|source| TexeError::Io {
        path: manifest_path,
        source,
    })?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| {
            TexeError::Manifest(format!(
                "{} has no parent directory",
                manifest_path.display()
            ))
        })?
        .to_path_buf();
    let manifest = ProjectManifest::load(&manifest_path)?;
    Ok(ProjectContext {
        root,
        manifest_path,
        manifest,
    })
}

pub(super) const fn verification_policy(deep: bool) -> VerificationPolicy {
    if deep {
        VerificationPolicy::Deep
    } else {
        VerificationPolicy::Interval
    }
}

pub(super) fn resolve_components(
    context: &ProjectContext,
    verification: VerificationPolicy,
    offline: bool,
) -> Result<(ResolvedToolchain, PqtyClient), TexeError> {
    let toolchain = match context.manifest.toolchain.provider.as_str() {
        "managed" => ManagedToolchainProvider.resolve(
            &context.root,
            &context.manifest.toolchain,
            verification,
            offline,
        )?,
        "system" => SystemToolchainProvider.resolve(
            &context.root,
            &context.manifest.toolchain,
            verification,
            offline,
        )?,
        provider => {
            return Err(TexeError::Toolchain(format!(
                "toolchain provider `{provider}` is not installed; available providers: managed, \
                 system"
            )));
        }
    };
    let tools = PqtyClient::resolve(&context.root, &context.manifest, offline)?;
    Ok((toolchain, tools))
}
