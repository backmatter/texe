//! Embedded, declarative managed-toolchain recipes.
//!
//! Recipe documents are release data rather than Rust implementation. They
//! are parsed once and validated before any path, URL, digest, or provider is
//! used. Adding a supported TeX Live snapshot therefore adds a document and
//! no Rust edit; changing the `stable` policy only edits catalog.toml.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::TexeError;
use crate::toolchain::platform;

const CATALOG_SCHEMA: &str = "texe.toolchain-catalog/v1";
const RECIPE_SCHEMA: &str = "texe.toolchain-recipe/v1";
const CATALOG_DOCUMENT: &str = include_str!("../../toolchains/catalog.toml");
include!(concat!(env!("OUT_DIR"), "/toolchain_recipe_documents.rs"));

static CATALOG: OnceLock<Result<ToolchainCatalog, String>> = OnceLock::new();

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    schema: String,
    aliases: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(super) struct ToolchainCatalog {
    pub(super) aliases: BTreeMap<String, String>,
    pub(super) snapshots: BTreeMap<String, SnapshotRecipe>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SnapshotRecipe {
    pub(super) schema: String,
    pub(super) snapshot: String,
    pub(super) tlnet_base: String,
    pub(super) sources: Vec<String>,
    pub(super) registry_sha256: String,
    pub(super) biber: BiberRecipe,
    pub(super) engines: BTreeMap<String, EngineRecipe>,
    pub(super) platforms: BTreeMap<String, PlatformRecipe>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BiberRecipe {
    pub(super) version: String,
    pub(super) component_recipe: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EngineRecipe {
    pub(super) runtime_name: String,
    pub(super) executable: String,
    pub(super) format_recipe: String,
    pub(super) bootstrap_providers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlatformRecipe {
    pub(super) executable_suffix: String,
    #[serde(default)]
    pub(super) biber_compatibility_library_entry: Option<String>,
    pub(super) biber: ManagedArtifact,
    pub(super) artifacts: BTreeMap<String, Vec<ManagedArtifact>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedArtifact {
    pub(super) provider: String,
    pub(super) sha512: String,
    pub(super) size: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ManagedSelection<'a> {
    pub(super) snapshot: &'a SnapshotRecipe,
    pub(super) engine_name: &'a str,
    pub(super) engine: &'a EngineRecipe,
    pub(super) target: &'static str,
    pub(super) platform: &'a PlatformRecipe,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PlatformSelection<'a> {
    pub(super) snapshot: &'a SnapshotRecipe,
    pub(super) target: &'static str,
    pub(super) platform: &'a PlatformRecipe,
}

pub(super) fn catalog() -> Result<&'static ToolchainCatalog, TexeError> {
    match CATALOG.get_or_init(parse_catalog) {
        Ok(catalog) => Ok(catalog),
        Err(message) => Err(TexeError::Toolchain(format!(
            "embedded managed-toolchain catalog is invalid: {message}"
        ))),
    }
}

pub(super) fn select(channel: &str, engine: &str) -> Result<ManagedSelection<'static>, TexeError> {
    let catalog = catalog()?;
    let snapshot_name = catalog.aliases.get(channel).map_or(channel, String::as_str);
    let snapshot = catalog.snapshots.get(snapshot_name).ok_or_else(|| {
        let available = catalog
            .aliases
            .keys()
            .chain(catalog.snapshots.keys())
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("`, `");
        TexeError::Toolchain(format!(
            "managed channel `{channel}` is unavailable; choose one of `{available}`"
        ))
    })?;
    let (engine_name, engine_recipe) = snapshot.engines.get_key_value(engine).ok_or_else(|| {
        TexeError::Toolchain(format!(
            "managed snapshot `{}` does not support engine `{engine}`; use provider = \
                 \"system\" for this engine",
            snapshot.snapshot
        ))
    })?;
    let target = platform::current_target()?;
    let platform_recipe = snapshot.platforms.get(target).ok_or_else(|| {
        TexeError::Toolchain(format!(
            "managed snapshot `{}` has no recipe for target `{target}`",
            snapshot.snapshot
        ))
    })?;
    Ok(ManagedSelection {
        snapshot,
        engine_name,
        engine: engine_recipe,
        target,
        platform: platform_recipe,
    })
}

pub(super) fn select_platform(channel: &str) -> Result<PlatformSelection<'static>, TexeError> {
    let catalog = catalog()?;
    let snapshot_name = catalog.aliases.get(channel).map_or(channel, String::as_str);
    let snapshot = catalog.snapshots.get(snapshot_name).ok_or_else(|| {
        TexeError::Toolchain(format!(
            "managed snapshot `{channel}` is not present in the embedded catalog"
        ))
    })?;
    let target = platform::current_target()?;
    let platform_recipe = snapshot.platforms.get(target).ok_or_else(|| {
        TexeError::Toolchain(format!(
            "managed snapshot `{}` has no recipe for target `{target}`",
            snapshot.snapshot
        ))
    })?;
    Ok(PlatformSelection {
        snapshot,
        target,
        platform: platform_recipe,
    })
}

fn parse_catalog() -> Result<ToolchainCatalog, String> {
    let document: CatalogDocument = toml::from_str(CATALOG_DOCUMENT)
        .map_err(|error| format!("toolchains/catalog.toml: {error}"))?;
    if document.schema != CATALOG_SCHEMA {
        return Err(format!(
            "toolchains/catalog.toml uses schema `{}`; expected `{CATALOG_SCHEMA}`",
            document.schema
        ));
    }
    if !document.aliases.contains_key("stable") {
        return Err("toolchains/catalog.toml must define the `stable` alias".to_string());
    }

    let mut snapshots = BTreeMap::new();
    for (filename, text) in RECIPE_DOCUMENTS {
        let recipe: SnapshotRecipe = toml::from_str(text)
            .map_err(|error| format!("toolchains/recipes/{filename}: {error}"))?;
        validate_snapshot(filename, &recipe)?;
        let snapshot = recipe.snapshot.clone();
        if snapshots.insert(snapshot.clone(), recipe).is_some() {
            return Err(format!("duplicate managed snapshot `{snapshot}`"));
        }
    }
    for (alias, snapshot) in &document.aliases {
        validate_identifier("catalog alias", alias)?;
        if snapshots.contains_key(alias) {
            return Err(format!(
                "catalog alias `{alias}` shadows an exact snapshot ID"
            ));
        }
        if !snapshots.contains_key(snapshot) {
            return Err(format!(
                "catalog alias `{alias}` names missing snapshot `{snapshot}`"
            ));
        }
    }
    Ok(ToolchainCatalog {
        aliases: document.aliases,
        snapshots,
    })
}

fn validate_snapshot(filename: &str, recipe: &SnapshotRecipe) -> Result<(), String> {
    validate_snapshot_identity(filename, recipe)?;
    validate_snapshot_sources(recipe)?;
    validate_digest("registry_sha256", &recipe.registry_sha256, 64)?;
    validate_identifier("Biber version", &recipe.biber.version)?;
    validate_identifier("Biber component recipe", &recipe.biber.component_recipe)?;
    validate_engines(recipe)?;
    validate_platforms(recipe)
}

fn validate_snapshot_identity(filename: &str, recipe: &SnapshotRecipe) -> Result<(), String> {
    if recipe.schema != RECIPE_SCHEMA {
        return Err(format!(
            "{filename} uses schema `{}`; expected `{RECIPE_SCHEMA}`",
            recipe.schema
        ));
    }
    validate_snapshot_name(&recipe.snapshot)?;
    if filename != format!("{}.toml", recipe.snapshot) {
        return Err(format!(
            "{filename} declares snapshot `{}`; recipe filename and snapshot must match",
            recipe.snapshot
        ));
    }
    Ok(())
}

fn validate_snapshot_sources(recipe: &SnapshotRecipe) -> Result<(), String> {
    let date = recipe
        .snapshot
        .strip_prefix("texlive-")
        .expect("validated snapshot prefix");
    let expected_suffix = format!("/{}/{}/{}/tlnet", &date[..4], &date[5..7], &date[8..10]);
    validate_https_base("tlnet_base", &recipe.tlnet_base)?;
    if !recipe.tlnet_base.ends_with(&expected_suffix) {
        return Err(format!(
            "{} tlnet_base does not match its dated snapshot",
            recipe.snapshot
        ));
    }
    if recipe.sources.is_empty() || !recipe.sources.contains(&recipe.tlnet_base) {
        return Err(format!(
            "{} sources must include its canonical tlnet_base",
            recipe.snapshot
        ));
    }
    for source in &recipe.sources {
        validate_https_base("snapshot source", source)?;
    }
    Ok(())
}

fn validate_engines(recipe: &SnapshotRecipe) -> Result<(), String> {
    if recipe.engines.is_empty() {
        return Err(format!("{} has no engine recipes", recipe.snapshot));
    }
    for (engine, engine_recipe) in &recipe.engines {
        validate_identifier("engine", engine)?;
        for (field, value) in [
            ("runtime_name", engine_recipe.runtime_name.as_str()),
            ("executable", engine_recipe.executable.as_str()),
            ("format_recipe", engine_recipe.format_recipe.as_str()),
        ] {
            validate_identifier(field, value)?;
        }
        validate_provider_list(
            &format!("engine `{engine}` bootstrap providers"),
            &engine_recipe.bootstrap_providers,
        )?;
    }
    Ok(())
}

fn validate_platforms(recipe: &SnapshotRecipe) -> Result<(), String> {
    let supported_targets = ["universal-darwin", "windows", "x86_64-linux"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let recipe_targets = recipe
        .platforms
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if recipe_targets != supported_targets {
        return Err(format!(
            "{} must provide exactly the supported targets: {}",
            recipe.snapshot,
            supported_targets.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    for (target, platform) in &recipe.platforms {
        validate_platform(recipe, target, platform)?;
    }
    Ok(())
}

fn validate_platform(
    recipe: &SnapshotRecipe,
    target: &str,
    platform: &PlatformRecipe,
) -> Result<(), String> {
    validate_identifier("target", target)?;
    let expected_suffix = if target == "windows" { ".exe" } else { "" };
    if platform.executable_suffix != expected_suffix {
        return Err(format!(
            "{} target `{target}` must use executable suffix `{expected_suffix}`",
            recipe.snapshot
        ));
    }
    if target == "x86_64-linux" && platform.biber_compatibility_library_entry.is_none() {
        return Err(format!(
            "{} target `{target}` must pin its Biber compatibility library entry",
            recipe.snapshot
        ));
    }
    if target != "x86_64-linux" && platform.biber_compatibility_library_entry.is_some() {
        return Err(format!(
            "{} target `{target}` has an unexpected Biber compatibility library entry",
            recipe.snapshot
        ));
    }
    if let Some(entry) = &platform.biber_compatibility_library_entry {
        validate_portable_relative_path("Biber compatibility library entry", entry)?;
    }
    validate_artifact("Biber artifact", &platform.biber)?;
    let expected_engines = recipe.engines.keys().collect::<BTreeSet<_>>();
    let artifact_engines = platform.artifacts.keys().collect::<BTreeSet<_>>();
    if artifact_engines != expected_engines {
        return Err(format!(
            "{} target `{target}` artifact tables do not match its engine recipes",
            recipe.snapshot
        ));
    }
    for (engine, artifacts) in &platform.artifacts {
        validate_artifact_table(&recipe.snapshot, target, engine, artifacts)?;
    }
    Ok(())
}

fn validate_artifact_table(
    snapshot: &str,
    target: &str,
    engine: &str,
    artifacts: &[ManagedArtifact],
) -> Result<(), String> {
    if artifacts.is_empty() {
        return Err(format!(
            "{snapshot} target `{target}` engine `{engine}` has no artifacts"
        ));
    }
    let mut providers = BTreeSet::new();
    for artifact in artifacts {
        validate_artifact("runtime artifact", artifact)?;
        if !providers.insert(artifact.provider.as_str()) {
            return Err(format!(
                "{snapshot} target `{target}` engine `{engine}` repeats provider `{}`",
                artifact.provider
            ));
        }
    }
    Ok(())
}

fn validate_snapshot_name(snapshot: &str) -> Result<(), String> {
    let Some(date) = snapshot.strip_prefix("texlive-") else {
        return Err(format!("snapshot `{snapshot}` must start with `texlive-`"));
    };
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return Err(format!(
            "snapshot `{snapshot}` must end in an ISO date (YYYY-MM-DD)"
        ));
    }
    let month = date[5..7].parse::<u8>().unwrap_or_default();
    let day = date[8..10].parse::<u8>().unwrap_or_default();
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(format!("snapshot `{snapshot}` contains an invalid date"));
    }
    Ok(())
}

fn validate_https_base(field: &str, value: &str) -> Result<(), String> {
    if !value.starts_with("https://")
        || value.ends_with('/')
        || value.contains('@')
        || value.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "{field} must be an absolute credential-free HTTPS URL without a trailing slash"
        ));
    }
    Ok(())
}

fn validate_identifier(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "{field} `{value}` must use only ASCII letters, digits, dots, underscores, and hyphens"
        ));
    }
    Ok(())
}

