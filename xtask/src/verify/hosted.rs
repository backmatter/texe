//! Hosted builds under the Citet latex-worker's outer sandbox.
//!
//! Hosted-only paths (`TEXE_HOSTED=1 --protected`: shared package trees, the
//! shared format store, the shared Biber runtime) run in Citet's latex-worker
//! under an outer Landlock policy that grants nothing on `/`. A path that
//! works everywhere else can fail only there (texe #52 did), so this case runs
//! the worker's stages exactly as it invokes them, under the same policy
//! (`crate::worker_sandbox`) and environment:
//!
//! 1. `texe fetch --frozen` of a Paper whose environment has no shared tree yet;
//! 2. `texe build --protected --frozen --offline --force` of a biblatex/Biber
//!    Paper (format generation, the shared Biber runtime) and of a BibTeX Paper;
//! 3. both stages again in a fresh directory, reusing the tree and format.
//!
//! The locks come from an ordinary build outside the sandbox, as the Citet
//! server keeps them. Landlock V3 must be enforceable: in CI a runner that
//! cannot enforce it fails this case; elsewhere the case is skipped.
use std::ffi::OsString;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::process::Output;

use super::SuiteBinaries;
use crate::Result;

#[cfg(not(target_os = "linux"))]
pub(super) fn run(_suite: &SuiteBinaries) -> Result<()> {
    Err(crate::message(
        "the hosted sandbox case needs Linux Landlock",
    ))
}

#[cfg(target_os = "linux")]
pub(super) fn run(suite: &SuiteBinaries) -> Result<()> {
    use std::fs;

    use crate::command::{ScratchDir, clean_environment, copy_tree, nonempty, repo_root, require};
    use crate::worker_sandbox::{self, Stage};

    if !worker_sandbox::enforceable() {
        let reason = "this kernel cannot fully enforce the worker's Landlock V3 policy";
        if std::env::var_os("CI").is_some() {
            return Err(crate::message(format!(
                "{reason}; the hosted sandbox case must run in CI"
            )));
        }
        println!("skipping the hosted sandbox case: {reason}");
        return Ok(());
    }
    let repo = repo_root()?;
    let scratch = ScratchDir::new("hosted")?;
    let root = scratch.path();
    for directory in ["bin", "home", "cache", "data/texe", "tmp", "jobs"] {
        fs::create_dir_all(root.join(directory))?;
    }
    let bin = root.join("bin");
    suite.install(&bin)?;
    let home = root.join("data/texe");

    // The Citet server's locks: ordinary builds outside the sandbox.
    let mut locks = Vec::new();
    for paper in ["biber", "bibliography"] {
        let warm = root.join(format!("warm-{paper}"));
        copy_tree(&repo.join("examples").join(paper), &warm)?;
        let mut command = std::process::Command::new(bin.join("texe"));
        clean_environment(&mut command, root, &bin);
        successful(
            &format!("building {paper} outside the sandbox"),
            &command
                .args(["build", "--project"])
                .arg(&warm)
                .args(["--yes", "--json"])
                .output()?,
        )?;
        locks.push((paper, warm));
    }
    // Everything hosted builds share must be created under the sandbox.
    for shared in ["package-trees", "formats/by-inputs"] {
        require(
            !home.join(shared).exists(),
            format!("{shared} existed before the hosted builds"),
        )?;
    }
    require(
        markers(&home.join("components"), ".texe-par-ready")?.is_empty(),
        "the Biber runtime was marked ready before the hosted builds",
    )?;

    let stage = |stage: Stage, job: &Path, arguments: &[&str]| -> Result<Output> {
        let command = std::iter::once(bin.join("texe").into_os_string())
            .chain(arguments.iter().map(OsString::from))
            .collect::<Vec<_>>();
        let mut launcher = worker_sandbox::command(stage, &home, job, &bin, &command)?;
        worker_environment(&mut launcher, &home, job, &bin);
        Ok(launcher.output()?)
    };
    let job = |name: &str, paper: &str| -> Result<PathBuf> {
        let (_, warm) = locks
            .iter()
            .find(|(candidate, _)| *candidate == paper)
            .expect("known paper");
        let job = root.join("jobs").join(name);
        copy_tree(&repo.join("examples").join(paper), &job)?;
        fs::copy(warm.join("texe.lock"), job.join("texe.lock"))?;
        Ok(job)
    };
    let hosted = |job: &Path| -> Result<serde_json::Value> {
        let project = job.to_str().expect("UTF-8 scratch path");
        successful(
            &format!("hosted fetch in {}", job.display()),
            &stage(
                Stage::Fetch,
                job,
                &["fetch", "--project", project, "--frozen", "--json"],
            )?,
        )?;
        let output = stage(
            Stage::Compile,
            job,
            &[
                "build",
                "--project",
                project,
                "--protected",
                "--frozen",
                "--offline",
                "--force",
                "--json",
            ],
        )?;
        successful(&format!("hosted build in {}", job.display()), &output)?;
        nonempty(&job.join("main.pdf"))?;
        Ok(serde_json::from_slice(&output.stdout)?)
    };

    // 1–2. A new shared tree, format and Biber runtime, under the sandbox.
    let first = job("biber-first", "biber")?;
    let report = hosted(&first)?;
    require(
        report["bibliography_runs"].as_u64() > Some(0),
        "the hosted Biber build ran no bibliography processor",
    )?;
    let trees = markers(&home.join("package-trees"), ".texe-tree-ready")?;
    require(trees.len() == 1, "the hosted fetch made no shared tree")?;
    require(
        !first.join(".texe/texmf/tex").exists(),
        "the hosted fetch copied packages into the Paper",
    )?;
    require(
        !markers(&home.join("formats/by-inputs"), "inputs.json")?.is_empty(),
        "the hosted build shared no format",
    )?;
    let runtime = markers(&home.join("components"), ".texe-par-ready")?;
    require(
        runtime.len() == 1,
        "the shared Biber runtime was not marked ready",
    )?;
    let runtime_marked = fs::metadata(&runtime[0])?.modified()?;

    let bibtex = job("bibtex", "bibliography")?;
    let report = hosted(&bibtex)?;
    require(
        report["bibliography_runs"].as_u64() > Some(0),
        "the hosted BibTeX build ran no bibliography processor",
    )?;

    // 3. A fresh directory reuses the tree, the format and the Biber runtime.
    let tree_count = markers(&home.join("package-trees"), ".texe-tree-ready")?.len();
    let reuse = job("biber-reuse", "biber")?;
    hosted(&reuse)?;
    require(
        markers(&home.join("package-trees"), ".texe-tree-ready")?.len() == tree_count,
        "a fresh directory with the same lock made another shared tree",
    )?;
    require(
        markers(&reuse.join(".texe/build/formats"), "pdflatex.log")?.is_empty(),
        "a fresh directory regenerated a format it could reuse",
    )?;
    require(
        fs::metadata(&runtime[0])?.modified()? == runtime_marked,
        "the shared Biber runtime was unpacked again",
    )?;

    // No hosted build wrote into a shared tree after it was marked complete.
    for marker in markers(&home.join("package-trees"), ".texe-tree-ready")? {
        let marked = fs::metadata(&marker)?.modified()?;
        let tree = marker.parent().expect("marker parent").join("texmf");
        require(
            newest(&tree)? <= marked,
            format!("{} changed after it was marked complete", tree.display()),
        )?;
    }
    println!("hosted builds passed under the latex-worker sandbox");
    Ok(())
}

