use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::command::{capture, output, repo_root};
use crate::{Result, message};

const SUITE_SCHEMA: &str = "texe.command-suite/v1";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SuiteLock {
    schema: String,
    pqty_version: String,
    pqty_revision: String,
    pqty_source_sha256: String,
    pqty_capabilities_schema: String,
    pqty_lock_schema: String,
    pqty_environment_schema: String,
    pqty_trace_schema: String,
    pqty_trace_report_schema: String,
    pqty_convergence_report_schema: String,
    pqty_progress_schema: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    schema: String,
    version: String,
    lock_schema: String,
    environment_schema: String,
    trace_schema: String,
    trace_report_schema: String,
    convergence_report_schema: String,
    progress_schema: String,
}

pub(crate) fn checkout() -> Result<PathBuf> {
    let path = std::env::var_os("PQTY_REPO")
        .map(PathBuf::from)
        .unwrap_or(repo_root()?.join("../pqty"));
    path.canonicalize()
        .map_err(|error| message(format!("no pqty checkout at {}: {error}", path.display())))
}

pub(crate) fn read_lock() -> Result<SuiteLock> {
    let text = fs::read_to_string(repo_root()?.join("suite.lock.toml"))?;
    Ok(toml::from_str(&text)?)
}

fn source_digest(checkout: &Path) -> Result<String> {
    let files = output(Command::new("git").current_dir(checkout).args([
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ]))?
    .stdout;
    let mut paths = files
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    paths.sort();
    let mut combined = Sha256::new();
    for bytes in paths {
        let relative = std::str::from_utf8(&bytes)?;
        let contents = fs::read(checkout.join(relative))?;
        let digest = hex::encode(Sha256::digest(contents));
        combined.update(&bytes);
        combined.update([0]);
        combined.update(digest.as_bytes());
        combined.update([0]);
    }
    Ok(format!("sha256:{}", hex::encode(combined.finalize())))
}

pub(crate) fn verify(selected: Option<&Path>) -> Result<()> {
    let checkout = match selected {
        Some(path) => path.canonicalize()?,
        None => checkout()?,
    };
    let lock = read_lock()?;
    require_equal("command suite schema", SUITE_SCHEMA, &lock.schema)?;
    let revision = git(&checkout, &["rev-parse", "HEAD"])?;
    require_equal("pqty revision", &lock.pqty_revision, revision.trim())?;
    let (pqty, adapter) = versions(&checkout)?;
    require_equal("pqty version", &lock.pqty_version, &pqty)?;
    require_equal("pqty-fls version", &lock.pqty_version, &adapter)?;
    let capabilities = capabilities(&checkout)?;
    require_equal(
        "pqty capabilities version",
        &lock.pqty_version,
        &capabilities.version,
    )?;
    require_equal(
        "pqty capabilities schema",
        &lock.pqty_capabilities_schema,
        &capabilities.schema,
    )?;
    require_equal(
        "pqty lock schema",
        &lock.pqty_lock_schema,
        &capabilities.lock_schema,
    )?;
    require_equal(
        "pqty environment schema",
        &lock.pqty_environment_schema,
        &capabilities.environment_schema,
    )?;
    require_equal(
        "pqty trace schema",
        &lock.pqty_trace_schema,
        &capabilities.trace_schema,
    )?;
    require_equal(
        "pqty trace report schema",
        &lock.pqty_trace_report_schema,
        &capabilities.trace_report_schema,
    )?;
    require_equal(
        "pqty convergence report schema",
        &lock.pqty_convergence_report_schema,
        &capabilities.convergence_report_schema,
    )?;
    require_equal(
        "pqty progress schema",
        &lock.pqty_progress_schema,
        &capabilities.progress_schema,
    )?;
    let digest = source_digest(&checkout)?;
    require_equal("pqty source digest", &lock.pqty_source_sha256, &digest)?;
    println!(
        "pqty {} source matches {}",
        lock.pqty_version, lock.pqty_source_sha256
    );
    Ok(())
}

