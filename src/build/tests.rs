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
    fs::write(&internal, b"%PDF-test").expect("internal artifact");
    fs::write(internal.with_extension("synctex.gz"), b"sync").expect("internal SyncTeX");

    let published = publish_artifact(directory.path(), &internal).expect("artifact is published");

    assert_eq!(published.artifact, directory.path().join("main.pdf"));
    assert_eq!(
        published.synctex,
        Some(directory.path().join("main.synctex.gz"))
    );
    assert_eq!(published.paths().len(), 2);
    assert_eq!(
        fs::read(&published.artifact).expect("published PDF"),
        b"%PDF-test"
    );
    assert_eq!(
        fs::read(directory.path().join("main.synctex.gz")).expect("published SyncTeX"),
        b"sync"
    );
    assert!(!internal.exists());
    assert!(!internal.with_extension("synctex.gz").exists());
}
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

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
