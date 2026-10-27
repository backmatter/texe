use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::generated;
use crate::TexeError;
use crate::config::ProjectManifest;
use crate::toolchain::ResolvedToolchain;

const CACHE_SCHEMA: &str = "texe.auxiliary-cache/v1";
pub(super) const CACHE_NAME: &str = "auxiliary-cache.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CachedOutput {
    pub(super) input_digest: String,
    pub(super) output: String,
    pub(super) output_digest: String,
}

#[derive(Debug, Default)]
pub(super) struct OutputCache {
    entries: BTreeMap<String, CachedOutput>,
    seen: BTreeSet<String>,
}

impl OutputCache {
    pub(super) fn from_entries(entries: BTreeMap<String, CachedOutput>) -> Self {
        Self {
            entries,
            seen: BTreeSet::new(),
        }
    }

    pub(super) fn retain(&mut self, key: Option<&str>) {
        if let Some(key) = key {
            self.seen.insert(key.to_string());
        }
    }

    pub(super) fn restore(
        &mut self,
        key: Option<&str>,
        input_digest: &str,
        output_root: &Path,
        output: &Path,
    ) -> bool {
        let Some(key) = key else {
            return false;
        };
        self.seen.insert(key.to_string());
        self.entries
            .get(key)
            .is_some_and(|cached| cached_output_matches(cached, input_digest, output_root, output))
    }

    pub(super) fn record(
        &mut self,
        key: Option<String>,
        input_digest: &str,
        output_root: &Path,
        output: &Path,
    ) {
        let Some(key) = key else {
            return;
        };
        self.seen.insert(key.clone());
        let Some(output_path) = relative_path(output_root, output) else {
            return;
        };
        let Some(output_digest) = file_digest(output) else {
            return;
        };
        self.entries.insert(
            key,
            CachedOutput {
                input_digest: input_digest.to_string(),
                output: output_path,
                output_digest,
            },
        );
    }