pub(crate) fn update(reference: &str, selected: Option<&Path>) -> Result<()> {
    let checkout = match selected {
        Some(path) => path.canonicalize()?,
        None => checkout()?,
    };
    let status = git(
        &checkout,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        return Err(message("pqty checkout must be clean"));
    }
    let revision = git(
        &checkout,
        &["rev-parse", &format!("{reference}^{{commit}}")],
    )?;
    let revision = revision.trim();
    let head = git(&checkout, &["rev-parse", "HEAD"])?;
    require_equal("pqty checkout revision", revision, head.trim())?;
    let (version, adapter) = versions(&checkout)?;
    require_equal("pqty and pqty-fls versions", &version, &adapter)?;
    if reference.starts_with('v') {
        require_equal("pqty tag", reference, &format!("v{version}"))?;
    }
    let capabilities = capabilities(&checkout)?;
    require_equal("pqty capabilities version", &version, &capabilities.version)?;
    let lock = SuiteLock {
        schema: SUITE_SCHEMA.to_string(),
        pqty_version: version.clone(),
        pqty_revision: revision.to_string(),
        pqty_source_sha256: source_digest(&checkout)?,
        pqty_capabilities_schema: capabilities.schema,
        pqty_lock_schema: capabilities.lock_schema,
        pqty_environment_schema: capabilities.environment_schema,
        pqty_trace_schema: capabilities.trace_schema,
        pqty_trace_report_schema: capabilities.trace_report_schema,
        pqty_convergence_report_schema: capabilities.convergence_report_schema,
        pqty_progress_schema: capabilities.progress_schema,
    };
    let path = repo_root()?.join("suite.lock.toml");
    fs::write(&path, toml::to_string_pretty(&lock)?)?;
    verify(Some(&checkout))?;
    println!("pinned pqty {version} at {revision}");
    Ok(())
}

fn capabilities(checkout: &Path) -> Result<Capabilities> {
    let text = capture(crate::command::cargo().current_dir(checkout).args([
        "run",
        "--quiet",
        "--locked",
        "--package",
        "pqty",
        "--",
        "--no-config",
        "capabilities",
    ]))?;
    Ok(serde_json::from_str(&text)?)
}

fn versions(checkout: &Path) -> Result<(String, String)> {
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
        version: String,
    }
    let text = capture(crate::command::cargo().current_dir(checkout).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]))?;
    let metadata: Metadata = serde_json::from_str(&text)?;
    let version = |name: &str| {
        metadata
            .packages
            .iter()
            .find(|package| package.name == name)
            .map(|package| package.version.clone())
            .ok_or_else(|| message(format!("pqty metadata has no {name} package")))
    };
    Ok((version("pqty")?, version("pqty-fls")?))
}

fn git(checkout: &Path, arguments: &[&str]) -> Result<String> {
    capture(Command::new("git").current_dir(checkout).args(arguments))
}

fn require_equal(label: &str, expected: &str, actual: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(message(format!(
            "{label} differs\n  expected: {expected}\n  actual:   {actual}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{SUITE_SCHEMA, SuiteLock};

    #[test]
    fn suite_lock_requires_every_consumed_protocol_schema() {
        let complete = format!(
            r#"
schema = "{SUITE_SCHEMA}"
pqty_version = "0.1.0"
pqty_revision = "revision"
pqty_source_sha256 = "sha256:digest"
pqty_capabilities_schema = "pqty.capabilities/v1"
pqty_lock_schema = "pqty.lock/v1"
pqty_environment_schema = "pqty.env/v1"
pqty_trace_schema = "pqty.trace/v1"
pqty_trace_report_schema = "pqty.trace-report/v1"
pqty_convergence_report_schema = "pqty.convergence-report/v1"
pqty_progress_schema = "pqty.progress/v1"
"#
        );
        assert!(toml::from_str::<SuiteLock>(&complete).is_ok());
        assert!(
            toml::from_str::<SuiteLock>(
                &complete.replace("pqty_progress_schema = \"pqty.progress/v1\"\n", "")
            )
            .is_err()
        );
        assert!(
            toml::from_str::<SuiteLock>(&format!("{complete}unexpected = \"value\"\n")).is_err()
        );
    }
}
