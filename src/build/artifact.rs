use sha2::{Digest, Sha256};
use std::ffi::OsStr;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::lockfile::read_source_date_epoch;

/// The build timestamp this run hands to the engine, and the one its lock
/// should keep afterwards. They differ exactly when a caller overrides the
/// project's pinned value for one build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BuildTimestamp {
    /// Passed to the engine as `SOURCE_DATE_EPOCH`, and part of the build
    /// fingerprint so an override still invalidates the no-op fast path.
    pub(super) effective: u64,
    /// Written back to `texe.lock`. A project pins this once, when it first
    /// locks, and keeps it from then on.
    pub(super) locked: u64,
}

/// Chooses both timestamps. An inherited `SOURCE_DATE_EPOCH` wins for this
/// build so a caller can pin what the engine renders; otherwise the project
/// reuses the timestamp already recorded in its lock, which is what makes a
/// committed lock reproduce the same PDF on another machine.
///
/// The override deliberately does not reach `locked`. Build environments that
/// export `SOURCE_DATE_EPOCH` globally — Nix, Guix, and reproducible-build CI —
/// would otherwise silently rewrite a committed lock, permanently changing what
/// `\today` renders for every later build on every machine.
pub(super) fn resolve_build_timestamp(project_root: &Path) -> BuildTimestamp {
    build_timestamp(
        read_source_date_epoch(project_root),
        inherited_source_date_epoch(),
        current_unix_time,
    )
}

pub(super) fn build_timestamp(
    pinned: Option<u64>,
    inherited: Option<u64>,
    now: impl FnOnce() -> u64,
) -> BuildTimestamp {
    let effective = inherited.or(pinned).unwrap_or_else(now);
    BuildTimestamp {
        effective,
        // An unlocked project pins whatever this first build used; a locked one
        // keeps what it already pinned.
        locked: pinned.unwrap_or(effective),
    }
}

fn inherited_source_date_epoch() -> Option<u64> {
    std::env::var_os("SOURCE_DATE_EPOCH")
        .as_deref()
        .and_then(OsStr::to_str)
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn current_unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs())
}

/// The files a successful build leaves in the project root.
#[derive(Debug, Clone)]
pub(super) struct PublishedBuild {
    pub(super) artifact: PathBuf,
    pub(super) synctex: Option<PathBuf>,
}

impl PublishedBuild {
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.artifact.clone()];
        paths.extend(self.synctex.clone());
        paths
    }
}

pub(super) fn publish_artifact(
    project_root: &Path,
    internal: &Path,
    project_lock: &[u8],
) -> Result<PublishedBuild, TexeError> {
    publish_artifact_at(
        project_root,
        internal,
        project_lock,
        &project_root.join(crate::lockfile::LOCK_NAME),
    )
}