/// The environment `runCommandWithOutput` gives a trusted texe stage.
#[cfg(target_os = "linux")]
fn worker_environment(command: &mut std::process::Command, home: &Path, job: &Path, bin: &Path) {
    let path = OsString::from(format!("{}:/usr/bin:/bin", bin.display()));
    command
        .env_clear()
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .env("HOME", job)
        .env("TMPDIR", job)
        .env("TMP", job)
        .env("TEMP", job)
        .env("XDG_CACHE_HOME", job.join(".cache"))
        .env("TEXMFVAR", job.join(".texmf-var"))
        .env("TEXMFCACHE", job.join(".texmf-var"))
        .env("TEXMFCONFIG", job.join(".texmf-config"))
        .env("TEXMFHOME", job)
        .env("openin_any", "p")
        .env("openout_any", "p")
        .env("max_print_line", "1000")
        .env("TEXE_HOME", home)
        .env("TEXE_HOSTED", "1")
        .env("TEXE_COMPILER_TIMEOUT_SECONDS", "600");
}

fn successful(what: &str, output: &Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    Err(crate::message(format!(
        "{what} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}

/// Files named `name` below `root` (none when it is absent).
#[cfg(target_os = "linux")]
fn markers(root: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if entry.file_name() == name {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The latest modification time of anything below `root`.
#[cfg(target_os = "linux")]
fn newest(root: &Path) -> Result<std::time::SystemTime> {
    let mut newest = std::fs::metadata(root)?.modified()?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            newest = newest.max(metadata.modified()?);
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(newest)
}