    pub(super) fn retained_entries(&self) -> BTreeMap<String, CachedOutput> {
        self.entries
            .iter()
            .filter(|(key, _)| self.seen.contains(*key))
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuxiliaryCache {
    schema: String,
    key: String,
    #[serde(default)]
    pub(super) bibliography: BTreeMap<String, CachedOutput>,
    #[serde(default)]
    pub(super) index: BTreeMap<String, CachedOutput>,
}

pub(super) fn cache_key(
    manifest: &ProjectManifest,
    toolchain: &ResolvedToolchain,
    environment_fingerprint: &str,
) -> Result<String, TexeError> {
    let mut hasher = Sha256::new();
    hasher.update(CACHE_SCHEMA.as_bytes());
    hasher.update(environment_fingerprint.as_bytes());
    hasher.update(
        serde_json::to_vec(&toolchain.identity).map_err(|source| TexeError::Json {
            path: PathBuf::from("toolchain identity"),
            source,
        })?,
    );
    hasher.update(
        serde_json::to_vec(&manifest.bibliography).map_err(|source| TexeError::Json {
            path: PathBuf::from("bibliography configuration"),
            source,
        })?,
    );
    hasher.update(
        serde_json::to_vec(&manifest.index).map_err(|source| TexeError::Json {
            path: PathBuf::from("index configuration"),
            source,
        })?,
    );
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

pub(super) fn read(path: &Path, expected_key: &str) -> AuxiliaryCache {
    let Ok(bytes) = generated::read_local(path, generated::MAX_METADATA) else {
        return AuxiliaryCache::empty(expected_key);
    };
    let Ok(cache) = serde_json::from_slice::<AuxiliaryCache>(&bytes) else {
        return AuxiliaryCache::empty(expected_key);
    };
    if cache.schema == CACHE_SCHEMA && cache.key == expected_key {
        cache
    } else {
        AuxiliaryCache::empty(expected_key)
    }
}

pub(super) fn write(path: &Path, cache: &AuxiliaryCache) -> Result<(), TexeError> {
    let mut bytes = serde_json::to_vec_pretty(cache).map_err(|source| TexeError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    let absolute = std::path::absolute(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let root = absolute
        .ancestors()
        .last()
        .expect("absolute filesystem root");
    generated::write(root, &absolute, &bytes)
}

impl AuxiliaryCache {
    fn empty(key: &str) -> Self {
        Self {
            schema: CACHE_SCHEMA.to_string(),
            key: key.to_string(),
            bibliography: BTreeMap::new(),
            index: BTreeMap::new(),
        }
    }

    pub(super) fn from_entries(
        key: &str,
        bibliography: BTreeMap<String, CachedOutput>,
        index: BTreeMap<String, CachedOutput>,
    ) -> Self {
        Self {
            schema: CACHE_SCHEMA.to_string(),
            key: key.to_string(),
            bibliography,
            index,
        }
    }
}

pub(super) fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

pub(super) fn file_digest(path: &Path) -> Option<String> {
    let bytes = generated::read_local(path, generated::MAX_ARTIFACT).ok()?;
    Some(bytes_digest(&bytes))
}

pub(super) fn required_file_digest(path: &Path) -> Result<String, TexeError> {
    // This is a resolved, trusted tool executable. System installations may
    // legitimately use symbolic or hard links; compiler output may not.
    let file = fs::File::open(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() > generated::MAX_ARTIFACT {
        return Err(TexeError::Build(
            "The auxiliary executable must be a bounded regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(generated::MAX_ARTIFACT + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > generated::MAX_ARTIFACT {
        return Err(TexeError::Build(
            "The auxiliary executable exceeds its size limit".into(),
        ));
    }
    Ok(bytes_digest(&bytes))
}

fn bytes_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(super) fn cached_output_matches(
    cached: &CachedOutput,
    input_digest: &str,
    output_root: &Path,
    output: &Path,
) -> bool {
    cached.input_digest == input_digest
        && relative_path(output_root, output).as_deref() == Some(cached.output.as_str())
        && file_digest(output).as_deref() == Some(cached.output_digest.as_str())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use crate::build::auxiliary::{
        AuxiliaryCache, CachedOutput, OutputCache, cached_output_matches, file_digest, read,
        required_file_digest, write,
    };

    #[test]
    fn cache_round_trips_only_for_the_matching_recipe_key() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("auxiliary-cache.json");
        let entry = CachedOutput {
            input_digest: "input".to_string(),
            output: "main.bbl".to_string(),
            output_digest: "output".to_string(),
        };
        let cache = AuxiliaryCache::from_entries(
            "recipe-a",
            BTreeMap::from([("main.bcf".to_string(), entry.clone())]),
            BTreeMap::new(),
        );
        write(&path, &cache).expect("write cache");

        assert_eq!(
            read(&path, "recipe-a").bibliography.get("main.bcf"),
            Some(&entry)
        );
        assert!(read(&path, "recipe-b").bibliography.is_empty());

        fs::write(&path, b"invalid").expect("invalid cache");
        assert!(read(&path, "recipe-a").bibliography.is_empty());
    }

    #[test]
    fn cached_outputs_require_matching_inputs_paths_and_bytes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join("main.bbl");
        fs::write(&output, b"generated bibliography").expect("output");
        let cached = CachedOutput {
            input_digest: "input-a".to_string(),
            output: "main.bbl".to_string(),
            output_digest: file_digest(&output).expect("output digest"),
        };

        assert!(cached_output_matches(
            &cached,
            "input-a",
            directory.path(),
            &output
        ));
        assert!(!cached_output_matches(
            &cached,
            "input-b",
            directory.path(),
            &output
        ));

        fs::write(&output, b"tampered bibliography").expect("tampered output");
        assert!(!cached_output_matches(
            &cached,
            "input-a",
            directory.path(),
            &output
        ));
    }

    #[test]
    fn output_cache_owns_restore_record_and_retention_bookkeeping() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join("main.bbl");
        fs::write(&output, b"generated bibliography").expect("output");
        let mut cache = OutputCache::default();

        cache.record(
            Some("main.aux".to_string()),
            "input-a",
            directory.path(),
            &output,
        );
        assert!(cache.restore(Some("main.aux"), "input-a", directory.path(), &output));
        assert_eq!(
            cache
                .retained_entries()
                .get("main.aux")
                .expect("retained cache")
                .output,
            "main.bbl"
        );

        let mut restored = OutputCache::from_entries(cache.retained_entries());
        assert!(!restored.restore(Some("main.aux"), "input-b", directory.path(), &output));
        restored.retain(Some("empty.idx"));
        assert!(restored.retained_entries().contains_key("main.aux"));
    }

    #[test]
    fn required_file_digests_track_executable_bytes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let executable = directory.path().join("processor");
        fs::write(&executable, b"first processor").expect("processor");
        let initial = required_file_digest(&executable).expect("initial digest");

        fs::write(&executable, b"replaced processor").expect("replacement");
        let changed = required_file_digest(&executable).expect("changed digest");

        assert_ne!(initial, changed);
    }

    #[cfg(unix)]
    #[test]
    fn compiler_cache_aliases_are_ignored_without_reading_research_files() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let research = directory.path().join("research.json");
        let cache = AuxiliaryCache::from_entries(
            "recipe",
            BTreeMap::from([(
                "private".into(),
                CachedOutput {
                    input_digest: "input".into(),
                    output: "output".into(),
                    output_digest: "digest".into(),
                },
            )]),
            BTreeMap::new(),
        );
        let original = serde_json::to_vec(&cache).unwrap();
        fs::write(&research, &original).unwrap();
        let alias = directory.path().join("cache.json");
        symlink(&research, &alias).unwrap();
        assert!(read(&alias, "recipe").bibliography.is_empty());
        assert_eq!(file_digest(&alias), None);
        fs::remove_file(&alias).unwrap();
        fs::hard_link(&research, &alias).unwrap();
        assert!(read(&alias, "recipe").bibliography.is_empty());
        assert_eq!(file_digest(&alias), None);
        assert_eq!(fs::read(&research).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn compiler_output_parent_aliases_cannot_redirect_cache_reads_or_writes() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let research = directory.path().join("research");
        fs::create_dir(&research).unwrap();
        fs::write(research.join("cache.json"), b"research source").unwrap();
        symlink(&research, directory.path().join("derived")).unwrap();
        let redirected = directory.path().join("derived/cache.json");
        assert_eq!(file_digest(&redirected), None);
        assert!(write(&redirected, &AuxiliaryCache::empty("recipe")).is_err());
        assert_eq!(
            fs::read(research.join("cache.json")).unwrap(),
            b"research source"
        );
        assert!(
            super::relative_path(directory.path(), &directory.path().join("../outside")).is_none()
        );
    }

    #[test]
    fn oversized_compiler_output_is_rejected_before_hashing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.bbl");
        fs::File::create(&path)
            .unwrap()
            .set_len(super::generated::MAX_ARTIFACT + 1)
            .unwrap();
        assert_eq!(file_digest(&path), None);
    }

    #[cfg(unix)]
    #[test]
    fn trusted_auxiliary_executable_aliases_keep_working() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("binary");
        fs::write(&binary, b"trusted executable").unwrap();
        let symbolic = directory.path().join("symlinked-executable");
        let hard = directory.path().join("hardlinked-executable");
        symlink(&binary, &symbolic).unwrap();
        fs::hard_link(&binary, &hard).unwrap();
        assert_eq!(
            required_file_digest(&symbolic).unwrap(),
            required_file_digest(&binary).unwrap()
        );
        assert_eq!(
            required_file_digest(&hard).unwrap(),
            required_file_digest(&binary).unwrap()
        );
    }
}
