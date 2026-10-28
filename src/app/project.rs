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
    let current = std::env::current_dir().map_err(|source| TexeError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let argument = argument.unwrap_or(&current);
    let start = if argument.is_dir() {
        argument.to_owned()
    } else {
        argument.parent().unwrap_or(Path::new(".")).to_owned()
    };
    let start = fs::canonicalize(&start).map_err(|source| TexeError::Io {
        path: start.clone(),
        source,
    })?;
    for root in start.ancestors() {
        if let Some(environment) = crate::portable::active(root)? {
            return Ok(ProjectContext {
                manifest_path: root.join(crate::portable::ACTIVE),
                manifest: crate::portable::manifest_for_project(root, &environment)?,
                root: root.to_owned(),
            });
        }
        if root.join(config::MANIFEST_NAME).is_file() {
            break;
        }
    }
    let manifest_path = config::resolve_manifest(Some(argument))?;
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
    if context.manifest.toolchain.provider == "system" {
        crate::toolchain::validate_protected_tools(&context.root, &context.manifest)?;
    }
    let toolchain = match context.manifest.toolchain.provider.as_str() {
        "portable" => crate::portable::native::resolve(
            &context.root,
            &context.manifest,
            &crate::portable::active(&context.root)?
                .ok_or_else(|| TexeError::Build("Portable environment is missing".into()))?,
            verification,
            offline,
        )?,
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
    if let Some(environment) = crate::portable::active(&context.root)? {
        crate::portable::verify_engine(&environment, &toolchain.engine_executable)?;
    }
    let tools = PqtyClient::resolve(&context.root, &context.manifest, offline)?;
    Ok((toolchain, tools))
}
