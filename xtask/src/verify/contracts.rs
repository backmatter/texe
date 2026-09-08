//! Small, local fixtures for the actual command-suite binaries.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{Journey, SuiteBinaries, path, require, successful};
use crate::Result;
use crate::command::{clean_environment, executable_name};

pub(super) fn run(suite: &SuiteBinaries) -> Result<()> {
    let journey = Journey::new("contracts", suite)?;
    let project = journey.root.join("Paper Δ with spaces");
    journey.success(&[
        "init",
        path(&project)?,
        "--yes",
        "--template",
        "empty",
        "--json",
    ])?;
    offline_build_failure(&journey, &project)?;
    package_round_trip(&journey, &project)?;
    println!(
        "local contracts passed: offline failure preservation, deterministic locks and environments, recorder interoperability, stale-trace rejection, cached install, and corrupt-store preservation"
    );
    Ok(())
}

fn offline_build_failure(journey: &Journey, project: &Path) -> Result<()> {
    let source = fs::read(project.join("main.tex"))?;
    let previous_pdf = b"previous artifact sentinel";
    let previous_lock = b"previous lock sentinel";
    fs::write(project.join("main.pdf"), previous_pdf)?;
    fs::write(project.join("texe.lock"), previous_lock)?;
    let output = journey.output(&[
        "build",
        "--project",
        path(project)?,
        "--offline",
        "--yes",
        "--json",
    ])?;
    require(
        !output.status.success(),
        "empty-cache offline build succeeded",
    )?;
    let error: Value = serde_json::from_slice(&output.stdout)?;
    require(
        error["schema"] == "texe.error/v1",
        "offline failure lost its JSON error contract",
    )?;
    require(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("offline")),
        "empty-cache build failed for a reason other than offline availability",
    )?;
    require(
        fs::read(project.join("main.pdf"))? == previous_pdf
            && fs::read(project.join("texe.lock"))? == previous_lock
            && fs::read(project.join("main.tex"))? == source,
        "offline failure changed project-owned files",
    )?;
    require(
        !super::contains_extension(&journey.root, "part")?,
        "offline failure left a partial download",
    )?;
    Ok(())
}

fn package_round_trip(journey: &Journey, project: &Path) -> Result<()> {
    let registry = journey.root.join("registry");
    let tlpdb = registry.join("tlpkg/texlive.tlpdb");
    let package_path = "tex/latex/fixture/fixture.sty";
    let package_bytes = b"% synthetic package\n";
    let package_source = registry.join("texmf-dist").join(package_path);
    fs::create_dir_all(package_source.parent().unwrap())?;
    fs::create_dir_all(tlpdb.parent().unwrap())?;
    fs::write(&package_source, package_bytes)?;
    fs::write(
        &tlpdb,
        format!(
            "name 00texlive.config\ncategory TLCore\ndepend release/2026\n\nname fixture\ncategory Package\nrevision 1\nrunfiles size=1\n texmf-dist/{package_path}\n"
        ),
    )?;
    fs::write(project.join("main.tex"), b"\\usepackage{fixture}\n")?;
    // Process consumers must ignore ambient project configuration.
    fs::write(project.join("pqty.toml"), b"invalid TOML [")?;
    let store = journey.root.join("store");
    let lock = project.join("pqty.lock");
    let second_lock = project.join("second.lock");
    for (output, cache) in [
        (&lock, store.clone()),
        (&second_lock, journey.root.join("fresh-store")),
    ] {
        successful(&pqty(
            journey,
            project,
            &[
                "lock",
                "main.tex",
                "--tlpdb",
                path(&tlpdb)?,
                "--store",
                path(&cache)?,
                "--output",
                path(output)?,
            ],
        )?)?;
    }
    let locked = fs::read(&lock)?;
    require(
        locked == fs::read(&second_lock)?,
        "fresh stores produced different locks",
    )?;
    let environment = pqty(journey, project, &["env", "--lock", path(&lock)?])?;
    successful(&environment)?;
    let second_environment = pqty(journey, project, &["env", "--lock", path(&second_lock)?])?;
    successful(&second_environment)?;
    require(
        environment.stdout == second_environment.stdout,
        "environment export is not deterministic",
    )?;
    let environment_path = project.join("environment.json");
    fs::write(&environment_path, &environment.stdout)?;
    let tree = project.join("packages");
    install(journey, project, &lock, &store, &tree)?;
    require(
        fs::read(tree.join(package_path))? == package_bytes,
        "installed bytes differ from the fixture",
    )?;
    recorder_round_trip(
        journey,
        project,
        &lock,
        &environment_path,
        &tree,
        package_path,
    )?;

    // With the recorded source gone, success must come entirely from the store.
    fs::remove_dir_all(&registry)?;
    install(
        journey,
        project,
        &lock,
        &store,
        &project.join("cached-packages"),
    )?;
    let digest = hex::encode(Sha256::digest(package_bytes));
    let object = store.join(&digest[..2]).join(&digest);
    // Rename instead of changing permissions on the immutable store object.
    fs::rename(&object, store.join("original-object"))?;
    fs::write(&object, b"corrupt object")?;
    let failure = pqty(
        journey,
        project,
        &[
            "install",
            "--lock",
            path(&lock)?,
            "--store",
            path(&store)?,
            "--out",
            path(&tree)?,
        ],
    )?;
    require(
        !failure.status.success(),
        "corrupt offline store was accepted",
    )?;
    require(
        fs::read(tree.join(package_path))? == package_bytes && fs::read(&lock)? == locked,
        "failed install replaced the working tree or lock",
    )?;
    require(
        store.join("quarantine").is_dir(),
        "corrupt object evidence was not quarantined",
    )?;
    Ok(())
}

