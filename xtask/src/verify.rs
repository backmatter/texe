use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use crate::command::{
    ScratchDir, append, cargo, clean_environment, copy_file, copy_tree, executable_name, nonempty,
    on_path, read_text, repo_root, require, require_absent, require_contains, run,
};
use crate::{Result, message, pqty};

const CASES: &[&str] = &["managed", "luatex", "common", "bibliography", "index"];

pub(crate) fn all() -> Result<()> {
    let repo = repo_root()?;
    println!("== Rust workspace");
    run(cargo()
        .current_dir(&repo)
        .args(["fmt", "--all", "--", "--check"]))?;
    run(cargo().current_dir(&repo).args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(cargo()
        .current_dir(&repo)
        .args(["test", "--workspace", "--locked"]))?;
    let mut docs = cargo();
    docs.current_dir(&repo)
        .env("RUSTDOCFLAGS", "-D warnings -D missing-docs")
        .args(["doc", "--workspace", "--no-deps", "--locked"]);
    run(&mut docs)?;
    let suite = SuiteBinaries::build()?;
    for selected in CASES {
        println!("== {selected}");
        case_with_suite(selected, &suite)?;
    }
    Ok(())
}

pub(crate) fn case(selected: &str, suite_bin: Option<&Path>) -> Result<()> {
    match selected {
        "platform" => platform(suite_bin),
        "managed" | "luatex" | "common" | "bibliography" | "index" | "local" => {
            let suite = SuiteBinaries::build()?;
            case_with_suite(selected, &suite)
        }
        _ => Err(message(format!("unknown verification case `{selected}`"))),
    }
}

fn case_with_suite(selected: &str, suite: &SuiteBinaries) -> Result<()> {
    match selected {
        "managed" => managed(suite),
        "luatex" => luatex(suite),
        "common" => common(suite),
        "bibliography" => bibliography(suite),
        "index" => index(suite),
        "local" => local(suite),
        _ => Err(message(format!(
            "`{selected}` is not an acceptance verification case"
        ))),
    }
}

fn platform(selected_bin: Option<&Path>) -> Result<()> {
    let repo = repo_root()?;
    let scratch = ScratchDir::new("platform")?;
    let root = scratch.path();
    for directory in ["bin", "home", "cache", "data/texe", "tmp"] {
        fs::create_dir_all(root.join(directory))?;
    }
    let bin = root.join("bin");
    let sources = if let Some(selected) = selected_bin {
        SuiteBinaries::from_directory(selected)?
    } else {
        SuiteBinaries::build()?
    };
    sources.install(&bin)?;

    let empty = root.join("Research Δ Results");
    let basic = root.join("Basic Paper");
    let biber = root.join("Biber References");
    successful(&texe(
        root,
        &bin,
        &[
            "init",
            path(&empty)?,
            "--yes",
            "--template",
            "empty",
            "--engine",
            "pdflatex",
            "--title",
            "Particle Results",
            "--author",
            "Ada Ångström",
        ],
    )?)?;
    let empty_first = texe(
        root,
        &bin,
        &["build", "--project", path(&empty)?, "--yes", "--json"],
    )?;
    successful(&empty_first)?;
    require_schema(&empty_first, "texe.build-report/v1")?;
    nonempty(&empty.join("main.pdf"))?;
    copy_file(&empty.join("main.pdf"), &root.join("empty-first.pdf"))?;

    successful(&texe(
        root,
        &bin,
        &[
            "init",
            path(&basic)?,
            "--yes",
            "--template",
            "basic",
            "--engine",
            "lualatex",
            "--title",
            "A Reproducible Study",
            "--author",
            "Grace Researcher",
        ],
    )?)?;
    let basic_first = texe(
        root,
        &bin,
        &["build", "--project", path(&basic)?, "--yes", "--json"],
    )?;
    successful(&basic_first)?;
    require_schema(&basic_first, "texe.build-report/v1")?;
    nonempty(&basic.join("main.pdf"))?;
    nonempty(&basic.join("references.bib"))?;
    copy_file(&basic.join("main.pdf"), &root.join("basic-first.pdf"))?;

    fs::create_dir_all(&biber)?;
    copy_tree(&repo.join("examples/biber"), &biber)?;
    let biber_report = texe(
        root,
        &bin,
        &["build", "--project", path(&biber)?, "--yes", "--json"],
    )?;
    successful(&biber_report)?;
    require(
        json(&biber_report)?["bibliography_runs"] == 1,
        "Biber journey did not run bibliography exactly once",
    )?;
    nonempty(&biber.join("main.pdf"))?;

    successful(&texe(
        root,
        &bin,
        &[
            "build",
            "--project",
            path(&empty)?,
            "--offline",
            "--force",
            "--yes",
            "--json",
        ],
    )?)?;
    nonempty(&empty.join("main.pdf"))?;

    fs::rename(root.join("data/texe"), root.join("data/texe.first"))?;
    fs::rename(root.join("cache"), root.join("cache.first"))?;
    fs::rename(empty.join(".texe"), empty.join(".texe.first"))?;
    fs::rename(basic.join(".texe"), basic.join(".texe.first"))?;
    fs::create_dir_all(root.join("data/texe"))?;
    fs::create_dir_all(root.join("cache"))?;
    successful(&texe(
        root,
        &bin,
        &[
            "build",
            "--project",
            path(&empty)?,
            "--frozen",
            "--yes",
            "--json",
        ],
    )?)?;
    successful(&texe(
        root,
        &bin,
        &[
            "build",
            "--project",
            path(&basic)?,
            "--frozen",
            "--yes",
            "--json",
        ],
    )?)?;
    require(
        fs::read(root.join("empty-first.pdf"))? == fs::read(empty.join("main.pdf"))?,
        "empty-cache frozen pdfLaTeX build was not byte-identical",
    )?;
    require(
        fs::read(root.join("basic-first.pdf"))? == fs::read(basic.join("main.pdf"))?,
        "empty-cache frozen LuaLaTeX build was not byte-identical",
    )?;

    let offline_root = root.join("offline-empty");
    for directory in ["home", "cache", "data/texe", "tmp"] {
        fs::create_dir_all(offline_root.join(directory))?;
    }
    let miss = texe_with_home(
        &offline_root,
        &bin,
        &[
            "build",
            "--project",
            path(&empty)?,
            "--offline",
            "--force",
            "--yes",
        ],
    )?;
    require(
        !miss.status.success(),
        "empty-cache offline build unexpectedly succeeded",
    )?;
    require(
        !contains_extension(&offline_root, "part")?,
        "offline build created a partial download",
    )?;

    let doctor = texe(
        root,
        &bin,
        &[
            "doctor",
            "--project",
            path(&basic)?,
            "--verify-toolchain",
            "--json",
        ],
    )?;
    successful(&doctor)?;
    require_schema(&doctor, "texe.doctor-report/v1")?;
    println!(
        "platform journey passed: both templates and engines, Biber, Unicode paths, offline \
         behavior, suite compatibility, and byte-identical empty-cache frozen reproduction"
    );
    Ok(())
}

fn managed(suite: &SuiteBinaries) -> Result<()> {
    let journey = Journey::new("managed", suite)?;
    let project = journey.root.join("project");
    fs::create_dir_all(&project)?;
    journey.success(&["init", path(&project)?, "--yes", "--engine", "pdflatex"])?;
    let source = project.join("main.tex");
    fs::write(
        &source,
        read_text(&source)?.replace(
            "\\begin{document}",
            "\\usepackage[english]{babel}\n\\begin{document}",
        ),
    )?;
    fs::write(
        project.join("pqty.toml"),
        "[registry]\nurl = \"https://example.invalid/should-not-be-read/tlpkg/texlive.tlpdb.xz\"\n",
    )?;
    let first = journey.output(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    successful(&first)?;
    fs::write(journey.root.join("build-progress.log"), &first.stderr)?;
    let report = json(&first)?;
    require(
        report["schema"] == "texe.build-report/v1",
        "wrong build schema",
    )?;
    nonempty(&project.join("texe.lock"))?;
    nonempty(&project.join("main.pdf"))?;
    let progress = String::from_utf8_lossy(&first.stderr);
    require(
        progress.contains("texe: packages download plan:")
            && progress.contains("texe: packages download complete:"),
        "managed build did not render package download progress",
    )?;
    require(
        !progress.contains("\"schema\":\"pqty.progress/v1\""),
        "raw pqty progress leaked into customer output",
    )?;
    let timings: Value =
        serde_json::from_str(&read_text(&project.join(".texe/build/timings.json"))?)?;
    require(
        timings["schema"] == "texe.timing-history/v1"
            && timings["samples"]
                .as_array()
                .is_some_and(|samples| samples.len() == 1),
        "timing history did not record the first v1 sample",
    )?;
    let lock = project.join(".texe/state/pqty.lock");
    require_contains(&lock, "\"provider\": \"babel-english\"")?;
    let lock_text = read_text(&lock)?;
    require(
        fs::metadata(&lock)?.len() < 500_000,
        "compact Babel lock exceeded 499999 bytes",
    )?;
    require(
        lock_text.matches("\"digest\"").count() == 1,
        "package lock contains per-file digest records",
    )?;
    require(
        tree_has_file(&journey.root.join("data/texe/pqty/store/manifests"), |_| {
            true
        })?,
        "shared pqty store has no manifest",
    )?;
    copy_file(
        &project.join("main.pdf"),
        &journey.root.join("main.pdf.first-build"),
    )?;
    require_contains(&project.join("texe.lock"), "\"source_date_epoch\"")?;

    let cached = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(
        cached["cached"] == true && cached["engine_passes"] == 0,
        "unchanged managed build did not use the no-op cache",
    )?;
    let timings: Value =
        serde_json::from_str(&read_text(&project.join(".texe/build/timings.json"))?)?;
    require(
        timings["samples"]
            .as_array()
            .is_some_and(|samples| samples.len() == 1),
        "no-op build recorded an extra timing sample",
    )?;
    let doctor = journey.json(&[
        "doctor",
        "--project",
        path(&project)?,
        "--verify-toolchain",
        "--json",
    ])?;
    require(
        doctor["toolchain_verification"] == "deep",
        "deep toolchain verification was not reported",
    )?;
    require(
        !tree_has_symlink(&project.join(".texe/texmf"))?,
        "copy-mode TEXMF contains a symlink",
    )?;
    require(
        !tree_has_writable_file(&project.join(".texe/texmf"))?,
        "copy-mode TEXMF contains a writable package file",
    )?;

    append(&source, "\n% body edit\n")?;
    let edited = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(
        edited["cached"] == false
            && edited["engine_passes"] == 1
            && edited["convergence_rounds"] == 0,
        "body-only edit did not take the single-pass fast path",
    )?;
    let pinned = source_date_epoch(&project.join("texe.lock"))?;
    let mut forced = journey.command(&["build", "--project", path(&project)?, "--force"]);
    forced.env("SOURCE_DATE_EPOCH", "1000000000");
    successful(&forced.output()?)?;
    require(
        source_date_epoch(&project.join("texe.lock"))? == pinned,
        "inherited SOURCE_DATE_EPOCH rewrote the project lock",
    )?;

    journey.success(&["clean", "--project", path(&project)?])?;
    require(!project.join(".texe").exists(), "clean left .texe behind")?;
    nonempty(&project.join("texe.lock"))?;
    nonempty(&project.join("main.pdf"))?;
    let rebuilt = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(rebuilt["cached"] == false, "clean rebuild was cached")?;
    copy_file(
        &project.join("main.pdf"),
        &journey.root.join("main.pdf.first-build"),
    )?;

    let nested = journey.root.join("nested-project");
    journey.success(&[
        "init",
        path(&nested)?,
        "--yes",
        "--engine",
        "pdflatex",
        "--entry",
        "paper/main.tex",
    ])?;
    journey.success(&["build", "--project", path(&nested)?, "--yes", "--json"])?;
    require_contains(&nested.join("texe.lock"), "\"root\": \"paper/main.tex\"")?;
    let nested_cached = journey.json(&["build", "--project", path(&nested)?, "--yes", "--json"])?;
    require(
        nested_cached["cached"] == true,
        "nested build was not cached",
    )?;
    append(&nested.join("paper/main.tex"), "\n% nested entry changed\n")?;
    let nested_edited = journey.json(&["build", "--project", path(&nested)?, "--yes", "--json"])?;
    require(
        nested_edited["cached"] == false && nested_edited["engine_passes"] != 0,
        "nested entry edit returned a stale cached artifact",
    )?;

    let basic = journey.root.join("basic-paper");
    journey.success(&[
        "init",
        path(&basic)?,
        "--yes",
        "--engine",
        "pdflatex",
        "--template",
        "basic",
        "--title",
        "Reliable Results",
        "--author",
        "Ada Researcher",
    ])?;
    let basic_report = journey.json(&["build", "--project", path(&basic)?, "--yes", "--json"])?;
    require(
        basic_report["bibliography_runs"] == 1,
        "basic starter did not run BibTeX",
    )?;
    nonempty(&basic.join("main.pdf"))?;
    nonempty(&basic.join("references.bib"))?;
    nonempty(&basic.join(".texe/build/output/main.bbl"))?;

    journey.reset_caches(&[&project])?;
    let frozen = journey.json(&[
        "build",
        "--project",
        path(&project)?,
        "--frozen",
        "--yes",
        "--json",
    ])?;
    require(
        frozen["convergence_rounds"] == 0,
        "frozen rebuild converged packages",
    )?;
    require(
        fs::read(journey.root.join("main.pdf.first-build"))? == fs::read(project.join("main.pdf"))?,
        "empty-cache frozen managed build was not byte-identical",
    )?;
    println!(
        "managed journey passed: install, lock, cache, edit, clean, nested entry, starter, deep \
         verification, and empty-cache frozen reproduction"
    );
    Ok(())
}

fn luatex(suite: &SuiteBinaries) -> Result<()> {
    let journey = Journey::new("luatex", suite)?;
    let project = journey.root.join("project");
    fs::create_dir_all(&project)?;
    copy_tree(&journey.repo.join("examples/managed-luatex"), &project)?;
    let first = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(
        first["engine"] == "lualatex",
        "LuaLaTeX engine not reported",
    )?;
    for file in ["texe.lock", "main.pdf", "main.synctex.gz"] {
        nonempty(&project.join(file))?;
    }
    let lock = project.join("texe.lock");
    require_contains(&lock, "\"engine\": \"lualatex\"")?;
    require_contains(&lock, "\"provider\": \"luahbtex.")?;
    require_absent(&lock, "\"provider\": \"pdftex.")?;
    require_contains(&project.join(".texe/build/output/main.log"), "LuaHBTeX")?;
    require_no_host_fonts(&project)?;
    copy_file(
        &project.join("main.pdf"),
        &journey.root.join("main.pdf.first"),
    )?;
    copy_file(
        &project.join("main.synctex.gz"),
        &journey.root.join("main.synctex.gz.first"),
    )?;
    let cached = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(
        cached["cached"] == true && cached["engine_passes"] == 0,
        "LuaLaTeX no-op build missed the cache",
    )?;
    let doctor = journey.json(&[
        "doctor",
        "--project",
        path(&project)?,
        "--verify-toolchain",
        "--json",
    ])?;
    require(
        doctor["toolchain_verification"] == "deep"
            && doctor["engine_version"]
                .as_str()
                .is_some_and(|version| version.contains("LuaHBTeX")),
        "LuaLaTeX deep verification report is incomplete",
    )?;
    journey.reset_caches(&[&project])?;
    let frozen = journey.json(&[
        "build",
        "--project",
        path(&project)?,
        "--frozen",
        "--yes",
        "--json",
    ])?;
    require(
        frozen["convergence_rounds"] == 0 && frozen["cached"] == false,
        "LuaLaTeX frozen rebuild used an invalid path",
    )?;
    require_no_host_fonts(&project)?;
    require_equal_files(
        &journey.root.join("main.pdf.first"),
        &project.join("main.pdf"),
        "LuaLaTeX PDF",
    )?;
    require_equal_files(
        &journey.root.join("main.synctex.gz.first"),
        &project.join("main.synctex.gz"),
        "LuaLaTeX SyncTeX",
    )?;
    println!("LuaLaTeX managed journey passed");
    Ok(())
}

fn common(suite: &SuiteBinaries) -> Result<()> {
    require(on_path("pdffonts"), "common journey requires pdffonts")?;
    let journey = Journey::new("common", suite)?;
    let project = journey.root.join("project");
    fs::create_dir_all(&project)?;
    copy_tree(&journey.repo.join("examples/managed-common"), &project)?;
    journey.success(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    for file in ["main.pdf", "main.synctex.gz"] {
        nonempty(&project.join(file))?;
        require(
            !project.join(".texe/build/output").join(file).exists(),
            format!("published {file} remained in the private output root"),
        )?;
    }
    let lock = project.join("texe.lock");
    for needle in [
        "\"provider\": \"amsfonts\"",
        "\"provider\": \"amsmath\"",
        "\"provider\": \"hyperref\"",
        "\"provider\": \"xcolor\"",
        "\"path\": \"commonreport.cls\"",
        "\"path\": \"projectnote.sty\"",
        "\"path\": \"sections/mathematics.tex\"",
        "\"resolved_path\": \"figures/box.mps\"",
    ] {
        require_contains(&lock, needle)?;
    }
    let log = project.join(".texe/build/output/main.log");
    require_absent(&log, "There were undefined references")?;
    require_absent(&log, "cannot open font map file")?;
    let fonts = crate::command::capture(Command::new("pdffonts").arg(project.join("main.pdf")))?;
    require(
        fonts.contains("Type 1") && !fonts.contains("Type 3"),
        "common PDF did not use vector Type 1 fonts",
    )?;
    copy_file(
        &project.join("main.pdf"),
        &journey.root.join("main.pdf.first"),
    )?;
    copy_file(
        &project.join("main.synctex.gz"),
        &journey.root.join("main.synctex.gz.first"),
    )?;
    journey.reset_caches(&[&project])?;
    let frozen = journey.json(&[
        "build",
        "--project",
        path(&project)?,
        "--frozen",
        "--yes",
        "--json",
    ])?;
    require(
        frozen["convergence_rounds"] == 0,
        "common frozen build converged",
    )?;
    require_equal_files(
        &journey.root.join("main.pdf.first"),
        &project.join("main.pdf"),
        "common PDF",
    )?;
    require_equal_files(
        &journey.root.join("main.synctex.gz.first"),
        &project.join("main.synctex.gz"),
        "common SyncTeX",
    )?;
    println!("common managed document journey passed");
    Ok(())
}

fn bibliography(suite: &SuiteBinaries) -> Result<()> {
    let journey = Journey::new("bibliography", suite)?;
    let bibtex = journey.root.join("bibtex");
    let biber = journey.root.join("biber");
    copy_tree(&journey.repo.join("examples/bibliography"), &bibtex)?;
    copy_tree(&journey.repo.join("examples/biber"), &biber)?;
    let bibtex_report = journey.json(&["build", "--project", path(&bibtex)?, "--yes", "--json"])?;
    require(
        bibtex_report["bibliography_runs"]
            .as_u64()
            .is_some_and(|runs| runs > 0),
        "BibTeX did not run",
    )?;
    require_contains(
        &bibtex.join(".texe/build/output/main.bbl"),
        "\\bibitem{knuth1984texbook}",
    )?;
    nonempty(&bibtex.join("main.pdf"))?;
    require(
        !journey.root.join("data/texe/components").exists(),
        "BibTeX eagerly installed the optional Biber component",
    )?;
    let biber_report = journey.json(&["build", "--project", path(&biber)?, "--yes", "--json"])?;
    require(
        biber_report["bibliography_runs"]
            .as_u64()
            .is_some_and(|runs| runs > 0),
        "Biber did not run",
    )?;
    require_contains(
        &biber.join(".texe/build/output/main.bbl"),
        "knuth1984texbook",
    )?;
    nonempty(&biber.join("main.pdf"))?;
    require(
        tree_has_file(&journey.root.join("data/texe/components"), |path| {
            path.file_name() == Some(executable_name("biber").as_os_str())
        })?,
        "Biber component executable was not installed",
    )?;
    if cfg!(target_os = "linux") {
        require(
            tree_has_file(&journey.root.join("data/texe/components"), |path| {
                path.file_name().is_some_and(|name| name == "libcrypt.so.1")
            })?,
            "Biber compatibility library was not installed",
        )?;
    }
    copy_file(
        &bibtex.join("main.pdf"),
        &journey.root.join("bibtex.pdf.first"),
    )?;
    copy_file(
        &biber.join("main.pdf"),
        &journey.root.join("biber.pdf.first"),
    )?;
    journey.reset_caches(&[&bibtex, &biber])?;
    let bibtex_frozen = journey.json(&[
        "build",
        "--project",
        path(&bibtex)?,
        "--frozen",
        "--yes",
        "--json",
    ])?;
    require(
        bibtex_frozen["bibliography_runs"]
            .as_u64()
            .is_some_and(|runs| runs > 0)
            && bibtex_frozen["convergence_rounds"] == 0,
        "frozen BibTeX rebuild did not run correctly",
    )?;
    let biber_frozen = journey.json(&[
        "build",
        "--project",
        path(&biber)?,
        "--frozen",
        "--yes",
        "--json",
    ])?;
    require(
        biber_frozen["bibliography_runs"]
            .as_u64()
            .is_some_and(|runs| runs > 0)
            && biber_frozen["convergence_rounds"] == 0,
        "frozen Biber rebuild did not run correctly",
    )?;
    require_equal_files(
        &journey.root.join("bibtex.pdf.first"),
        &bibtex.join("main.pdf"),
        "BibTeX PDF",
    )?;
    require_equal_files(
        &journey.root.join("biber.pdf.first"),
        &biber.join("main.pdf"),
        "Biber PDF",
    )?;
    println!("managed BibTeX and Biber journeys passed");
    Ok(())
}

fn index(suite: &SuiteBinaries) -> Result<()> {
    let journey = Journey::new("index", suite)?;
    let project = journey.root.join("project");
    copy_tree(&journey.repo.join("examples/index-glossary"), &project)?;
    let report = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(
        report["index_runs"].as_u64().is_some_and(|runs| runs >= 3),
        "index/glossary build ran fewer than three index tools",
    )?;
    require_contains(&project.join(".texe/build/output/main.ind"), "automation")?;
    require_contains(&project.join(".texe/build/output/main.gls"), "typesetting")?;
    require_contains(
        &project.join(".texe/build/output/main.acr"),
        "glossentry{api}",
    )?;
    nonempty(&project.join("main.pdf"))?;
    require(
        tree_has_file(&journey.root.join("data/texe/toolchains"), |path| {
            path.file_name() == Some(executable_name("makeindex").as_os_str())
        })?,
        "managed runtime has no makeindex executable",
    )?;
    copy_file(
        &project.join("main.pdf"),
        &journey.root.join("main.pdf.first"),
    )?;
    journey.reset_caches(&[&project])?;
    let frozen = journey.json(&[
        "build",
        "--project",
        path(&project)?,
        "--frozen",
        "--yes",
        "--json",
    ])?;
    require(
        frozen["index_runs"].as_u64().is_some_and(|runs| runs >= 3)
            && frozen["convergence_rounds"] == 0,
        "frozen index/glossary rebuild did not run correctly",
    )?;
    require_equal_files(
        &journey.root.join("main.pdf.first"),
        &project.join("main.pdf"),
        "index/glossary PDF",
    )?;
    println!("managed index and glossary journey passed");
    Ok(())
}

fn local(suite: &SuiteBinaries) -> Result<()> {
    require(
        on_path("pdflatex") && on_path("kpsewhich"),
        "local journey requires pdflatex and kpsewhich",
    )?;
    let journey = Journey::new("local", suite)?;
    let project = journey.root.join("project");
    copy_tree(&journey.repo.join("examples/convergence"), &project)?;
    journey.success(&["doctor", "--project", path(&project)?])?;
    let first = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(first["cached"] == false, "system build was cached")?;
    require_contains(
        &project.join(".texe/state/pqty.lock"),
        "\"provider\": \"xcolor\"",
    )?;
    nonempty(&project.join("main.pdf"))?;
    copy_file(
        &project.join("main.pdf"),
        &journey.root.join("main.pdf.first"),
    )?;
    let second = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(
        second["cached"] == false,
        "system provider used no-op cache",
    )?;
    require_equal_files(
        &journey.root.join("main.pdf.first"),
        &project.join("main.pdf"),
        "repeated system PDF",
    )?;
    append(&project.join("main.tex"), "\n% edit\n")?;
    let edited = journey.json(&["build", "--project", path(&project)?, "--yes", "--json"])?;
    require(edited["cached"] == false, "edited system build was cached")?;
    let frozen = journey.json(&[
        "build",
        "--project",
        path(&project)?,
        "--frozen",
        "--force",
        "--yes",
        "--json",
    ])?;
    require(
        frozen["cached"] == false && frozen["convergence_rounds"] == 0,
        "forced frozen system rebuild did not run correctly",
    )?;
    println!("system-provider journey passed");
    Ok(())
}

struct Journey {
    _scratch: ScratchDir,
    repo: PathBuf,
    root: PathBuf,
    bin: PathBuf,
    inherit_host_path: bool,
}

impl Journey {
    fn new(label: &str, suite: &SuiteBinaries) -> Result<Self> {
        let repo = repo_root()?;
        let scratch = ScratchDir::new(label)?;
        let root = scratch.path().to_path_buf();
        for directory in ["bin", "home", "cache", "data/texe", "tmp"] {
            fs::create_dir_all(root.join(directory))?;
        }
        let bin = root.join("bin");
        suite.install(&bin)?;
        Ok(Self {
            _scratch: scratch,
            repo,
            root,
            bin,
            inherit_host_path: label == "local",
        })
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(self.bin.join(executable_name("texe")));
        clean_environment(&mut command, &self.root, &self.bin);
        if self.inherit_host_path
            && let Some(host) = std::env::var_os("PATH")
            && let Ok(path) = std::env::join_paths(
                std::iter::once(self.bin.as_os_str().to_owned())
                    .chain(std::env::split_paths(&host).map(PathBuf::into_os_string)),
            )
        {
            command.env("PATH", path);
        }
        command.args(arguments);
        command
    }

    fn output(&self, arguments: &[&str]) -> Result<Output> {
        Ok(self.command(arguments).output()?)
    }

    fn success(&self, arguments: &[&str]) -> Result<()> {
        successful(&self.output(arguments)?)
    }

    fn json(&self, arguments: &[&str]) -> Result<Value> {
        let output = self.output(arguments)?;
        successful(&output)?;
        json(&output)
    }

    fn reset_caches(&self, projects: &[&Path]) -> Result<()> {
        fs::rename(
            self.root.join("data/texe"),
            self.root.join("data/texe.first-build"),
        )?;
        fs::rename(self.root.join("cache"), self.root.join("cache.first-build"))?;
        for project in projects {
            fs::rename(project.join(".texe"), project.join(".texe.first-build"))?;
        }
        fs::create_dir_all(self.root.join("data/texe"))?;
        fs::create_dir_all(self.root.join("cache"))?;
        Ok(())
    }
}

fn source_date_epoch(lock: &Path) -> Result<u64> {
    let value: Value = serde_json::from_str(&read_text(lock)?)?;
    value["source_date_epoch"]
        .as_u64()
        .ok_or_else(|| message("texe.lock has no source_date_epoch"))
}

fn require_no_host_fonts(project: &Path) -> Result<()> {
    require(
        !tree_text_contains(
            &project.join(".texe/build"),
            &[
                "INPUT /etc/fonts",
                "INPUT /usr/share/fonts",
                "INPUT /usr/local/share/fonts",
            ],
        )?,
        "managed LuaLaTeX read host font configuration or files",
    )
}

fn require_equal_files(first: &Path, second: &Path, label: &str) -> Result<()> {
    require(
        fs::read(first)? == fs::read(second)?,
        format!("{label} was not byte-identical"),
    )
}

fn tree_has_file(root: &Path, predicate: impl Fn(&Path) -> bool) -> Result<bool> {
    if !root.exists() {
        return Ok(false);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_type()?.is_file() && predicate(&entry.path()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn tree_has_symlink(root: &Path) -> Result<bool> {
    if !root.exists() {
        return Ok(false);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Ok(true);
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(false)
}

fn tree_has_writable_file(root: &Path) -> Result<bool> {
    if !root.exists() {
        return Ok(false);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_type()?.is_file() && !entry.metadata()?.permissions().readonly() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn tree_text_contains(root: &Path, needles: &[&str]) -> Result<bool> {
    if !root.exists() {
        return Ok(false);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.file_type()?.is_file() {
                let bytes = fs::read(entry.path())?;
                let text = String::from_utf8_lossy(&bytes);
                if needles.iter().any(|needle| text.contains(needle)) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn texe(root: &Path, bin: &Path, arguments: &[&str]) -> Result<Output> {
    texe_with_home(root, bin, arguments)
}

fn texe_with_home(home: &Path, bin: &Path, arguments: &[&str]) -> Result<Output> {
    let mut command = Command::new(bin.join(executable_name("texe")));
    clean_environment(&mut command, home, bin);
    Ok(command.args(arguments).output()?)
}

fn successful(output: &Output) -> Result<()> {
    require(
        output.status.success(),
        format!(
            "texe failed with {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn json(output: &Output) -> Result<Value> {
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn require_schema(output: &Output, schema: &str) -> Result<()> {
    let report = json(output)?;
    require(
        report["schema"] == schema,
        format!("expected {schema}, got {}", report["schema"]),
    )
}

fn path(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| message(format!("non-UTF-8 path: {}", path.display())))
}

fn contains_extension(root: &Path, extension: &str) -> Result<bool> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry.path().extension().and_then(|value| value.to_str()) == Some(extension) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

struct SuiteBinaries {
    texe: PathBuf,
    pqty: PathBuf,
    pqty_fls: PathBuf,
}

impl SuiteBinaries {
    fn from_directory(directory: &Path) -> Result<Self> {
        let binary = |name: &str| directory.join(executable_name(name));
        let suite = Self {
            texe: binary("texe"),
            pqty: binary("pqty"),
            pqty_fls: binary("pqty-fls"),
        };
        suite.validate()?;
        Ok(suite)
    }

    fn build() -> Result<Self> {
        let repo = repo_root()?;
        let pqty_repo = pqty::checkout()?;
        pqty::verify(Some(&pqty_repo))?;
        run(cargo().current_dir(&repo).args([
            "build",
            "--quiet",
            "--locked",
            "--package",
            "texe",
        ]))?;
        run(cargo()
            .current_dir(&pqty_repo)
            .args(["build", "--quiet", "--locked", "--workspace"]))?;
        let texe_target = target_directory(&repo)?.join("debug");
        let pqty_target = target_directory(&pqty_repo)?.join("debug");
        let suite = Self {
            texe: texe_target.join(executable_name("texe")),
            pqty: pqty_target.join(executable_name("pqty")),
            pqty_fls: pqty_target.join(executable_name("pqty-fls")),
        };
        suite.validate()?;
        Ok(suite)
    }

    fn validate(&self) -> Result<()> {
        for path in [&self.texe, &self.pqty, &self.pqty_fls] {
            require(
                path.is_file(),
                format!("suite is missing {}", path.display()),
            )?;
        }
        Ok(())
    }

    fn install(&self, destination: &Path) -> Result<()> {
        copy_file(&self.texe, &destination.join(executable_name("texe")))?;
        copy_file(&self.pqty, &destination.join(executable_name("pqty")))?;
        copy_file(
            &self.pqty_fls,
            &destination.join(executable_name("pqty-fls")),
        )?;
        Ok(())
    }
}

fn target_directory(repo: &Path) -> Result<PathBuf> {
    let output = crate::command::output(cargo().current_dir(repo).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]))?;
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    metadata["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| message("cargo metadata did not report target_directory"))
}
