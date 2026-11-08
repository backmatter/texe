//! The Citet latex-worker's outer Landlock policy, applied before texe runs.
//!
//! This mirrors `sandboxCommand` in Citet's `services/latex-worker/src/process.ts`
//! for the trusted texe stages (`texeStage: "fetch" | "compile"`), which the
//! worker applies with `setpriv --landlock-access ... --landlock-rule ...`:
//!
//! - every Landlock V3 filesystem right is handled;
//! - system trees are read-only (read-file, read-dir, execute), with external
//!   font directories partitioned out exactly as the worker does;
//! - a few system files are read-file only;
//! - `/proc` is read-file and read-dir;
//! - `TEXE_HOME` (the worker's `TEXE_CACHE`) has write access plus execute;
//! - the job directory has write access (never execute);
//! - `/dev/null` is read-file, write-file and truncate;
//! - nothing is granted on `/` itself.
//!
//! The worker installs texe, pqty and pqty-fls in `/usr/local/bin`, inside its
//! read-only `/usr`; here the directory holding the suite binaries stands in
//! for it with the same rights. The worker's operator seccomp filter and its
//! Shipout-only paths are not part of this policy.
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible as _, PathBeneath, PathFd, Ruleset,
    RulesetAttr as _, RulesetCreated, RulesetCreatedAttr as _, RulesetStatus,
};

use crate::{Result, message};

/// The hidden xtask subcommand that applies the policy and runs a command.
pub(crate) const LAUNCHER: &str = "__worker-sandbox";

const ABI_VERSION: ABI = ABI::V3;

/// The worker's `READ_ONLY_DIRECTORIES`, without its Shipout Python packages.
const READ_ONLY_DIRECTORIES: &[&str] = &[
    "/usr",
    "/bin",
    "/lib",
    "/lib64",
    "/opt",
    "/etc/fonts",
    "/etc/texmf",
    "/var/cache/fontconfig",
];
/// The worker's `READ_ONLY_FILES`, without its Shipout outline helper.
const READ_ONLY_FILES: &[&str] = &[
    "/etc/ld.so.cache",
    "/etc/localtime",
    "/etc/passwd",
    "/etc/group",
    "/etc/nsswitch.conf",
    "/dev/urandom",
];
const EXTERNAL_FONT_DIRECTORIES: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/usr/local/lib/X11/fonts",
    "/opt/fonts",
];
/// Resolver files only the fetch stage may read.
const FETCH_FILES: &[&str] = &["/etc/resolv.conf", "/etc/hosts", "/etc/gai.conf"];

/// A worker stage, as `texeStage` names it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    Fetch,
    Compile,
}

impl Stage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Compile => "compile",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "fetch" => Ok(Self::Fetch),
            "compile" => Ok(Self::Compile),
            _ => Err(message(format!("unknown worker stage `{value}`"))),
        }
    }
}

/// `READ_ACCESS`: read-file, read-dir, execute.
fn read_access() -> BitFlags<AccessFs> {
    AccessFs::ReadFile | AccessFs::ReadDir | AccessFs::Execute
}

/// `WRITE_ACCESS`: reading and every write right except execute and device nodes.
fn write_access() -> BitFlags<AccessFs> {
    AccessFs::ReadFile
        | AccessFs::ReadDir
        | AccessFs::WriteFile
        | AccessFs::RemoveFile
        | AccessFs::RemoveDir
        | AccessFs::MakeDir
        | AccessFs::MakeReg
        | AccessFs::MakeSym
        | AccessFs::MakeFifo
        | AccessFs::MakeSock
        | AccessFs::Refer
        | AccessFs::Truncate
}

/// The worker's rules for one stage, as (path, rights).
fn rules(
    stage: Stage,
    texe_home: &Path,
    job: &Path,
    binaries: &Path,
) -> Vec<(PathBuf, BitFlags<AccessFs>)> {
    let mut rules = Vec::new();
    let exclusions = EXTERNAL_FONT_DIRECTORIES
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<Vec<_>>();
    let mut seen = Vec::new();
    for directory in READ_ONLY_DIRECTORIES
        .iter()
        .map(PathBuf::from)
        .chain(std::iter::once(binaries.to_path_buf()))
    {
        read_only_tree(&directory, &exclusions, &mut seen, &mut rules);
    }
    for file in READ_ONLY_FILES {
        rules.push((PathBuf::from(file), AccessFs::ReadFile.into()));
    }
    rules.push((
        PathBuf::from("/proc"),
        AccessFs::ReadFile | AccessFs::ReadDir,
    ));
    rules.push((texe_home.to_path_buf(), write_access() | AccessFs::Execute));
    if matches!(stage, Stage::Fetch) {
        for file in FETCH_FILES {
            rules.push((PathBuf::from(file), AccessFs::ReadFile.into()));
        }
    }
    rules.push((job.to_path_buf(), write_access()));
    rules.push((
        PathBuf::from("/dev/null"),
        AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::Truncate,
    ));
    rules
        .into_iter()
        .filter(|(path, _)| path.exists())
        .collect()
}