#[allow(clippy::too_many_lines)]
pub(super) fn publish_artifact_at(
    project_root: &Path,
    internal: &Path,
    project_lock: &[u8],
    lock_path: &Path,
) -> Result<PublishedBuild, TexeError> {
    if !internal.starts_with(project_root.join(".texe"))
        || !matches!(
            internal.extension().and_then(OsStr::to_str),
            Some("pdf" | "dvi")
        )
    {
        return Err(TexeError::Build(
            "Built artifacts must use the private PDF or DVI output path".into(),
        ));
    }
    let name = internal.file_name().ok_or_else(|| {
        TexeError::Build(format!(
            "built artifact has no filename: {}",
            internal.display()
        ))
    })?;
    let published = project_root.join(name);
    reject_source_output_overlap(project_root, &published, project_lock)?;
    let output_root = internal
        .parent()
        .ok_or_else(|| TexeError::Build("Invalid artifact directory".into()))?;
    let bytes = super::generated::read(
        project_root,
        output_root,
        internal,
        super::generated::MAX_ARTIFACT,
    )?;
    if (internal.extension() == Some(OsStr::new("pdf")) && !super::generated::valid_pdf(&bytes))
        || (internal.extension() == Some(OsStr::new("dvi")) && bytes.first() != Some(&247))
    {
        return Err(TexeError::Build(
            "The compiler produced an invalid PDF or DVI. The last PDF has been kept.".into(),
        ));
    }
    let synctex = internal.with_extension("synctex.gz");
    let published_synctex = published.with_extension("synctex.gz");
    let sync_bytes = match super::generated::optional(
        project_root,
        output_root,
        &synctex,
        super::generated::MAX_ARTIFACT,
    )? {
        Some(bytes) => Some(
            super::synctex::for_publication(&bytes, project_root).map_err(|source| {
                TexeError::Io {
                    path: synctex.clone(),
                    source,
                }
            })?,
        ),
        None => None,
    };
    // Validate rollback inputs before writing any backup or publication state.
    super::generated::optional(
        project_root,
        project_root,
        lock_path,
        super::generated::MAX_METADATA,
    )?;
    for path in [&published, &published_synctex] {
        super::generated::optional(
            project_root,
            project_root,
            path,
            super::generated::MAX_ARTIFACT,
        )?;
    }
    // Preserve manually supplied or replaced artifacts before the first write.
    // Keep one previous owned generation as well, so a bad compiler result can
    // always be recovered without accumulating every successful build.
    crate::portable::private_directory(project_root, Path::new(".texe/recovery"))?;
    let ownership_path = project_root.join(".texe/published.json");
    let mut ownership: std::collections::BTreeMap<String, String> = super::generated::optional(
        project_root,
        project_root,
        &ownership_path,
        super::generated::MAX_METADATA,
    )?
    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    .unwrap_or_default();
    let mut owned_previous = [None, None];
    for (position, path) in [&published, &published_synctex].into_iter().enumerate() {
        let key = path.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(previous) = super::generated::optional(
            project_root,
            project_root,
            path,
            super::generated::MAX_ARTIFACT,
        )? {
            let checksum = hex::encode(Sha256::digest(&previous));
            if ownership.get(&key) == Some(&checksum) {
                owned_previous[position] = Some(previous);
            } else {
                let reusable = project_root
                    .join(".texe/recovery")
                    .join(format!("{checksum}-{key}"));
                let existing = super::generated::optional(
                    project_root,
                    project_root,
                    &reusable,
                    super::generated::MAX_ARTIFACT,
                )?;
                if existing.as_ref() == Some(&previous) {
                    continue;
                }
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let recovery = if existing.is_none() {
                    reusable
                } else {
                    project_root
                        .join(".texe/recovery")
                        .join(format!("{stamp}-{}-{key}", std::process::id()))
                };
                super::generated::write(project_root, &recovery, &previous)?;
            }
        }
    }
    if owned_previous[0].is_some() {
        let recovery = project_root.join(".texe/recovery/last-build");
        crate::portable::private_directory(project_root, Path::new(".texe/recovery/last-build"))?;
        // Both files are snapshotted and fsynced before the publication commit.
        // Missing SyncTeX removes the older generation's unrelated mapping.
        super::generated::replace_files(
            project_root,
            &[
                (&recovery.join(name), owned_previous[0].as_deref()),
                (
                    &recovery.join(published_synctex.file_name().unwrap()),
                    owned_previous[1].as_deref(),
                ),
            ],
        )?;
    }
    ownership.insert(
        published
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        hex::encode(Sha256::digest(&bytes)),
    );
    let sync_key = published_synctex
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    if let Some(bytes) = &sync_bytes {
        ownership.insert(sync_key, hex::encode(Sha256::digest(bytes)));
    } else {
        ownership.remove(&sync_key);
    }
    let ownership_bytes = serde_json::to_vec(&ownership).map_err(|source| TexeError::Json {
        path: ownership_path.clone(),
        source,
    })?;
    // replace_files snapshots old destinations for rollback. Validate that
    // snapshot too, particularly the compiler-writable private execution lock.
    super::generated::optional(
        project_root,
        project_root,
        lock_path,
        super::generated::MAX_METADATA,
    )?;
    for path in [&ownership_path, lock_path, &published_synctex, &published] {
        super::generated::validate_parent(project_root, path)?;
    }
    // The PDF is the final commit point. Earlier writes roll back on failure.
    super::generated::replace_files(
        project_root,
        &[
            (&ownership_path, Some(&ownership_bytes)),
            (lock_path, Some(project_lock)),
            (&published_synctex, sync_bytes.as_deref()),
            (&published, Some(&bytes)),
        ],
    )?;
    // A cleanup failure cannot turn an already published build into a failure.
    for path in [internal, synctex.as_path()] {
        if let Err(error) = super::generated::remove(project_root, path) {
            eprintln!(
                "texe: warning: could not remove internal artifact {}: {error}",
                path.display()
            );
        }
    }
    Ok(PublishedBuild {
        artifact: published,
        synctex: sync_bytes.map(|_| published_synctex),
    })
}