fn validate_provider_list(field: &str, providers: &[String]) -> Result<(), String> {
    if providers.is_empty() {
        return Err(format!("{field} cannot be empty"));
    }
    let mut unique = BTreeSet::new();
    for provider in providers {
        validate_identifier("provider", provider)?;
        if !unique.insert(provider) {
            return Err(format!("{field} repeats provider `{provider}`"));
        }
    }
    Ok(())
}

fn validate_artifact(field: &str, artifact: &ManagedArtifact) -> Result<(), String> {
    validate_identifier(field, &artifact.provider)?;
    validate_digest("artifact sha512", &artifact.sha512, 128)?;
    if artifact.size == 0 {
        return Err(format!("{field} `{}` has zero size", artifact.provider));
    }
    Ok(())
}

fn validate_digest(field: &str, value: &str, length: usize) -> Result<(), String> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{field} must be a lowercase {length}-character hexadecimal digest"
        ));
    }
    Ok(())
}

fn validate_portable_relative_path(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value.contains('\\')
    {
        return Err(format!("{field} `{value}` is not a portable relative path"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::toolchain::catalog::{catalog, select, select_platform};

    #[test]
    fn embedded_catalog_is_complete_and_valid() {
        let catalog = catalog().expect("valid embedded catalog");
        assert_eq!(
            catalog.aliases.get("stable").map(String::as_str),
            Some("texlive-2026-07-26")
        );
        let snapshot = catalog
            .snapshots
            .get("texlive-2026-07-26")
            .expect("dated snapshot");
        assert_eq!(snapshot.engines.len(), 2);
        assert_eq!(snapshot.platforms.len(), 3);
    }

    #[test]
    fn stable_and_exact_snapshot_select_the_same_recipe() {
        let stable = select("stable", "pdflatex").expect("stable recipe");
        let exact = select("texlive-2026-07-26", "pdflatex").expect("exact recipe");
        assert!(std::ptr::eq(stable.snapshot, exact.snapshot));
        assert!(std::ptr::eq(stable.engine, exact.engine));
        assert!(std::ptr::eq(stable.platform, exact.platform));
    }

    #[test]
    fn unknown_snapshots_and_engines_are_rejected() {
        assert!(select_platform("texlive-2099-01-01").is_err());
        assert!(select("stable", "xelatex").is_err());
    }
}
