use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::TexeError;
use crate::config::ToolchainConfig;

/// Give up on a source that will not answer rather than hanging a build, and
/// bound the wait a stalled connection can impose on one read.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Retry a temporarily unavailable snapshot source before reporting an outage.
const DOWNLOAD_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(500);
const READY_MARKER: &str = ".texe-runtime.json";
const COMPONENT_MARKER: &str = ".texe-component.json";
const VERIFIED_MARKER: &str = ".texe-verified.json";
const VERIFICATION_SCHEMA: &str = "texe.verification/v1";
/// A managed runtime is immutable and owned by the account that installed it,
/// so rehashing every installed file on every resolve costs a build loop far
/// more than it defends. Rehash on this interval instead, or on demand.
const VERIFICATION_INTERVAL_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Package providers needed to dump a format whose LaTeX/L3 layer comes from
/// pqty rather than a prebuilt system format.
pub(crate) fn locked_format_bootstrap_providers(
    engine: &str,
) -> Result<&'static [String], TexeError> {
    let recipe_engine = match engine {
        "pdflatex" | "xelatex" => "pdflatex",
        "lualatex" => "lualatex",
        _ => {
            return Err(TexeError::Toolchain(format!(
                "remote packages require a locked format, but engine `{engine}` has no format \
                 recipe"
            )));
        }
    };
    Ok(&catalog::select("stable", recipe_engine)?
        .engine
        .bootstrap_providers)
}

pub trait ToolchainProvider {
    fn resolve(
        &self,
        project_root: &Path,
        request: &ToolchainConfig,
        verification: VerificationPolicy,
        offline: bool,
    ) -> Result<ResolvedToolchain, TexeError>;
}

