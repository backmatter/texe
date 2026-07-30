use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::config::ProjectManifest;
use crate::toolchain::ResolvedToolchain;
use crate::{TexeError, atomic};

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
    let Ok(bytes) = fs::read(path) else {
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
    atomic::write(path, &bytes)
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
    if relative.as_os_str().is_empty() {
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
    let bytes = fs::read(path).ok()?;
    Some(bytes_digest(&bytes))
}

pub(super) fn required_file_digest(path: &Path) -> Result<String, TexeError> {
    let bytes = fs::read(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
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
}
