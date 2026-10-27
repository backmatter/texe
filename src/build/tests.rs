#[test]
fn mirrors_nested_source_directories_for_include_auxiliaries() {
    let project = tempfile::tempdir().expect("temporary project");
    let build_root = project.path().join(".texe/build");
    let output = build_root.join("output");
    let texmf = project.path().join(".texe/texmf");
    fs::create_dir_all(project.path().join("Sources/chapters")).expect("source directories");
    fs::create_dir_all(&output).expect("output directory");
    fs::create_dir_all(texmf.join("tex/latex/example")).expect("package directory");

    mirror_project_directories(project.path(), &output, &[&build_root, &texmf])
        .expect("directory mirror");

    assert!(output.join("Sources/chapters").is_dir());
    assert!(!output.join(".texe/texmf/tex/latex/example").exists());
    assert!(!output.join(".texe/build/output").exists());
}

#[test]
fn directory_mirror_skips_old_builds_and_recovery_at_every_depth() {
    let project = tempfile::tempdir().expect("temporary project");
    let build_root = project.path().join(".texe/environments/current/build");
    let output = build_root.join("output");
    let texmf = project.path().join(".texe/environments/current/texmf");
    fs::create_dir_all(&output).unwrap();
    fs::create_dir_all(project.path().join("Sources/chapters")).unwrap();
    fs::create_dir_all(project.path().join("nested/chapters")).unwrap();
    for prefix in ["", "nested/"] {
        for private in [
            ".texe/environments/old/build/output/old-build/chapters",
            ".citet/recovery/saved-drafts",
            ".git/objects/ab",
        ] {
            fs::create_dir_all(project.path().join(format!("{prefix}{private}"))).unwrap();
        }
    }
    fs::write(project.path().join("main.tex"), b"research source").unwrap();
    fs::write(project.path().join("main.pdf"), b"last successful PDF").unwrap();
    fs::write(
        project
            .path()
            .join("nested/.citet/recovery/saved-drafts/draft.tex"),
        b"recovery draft",
    )
    .unwrap();

    mirror_project_directories(project.path(), &output, &[&build_root, &texmf]).unwrap();

    assert!(output.join("Sources/chapters").is_dir());
    assert!(output.join("nested/chapters").is_dir());
    for prefix in ["", "nested/"] {
        for private in [".texe", ".citet", ".git"] {
            assert!(!output.join(format!("{prefix}{private}")).exists());
        }
    }
    assert_eq!(
        fs::read(project.path().join("main.tex")).unwrap(),
        b"research source"
    );
    assert_eq!(
        fs::read(project.path().join("main.pdf")).unwrap(),
        b"last successful PDF"
    );
    assert_eq!(
        fs::read(
            project
                .path()
                .join("nested/.citet/recovery/saved-drafts/draft.tex")
        )
        .unwrap(),
        b"recovery draft"
    );
}