/// How much of an already-installed managed runtime or component to reverify
/// when it is resolved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerificationPolicy {
    /// Check the recipe identity on every resolve and rehash every installed
    /// file only when the recorded verification interval has elapsed.
    #[default]
    Interval,
    /// Rehash every installed file now.
    Deep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainIdentity {
    pub provider: String,
    pub engine: String,
    pub channel: String,
    pub target: String,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_metadata_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ToolchainArtifactIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainArtifactIdentity {
    pub provider: String,
    pub sha512: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRuntime {
    /// Exact embedded snapshot used by lazily installed components.
    pub snapshot: String,
    pub root: PathBuf,
    /// Recipe-selected platform binary directory. Keeping it explicit avoids
    /// pretending the managed provider can infer unsupported targets.
    pub binary_dir: PathBuf,
    pub format_cache: PathBuf,
    pub component_cache: PathBuf,
    pub downloads: PathBuf,
    pub registry_url: String,
    pub registry_metadata_sha256: String,
    pub bootstrap_providers: Vec<String>,
    /// Applied to components installed lazily below this runtime, so a Biber
    /// download is verified exactly as deeply as the runtime that needs it.
    pub verification: VerificationPolicy,
    /// Forbid every managed component network request.
    pub offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedBiber {
    pub executable: PathBuf,
    pub library_dir: PathBuf,
    pub cache_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedToolchain {
    pub provider: String,
    pub engine: String,
    pub engine_executable: PathBuf,
    pub kpsewhich_executable: PathBuf,
    pub texmf_dist: PathBuf,
    pub engine_roots: Vec<PathBuf>,
    pub identity: ToolchainIdentity,
    #[serde(skip)]
    pub managed: Option<ManagedRuntime>,
    #[serde(skip)]
    pub(crate) verification: VerificationPolicy,
    #[serde(skip)]
    pub(crate) offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadPreflight {
    pub engine: String,
    pub runtime_ready: bool,
    pub missing_components: usize,
    pub missing_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeMarker {
    schema: String,
    identity: ToolchainIdentity,
    files: Vec<RuntimeFileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeFileIdentity {
    path: String,
    sha256: String,
}

/// Records when every installed file below a runtime or component directory
/// last hashed as expected. Excluded from the hashed file set, so writing it
/// cannot invalidate the directory it describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct VerificationStamp {
    schema: String,
    fingerprint: String,
    verified_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ComponentMarker {
    schema: String,
    artifacts: Vec<ToolchainArtifactIdentity>,
    files: Vec<RuntimeFileIdentity>,
}

#[derive(Debug, Default)]
pub struct ManagedToolchainProvider;

impl ToolchainProvider for ManagedToolchainProvider {
    fn resolve(
        &self,
        _project_root: &Path,
        request: &ToolchainConfig,
        verification: VerificationPolicy,
        offline: bool,
    ) -> Result<ResolvedToolchain, TexeError> {
        if request.provider != "managed" {
            return Err(TexeError::Toolchain(format!(
                "managed provider cannot resolve provider {}",
                request.provider
            )));
        }
        if request.adapter != "kpathsea" {
            return Err(TexeError::Toolchain(format!(
                "managed engine adapter {} is unavailable; use `kpathsea`",
                request.adapter
            )));
        }
        let selection = catalog::select(&request.channel, &request.engine)?;
        let artifacts = selection
            .platform
            .artifacts
            .get(selection.engine_name)
            .expect("catalog validation guarantees an artifact table");
        let identity = managed_identity(selection);
        let home = texe_data_home()?;
        let short_fingerprint = &identity.fingerprint[..16];
        let root = home.join("toolchains").join(format!(
            "{}-{}-{}-{short_fingerprint}",
            selection.snapshot.snapshot, selection.engine.runtime_name, selection.target
        ));
        install_managed_runtime(
            &home,
            &root,
            &identity,
            artifacts,
            &selection.snapshot.sources,
            verification,
            offline,
        )?;

        let binary_dir = root.join("bin").join(selection.target);
        let executable = |name: &str| {
            binary_dir.join(format!("{}{}", name, selection.platform.executable_suffix))
        };
        let engine_executable = executable(&selection.engine.executable);
        let kpsewhich_executable = executable("kpsewhich");
        let bibtex_executable = executable("bibtex");
        let makeindex_executable = executable("makeindex");
        let texmf_dist = root.join("texmf-dist");
        for required in [
            &engine_executable,
            &kpsewhich_executable,
            &bibtex_executable,
            &makeindex_executable,
        ] {
            if !required.is_file() {
                return Err(TexeError::Toolchain(format!(
                    "managed runtime is incomplete: {} is missing",
                    required.display()
                )));
            }
        }

        Ok(ResolvedToolchain {
            provider: "managed".to_string(),
            engine: request.engine.clone(),
            engine_executable,
            kpsewhich_executable,
            texmf_dist,
            engine_roots: vec![root.clone()],
            identity,
            managed: Some(ManagedRuntime {
                snapshot: selection.snapshot.snapshot.clone(),
                binary_dir,
                root,
                format_cache: home.join("formats"),
                component_cache: home.join("components"),
                downloads: home.join("downloads"),
                registry_url: format!("{}/tlpkg/texlive.tlpdb.xz", selection.snapshot.tlnet_base),
                registry_metadata_sha256: selection.snapshot.registry_sha256.clone(),
                bootstrap_providers: selection.engine.bootstrap_providers.clone(),
                verification,
                offline,
            }),
            verification,
            offline,
        })
    }
}

/// Inspect the immutable managed-runtime recipe and local cache without
/// opening a network connection.
pub fn download_preflight(
    request: &ToolchainConfig,
) -> Result<Option<DownloadPreflight>, TexeError> {
    if request.provider != "managed" {
        return Ok(None);
    }
    let selection = catalog::select(&request.channel, &request.engine)?;
    let artifacts = selection
        .platform
        .artifacts
        .get(selection.engine_name)
        .expect("catalog validation guarantees an artifact table");
    let identity = managed_identity(selection);
    let home = texe_data_home()?;
    let root = home.join("toolchains").join(format!(
        "{}-{}-{}-{}",
        selection.snapshot.snapshot,
        selection.engine.runtime_name,
        selection.target,
        &identity.fingerprint[..16]
    ));
    let runtime_ready = root.join(READY_MARKER).is_file();
    let (missing_components, missing_bytes) = if runtime_ready {
        (0, 0)
    } else {
        artifacts
            .iter()
            .filter(|artifact| {
                !home
                    .join("downloads")
                    .join(format!("{}.tar.xz", artifact.sha512))
                    .is_file()
            })
            .fold((0, 0), |(items, bytes), artifact| {
                (items + 1, bytes + artifact.size)
            })
    };
    Ok(Some(DownloadPreflight {
        engine: request.engine.clone(),
        runtime_ready,
        missing_components,
        missing_bytes,
    }))
}

/// What a cache sweep must keep: the directory name of every managed runtime a
/// current recipe resolves to, the full toolchain fingerprint that names its
/// format cache, and the download file names its artifacts occupy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveToolchainArtifacts {
    pub home: PathBuf,
    pub runtimes: BTreeSet<String>,
    pub fingerprints: BTreeSet<String>,
    pub components: BTreeSet<String>,
    pub downloads: BTreeSet<String>,
}

/// Describe everything below `TEXE_HOME` that a current recipe still needs.
///
/// Anything else there was installed by a recipe this binary no longer has, so
/// no lock can name it and no build can reach it.
///
/// # Errors
///
/// Returns an error when the managed storage directory cannot be chosen.
pub fn live_toolchain_artifacts() -> Result<LiveToolchainArtifacts, TexeError> {
    let target = platform::current_target()?;
    let catalog = catalog::catalog()?;
    let mut live = LiveToolchainArtifacts {
        home: texe_data_home()?,
        runtimes: BTreeSet::new(),
        fingerprints: BTreeSet::new(),
        components: BTreeSet::new(),
        downloads: BTreeSet::new(),
    };
    for snapshot in catalog.snapshots.values() {
        let Some(platform_recipe) = snapshot.platforms.get(target) else {
            continue;
        };
        let platform_selection = catalog::PlatformSelection {
            snapshot,
            target,
            platform: platform_recipe,
        };
        live.components
            .insert(biber_component_name(platform_selection));
        live.downloads.insert(format!(
            "{}.tar.xz",
            platform_selection.platform.biber.sha512
        ));
        for (engine_name, engine) in &snapshot.engines {
            let selection = catalog::ManagedSelection {
                snapshot,
                engine_name,
                engine,
                target,
                platform: platform_recipe,
            };
            let identity = managed_identity(selection);
            live.runtimes.insert(format!(
                "{}-{}-{}-{}",
                snapshot.snapshot,
                engine.runtime_name,
                target,
                &identity.fingerprint[..16]
            ));
            live.fingerprints.insert(identity.fingerprint);
            for artifact in platform_recipe
                .artifacts
                .get(engine_name)
                .expect("catalog validation guarantees an artifact table")
            {
                live.downloads.insert(format!("{}.tar.xz", artifact.sha512));
            }
        }
    }
    Ok(live)
}

fn biber_component_name(selection: catalog::PlatformSelection<'_>) -> String {
    format!(
        "biber-{}-{}-{}-{}",
        selection.snapshot.biber.version,
        selection.target,
        selection.snapshot.biber.component_recipe,
        &selection.platform.biber.sha512[..16]
    )
}

fn managed_identity(selection: catalog::ManagedSelection<'_>) -> ToolchainIdentity {
    let runtime_artifacts = selection
        .platform
        .artifacts
        .get(selection.engine_name)
        .expect("catalog validation guarantees an artifact table");
    let mut artifacts = runtime_artifacts
        .iter()
        .map(|artifact| ToolchainArtifactIdentity {
            provider: artifact.provider.clone(),
            sha512: format!("sha512:{}", artifact.sha512),
        })
        .collect::<Vec<_>>();
    artifacts.push(ToolchainArtifactIdentity {
        provider: selection.platform.biber.provider.clone(),
        sha512: format!("sha512:{}", selection.platform.biber.sha512),
    });
    let mut hasher = Sha256::new();
    hasher.update(selection.snapshot.snapshot.as_bytes());
    hasher.update(selection.target.as_bytes());
    hasher.update(selection.engine_name.as_bytes());
    hasher.update(selection.snapshot.registry_sha256.as_bytes());
    hasher.update(selection.engine.format_recipe.as_bytes());
    for provider in &selection.engine.bootstrap_providers {
        hasher.update(provider.as_bytes());
    }
    for artifact in runtime_artifacts {
        hasher.update(artifact.provider.as_bytes());
        hasher.update(artifact.sha512.as_bytes());
    }
    hasher.update(selection.platform.biber.provider.as_bytes());
    hasher.update(selection.platform.biber.sha512.as_bytes());
    hasher.update(selection.snapshot.biber.component_recipe.as_bytes());
    ToolchainIdentity {
        provider: "managed".to_string(),
        engine: selection.engine_name.to_string(),
        channel: selection.snapshot.snapshot.clone(),
        target: selection.target.to_string(),
        fingerprint: hex::encode(hasher.finalize()),
        registry_url: Some(selection.snapshot.tlnet_base.clone()),
        registry_metadata_digest: Some(format!("sha256:{}", selection.snapshot.registry_sha256)),
        artifacts,
    }
}

pub(crate) fn texe_data_home() -> Result<PathBuf, TexeError> {
    if let Some(path) = env::var_os("TEXE_HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = env::var_os("LOCALAPPDATA").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path).join("texe"));
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = env::var_os("HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path).join("Library/Application Support/texe"));
    }
    if let Some(path) = env::var_os("XDG_DATA_HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path).join("texe"));
    }
    env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join(".local/share/texe"))
        .ok_or_else(|| {
            TexeError::Toolchain(
                "cannot choose managed toolchain storage; set TEXE_HOME or HOME".to_string(),
            )
        })
}

mod artifact;
mod catalog;
mod component;
mod managed;
mod platform;
mod system;

pub use component::{ensure_bundled_biber, ensure_managed_biber};
use managed::install_managed_runtime;
pub use system::{SystemToolchainProvider, executable_version, resolve_executable};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::TexeError;
    use crate::toolchain::artifact::download_artifact;
    use crate::toolchain::catalog::{self, ManagedArtifact};
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    use crate::toolchain::component::extract_biber_compatibility_library;
    use crate::toolchain::component::{
        biber_artifact_identities, runtime_file_identities, verify_managed_component,
    };
    use crate::toolchain::managed::{record_verification, unix_time, verify_installed_runtime};
    use crate::toolchain::{
        COMPONENT_MARKER, ComponentMarker, READY_MARKER, RuntimeMarker, ToolchainIdentity,
        VERIFICATION_INTERVAL_SECONDS, VERIFICATION_SCHEMA, VERIFIED_MARKER, VerificationPolicy,
        VerificationStamp, managed_identity, resolve_executable,
    };
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    use std::io::Write as _;

    #[test]
    fn managed_identities_are_fully_pinned_and_engine_specific() {
        for engine in ["pdflatex", "lualatex"] {
            let selection = catalog::select("stable", engine).expect("managed selection");
            let identity = managed_identity(selection);
            assert_eq!(identity.provider, "managed");
            assert_eq!(identity.engine, engine);
            assert_eq!(identity.channel, "texlive-2026-07-26");
            assert_eq!(
                identity.artifacts.len(),
                selection
                    .platform
                    .artifacts
                    .get(engine)
                    .expect("engine artifacts")
                    .len()
                    + 1
            );
            assert!(
                identity
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.provider == selection.platform.biber.provider)
            );
            assert_eq!(
                identity.registry_metadata_digest.as_deref(),
                Some(concat!(
                    "sha256:",
                    "f62de536a6da5ff49d6e56d9fc0f526da21addcdd59d420b4a4b19b8983e18cf"
                ))
            );
        }
        let pdf =
            managed_identity(catalog::select("stable", "pdflatex").expect("pdfLaTeX selection"));
        let lua =
            managed_identity(catalog::select("stable", "lualatex").expect("LuaLaTeX selection"));
        assert_ne!(pdf.fingerprint, lua.fingerprint);
        let pdf_runtime = format!("pdftex.{}", pdf.target);
        let lua_runtime = format!("luahbtex.{}", lua.target);
        assert!(
            pdf.artifacts
                .iter()
                .any(|artifact| artifact.provider == pdf_runtime)
        );
        assert!(
            lua.artifacts
                .iter()
                .any(|artifact| artifact.provider == lua_runtime)
        );
        assert!(
            lua.artifacts
                .iter()
                .all(|artifact| !artifact.provider.starts_with("pdftex"))
        );
    }

    #[test]
    fn every_snapshot_source_is_a_usable_archive_base() {
        for snapshot in catalog::catalog()
            .expect("embedded catalog")
            .snapshots
            .values()
        {
            assert!(snapshot.sources.contains(&snapshot.tlnet_base));
            for source in &snapshot.sources {
                assert!(
                    source.starts_with("https://"),
                    "{source} is not an absolute HTTPS URL"
                );
                assert!(!source.ends_with('/'), "{source} has a trailing slash");
            }
        }
    }

    /// The fingerprint names the directory an installed runtime lives in and
    /// is what a lock pins, so it must change when the recipe changes and only
    /// then. Pinning it here makes both halves of that visible in review: a
    /// source or timeout edit leaves this test passing, and a snapshot,
    /// artifact, or format-recipe edit fails it until the value is updated
    /// deliberately.
    #[test]
    fn the_recipe_fingerprint_is_pinned() {
        let pdf =
            managed_identity(catalog::select("stable", "pdflatex").expect("pdfLaTeX selection"));
        let lua =
            managed_identity(catalog::select("stable", "lualatex").expect("LuaLaTeX selection"));
        let expected = match pdf.target.as_str() {
            "x86_64-linux" => (
                "e5a62dc62ece5a0767fc1a6f0f32e099d14bed1893b3d7ea99f73a0ec4e1f029",
                "042d2d619304efb52f44a6c7c486247be2a2ee386fedf7fca5afcb65a71f18c1",
            ),
            "windows" => (
                "550cbf9b6f5848d272fc43e3965a352ceb2e28bd3cef7e821c40ca0c1581f8fc",
                "62c60da8b98fa07458bba2c13e4187761b0d0f89c92dd8160b4c9e4aa914d998",
            ),
            "universal-darwin" => (
                "2c756c020bd350fb5689b516e2edde1a0c92ae28f6eb43fe75678415d825109b",
                "df3c9239dd35b05cd6e92cc31f69e326237561d7d7b6aa95d4199c8789777b0f",
            ),
            target => panic!("unexpected managed target {target}"),
        };
        assert_eq!(
            pdf.fingerprint, expected.0,
            "the managed pdfLaTeX recipe changed; update this value if that was intended"
        );
        assert_eq!(
            lua.fingerprint, expected.1,
            "the managed LuaLaTeX recipe changed; update this value if that was intended"
        );
    }

    fn installed_runtime(root: &Path) -> ToolchainIdentity {
        fs::create_dir_all(root.join("bin")).expect("bin directory");
        fs::write(root.join("bin/pdftex"), b"engine").expect("runtime file");
        let identity =
            managed_identity(catalog::select("stable", "pdflatex").expect("managed selection"));
        let marker = RuntimeMarker {
            schema: "texe.runtime/v1".to_string(),
            identity: identity.clone(),
            files: runtime_file_identities(root).expect("runtime identity"),
        };
        fs::write(
            root.join(READY_MARKER),
            serde_json::to_vec(&marker).expect("marker JSON"),
        )
        .expect("marker");
        identity
    }

    #[test]
    fn installed_runtime_verification_detects_tampering() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let identity = installed_runtime(root);
        verify_installed_runtime(root, &identity, VerificationPolicy::Deep)
            .expect("untampered runtime");
        fs::write(root.join("bin/pdftex"), b"changed").expect("tamper runtime");
        assert!(verify_installed_runtime(root, &identity, VerificationPolicy::Deep).is_err());
    }

    #[test]
    fn an_unstamped_runtime_is_rehashed_even_under_the_interval_policy() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let identity = installed_runtime(root);
        fs::write(root.join("bin/pdftex"), b"changed").expect("tamper runtime");
        assert!(verify_installed_runtime(root, &identity, VerificationPolicy::Interval).is_err());
    }

    #[test]
    fn a_recent_stamp_skips_the_rehash_until_the_interval_elapses() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let identity = installed_runtime(root);
        verify_installed_runtime(root, &identity, VerificationPolicy::Deep)
            .expect("untampered runtime");
        assert!(root.join(VERIFIED_MARKER).is_file());

        // The stamp is what the interval policy trusts; the deep check and an
        // expired stamp both still read the files.
        fs::write(root.join("bin/pdftex"), b"changed").expect("tamper runtime");
        verify_installed_runtime(root, &identity, VerificationPolicy::Interval)
            .expect("a fresh stamp is trusted");
        assert!(verify_installed_runtime(root, &identity, VerificationPolicy::Deep).is_err());

        let stamp = VerificationStamp {
            schema: VERIFICATION_SCHEMA.to_string(),
            fingerprint: identity.fingerprint.clone(),
            verified_at: unix_time().expect("clock") - VERIFICATION_INTERVAL_SECONDS - 1,
        };
        fs::write(
            root.join(VERIFIED_MARKER),
            serde_json::to_vec(&stamp).expect("stamp JSON"),
        )
        .expect("expired stamp");
        assert!(verify_installed_runtime(root, &identity, VerificationPolicy::Interval).is_err());
    }

    #[test]
    fn the_verification_stamp_is_not_part_of_the_verified_file_set() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let identity = installed_runtime(root);
        let before = runtime_file_identities(root).expect("file identities");
        record_verification(root, &identity.fingerprint);
        assert!(root.join(VERIFIED_MARKER).is_file());
        assert_eq!(before, runtime_file_identities(root).expect("identities"));
        verify_installed_runtime(root, &identity, VerificationPolicy::Deep)
            .expect("stamped runtime still verifies");
    }

    #[test]
    fn installed_component_verification_detects_tampering() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        fs::create_dir_all(root.join("bin")).expect("bin directory");
        fs::write(root.join("bin/biber"), b"processor").expect("component file");
        let artifacts =
            biber_artifact_identities(catalog::select_platform("stable").expect("stable platform"));
        let marker = ComponentMarker {
            schema: "texe.component/v1".to_string(),
            artifacts: artifacts.clone(),
            files: runtime_file_identities(root).expect("component identity"),
        };
        fs::write(
            root.join(COMPONENT_MARKER),
            serde_json::to_vec(&marker).expect("marker JSON"),
        )
        .expect("marker");
        verify_managed_component(root, &artifacts, VerificationPolicy::Deep)
            .expect("untampered component");
        fs::write(root.join("bin/biber"), b"changed").expect("tamper component");
        assert!(verify_managed_component(root, &artifacts, VerificationPolicy::Deep).is_err());
    }

    #[test]
    fn offline_missing_runtime_component_fails_before_creating_a_download() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let artifact = ManagedArtifact {
            provider: "offline-fixture".to_string(),
            sha512: "00".to_string(),
            size: 1,
        };
        let error = download_artifact(directory.path(), &artifact, &[], true)
            .expect_err("an empty offline cache must fail");
        assert!(error.to_string().contains("offline mode requires"));
        assert!(
            fs::read_dir(directory.path())
                .expect("downloads")
                .next()
                .is_none(),
            "offline mode must not create a partial download"
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn biber_install_extracts_its_pinned_bootstrap_library() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let executable = directory.path().join("biber");
        let selection = catalog::select_platform("stable").expect("stable platform");
        let entry = selection
            .platform
            .biber_compatibility_library_entry
            .as_deref()
            .expect("Linux compatibility library");
        let mut archive =
            zip::ZipWriter::new(fs::File::create(&executable).expect("Biber fixture"));
        archive
            .start_file(
                entry,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .expect("library entry");
        archive.write_all(b"pinned libcrypt byte").expect("library");
        archive.finish().expect("finish archive");

        let destination = directory.path().join("lib");
        extract_biber_compatibility_library(&executable, &destination, entry)
            .expect("extract compatibility library");

        assert_eq!(
            fs::read(destination.join("libcrypt.so.1")).expect("extracted library"),
            b"pinned libcrypt byte"
        );
    }

    #[test]
    fn explicit_missing_executable_is_rejected() {
        let error = resolve_executable(Path::new("/tmp"), "./definitely-missing")
            .expect_err("missing executable should fail");
        assert!(matches!(error, TexeError::ToolNotFound(_)));
    }
}