fn recorder_round_trip(
    journey: &Journey,
    project: &Path,
    lock: &Path,
    environment: &Path,
    tree: &Path,
    package: &str,
) -> Result<()> {
    let output = project.join("output");
    fs::create_dir(&output)?;
    let fls = output.join("main.fls");
    fs::write(
        &fls,
        format!(
            "PWD {}\nINPUT {}\nINPUT {}\nOUTPUT {}\n",
            project.display(),
            project.join("main.tex").display(),
            tree.join(package).display(),
            output.join("main.pdf").display(),
        ),
    )?;
    let adapted = tool(
        journey,
        project,
        "pqty-fls",
        &[
            "--fls",
            path(&fls)?,
            "--environment",
            path(environment)?,
            "--project-root",
            path(project)?,
            "--package-root",
            path(tree)?,
            "--output-root",
            path(&output)?,
        ],
    )?;
    successful(&adapted)?;
    let trace = project.join("trace.json");
    fs::write(&trace, &adapted.stdout)?;
    let checked = pqty(
        journey,
        project,
        &[
            "check-trace",
            "--lock",
            path(lock)?,
            "--trace",
            path(&trace)?,
        ],
    )?;
    successful(&checked)?;
    let report: Value = serde_json::from_slice(&checked.stdout)?;
    require(
        report["missing"] == json!([]),
        "recorder trace did not match the installed environment",
    )?;
    let locked = fs::read(lock)?;
    let mut stale: Value = serde_json::from_slice(&adapted.stdout)?;
    stale["environment_fingerprint"] = json!(format!("sha256:{}", "00".repeat(32)));
    fs::write(&trace, serde_json::to_vec(&stale)?)?;
    let rejected = pqty(
        journey,
        project,
        &[
            "check-trace",
            "--lock",
            path(lock)?,
            "--trace",
            path(&trace)?,
        ],
    )?;
    require(
        !rejected.status.success(),
        "stale recorder trace was accepted",
    )?;
    require(fs::read(lock)? == locked, "trace check changed the lock")?;
    Ok(())
}

fn install(
    journey: &Journey,
    project: &Path,
    lock: &Path,
    store: &Path,
    tree: &Path,
) -> Result<()> {
    successful(&pqty(
        journey,
        project,
        &[
            "install",
            "--lock",
            path(lock)?,
            "--store",
            path(store)?,
            "--out",
            path(tree)?,
        ],
    )?)
}

fn pqty(journey: &Journey, project: &Path, arguments: &[&str]) -> Result<Output> {
    let mut explicit = vec!["--no-config", "--offline", "--progress", "json"];
    explicit.extend_from_slice(arguments);
    tool(journey, project, "pqty", &explicit)
}

fn tool(journey: &Journey, project: &Path, name: &str, arguments: &[&str]) -> Result<Output> {
    let mut command = Command::new(journey.bin.join(executable_name(name)));
    clean_environment(&mut command, &journey.root, &journey.bin);
    Ok(command.current_dir(project).args(arguments).output()?)
}
