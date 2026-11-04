use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::command::{capture, repo_root};
use crate::{Result, message};

/// The pqty source that texe releases ship, pinned by Git revision.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SuiteLock {
    pqty_version: String,
    pqty_revision: String,
    pqty_capabilities_schema: String,
}

#[derive(Debug, Deserialize)]
struct Capabilities {
    schema: String,
    version: String,
}

pub(crate) fn checkout() -> Result<PathBuf> {
    let path = std::env::var_os("PQTY_REPO")
        .map(PathBuf::from)
        .unwrap_or(repo_root()?.join("../pqty"));
    path.canonicalize()
        .map_err(|error| message(format!("no pqty checkout at {}: {error}", path.display())))
}

fn read_lock() -> Result<SuiteLock> {
    let text = fs::read_to_string(repo_root()?.join("suite.lock.toml"))?;
    Ok(toml::from_str(&text)?)
}

pub(crate) fn revision() -> Result<String> {
    let lock = read_lock()?;
    validate_revision(&lock.pqty_revision)?;
    Ok(lock.pqty_revision)
}

fn validate_revision(revision: &str) -> Result<()> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(message(
            "pqty_revision must be a full 40-character Git commit ID",
        ))
    }
}

/// Checks that the pqty checkout is at the pinned revision.
pub(crate) fn verify(selected: Option<&Path>) -> Result<()> {
    let checkout = match selected {
        Some(path) => path.canonicalize()?,
        None => checkout()?,
    };
    let expected = revision()?;
    let head = git(&checkout, &["rev-parse", "HEAD"])?;
    if head.trim() != expected {
        return Err(message(format!(
            "pqty checkout is at {}, but suite.lock.toml pins {expected}",
            head.trim()
        )));
    }
    println!("pqty checkout is at the pinned revision {expected}");
    Ok(())
}

/// Pins the pqty checkout, which must be at `reference`, in suite.lock.toml.
pub(crate) fn update(reference: &str, selected: Option<&Path>) -> Result<()> {
    let checkout = match selected {
        Some(path) => path.canonicalize()?,
        None => checkout()?,
    };
    let revision = git(
        &checkout,
        &["rev-parse", &format!("{reference}^{{commit}}")],
    )?;
    let revision = revision.trim();
    let head = git(&checkout, &["rev-parse", "HEAD"])?;
    if head.trim() != revision {
        return Err(message(format!(
            "check out {reference} in {} first",
            checkout.display()
        )));
    }
    let capabilities = capabilities(&checkout)?;
    let lock = SuiteLock {
        pqty_version: capabilities.version.clone(),
        pqty_revision: revision.to_string(),
        pqty_capabilities_schema: capabilities.schema,
    };
    let path = repo_root()?.join("suite.lock.toml");
    fs::write(&path, toml::to_string_pretty(&lock)?)?;
    println!("pinned pqty {} at {revision}", capabilities.version);
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

fn git(checkout: &Path, arguments: &[&str]) -> Result<String> {
    capture(Command::new("git").current_dir(checkout).args(arguments))
}

#[cfg(test)]
mod tests {
    #[test]
    fn checkout_revision_must_be_a_full_commit_id() {
        assert!(super::validate_revision("8dfaa9597c364039eab85763b3629f4e6cd877f7").is_ok());
        for value in [
            "",
            "main",
            "v0.1.0",
            "8dfaa95",
            "../../pqty",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(super::validate_revision(value).is_err(), "{value}");
        }
    }
}