#[cfg(unix)]
#[test]
fn directory_mirror_rejects_a_symlink_without_touching_its_target() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let build_root = project.path().join(".texe/build");
    let output = build_root.join("output");
    let texmf = project.path().join(".texe/texmf");
    fs::create_dir_all(project.path().join("Sources/chapters")).unwrap();
    fs::create_dir_all(&output).unwrap();
    fs::write(outside.path().join("research.tex"), b"outside research").unwrap();
    fs::write(outside.path().join("research.pdf"), b"outside last PDF").unwrap();
    std::os::unix::fs::symlink(outside.path(), output.join("Sources")).unwrap();

    assert!(mirror_project_directories(project.path(), &output, &[&build_root, &texmf]).is_err());

    assert_eq!(
        fs::read(outside.path().join("research.tex")).unwrap(),
        b"outside research"
    );
    assert_eq!(
        fs::read(outside.path().join("research.pdf")).unwrap(),
        b"outside last PDF"
    );
    assert!(!outside.path().join("chapters").exists());
    assert!(
        fs::symlink_metadata(output.join("Sources"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn convergence_snapshot_tracks_nested_include_auxiliaries() {
    let directory = tempfile::tempdir().expect("temporary output");
    let nested = directory.path().join("chapters");
    fs::create_dir_all(&nested).expect("nested output directory");
    fs::write(directory.path().join("main.aux"), b"main").expect("main auxiliary");
    fs::write(nested.join("one.aux"), b"first").expect("nested auxiliary");
    fs::write(nested.join("ignored.log"), b"log").expect("non-convergence output");

    let first = auxiliary_snapshot(directory.path()).expect("first snapshot");
    assert_eq!(first.get(Path::new("main.aux")), Some(&b"main".to_vec()));
    assert_eq!(
        first.get(Path::new("chapters/one.aux")),
        Some(&b"first".to_vec())
    );
    assert!(!first.contains_key(Path::new("chapters/ignored.log")));

    fs::write(nested.join("one.aux"), b"second").expect("changed nested auxiliary");
    let second = auxiliary_snapshot(directory.path()).expect("second snapshot");
    assert_ne!(first, second);
}

#[test]
fn frozen_texinputs_has_no_system_fallback() {
    let value = package_search_path_value(
        Path::new("/tmp/project"),
        Path::new("/tmp/project/.texe/build/output"),
        Path::new("/tmp/texmf"),
        &[],
        false,
    );
    let value = value.to_string_lossy();
    assert!(value.starts_with("/tmp/project/.texe/build/output/.texe-generated//"));
    assert!(value.contains("/tmp/project/.texe/build/output//"));
    assert!(value.contains("/tmp/texmf//"));
    assert!(!value.ends_with(if cfg!(windows) { ';' } else { ':' }));
}

#[test]
fn discovery_texinputs_has_system_fallback() {
    let value = package_search_path_value(
        Path::new("/tmp/project"),
        Path::new("/tmp/project/.texe/build/discovery"),
        Path::new("/tmp/texmf"),
        &[],
        true,
    );
    let value = value.to_string_lossy();
    assert!(value.ends_with(if cfg!(windows) { ';' } else { ':' }));
}

#[test]
fn discovery_collects_multiple_errors_while_final_passes_fail_fast() {
    let discovery = engine_interaction_arguments(false);
    assert!(discovery.contains(&OsString::from("-interaction=nonstopmode")));
    assert!(!discovery.contains(&OsString::from("-halt-on-error")));

    let final_pass = engine_interaction_arguments(true);
    assert!(final_pass.contains(&OsString::from("-halt-on-error")));
}

#[test]
fn package_retries_do_not_consume_the_auxiliary_pass_budget() {
    let mut budget = AuxiliaryPassBudget::new(2);

    assert_eq!(budget.next(), Some(1));
    // A package convergence retry does not record a stable-environment pass.
    assert_eq!(budget.next(), Some(1));

    budget.record_stable_environment_pass();
    assert_eq!(budget.next(), Some(2));
    budget.record_stable_environment_pass();
    assert_eq!(budget.next(), None);
}

#[test]
fn generated_inputs_are_private_and_stale_files_are_removed() {
    let directory = tempfile::tempdir().expect("temporary output");
    let output = directory.path().join("output");
    fs::create_dir_all(&output).expect("output directory");
    let first = [GeneratedInput {
        path: PathBuf::from("nested/Version.tex"),
        content: "first".to_string(),
    }];
    write_generated_inputs(&output, &first).expect("first generated input");
    let generated = output.join(".texe-generated/nested/Version.tex");
    assert_eq!(fs::read_to_string(&generated).expect("generated"), "first");

    write_generated_inputs(&output, &[]).expect("clear generated inputs");
    assert!(!generated.exists());
    assert!(!output.join(".texe-generated").exists());
}

fn system_toolchain() -> ResolvedToolchain {
    ResolvedToolchain {
        provider: "system".to_string(),
        engine: "pdflatex".to_string(),
        engine_executable: PathBuf::from("/usr/bin/pdflatex"),
        kpsewhich_executable: PathBuf::from("/usr/bin/kpsewhich"),
        texmf_dist: PathBuf::from("/usr/share/texmf-dist"),
        engine_roots: Vec::new(),
        identity: crate::toolchain::ToolchainIdentity {
            provider: "system".to_string(),
            engine: "pdflatex".to_string(),
            channel: "system".to_string(),
            target: "test".to_string(),
            fingerprint: "test".to_string(),
            registry_url: None,
            registry_metadata_digest: None,
            artifacts: Vec::new(),
        },
        managed: None,
        verification: crate::toolchain::VerificationPolicy::Interval,
        offline: false,
    }
}

#[test]
fn engine_environment_pins_every_observable_clock() {
    let toolchain = system_toolchain();
    let environment = engine_environment(
        &toolchain,
        &EngineEnvironmentContext {
            working_directory: Path::new("/tmp/project"),
            texmf: Path::new("/tmp/project/.texe/texmf"),
            build_root: Path::new(".texe/build"),
            input_roots: &[],
            managed_format: None,
            discovery: false,
            source_date_epoch: 1_700_000_000,
            shell_escape: false,
        },
    )
    .into_iter()
    .map(|(name, value)| {
        (
            name.to_string_lossy().to_string(),
            value.to_string_lossy().to_string(),
        )
    })
    .collect::<BTreeMap<_, _>>();

    assert_eq!(
        environment.get("SOURCE_DATE_EPOCH").map(String::as_str),
        Some("1700000000")
    );
    assert_eq!(
        environment.get("FORCE_SOURCE_DATE").map(String::as_str),
        Some("1")
    );
}

#[test]
fn managed_lualatex_environment_confines_font_inputs_and_cache() {
    let mut toolchain = system_toolchain();
    toolchain.provider = "managed".to_string();
    toolchain.engine = "lualatex".to_string();
    toolchain.managed = Some(crate::toolchain::ManagedRuntime {
        snapshot: "texlive-2026-07-26".to_string(),
        root: PathBuf::from("/data/texe/toolchains/luahbtex"),
        binary_dir: PathBuf::from("/data/texe/toolchains/luahbtex/bin/x86_64-linux"),
        format_cache: PathBuf::from("/data/texe/formats"),
        component_cache: PathBuf::from("/data/texe/components"),
        downloads: PathBuf::from("/data/texe/downloads"),
        registry_url: "https://example.invalid/texlive.tlpdb.xz".to_string(),
        registry_metadata_sha256: "00".repeat(32),
        bootstrap_providers: Vec::new(),
        verification: crate::toolchain::VerificationPolicy::Interval,
        offline: false,
    });
    let format = ManagedFormat {
        root: PathBuf::from("/data/texe/formats/lualatex"),
        formats: PathBuf::from("/data/texe/formats/lualatex/formats"),
    };
    let environment = engine_environment(
        &toolchain,
        &EngineEnvironmentContext {
            working_directory: Path::new("/work/project"),
            texmf: Path::new("/work/project/.texe/texmf"),
            build_root: Path::new("/work/project/.texe/build/output"),
            input_roots: &[],
            managed_format: Some(&format),
            discovery: false,
            source_date_epoch: 1_700_000_000,
            shell_escape: false,
        },
    )
    .into_iter()
    .map(|(name, value)| {
        (
            name.to_string_lossy().to_string(),
            value.to_string_lossy().to_string(),
        )
    })
    .collect::<BTreeMap<_, _>>();

    let expected_osfontdir = Path::new("/work/project/.texe/texmf")
        .join("fonts")
        .to_string_lossy()
        .replace('\\', "/");
    let expected_cache = Path::new("/work/project/.texe/build/output")
        .join("texmf-var")
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(
        environment.get("OSFONTDIR").map(String::as_str),
        Some(expected_osfontdir.as_str())
    );
    assert_eq!(
        environment.get("TEXMFCACHE").map(String::as_str),
        Some(expected_cache.as_str())
    );
    assert!(
        environment
            .get("LUAINPUTS")
            .is_some_and(|value| value.contains("/data/texe/formats/lualatex/config//"))
    );
}

#[test]
fn locked_system_format_uses_generated_inputs_without_host_tex_fallback() {
    let mut toolchain = system_toolchain();
    toolchain.engine = "xelatex".to_string();
    let format = ManagedFormat {
        root: PathBuf::from("/work/project/.texe/build/formats/xelatex"),
        formats: PathBuf::from("/work/project/.texe/build/formats/xelatex/formats"),
    };
    let environment = engine_environment(
        &toolchain,
        &EngineEnvironmentContext {
            working_directory: Path::new("/work/project"),
            texmf: Path::new("/work/project/.texe/texmf"),
            build_root: Path::new("/work/project/.texe/build/output"),
            input_roots: &[PathBuf::from("colorthemes")],
            managed_format: Some(&format),
            discovery: true,
            source_date_epoch: 1_700_000_000,
            shell_escape: false,
        },
    )
    .into_iter()
    .map(|(name, value)| {
        (
            name.to_string_lossy().to_string(),
            value.to_string_lossy().to_string(),
        )
    })
    .collect::<BTreeMap<_, _>>();

    let texinputs = environment.get("TEXINPUTS").expect("TEXINPUTS");
    assert!(texinputs.starts_with("/work/project/.texe/build/output/.texe-generated//"));
    assert!(texinputs.contains("colorthemes//"));
    assert!(texinputs.contains("/work/project/.texe/texmf//"));
    assert!(!texinputs.ends_with(if cfg!(windows) { ';' } else { ':' }));
    assert!(
        environment
            .get("OSFONTDIR")
            .is_some_and(|value| value.contains("/work/project/.texe/texmf/fonts//"))
    );
    let expected_fontconfig = Path::new("/work/project/.texe/build/output")
        .join("fontconfig.conf")
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(
        environment.get("FONTCONFIG_FILE").map(String::as_str),
        Some(expected_fontconfig.as_str())
    );
    assert!(!environment.contains_key("PATH"));
}

#[test]
fn system_fontconfig_layers_locked_fonts_over_host_fonts() {
    let directory = tempfile::tempdir().expect("temporary project");
    let output = directory.path().join("output");
    let texmf = directory.path().join("tree&locked");
    write_system_fontconfig(&output, &texmf).expect("font configuration");
    let configuration = fs::read_to_string(output.join("fontconfig.conf")).expect("configuration");
    let locked_fonts = texmf.join("fonts").to_string_lossy().replace('&', "&amp;");
    let cache = output.join("fontconfig-cache");
    assert!(configuration.contains("/etc/fonts/fonts.conf"));
    assert!(configuration.contains(&locked_fonts));
    assert!(configuration.contains(cache.to_string_lossy().as_ref()));
}

#[test]
fn an_inherited_timestamp_renders_a_build_without_repinning_the_project() {
    // A project that has never locked pins whatever its first build used.
    assert_eq!(
        build_timestamp(None, None, || 1_700_000_000),
        BuildTimestamp {
            effective: 1_700_000_000,
            locked: 1_700_000_000
        }
    );
    assert_eq!(
        build_timestamp(None, Some(2_000), || 1_700_000_000),
        BuildTimestamp {
            effective: 2_000,
            locked: 2_000
        }
    );

    // A locked project reuses its pinned value, which keeps date-dependent PDF
    // fields stable on another machine.
    assert_eq!(
        build_timestamp(Some(1_000), None, || 1_700_000_000),
        BuildTimestamp {
            effective: 1_000,
            locked: 1_000
        }
    );

    // An override changes what this build renders and nothing else. A
    // global SOURCE_DATE_EPOCH — Nix, Guix, reproducible-build CI — must
    // not rewrite a committed lock and permanently change what `\today`
    // renders for every later build on every machine.
    assert_eq!(
        build_timestamp(Some(1_000), Some(2_000), || 1_700_000_000),
        BuildTimestamp {
            effective: 2_000,
            locked: 1_000
        }
    );
}

#[test]
fn an_engine_failure_reports_the_diagnostic_not_the_memory_statistics() {
    let log = "\
(/texmf/tex/latex/base/article.cls)
! Undefined control sequence.
l.3 \\undefinedmacro

The control sequence at the end of the top line
was never \\def'ed.

Here is how much of TeX's memory you used:
 419 strings out of 469515
 433756 words of memory out of 5000000
 35i,0n,38p,257b,37s stack positions out of 10000i
!  ==> Fatal error occurred, no output PDF file produced!
";
    let excerpt = engine_log_excerpt(log);

    assert!(excerpt.starts_with("! Undefined control sequence."));
    assert!(excerpt.contains("l.3 \\undefinedmacro"));
    assert!(
        excerpt.contains("Fatal error occurred"),
        "the verdict survives the statistics block"
    );
    assert!(!excerpt.contains("strings out of"));
    assert!(!excerpt.contains("stack positions"));
    assert!(!excerpt.contains("article.cls"), "noise above the error");
}

#[test]
fn an_engine_failure_keeps_pdf_driver_stderr() {
    let detail = engine_failure_detail(
        "! LaTeX Warning: rerun.\n",
        "duplicate transcript",
        "xdvipdfmx:fatal: Unrecognized paper format: a4\n",
    );
    assert!(detail.contains("LaTeX Warning"));
    assert!(detail.contains("xdvipdfmx:fatal"));
    assert!(!detail.contains("duplicate transcript"));
}

#[test]
fn a_log_without_a_diagnostic_falls_back_to_its_tail() {
    let log = (1..=60)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let excerpt = engine_log_excerpt(&log);
    assert_eq!(excerpt.lines().count(), LOG_EXCERPT_LINES);
    assert!(excerpt.ends_with("line 60"));
    assert!(!excerpt.contains("line 30\n"));
}

#[test]
fn warnings_are_deduplicated_and_prioritized_for_scientists() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let log = directory.path().join("main.log");
    fs::write(
        &log,
        "\
Overfull \\hbox (2.0pt too wide)
LaTeX Warning: Label(s) may have changed.
LaTeX Warning: Citation `paper' on page 1 undefined.
LaTeX Warning: Reference `result' on page 1 undefined.
Missing character: There is no α in font cmr10!
LaTeX Warning: Citation `paper' on page 1 undefined.
",
    )
    .expect("log");
    let warnings = collect_warnings(&log);
    assert_eq!(warnings.len(), 5);
    assert_eq!(warnings[0].kind, "unresolved-citation");
    assert_eq!(warnings[1].kind, "unresolved-reference");
    assert_eq!(warnings[2].kind, "missing-character");
    assert_eq!(warnings[3].kind, "latex");
    assert_eq!(warnings[4].kind, "layout");
}

#[test]
fn publishes_only_the_final_artifact_at_the_project_root() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let internal = directory.path().join(".texe/build/output/main.pdf");
    fs::create_dir_all(internal.parent().expect("output parent")).expect("output directory");
    let pdf = crate::build::artifact::pdf_fixture("test artifact");
    fs::write(&internal, &pdf).expect("internal artifact");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, b"SyncTeX Version:1\n").unwrap();
    let sync = encoder.finish().unwrap();
    fs::write(internal.with_extension("synctex.gz"), &sync).expect("internal SyncTeX");

    let published =
        publish_artifact(directory.path(), &internal, b"new lock").expect("artifact is published");

    assert_eq!(published.artifact, directory.path().join("main.pdf"));
    assert_eq!(
        published.synctex,
        Some(directory.path().join("main.synctex.gz"))
    );
    assert_eq!(published.paths().len(), 2);
    assert_eq!(fs::read(&published.artifact).expect("published PDF"), pdf);
    assert_eq!(
        fs::read(directory.path().join("main.synctex.gz")).expect("published SyncTeX"),
        sync
    );
    assert!(!internal.exists());
    assert!(!internal.with_extension("synctex.gz").exists());
}
#[test]
fn publication_failure_preserves_the_previous_pdf_and_lock() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    let internal = root.join(".texe/build/output/main.pdf");
    fs::create_dir_all(internal.parent().expect("parent")).expect("output");
    fs::write(
        &internal,
        crate::build::artifact::pdf_fixture("new artifact"),
    )
    .expect("PDF");
    fs::write(root.join("main.pdf"), b"old PDF").expect("previous PDF");
    fs::write(root.join("texe.lock"), b"old lock").expect("previous lock");
    fs::create_dir(root.join("main.synctex.gz")).expect("blocked SyncTeX destination");
    assert!(publish_artifact(root, &internal, b"new lock").is_err());
    assert_eq!(fs::read(root.join("main.pdf")).expect("PDF"), b"old PDF");
    assert_eq!(fs::read(root.join("texe.lock")).expect("lock"), b"old lock");
    assert!(internal.is_file());
}

#[test]
fn publication_removes_stale_synctex_when_the_engine_did_not_produce_it() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    let internal = root.join(".texe/build/output/main.pdf");
    fs::create_dir_all(internal.parent().expect("parent")).expect("output");
    fs::write(
        &internal,
        crate::build::artifact::pdf_fixture("new artifact"),
    )
    .expect("PDF");
    fs::write(root.join("main.synctex.gz"), b"stale sync").expect("old SyncTeX");
    let published = publish_artifact(root, &internal, b"new lock").expect("publish");
    assert!(published.synctex.is_none());
    assert!(!root.join("main.synctex.gz").exists());
    assert_eq!(fs::read(root.join("texe.lock")).expect("lock"), b"new lock");
}

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::build::AuxiliaryPassBudget;
use crate::build::artifact::{BuildTimestamp, build_timestamp, publish_artifact};
use crate::build::engine::{
    engine_interaction_arguments, write_generated_inputs, write_system_fontconfig,
};
use crate::build::environment::{EngineEnvironmentContext, engine_environment};
use crate::build::errors::{LOG_EXCERPT_LINES, engine_failure_detail, engine_log_excerpt};
use crate::build::filesystem::{
    auxiliary_snapshot, mirror_project_directories, package_search_path_value,
};
use crate::build::format::ManagedFormat;
use crate::build::warnings::collect_warnings;
use crate::config::GeneratedInput;
use crate::toolchain::ResolvedToolchain;

#[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
#[test]
fn protected_local_validation_rejects_source_aliases_before_any_parent_write() {
    for fault in ["entry", "inputs", "bibliography", "build", "texmf", "lock"] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let outside = fixture.path().join("research");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let original = b"research worth retaining";
        fs::write(root.join("main.tex"), original).unwrap();
        fs::write(root.join("main.pdf"), b"last good PDF").unwrap();
        fs::write(outside.join("notes.tex"), original).unwrap();
        let mut manifest: crate::config::ProjectManifest =
            toml::from_str("schema='texe.project/v1'\n[project]\nentry='main.tex'\n[toolchain]\nengine='pdflatex'\n").unwrap();
        if matches!(fault, "entry" | "inputs" | "bibliography") {
            let output = root.join(".texe/build/output");
            fs::create_dir_all(&output).unwrap();
            fs::write(output.join("notes.tex"), original).unwrap();
            if fault == "entry" {
                fs::remove_file(root.join("main.tex")).unwrap();
                std::os::unix::fs::symlink(output.join("notes.tex"), root.join("main.tex"))
                    .unwrap();
            } else {
                std::os::unix::fs::symlink(&output, root.join("source-alias")).unwrap();
                if fault == "inputs" {
                    manifest.inputs.roots.push("source-alias".into());
                } else {
                    manifest.bibliography.roots.push("source-alias".into());
                }
            }
        } else {
            fs::create_dir(root.join(".texe")).unwrap();
            let target = match fault {
                "build" => &manifest.project.build_dir,
                "texmf" => &manifest.packages.texmf,
                "lock" => &manifest.packages.lock,
                _ => unreachable!(),
            };
            fs::create_dir_all(root.join(target).parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(
                if fault == "lock" {
                    outside.join("notes.tex")
                } else {
                    outside.clone()
                },
                root.join(target),
            )
            .unwrap();
        }
        let before = fs::read_dir(&outside).unwrap().count();
        assert!(
            super::protected_manifest(&root, &manifest).is_err(),
            "{fault}"
        );
        assert_eq!(
            fs::read(root.join("main.tex")).unwrap(),
            original,
            "{fault}"
        );
        assert_eq!(
            fs::read(root.join("main.pdf")).unwrap(),
            b"last good PDF",
            "{fault}"
        );
        assert_eq!(
            fs::read(outside.join("notes.tex")).unwrap(),
            original,
            "{fault}"
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), before, "{fault}");
        assert!(!root.join(".texe/project").exists(), "{fault}");
    }
}

#[test]
fn explicit_protection_is_scoped_and_restores_nested_invocations() {
    assert!(!super::process::protection_requested());
    {
        let _outer = super::process::ProtectionScope::enter(true);
        assert!(super::process::protection_requested());
        {
            let _inner = super::process::ProtectionScope::enter(false);
            assert!(super::process::protection_requested());
        }
        assert!(super::process::protection_requested());
    }
    assert!(!super::process::protection_requested());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn protected_local_layout_keeps_declared_hidden_paths_and_general_source_root() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("main.tex"), b"research source").unwrap();
    let manifest: crate::config::ProjectManifest = toml::from_str(
        "schema='texe.project/v1'\n[project]\nentry='main.tex'\nbuild_dir='.texe/custom-build'\n[toolchain]\nengine='lualatex'\n[inputs]\nroots=['.']\n",
    ).unwrap();
    let effective = super::protected_manifest(root, &manifest).unwrap();
    assert_eq!(effective, manifest);
    assert!(!root.join(".texe").exists());
    assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"research source");
    for reserved in [
        ".texe/recovery",
        ".texe/editor",
        ".texe/project",
        ".texe/environments",
        ".texe/runtime",
    ] {
        let mut conflicting = manifest.clone();
        conflicting.project.build_dir = reserved.into();
        assert!(
            super::protected_manifest(root, &conflicting).is_err(),
            "{reserved}"
        );
        assert!(!root.join(".texe").exists(), "{reserved}");
    }
}

#[test]
fn finds_artifacts_without_stripping_dotted_jobnames() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("paper.v2.pdf"), b"%PDF").unwrap();
    assert_eq!(
        super::filesystem::find_artifact(directory.path(), Path::new("sources/paper.v2.tex")),
        Some(directory.path().join("paper.v2.pdf"))
    );
}

#[test]
fn changing_sources_between_render_and_cache_record_forces_the_next_render() {
    for during_record in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("paper.v2.tex"), b"\\input{chapter one.tex}").unwrap();
        fs::write(root.join("chapter one.tex"), b"original research").unwrap();
        let manifest: crate::ProjectManifest = toml::from_str(
            "schema='texe.project/v1'\n[project]\nentry='paper.v2.tex'\n[toolchain]\nengine='pdflatex'\n",
        ).unwrap();
        let state_path = root.join(".texe/build").join(crate::state::STATE_NAME);
        let pdf = root.join("paper.v2.pdf");
        fs::write(&pdf, b"derived from original research").unwrap();
        crate::state::write(
            &state_path,
            "previous",
            "environment",
            root,
            std::slice::from_ref(&pdf),
        )
        .unwrap();
        let rendered = crate::state::source_fingerprint(root, &manifest)
            .unwrap()
            .unwrap();
        if !during_record {
            fs::write(
                root.join("chapter one.tex"),
                b"new research saved during rendering",
            )
            .unwrap();
        }
        let called = std::cell::Cell::new(false);
        super::record_if_sources_unchanged(root, &manifest, &state_path, Some(&rendered), || {
            called.set(true);
            fs::write(
                root.join("chapter one.tex"),
                b"new research saved during rendering",
            )
            .unwrap();
            crate::state::write(
                &state_path,
                "incorrect post-render fingerprint",
                "environment",
                root,
                std::slice::from_ref(&pdf),
            )
        })
        .unwrap();
        assert_eq!(called.get(), during_record);
        assert!(
            crate::state::read(&state_path).is_none(),
            "the next build must miss the no-op cache"
        );
        assert_eq!(fs::read(&pdf).unwrap(), b"derived from original research");
        assert_eq!(
            fs::read(root.join("chapter one.tex")).unwrap(),
            b"new research saved during rendering"
        );

        // The queued retry can record only the PDF rendered from those new
        // bytes; dotted output names must stay outside the source snapshot.
        let retry_sources = crate::state::source_fingerprint(root, &manifest)
            .unwrap()
            .unwrap();
        fs::write(&pdf, b"derived from new research").unwrap();
        super::record_if_sources_unchanged(
            root,
            &manifest,
            &state_path,
            Some(&retry_sources),
            || {
                crate::state::write(
                    &state_path,
                    "new render",
                    "environment",
                    root,
                    std::slice::from_ref(&pdf),
                )
            },
        )
        .unwrap();
        assert_eq!(
            crate::state::read(&state_path).unwrap().fingerprint,
            "new render"
        );
    }
}

#[test]
fn edit_and_revert_during_cache_record_is_detected_even_with_the_original_mtime() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("main.tex");
    fs::write(&source, b"original source").unwrap();
    let original_mtime = fs::metadata(&source).unwrap().modified().unwrap();
    let manifest: crate::ProjectManifest = toml::from_str(
        "schema='texe.project/v1'\n[project]\nentry='main.tex'\n[toolchain]\nengine='pdflatex'\n",
    )
    .unwrap();
    let pdf = root.join("main.pdf");
    fs::write(&pdf, b"PDF from transient source").unwrap();
    let state_path = root.join(".texe/build").join(crate::state::STATE_NAME);
    let rendered = crate::state::source_fingerprint(root, &manifest)
        .unwrap()
        .unwrap();
    super::record_if_sources_unchanged(root, &manifest, &state_path, Some(&rendered), || {
        fs::write(&source, b"transient source").unwrap();
        let replacement = root.join("save-replacement");
        fs::write(&replacement, b"original source").unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original_mtime))
            .unwrap();
        fs::rename(&replacement, &source).unwrap();
        crate::state::write(
            &state_path,
            "incorrect reverted-source fingerprint",
            "environment",
            root,
            std::slice::from_ref(&pdf),
        )
    })
    .unwrap();
    assert!(crate::state::read(&state_path).is_none());
    assert_eq!(fs::read(&source).unwrap(), b"original source");
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        original_mtime
    );
    assert_eq!(fs::read(&pdf).unwrap(), b"PDF from transient source");
}

#[test]
fn an_unavailable_render_source_snapshot_disables_and_removes_the_no_op_cache() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("main.tex"), b"research source").unwrap();
    let pdf = root.join("main.pdf");
    fs::write(&pdf, b"last PDF").unwrap();
    let manifest: crate::ProjectManifest = toml::from_str(
        "schema='texe.project/v1'\n[project]\nentry='main.tex'\n[toolchain]\nengine='pdflatex'\n",
    )
    .unwrap();
    let state_path = root.join(".texe/build").join(crate::state::STATE_NAME);
    crate::state::write(
        &state_path,
        "previous",
        "environment",
        root,
        std::slice::from_ref(&pdf),
    )
    .unwrap();
    super::record_if_sources_unchanged(root, &manifest, &state_path, None, || {
        panic!("a failed/bounded source snapshot cannot record a reusable build")
    })
    .unwrap();
    assert!(crate::state::read(&state_path).is_none());
    assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"research source");
    assert_eq!(fs::read(pdf).unwrap(), b"last PDF");
}

#[test]
fn changed_and_reverted_render_inputs_keep_the_previous_pdf_and_retry_publication() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("main.tex");
    fs::write(&source, b"original researcher source").unwrap();
    let manifest: crate::ProjectManifest = toml::from_str(
        "schema='texe.project/v1'\n[project]\nentry='main.tex'\n[toolchain]\nengine='pdflatex'\n",
    )
    .unwrap();
    let last_pdf = crate::build::artifact::pdf_fixture("previous complete research");
    fs::write(root.join("main.pdf"), &last_pdf).unwrap();
    fs::write(root.join("texe.lock"), b"researcher-owned lock").unwrap();
    let internal = root.join(".texe/build/output/main.pdf");
    fs::create_dir_all(internal.parent().unwrap()).unwrap();
    fs::write(
        &internal,
        crate::build::artifact::pdf_fixture("transient source render"),
    )
    .unwrap();
    let rendered = super::capture_render_sources(root, &manifest).unwrap();
    let original_time = fs::metadata(&source).unwrap().modified().unwrap();
    fs::write(&source, b"transient input seen by the compiler").unwrap();
    let replacement = root.join("save-replacement");
    fs::write(&replacement, b"original researcher source").unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_time))
        .unwrap();
    fs::rename(&replacement, &source).unwrap();
    let called = std::cell::Cell::new(false);
    let lock = root.join(".texe/build/texe.lock");
    let error = super::publish_if_sources_unchanged(root, &manifest, &rendered, || {
        called.set(true);
        super::artifact::publish_artifact_at(root, &internal, b"execution lock", &lock)
    })
    .unwrap_err();
    assert!(matches!(error, crate::TexeError::SourcesChanged));
    assert!(
        !called.get(),
        "an unstable render must never enter publication"
    );
    assert_eq!(fs::read(root.join("main.pdf")).unwrap(), last_pdf);
    assert_eq!(fs::read(&source).unwrap(), b"original researcher source");
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        original_time
    );
    assert_eq!(
        fs::read(root.join("texe.lock")).unwrap(),
        b"researcher-owned lock"
    );
    assert!(internal.is_file());
    assert!(!lock.exists());
    assert!(!root.join(".texe/recovery").exists());

    let stable = super::capture_render_sources(root, &manifest).unwrap();
    let retry_pdf = super::artifact::pdf_fixture("fresh original source render");
    fs::write(&internal, &retry_pdf).unwrap();
    super::publish_if_sources_unchanged(root, &manifest, &stable, || {
        super::artifact::publish_artifact_at(root, &internal, b"execution lock", &lock)
    })
    .unwrap();
    assert_eq!(fs::read(root.join("main.pdf")).unwrap(), retry_pdf);
    assert_eq!(
        fs::read(root.join("texe.lock")).unwrap(),
        b"researcher-owned lock"
    );
}