/// A hosted PDF is stored separately from uploaded files, but local output has
/// a project filename. A figure or input with that name must remain a source.
fn reject_source_output_overlap(
    root: &Path,
    output: &Path,
    lock_bytes: &[u8],
) -> Result<(), TexeError> {
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(lock_bytes) else {
        return Ok(());
    };
    let packages = &lock["packages"];
    let mut paths = Vec::new();
    if let Some(path) = packages["root"].as_str() {
        paths.push(path);
    }
    if let Some(sources) = packages["sources"].as_array() {
        paths.extend(sources.iter().filter_map(|source| source["path"].as_str()));
    }
    for field in ["inputs", "graphics", "bibliographies"] {
        if let Some(inputs) = packages[field].as_array() {
            paths.extend(
                inputs
                    .iter()
                    .filter_map(|input| input["resolved_path"].as_str()),
            );
        }
    }
    let outputs = [output.to_owned(), output.with_extension("synctex.gz")];
    for source in paths {
        let source = root.join(source);
        let resolved = source.canonicalize().ok();
        if outputs
            .iter()
            .any(|output| source == *output || resolved.as_ref() == Some(output))
        {
            return Err(TexeError::Build(format!(
                "The output filename {} is also used as a research input. Use another filename for that input or the paper entry. Sources and the last PDF have been kept.",
                source.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn pdf_fixture(label: &str) -> Vec<u8> {
    let mut bytes = format!("%PDF-1.4\n% {label}\n").into_bytes();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        bytes.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    bytes
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn restoring_the_same_manual_pdf_reuses_its_recovery_copy() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let internal = root.join(".texe/build/output/main.pdf");
        fs::create_dir_all(internal.parent().unwrap()).unwrap();
        fs::write(root.join("main.tex"), b"research source").unwrap();
        for _ in 0..2 {
            fs::write(root.join("main.pdf"), b"%PDF-manual version").unwrap();
            fs::write(&internal, pdf_fixture("generated version")).unwrap();
            publish_artifact(root, &internal, b"build lock").unwrap();
        }
        let backups = fs::read_dir(root.join(".texe/recovery"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), b"%PDF-manual version");
        assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"research source");
    }

    #[test]
    fn a_primary_pdf_used_as_a_research_figure_is_never_overwritten() {
        for (field, key, alias) in [
            ("graphics", "resolved_path", false),
            ("inputs", "resolved_path", true),
            ("sources", "path", false),
            ("bibliographies", "resolved_path", true),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path();
            let figure = root.join("main.pdf");
            let original = pdf_fixture("research figure, not the built output");
            fs::write(&figure, &original).unwrap();
            let internal = root.join(".texe/build/output/main.pdf");
            fs::create_dir_all(internal.parent().unwrap()).unwrap();
            fs::write(&internal, pdf_fixture("new compiled paper")).unwrap();
            let source = if alias {
                std::os::unix::fs::symlink(&figure, root.join("figure-alias.pdf")).unwrap();
                "figure-alias.pdf"
            } else {
                "main.pdf"
            };
            let mut packages = serde_json::json!({"schema":"pqty.lock/v1"});
            let mut input = serde_json::json!({});
            input[key] = source.into();
            packages[field] = serde_json::json!([input]);
            // Composite classical locks and native descriptors both expose this
            // packages object; neither may overwrite its source or alias target.
            let lock = serde_json::to_vec(&serde_json::json!({"packages":packages})).unwrap();
            let error = publish_artifact_at(
                root,
                &internal,
                &lock,
                &root.join(".texe/build/execution.lock"),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("also used as a research input"),
                "{error}"
            );
            assert_eq!(fs::read(figure).unwrap(), original);
            assert!(internal.is_file());
            assert!(!root.join(".texe/recovery").exists());
        }
    }

    #[test]
    fn keeps_one_previous_owned_generation_and_matching_synctex() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let internal = root.join(".texe/build/output/main.pdf");
        fs::create_dir_all(internal.parent().unwrap()).unwrap();
        fs::write(root.join("main.tex"), b"research source").unwrap();
        let mut previous_pdf = Vec::new();
        let mut previous_sync = None;
        for generation in 0..4 {
            let pdf = pdf_fixture(&format!("generation {generation}"));
            let sync = if generation == 2 {
                None
            } else {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder
                    .write_all(format!("SyncTeX Version:1\nCount:{generation}\n").as_bytes())
                    .unwrap();
                Some(encoder.finish().unwrap())
            };
            fs::write(&internal, &pdf).unwrap();
            if let Some(sync) = &sync {
                fs::write(internal.with_extension("synctex.gz"), sync).unwrap();
            }
            publish_artifact(root, &internal, b"build lock").unwrap();
            if generation > 0 {
                let recovery = root.join(".texe/recovery/last-build");
                assert_eq!(fs::read(recovery.join("main.pdf")).unwrap(), previous_pdf);
                assert_eq!(
                    fs::read(recovery.join("main.synctex.gz")).ok(),
                    previous_sync
                );
                assert_eq!(
                    fs::read_dir(&recovery).unwrap().count(),
                    1 + usize::from(previous_sync.is_some())
                );
                assert_eq!(
                    fs::read_dir(root.join(".texe/recovery")).unwrap().count(),
                    1
                );
            }
            assert_eq!(fs::read(root.join("main.pdf")).unwrap(), pdf);
            previous_pdf = pdf;
            previous_sync = sync;
        }
        assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"research source");
    }

    #[test]
    fn recovery_failure_keeps_owned_pdf_synctex_and_lock() {
        for fault in ["directory-link", "pdf-link", "pdf-hardlink"] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("project");
            let internal = root.join(".texe/build/output/main.pdf");
            fs::create_dir_all(internal.parent().unwrap()).unwrap();
            let old = pdf_fixture("last good generation");
            fs::write(&internal, &old).unwrap();
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(b"SyncTeX Version:1\n").unwrap();
            let sync = encoder.finish().unwrap();
            fs::write(internal.with_extension("synctex.gz"), &sync).unwrap();
            publish_artifact(&root, &internal, b"old lock").unwrap();
            let outside = fixture.path().join("researcher-copy.pdf");
            fs::write(&outside, b"outside recovery source").unwrap();
            let recovery = root.join(".texe/recovery/last-build");
            if fault == "directory-link" {
                std::os::unix::fs::symlink(fixture.path(), &recovery).unwrap();
            } else {
                fs::create_dir(&recovery).unwrap();
                if fault == "pdf-link" {
                    std::os::unix::fs::symlink(&outside, recovery.join("main.pdf")).unwrap();
                } else {
                    fs::hard_link(&outside, recovery.join("main.pdf")).unwrap();
                }
            }
            fs::write(&internal, pdf_fixture("next generation")).unwrap();
            assert!(
                publish_artifact(&root, &internal, b"new lock").is_err(),
                "{fault}"
            );
            assert_eq!(fs::read(root.join("main.pdf")).unwrap(), old, "{fault}");
            assert_eq!(
                fs::read(root.join("main.synctex.gz")).unwrap(),
                sync,
                "{fault}"
            );
            assert_eq!(
                fs::read(root.join("texe.lock")).unwrap(),
                b"old lock",
                "{fault}"
            );
            assert_eq!(
                fs::read(&outside).unwrap(),
                b"outside recovery source",
                "{fault}"
            );
        }
    }

    #[test]
    fn rejects_incomplete_pdf_and_broken_cross_reference_before_publication() {
        let good = pdf_fixture("complete PDF");
        assert!(super::super::generated::valid_pdf(&good));
        for invalid in [
            b"%PDF-1.7\ntruncated compiler output".to_vec(),
            b"%PDF-1.7\nstartxref\n0\n%%EOF\n".to_vec(),
            [good.as_slice(), b"compiler failure after EOF"].concat(),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path();
            let internal = root.join(".texe/build/output/main.pdf");
            fs::create_dir_all(internal.parent().unwrap()).unwrap();
            fs::write(root.join("main.pdf"), &good).unwrap();
            fs::write(root.join("texe.lock"), b"old lock").unwrap();
            fs::write(&internal, &invalid).unwrap();
            assert!(publish_artifact(root, &internal, b"new lock").is_err());
            assert_eq!(fs::read(root.join("main.pdf")).unwrap(), good);
            assert_eq!(fs::read(root.join("texe.lock")).unwrap(), b"old lock");
        }
    }

    #[test]
    fn pdf_completion_guard_accepts_xref_streams_and_incremental_updates() {
        let mut stream_pdf = pdf_fixture("xref stream PDF");
        let xref = stream_pdf.len();
        stream_pdf.extend(
            b"5 0 obj\n<< /Type /XRef /Size 6 /Root 1 0 R /W [1 4 2] /Length 42 >>\nstream\n",
        );
        let mut entries = vec![0, 0, 0, 0, 0, 255, 255];
        let mut offsets = Vec::new();
        for object in 1..=4 {
            let marker = format!("{object} 0 obj\n");
            offsets.push(
                u32::try_from(
                    stream_pdf
                        .windows(marker.len())
                        .position(|part| part == marker.as_bytes())
                        .unwrap(),
                )
                .unwrap(),
            );
        }
        offsets.push(u32::try_from(xref).unwrap());
        for offset in offsets {
            entries.push(1);
            entries.extend(offset.to_be_bytes());
            entries.extend([0, 0]);
        }
        stream_pdf.extend(entries);
        stream_pdf.extend(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        assert!(super::super::generated::valid_pdf(&stream_pdf));
        let mut incremental = pdf_fixture("incremental PDF");
        let previous = incremental
            .windows(5)
            .position(|part| part == b"xref\n")
            .unwrap();
        let xref = incremental.len();
        incremental.extend(format!("xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n\0").as_bytes());
        assert!(super::super::generated::valid_pdf(&incremental));
        let ordinary_object = stream_pdf
            .windows(b"/Type /XRef".len())
            .position(|part| part == b"/Type /XRef")
            .unwrap();
        stream_pdf[ordinary_object..ordinary_object + b"/Type /XRef".len()]
            .copy_from_slice(b"/Type /Page");
        assert!(!super::super::generated::valid_pdf(&stream_pdf));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn output_aliases_never_publish_outside_bytes_or_replace_the_last_pdf() {
        for fault in [
            "pdf-link",
            "pdf-hardlink",
            "sync-link",
            "sync-hardlink",
            "directory-link",
            "lock-link",
            "oversize-pdf",
            "invalid-pdf",
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("project");
            let output = root.join(".texe/build/output");
            fs::create_dir_all(&output).unwrap();
            let internal = output.join("main.pdf");
            fs::write(&internal, pdf_fixture("new generation")).unwrap();
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(b"SyncTeX Version:1\n").unwrap();
            fs::write(
                internal.with_extension("synctex.gz"),
                encoder.finish().unwrap(),
            )
            .unwrap();
            fs::write(root.join("main.tex"), b"research source").unwrap();
            fs::write(root.join("main.pdf"), b"%PDF-last-good").unwrap();
            fs::write(root.join("texe.lock"), b"researcher lock").unwrap();
            fs::write(root.join(".texe/environment.json"), b"active pointer").unwrap();
            let outside = fixture.path().join("outside");
            fs::create_dir(&outside).unwrap();
            let sentinel = outside.join("main.pdf");
            fs::write(&sentinel, b"outside private sentinel").unwrap();
            let lock = root.join(".texe/build/execution.lock");
            match fault {
                "pdf-link" | "pdf-hardlink" => {
                    fs::remove_file(&internal).unwrap();
                    if fault == "pdf-link" {
                        std::os::unix::fs::symlink(&sentinel, &internal).unwrap();
                    } else {
                        fs::hard_link(&sentinel, &internal).unwrap();
                    }
                }
                "sync-link" | "sync-hardlink" => {
                    let sync = internal.with_extension("synctex.gz");
                    fs::remove_file(&sync).unwrap();
                    if fault == "sync-link" {
                        std::os::unix::fs::symlink(&sentinel, sync).unwrap();
                    } else {
                        fs::hard_link(&sentinel, sync).unwrap();
                    }
                }
                "directory-link" => {
                    fs::remove_dir_all(&output).unwrap();
                    std::os::unix::fs::symlink(&outside, &output).unwrap();
                }
                "lock-link" => {
                    std::os::unix::fs::symlink(&sentinel, &lock).unwrap();
                }
                "oversize-pdf" => {
                    fs::File::create(&internal)
                        .unwrap()
                        .set_len(super::super::generated::MAX_ARTIFACT + 1)
                        .unwrap();
                }
                "invalid-pdf" => {
                    fs::write(&internal, b"not a PDF").unwrap();
                }
                _ => unreachable!(),
            }
            let failure =
                publish_artifact_at(&root, &internal, b"new execution lock", &lock).unwrap_err();
            assert!(
                !failure.to_string().contains("outside private sentinel"),
                "{fault}"
            );
            assert_eq!(
                fs::read(root.join("main.tex")).unwrap(),
                b"research source",
                "{fault}"
            );
            assert_eq!(
                fs::read(root.join("main.pdf")).unwrap(),
                b"%PDF-last-good",
                "{fault}"
            );
            assert_eq!(
                fs::read(root.join("texe.lock")).unwrap(),
                b"researcher lock",
                "{fault}"
            );
            assert_eq!(
                fs::read(root.join(".texe/environment.json")).unwrap(),
                b"active pointer",
                "{fault}"
            );
            assert_eq!(
                fs::read(&sentinel).unwrap(),
                b"outside private sentinel",
                "{fault}"
            );
        }
    }
}