/// The worker's `readOnlyTree`: grant `READ_ACCESS` on a tree unless it holds
/// an external font directory, whose ancestors get only read-dir while their
/// other children are granted one by one.
fn read_only_tree(
    path: &Path,
    exclusions: &[PathBuf],
    seen: &mut Vec<PathBuf>,
    rules: &mut Vec<(PathBuf, BitFlags<AccessFs>)>,
) {
    let Ok(canonical) = fs::canonicalize(path) else {
        return;
    };
    if exclusions.iter().any(|fonts| canonical.starts_with(fonts)) || seen.contains(&canonical) {
        return;
    }
    seen.push(canonical.clone());
    if !canonical.is_dir() {
        rules.push((path.to_path_buf(), AccessFs::ReadFile | AccessFs::Execute));
        return;
    }
    if !exclusions
        .iter()
        .any(|fonts| fonts.starts_with(&canonical) && fonts != &canonical)
    {
        rules.push((path.to_path_buf(), read_access()));
        return;
    }
    rules.push((path.to_path_buf(), AccessFs::ReadDir.into()));
    if let Ok(entries) = fs::read_dir(path) {
        let mut children = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            read_only_tree(&child, exclusions, seen, rules);
        }
    }
}

fn ruleset() -> Result<RulesetCreated> {
    Ok(Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI_VERSION))?
        .create()?)
}

/// Whether this kernel fully enforces the Landlock V3 policy. Probed on a
/// throwaway thread: Landlock restricts only the thread that applies it.
pub(crate) fn enforceable() -> bool {
    std::thread::spawn(|| {
        ruleset()
            .and_then(|ruleset| Ok(ruleset.restrict_self()?))
            .is_ok_and(|status| status.ruleset == RulesetStatus::FullyEnforced)
    })
    .join()
    .unwrap_or(false)
}

/// Apply the policy to this process and replace it with `command`.
pub(crate) fn launch(
    stage: &str,
    texe_home: &Path,
    job: &Path,
    binaries: &Path,
    command: &[OsString],
) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let stage = Stage::parse(stage)?;
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| message("the worker sandbox needs a command"))?;
    let mut ruleset = ruleset()?;
    for (path, rights) in rules(stage, texe_home, job, binaries) {
        let rights = if path.is_dir() {
            rights
        } else {
            // Landlock accepts only file rights on a file.
            rights
                & (AccessFs::ReadFile
                    | AccessFs::WriteFile
                    | AccessFs::Execute
                    | AccessFs::Truncate)
        };
        ruleset = ruleset.add_rule(PathBeneath::new(PathFd::new(&path)?, rights))?;
    }
    let status = ruleset.restrict_self()?;
    if status.ruleset != RulesetStatus::FullyEnforced {
        return Err(message(
            "the kernel cannot fully enforce the worker's Landlock V3 policy",
        ));
    }
    Err(std::process::Command::new(program)
        .args(arguments)
        .exec()
        .into())
}

/// The arguments that run `command` under the policy through this xtask.
pub(crate) fn command(
    stage: Stage,
    texe_home: &Path,
    job: &Path,
    binaries: &Path,
    command: &[OsString],
) -> Result<std::process::Command> {
    let mut launcher = std::process::Command::new(std::env::current_exe()?);
    launcher
        .arg(LAUNCHER)
        .arg("--stage")
        .arg(stage.as_str())
        .arg("--texe-home")
        .arg(texe_home)
        .arg("--job")
        .arg(job)
        .arg("--binaries")
        .arg(binaries)
        .arg("--")
        .args(command);
    Ok(launcher)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use landlock::AccessFs;

    use super::{Stage, rules};

    #[test]
    fn worker_policy_grants_nothing_on_the_root_and_never_executes_the_job() {
        let home = tempfile::tempdir().unwrap();
        let job = tempfile::tempdir().unwrap();
        let binaries = tempfile::tempdir().unwrap();
        for stage in [Stage::Fetch, Stage::Compile] {
            let rules = rules(stage, home.path(), job.path(), binaries.path());
            assert!(rules.iter().all(|(path, _)| path != Path::new("/")));
            let rights = |wanted: &Path| {
                rules
                    .iter()
                    .filter(|(path, _)| path == wanted)
                    .map(|(_, rights)| *rights)
                    .collect::<Vec<_>>()
            };
            let job_rights = rights(job.path());
            assert_eq!(job_rights.len(), 1);
            assert!(!job_rights[0].contains(AccessFs::Execute));
            assert!(rights(home.path())[0].contains(AccessFs::Execute | AccessFs::WriteFile));
            assert!(!rights(binaries.path())[0].contains(AccessFs::WriteFile));
            let resolver = rules
                .iter()
                .any(|(path, _)| path == Path::new("/etc/hosts"));
            assert_eq!(resolver, matches!(stage, Stage::Fetch));
        }
    }
}
