//! Isolated compiler processes for portable projects. The main CLI prepares
//! trusted components first; a fresh, single-threaded child then confines every
//! compiler and the helpers it starts before executing any project input.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::TexeError;

const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const MEMORY_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
// Shipout guards its 64 MiB live arena with a 32 GiB PROT_NONE reservation.
// Keep room for retained/rerun arenas without granting more committed memory.
const SHIPOUT_ADDRESS_SPACE_LIMIT: u64 = 128 * 1024 * 1024 * 1024;
const POLICY_VERSION: u32 = 1;
const PRIVATE_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const PRIVATE_ENTRY_LIMIT: usize = 100_000;
const DISK_RESERVE: u64 = 512 * 1024 * 1024;
const BUDGET_INTERVAL: Duration = Duration::from_millis(100);
static CANCELLED: AtomicBool = AtomicBool::new(false);
static GROUPS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static SIGNAL_HANDLER: OnceLock<()> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    version: u32,
    project: PathBuf,
    write_roots: Vec<PathBuf>,
    read_roots: Vec<PathBuf>,
    execute_roots: Vec<PathBuf>,
    #[serde(default)]
    shipout_sparse_arena: bool,
    #[serde(default = "private_output_limit")]
    private_output_bytes: u64,
    #[serde(default = "private_entry_limit")]
    private_output_entries: usize,
}

const fn private_output_limit() -> u64 {
    PRIVATE_OUTPUT_LIMIT
}
const fn private_entry_limit() -> usize {
    PRIVATE_ENTRY_LIMIT
}

fn error(message: impl std::fmt::Display) -> TexeError {
    TexeError::Build(format!(
        "The compiler could not run safely: {message}. Sources and the last PDF have been kept."
    ))
}

fn canonical(path: &Path) -> Result<PathBuf, TexeError> {
    fs::canonicalize(path).map_err(|source| TexeError::Io {
        path: path.to_owned(),
        source,
    })
}

/// A portable renderer must resolve font families from its pinned bytes, rather
/// than an ambient fontconfig installation or the researcher's home directory.
pub(crate) fn font_environment(
    project: &Path,
    private: &Path,
    fonts: &[PathBuf],
) -> Result<Vec<(OsString, OsString)>, TexeError> {
    let cache = private.join(".fontconfig-cache");
    crate::portable::private_directory(project, cache.strip_prefix(project).map_err(error)?)?;
    let escape = |value: &Path| {
        value
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let mut config = String::from("<?xml version=\"1.0\"?>\n<fontconfig>\n");
    for font in fonts.iter().filter(|path| path.is_dir()) {
        writeln!(config, "<dir>{}</dir>", escape(font)).expect("write to String");
    }
    writeln!(
        config,
        "<cachedir>{}</cachedir>\n</fontconfig>",
        escape(&cache)
    )
    .expect("write to String");
    let path = private.join(".fontconfig.conf");
    crate::build::generated::write(project, &path, config.as_bytes())?;
    Ok(vec![
        ("FONTCONFIG_FILE".into(), path.into_os_string()),
        ("FONTCONFIG_PATH".into(), private.as_os_str().into()),
    ])
}

#[cfg(target_os = "linux")]
fn system_read_paths() -> Result<Vec<(PathBuf, bool)>, TexeError> {
    // Landlock adds permissions. Granting /usr would also regrant all excluded
    // fonts, so partition only the ancestors of ambient font directories.
    let excluded = [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/usr/local/lib/X11/fonts",
        "/opt/fonts",
    ]
    .iter()
    .filter_map(|path| fs::canonicalize(path).ok())
    .collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = ["/usr", "/bin", "/lib", "/lib64"]
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<Vec<_>>();
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) || excluded.iter().any(|fonts| path.starts_with(fonts)) {
            continue;
        }
        let partition = path.is_dir() && excluded.iter().any(|fonts| fonts.starts_with(&path));
        result.push((path.clone(), partition));
        if partition {
            for entry in fs::read_dir(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| TexeError::Io {
                    path: path.clone(),
                    source,
                })?;
                match fs::canonicalize(entry.path()) {
                    Ok(path) => pending.push(path),
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(TexeError::Io {
                            path: entry.path(),
                            source,
                        });
                    }
                }
            }
        }
    }
    Ok(result)
}

impl Policy {
    pub(crate) fn new(
        project: &Path,
        write_roots: &[PathBuf],
        read_roots: &[PathBuf],
        execute_roots: &[PathBuf],
    ) -> Result<Self, TexeError> {
        let value = Self {
            version: POLICY_VERSION,
            project: canonical(project)?,
            write_roots: write_roots
                .iter()
                .map(|p| canonical(p))
                .collect::<Result<_, _>>()?,
            read_roots: read_roots
                .iter()
                .map(|p| canonical(p))
                .collect::<Result<_, _>>()?,
            execute_roots: execute_roots
                .iter()
                .map(|p| canonical(p))
                .collect::<Result<_, _>>()?,
            shipout_sparse_arena: false,
            private_output_bytes: PRIVATE_OUTPUT_LIMIT,
            private_output_entries: PRIVATE_ENTRY_LIMIT,
        };
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn with_shipout_arena(mut self, enabled: bool) -> Self {
        self.shipout_sparse_arena = enabled;
        self
    }

    fn validate(&self) -> Result<(), TexeError> {
        if self.version != POLICY_VERSION || self.write_roots.is_empty() {
            return Err(error("invalid isolation policy"));
        }
        if self.private_output_bytes == 0
            || self.private_output_bytes > PRIVATE_OUTPUT_LIMIT
            || self.private_output_entries == 0
            || self.private_output_entries > PRIVATE_ENTRY_LIMIT
        {
            return Err(error("invalid private output budget"));
        }
        let private = self.project.join(".texe");
        for path in &self.write_roots {
            if !path.starts_with(&private) || path == &private {
                return Err(error(
                    "compiler writes must use a private project build directory",
                ));
            }
            // Existing hard links could otherwise turn a permitted output write
            // into an outside source write. Symlinks cannot grant Landlock rights.
            reject_hardlinks(path)?;
        }
        for path in std::iter::once(&self.project)
            .chain(&self.write_roots)
            .chain(&self.read_roots)
            .chain(&self.execute_roots)
        {
            if !path.is_absolute() || canonical(path)? != *path {
                return Err(error("an isolation path changed during preparation"));
            }
        }
        Ok(())
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(clippy::too_many_lines)]
fn private_output_usage(policy: &Policy, group: Option<u32>) -> Result<(u64, u64), TexeError> {
    use rustix::fs::{
        AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags, fstatvfs, open, openat2, statat,
    };
    use std::collections::BTreeSet;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;
    let mut count = 0;
    let mut bytes = 0_u64;
    let mut inodes = BTreeSet::new();
    let mut free = u64::MAX;
    let mut account = |device, inode, logical: u64, blocks: u64| -> Result<(), TexeError> {
        if inodes.insert((device, inode)) {
            bytes = bytes.saturating_add(logical.max(blocks.saturating_mul(512)));
            if bytes > policy.private_output_bytes {
                return Err(error(
                    "private compiler output exceeded its disk budget; run `texe clean` to remove derived files and rebuild",
                ));
            }
        }
        Ok(())
    };
    let mut roots = policy.write_roots.clone();
    roots.sort();
    let mut covered = Vec::<PathBuf>::new();
    for root in roots {
        if covered.iter().any(|ancestor| root.starts_with(ancestor)) {
            continue;
        }
        covered.push(root.clone());
        let filesystem = open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?;
        let resolve =
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
        let anchor = openat2(
            &filesystem,
            root.strip_prefix("/").map_err(error)?,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            resolve,
        )
        .map_err(error)?;
        let capacity = fstatvfs(&anchor).map_err(error)?;
        free = free.min(capacity.f_bavail.saturating_mul(capacity.f_frsize));
        if free <= DISK_RESERVE {
            return Err(error(
                "less than 512 MiB of free disk space remains; free disk space or run `texe clean` to remove derived files and rebuild",
            ));
        }
        let mut pending = vec![PathBuf::from(".")];
        while let Some(relative) = pending.pop() {
            let directory = match openat2(
                &anchor,
                &relative,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                resolve,
            ) {
                Ok(directory) => directory,
                Err(rustix::io::Errno::NOENT) => continue,
                Err(source) => return Err(error(source)),
            };
            for entry in Dir::read_from(&directory).map_err(error)? {
                let entry = entry.map_err(error)?;
                let name = entry.file_name().to_bytes();
                if matches!(name, b"." | b"..") {
                    continue;
                }
                count += 1;
                if count > policy.private_output_entries {
                    return Err(error(
                        "private compiler output contains too many files; run `texe clean` to remove derived files and rebuild",
                    ));
                }
                let name = OsStr::from_bytes(name);
                let stat = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) => stat,
                    Err(rustix::io::Errno::NOENT) => continue,
                    Err(source) => return Err(error(source)),
                };
                account(
                    stat.st_dev,
                    stat.st_ino,
                    u64::try_from(stat.st_size).unwrap_or(0),
                    u64::try_from(stat.st_blocks).unwrap_or(0),
                )?;
                if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                    pending.push(relative.join(name));
                }
            }
        }
    }
    // An open temporary file still consumes disk after unlink. Count those
    // owned private files through kernel metadata, without reading contents.
    if let Some(group) = group {
        let mut handles = 0;
        for process in fs::read_dir("/proc").map_err(error)? {
            let process = process.map_err(error)?;
            if !process
                .file_name()
                .as_encoded_bytes()
                .iter()
                .all(u8::is_ascii_digit)
            {
                continue;
            }
            let Ok(stat) = fs::read_to_string(process.path().join("stat")) else {
                continue;
            };
            let found = stat
                .rsplit_once(") ")
                .and_then(|(_, fields)| fields.split_whitespace().nth(2))
                .and_then(|field| field.parse::<u32>().ok());
            if found != Some(group) {
                continue;
            }
            let descriptors = match fs::read_dir(process.path().join("fd")) {
                Ok(descriptors) => descriptors,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(error(source)),
            };
            for descriptor in descriptors {
                let descriptor = descriptor.map_err(error)?;
                handles += 1;
                if handles > PRIVATE_ENTRY_LIMIT {
                    return Err(error(
                        "private compiler output has too many files or open handles; run `texe clean` and rebuild",
                    ));
                }
                let Ok(target) = fs::read_link(descriptor.path()) else {
                    continue;
                };
                if !target.starts_with(policy.project.join(".texe")) {
                    continue;
                }
                let Ok(metadata) = fs::metadata(descriptor.path()) else {
                    continue;
                };
                if metadata.is_file() && metadata.nlink() == 0 {
                    count += 1;
                    if count > policy.private_output_entries {
                        return Err(error(
                            "private compiler output contains too many files; run `texe clean` and rebuild",
                        ));
                    }
                    account(
                        metadata.dev(),
                        metadata.ino(),
                        metadata.len(),
                        metadata.blocks(),
                    )?;
                }
            }
        }
    }
    Ok((bytes, free))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn initial_output_budget(policy: &Policy) -> Result<u64, TexeError> {
    let (used, free) = private_output_usage(policy, None)?;
    budget_for_space(policy.private_output_bytes, used, free)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn budget_for_space(limit: u64, used: u64, free: u64) -> Result<u64, TexeError> {
    if free.saturating_sub(DISK_RESERVE) < 8 * 1024 * 1024 {
        return Err(error(
            "insufficient disk space for a protected build; free disk space or run `texe clean` and rebuild",
        ));
    }
    // Warm caches remain usable. Limit additional output to available space
    // above the reserve, and also retain the absolute aggregate ceiling.
    Ok(limit.min(used.saturating_add(free.saturating_sub(DISK_RESERVE))))
}

fn reject_hardlinks(root: &Path) -> Result<(), TexeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut pending = vec![root.to_owned()];
        let mut count = 0usize;
        while let Some(path) = pending.pop() {
            count += 1;
            if count > 100_000 {
                return Err(error("the private build directory has too many files"));
            }
            let meta = fs::symlink_metadata(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            if meta.is_file() && meta.nlink() > 1 {
                return Err(error(
                    "the private build directory contains a linked outside file",
                ));
            }
            if meta.is_dir() {
                for child in fs::read_dir(&path).map_err(|source| TexeError::Io {
                    path: path.clone(),
                    source,
                })? {
                    pending.push(
                        child
                            .map_err(|source| TexeError::Io {
                                path: path.clone(),
                                source,
                            })?
                            .path(),
                    );
                }
            }
        }
    }
    Ok(())
}

fn allowed_environment(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    matches!(
        key,
        "PATH"
            | "LANG"
            | "LC_ALL"
            | "LC_CTYPE"
            | "TZ"
            | "SOURCE_DATE_EPOCH"
            | "FORCE_SOURCE_DATE"
            | "LD_LIBRARY_PATH"
            | "SHIPOUT_ASSETS_ROOT"
            | "TECTONIC_CACHE_DIR"
            | "TECTONIC_UNTRUSTED_MODE"
            | "PAR_GLOBAL_TMPDIR"
            | "PAR_GLOBAL_TEMP"
            | "FONTCONFIG_PATH"
            | "FONTCONFIG_FILE"
            | "TEXMF"
            | "TEXMFDIST"
            | "TEXMFHOME"
            | "TEXMFLOCAL"
            | "TEXMFVAR"
            | "TEXMFCONFIG"
            | "TEXMFCACHE"
            | "TEXMFDBS"
            | "TEXMFROOT"
            | "TEXMFSYSVAR"
            | "TEXMFSYSCONFIG"
            | "TEXFORMATS"
            | "TEXINPUTS"
            | "BIBINPUTS"
            | "BSTINPUTS"
            | "TEXFONTMAPS"
            | "TFMFONTS"
            | "VFFONTS"
            | "ENCFONTS"
            | "T1FONTS"
            | "TTFONTS"
            | "OPENTYPEFONTS"
            | "OSFONTDIR"
            | "LUAINPUTS"
            | "CLUAINPUTS"
            | "TEXINPUTS_pdflatex"
            | "TEXINPUTS_xelatex"
            | "TEXINPUTS_lualatex"
            | "LUAINPUTS_pdflatex"
            | "LUAINPUTS_xelatex"
            | "LUAINPUTS_lualatex"
            | "openin_any"
            | "openout_any"
            | "shell_escape"
    )
}

fn environment(
    command: &mut std::process::Command,
    supplied: &[(OsString, OsString)],
    policy: &Policy,
) {
    command.env_clear();
    command.env("PATH", "/usr/local/bin:/usr/bin:/bin");
    command.env("LANG", "C.UTF-8");
    command.env("LC_ALL", "C.UTF-8");
    for (key, value) in supplied {
        if allowed_environment(key) {
            command.env(key, value);
        }
    }
    let private = &policy.write_roots[0];
    for key in ["HOME", "TMPDIR", "TMP", "TEMP", "XDG_CACHE_HOME"] {
        command.env(key, private);
    }
    command.env("openin_any", "p");
    command.env("openout_any", "p");
    command.env("shell_escape", "f");
    command.env("TECTONIC_UNTRUSTED_MODE", "1");
}

pub(crate) fn cancel_active() {
    CANCELLED.store(true, Ordering::SeqCst);
    if let Ok(groups) = GROUPS.lock() {
        for &pid in groups.iter() {
            kill_group(pid);
        }
    }
}

#[cfg(target_os = "linux")]
fn kill_group(pid: u32) {
    if let Ok(raw) = i32::try_from(pid)
        && let Some(pid) = rustix::process::Pid::from_raw(raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

#[cfg(not(target_os = "linux"))]
fn kill_group(_: u32) {}

fn collect(mut pipe: impl Read, total: &AtomicUsize, overflow: &AtomicBool) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(length) => {
                let previous = total.fetch_add(length, Ordering::SeqCst);
                let keep = length.min(OUTPUT_LIMIT.saturating_sub(previous));
                output.extend_from_slice(&buffer[..keep]);
                if keep < length {
                    overflow.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    }
    output
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn process_group_resident_bytes(group: u32) -> Result<u64, TexeError> {
    let mut total = 0u64;
    for entry in fs::read_dir("/proc").map_err(error)? {
        let entry = entry.map_err(error)?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let path = entry.path().join("stat");
        let stat = match fs::read_to_string(&path) {
            Ok(stat) => stat,
            // Unrelated users' processes can be hidden, and exited processes
            // disappear during enumeration. Our group retains the caller's UID.
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue;
            }
            Err(source) => return Err(error(source)),
        };
        total = total.saturating_add(resident_bytes_in_group(&stat, group)?);
    }
    Ok(total)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn resident_bytes_in_group(stat: &str, group: u32) -> Result<u64, TexeError> {
    // Linux stat fields 5/24 are process group/resident pages. The command name
    // may itself contain spaces or parentheses, so split after its final ')'.
    let fields = stat
        .rsplit_once(") ")
        .map(|(_, fields)| fields)
        .ok_or_else(|| error("could not read compiler memory usage"))?
        .split_whitespace()
        .collect::<Vec<_>>();
    let process_group = fields
        .get(2)
        .and_then(|field| field.parse::<u32>().ok())
        .ok_or_else(|| error("could not read compiler process group"))?;
    if process_group != group {
        return Ok(0);
    }
    let resident_pages = fields
        .get(21)
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(|| error("could not read compiler resident memory"))?;
    // The protected launcher supports Linux x86_64, whose base pages are 4 KiB.
    Ok(resident_pages.saturating_mul(4096))
}

#[allow(clippy::too_many_lines)]
pub(crate) fn run(
    tool: &Path,
    args: &[OsString],
    cwd: &Path,
    supplied_environment: &[(OsString, OsString)],
    policy: &Policy,
) -> Result<Output, TexeError> {
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (tool, args, cwd, supplied_environment, policy);
        return Err(error(
            "this system does not support protected portable compilation",
        ));
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use std::os::unix::process::CommandExt;
        policy.validate()?;
        let mut bounded = policy.clone();
        bounded.private_output_bytes = initial_output_budget(policy)?;
        let policy = &bounded;
        if CANCELLED.load(Ordering::SeqCst) {
            return Err(TexeError::Prompt("cancelled".into()));
        }
        SIGNAL_HANDLER.get_or_init(|| {
            // Watch already owns the handler and forwards cancellation here.
            let _ = ctrlc::set_handler(cancel_active);
        });
        let launcher = std::env::current_exe().map_err(|source| TexeError::Spawn {
            tool: "texe compiler isolation".into(),
            source,
        })?;
        let serialized = serde_json::to_string(policy).map_err(error)?;
        let mut command = crate::process::command(launcher);
        command
            .arg("__sandbox-run")
            .arg("--policy")
            .arg(serialized)
            .arg("--executable")
            .arg(tool)
            .arg("--")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        environment(&mut command, supplied_environment, policy);
        let mut child = command.spawn().map_err(|source| TexeError::Spawn {
            tool: tool.to_owned(),
            source,
        })?;
        let pid = child.id();
        GROUPS
            .lock()
            .map_err(|_| error("could not track compiler cancellation"))?
            .push(pid);
        let total = Arc::new(AtomicUsize::new(0));
        let overflow = Arc::new(AtomicBool::new(false));
        let stdout = child.stdout.take().expect("piped compiler stdout");
        let stderr = child.stderr.take().expect("piped compiler stderr");
        let out_total = Arc::clone(&total);
        let out_overflow = Arc::clone(&overflow);
        let out = std::thread::spawn(move || collect(stdout, &out_total, &out_overflow));
        let err_overflow = Arc::clone(&overflow);
        let err = std::thread::spawn(move || collect(stderr, &total, &err_overflow));
        let timeout = std::env::var("TEXE_COMPILER_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(120)
            .clamp(1, 1800);
        let start = Instant::now();
        let mut next_memory_check = Instant::now();
        let mut failure = None;
        let status = loop {
            if overflow.load(Ordering::SeqCst) {
                failure = Some(error("compiler output exceeded the 16 MiB limit"));
            } else if CANCELLED.load(Ordering::SeqCst) {
                failure = Some(TexeError::Prompt("cancelled".into()));
            } else if start.elapsed() >= Duration::from_secs(timeout) {
                failure = Some(error(format!("compilation exceeded {timeout} seconds")));
            }
            if failure.is_none() && Instant::now() >= next_memory_check {
                next_memory_check = Instant::now() + Duration::from_millis(100);
                match process_group_resident_bytes(pid) {
                    Ok(bytes) if bytes > MEMORY_LIMIT => {
                        failure = Some(error("compiler resident memory exceeded the 2 GiB limit"));
                    }
                    Ok(_) => (),
                    Err(source) => failure = Some(source),
                }
            }
            if failure.is_some() {
                kill_group(pid);
                break child.wait().map_err(|source| TexeError::Spawn {
                    tool: tool.to_owned(),
                    source,
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(source) => {
                    break Err(TexeError::Spawn {
                        tool: tool.to_owned(),
                        source,
                    });
                }
            }
        };
        // Close inherited helper pipes and prevent background jobs outliving a build.
        kill_group(pid);
        if failure.is_none()
            && let Err(source) = private_output_usage(policy, None)
        {
            failure = Some(source);
        }
        if let Ok(mut groups) = GROUPS.lock() {
            groups.retain(|group| *group != pid);
        }
        let stdout = out
            .join()
            .map_err(|_| error("could not collect compiler output"))?;
        let stderr = err
            .join()
            .map_err(|_| error("could not collect compiler errors"))?;
        if let Some(failure) = failure {
            return Err(failure);
        }
        Ok(Output {
            status: status?,
            stdout,
            stderr,
        })
    }
}

pub(crate) fn launch(serialized: &str, tool: &Path, args: &[OsString]) -> Result<(), TexeError> {
    let mut policy: Policy = serde_json::from_str(serialized).map_err(error)?;
    policy.validate()?;
    let tool = canonical(tool)?;
    if !policy
        .execute_roots
        .iter()
        .any(|root| tool.starts_with(root))
        && !["/usr/bin", "/usr/local/bin", "/bin"]
            .iter()
            .any(|root| tool.starts_with(root))
    {
        return Err(error(
            "the requested compiler is outside its installed runtime",
        ));
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use std::os::unix::process::CommandExt;
        policy.private_output_bytes = initial_output_budget(&policy)?;
        // The monitor shares the compiler's private process group and uses a
        // kernel PID descriptor, so even SIGKILL of the parent cannot leave
        // bibliography helpers or background descendants running.
        let parent =
            rustix::process::getppid().ok_or_else(|| error("the compiler parent stopped"))?;
        let group = std::process::id();
        if parent.as_raw_nonzero().get() <= 1
            || u32::try_from(rustix::process::getpgrp().as_raw_nonzero().get()).map_err(error)?
                != group
        {
            return Err(error("the compiler must run in its own process group"));
        }
        let launcher = std::env::current_exe().map_err(|source| TexeError::Spawn {
            tool: "texe compiler monitor".into(),
            source,
        })?;
        crate::process::command(launcher)
            .env_clear()
            .arg("__sandbox-watch")
            .arg("--parent")
            .arg(parent.as_raw_nonzero().get().to_string())
            .arg("--group")
            .arg(group.to_string())
            .arg("--policy")
            .arg(serde_json::to_string(&policy).map_err(error)?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|source| TexeError::Spawn {
                tool: "texe compiler monitor".into(),
                source,
            })?;
        apply(&policy)?;
        let mut command = crate::process::command(&tool);
        command.args(args);
        let supplied: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| allowed_environment(key))
            .collect();
        environment(&mut command, &supplied, &policy);
        Err(TexeError::Spawn {
            tool: tool.clone(),
            source: command.exec(),
        })
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (tool, args);
        Err(error(
            "this system does not support protected portable compilation",
        ))
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(clippy::too_many_lines)]
fn apply(policy: &Policy) -> Result<(), TexeError> {
    use landlock::{
        ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus,
    };
    use rustix::process::{Resource, Rlimit, Signal};
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };
    use std::collections::BTreeMap;

    rustix::process::set_parent_process_death_signal(Some(Signal::KILL)).map_err(error)?;
    for (resource, limit) in [
        (Resource::Core, 0),
        (
            Resource::As,
            if policy.shipout_sparse_arena {
                SHIPOUT_ADDRESS_SPACE_LIMIT
            } else {
                MEMORY_LIMIT
            },
        ),
        (Resource::Data, MEMORY_LIMIT),
        (Resource::Fsize, 1024 * 1024 * 1024),
        (Resource::Cpu, 1800),
    ] {
        rustix::process::setrlimit(
            resource,
            Rlimit {
                current: Some(limit),
                maximum: Some(limit),
            },
        )
        .map_err(error)?;
    }
    let abi = ABI::V3; // V3 is required: output truncation must also be confined.
    let all = AccessFs::from_all(abi);
    let read = AccessFs::ReadFile | AccessFs::ReadDir;
    let writable = all & !(AccessFs::Execute | AccessFs::MakeChar | AccessFs::MakeBlock);
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(all)
        .map_err(error)?
        .create()
        .map_err(error)?;
    let mut rules = vec![(policy.project.clone(), read)];
    rules.extend(policy.read_roots.iter().cloned().map(|path| (path, read)));
    rules.extend(system_read_paths()?.into_iter().map(|(path, ancestor)| {
        (
            path,
            if ancestor {
                AccessFs::ReadDir.into()
            } else {
                read
            },
        )
    }));
    for path in [
        "/etc/fonts",
        "/var/cache/fontconfig",
        "/etc/ld.so.cache",
        "/etc/localtime",
        "/etc/passwd",
        "/etc/group",
        "/etc/nsswitch.conf",
        "/dev/urandom",
    ] {
        if Path::new(path).exists() {
            rules.push((PathBuf::from(path), read));
        }
    }
    rules.extend(
        policy
            .write_roots
            .iter()
            .cloned()
            .map(|path| (path, writable)),
    );
    rules.extend(
        policy
            .execute_roots
            .iter()
            .cloned()
            .map(|path| (path, read | AccessFs::Execute)),
    );
    for path in ["/usr/bin", "/usr/local/bin", "/bin"] {
        if Path::new(path).exists() {
            rules.push((PathBuf::from(path), read | AccessFs::Execute));
        }
    }
    // execve also opens a dynamically linked executable's ELF interpreter
    // with execute access. Grant the loader file, not the entire library tree.
    for path in [
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
        "/lib/ld-linux-x86-64.so.2",
    ] {
        if Path::new(path).exists() {
            rules.push((PathBuf::from(path), read | AccessFs::Execute));
        }
    }
    rules.push((
        PathBuf::from("/dev/null"),
        AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::Truncate,
    ));
    for (path, mut access) in rules {
        if !path.is_dir() {
            access &= AccessFs::from_file(abi);
        }
        ruleset = ruleset
            .add_rule(
                PathBeneath::new(PathFd::new(&path).map_err(error)?, access)
                    .set_compatibility(CompatLevel::HardRequirement),
            )
            .map_err(error)?;
    }
    let status = ruleset.restrict_self().map_err(error)?;
    if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
        return Err(error(
            "the kernel cannot fully enforce compiler file isolation (Linux Landlock V3 is required)",
        ));
    }
    // Landlock does not cover file metadata mutation. These calls must remain
    // denied even for read-only descriptors to researcher-owned source files.
    let denied = [
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_sendto,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        libc::SYS_recvfrom,
        libc::SYS_recvmsg,
        libc::SYS_recvmmsg,
        libc::SYS_shutdown,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_getfd,
        libc::SYS_pidfd_send_signal,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_chmod,
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        452, // fchmodat2
        libc::SYS_chown,
        libc::SYS_fchown,
        libc::SYS_lchown,
        libc::SYS_fchownat,
        libc::SYS_utime,
        libc::SYS_utimes,
        libc::SYS_futimesat,
        libc::SYS_utimensat,
        libc::SYS_setxattr,
        libc::SYS_lsetxattr,
        libc::SYS_fsetxattr,
        libc::SYS_removexattr,
        libc::SYS_lremovexattr,
        libc::SYS_fremovexattr,
        libc::SYS_setsid,
        libc::SYS_setpgid,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
    ];
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> =
        denied.into_iter().map(|call| (call, vec![])).collect();
    // Biber's packaged Perl sets its own process title. Allow only that
    // harmless thread metadata; parent-death, dumpability and privilege
    // controls remain denied.
    rules.insert(
        libc::SYS_prctl,
        vec![
            SeccompRule::new(vec![
                SeccompCondition::new(
                    0,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    libc::PR_SET_NAME as u64,
                )
                .map_err(error)?,
                SeccompCondition::new(
                    0,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    libc::PR_GET_NAME as u64,
                )
                .map_err(error)?,
            ])
            .map_err(error)?,
        ],
    );
    // Rust's external-tool runner toggles nonblocking mode on its private
    // pipe. This changes descriptor flags, not source bytes or inode metadata.
    // Every other ioctl remains denied, including filesystem mutation.
    rules.insert(
        libc::SYS_ioctl,
        vec![
            SeccompRule::new(vec![
                SeccompCondition::new(1, SeccompCmpArgLen::Qword, SeccompCmpOp::Ne, libc::FIONBIO)
                    .map_err(error)?,
            ])
            .map_err(error)?,
        ],
    );
    // File-descriptor signal ownership can otherwise send SIGIO to the parent
    // without kill(2); leases can stall other editors of a researcher-owned file.
    let mut fcntl = Vec::new();
    for command in [8, 10, 15, 1024, 1026] {
        // F_SETOWN, F_SETSIG, F_SETOWN_EX, F_SETLEASE, F_NOTIFY
        fcntl.push(
            SeccompRule::new(vec![
                SeccompCondition::new(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, command)
                    .map_err(error)?,
            ])
            .map_err(error)?,
        );
    }
    rules.insert(libc::SYS_fcntl, fcntl);
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        TargetArch::x86_64,
    )
    .map_err(error)?;
    let mut program: BpfProgram = filter.try_into().map_err(error)?;
    // AUDIT_ARCH_X86_64 is shared with the x32 ABI. Reject its syscall-number
    // namespace before the ordinary architecture-aware deny filter.
    program.splice(
        0..0,
        [
            seccompiler::sock_filter {
                code: 0x20,
                jt: 0,
                jf: 0,
                k: 0,
            },
            seccompiler::sock_filter {
                code: 0x35,
                jt: 0,
                jf: 1,
                k: 0x4000_0000,
            },
            seccompiler::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x0005_0000 | libc::ENOSYS as u32,
            },
        ],
    );
    // New syscall families must be reviewed before gaining access. Existing
    // engines need no calls introduced after Linux's fchmodat2 (452).
    program.splice(
        3..3,
        [
            seccompiler::sock_filter {
                code: 0x35,
                jt: 0,
                jf: 1,
                k: 453,
            },
            seccompiler::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x0005_0000 | libc::EPERM as u32,
            },
        ],
    );
    seccompiler::apply_filter(&program).map_err(error)?;
    Ok(())
}

/// Prepare PAR's lazy native module copies from a freshly unpacked, trusted
/// Biber runtime. No project bibliography or compiler-produced bytes may be
/// passed to this parent-side preparation step.
pub(crate) fn prepare_biber_native_libraries(
    project: &Path,
    cache: &Path,
) -> Result<(), TexeError> {
    let cache = canonical(cache)?;
    Policy::new(project, std::slice::from_ref(&cache), &[], &[])?;
    let mut pending = vec![cache.clone()];
    let mut files = 0usize;
    while let Some(path) = pending.pop() {
        files += 1;
        if files > 100_000 {
            return Err(error("the bibliography runtime has too many files"));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.is_dir() {
            for item in fs::read_dir(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })? {
                pending.push(
                    item.map_err(|source| TexeError::Io {
                        path: path.clone(),
                        source,
                    })?
                    .path(),
                );
            }
        } else if metadata.is_file() && path.extension() == Some(OsStr::new("so")) {
            let Some(inc) = path
                .ancestors()
                .skip(1)
                .find(|path| path.file_name() == Some(OsStr::new("inc")))
            else {
                continue;
            };
            let destination = inc
                .parent()
                .ok_or_else(|| error("invalid bibliography native module path"))?;
            if !destination.starts_with(&cache) || metadata.len() > 64 * 1024 * 1024 {
                return Err(error("invalid bibliography native module"));
            }
            let bytes = fs::read(&path).map_err(|source| TexeError::Io {
                path: path.clone(),
                source,
            })?;
            let target = destination.join(format!("{:08x}.so", crc32fast::hash(&bytes)));
            if let Ok(existing) = fs::symlink_metadata(&target) {
                if !existing.is_file()
                    || existing.file_type().is_symlink()
                    || existing.len() != bytes.len() as u64
                    || fs::read(&target).map_err(|source| TexeError::Io {
                        path: target.clone(),
                        source,
                    })? != bytes
                {
                    return Err(error(
                        "the bibliography native module cache contains a CRC collision",
                    ));
                }
            } else {
                crate::atomic::write(&target, &bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).map_err(
                        |source| TexeError::Io {
                            path: target,
                            source,
                        },
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn watch_parent(parent: u32, group: u32, serialized: &str) -> Result<(), TexeError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use rustix::event::{PollFd, PollFlags};
        use rustix::process::{Pid, PidfdFlags};
        let expected = Pid::from_raw(i32::try_from(group).map_err(error)?)
            .ok_or_else(|| error("invalid compiler process group"))?;
        if rustix::process::getpgrp() != expected || group <= 1 || parent <= 1 {
            return Err(error("invalid compiler cancellation monitor"));
        }
        let parent = Pid::from_raw(i32::try_from(parent).map_err(error)?)
            .ok_or_else(|| error("invalid compiler parent"))?;
        let Ok(parent) = rustix::process::pidfd_open(parent, PidfdFlags::empty()) else {
            kill_group(group);
            return Err(error("the compiler parent stopped"));
        };
        let Ok(leader) = rustix::process::pidfd_open(expected, PidfdFlags::empty()) else {
            kill_group(group);
            return Ok(());
        };
        let mut events = [
            PollFd::new(&parent, PollFlags::IN),
            PollFd::new(&leader, PollFlags::IN),
        ];
        let policy: Policy = if let Ok(policy) = serde_json::from_str(serialized) {
            policy
        } else {
            kill_group(group);
            return Err(error("invalid output monitor policy"));
        };
        let interval = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: i64::try_from(BUDGET_INTERVAL.as_nanos())
                .expect("budget interval fits timespec"),
        };
        loop {
            match rustix::event::poll(&mut events, Some(&interval)) {
                Ok(0) => {
                    if let Err(failure) = private_output_usage(&policy, Some(group)) {
                        eprintln!("{failure}");
                        kill_group(group);
                        return Err(failure);
                    }
                }
                Ok(_) => {
                    kill_group(group);
                    return Ok(());
                }
                Err(rustix::io::Errno::INTR) => (),
                Err(_) => {
                    kill_group(group);
                    return Err(error("compiler cancellation monitoring failed"));
                }
            }
        }
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (parent, group, serialized);
        Err(error(
            "this system does not support protected portable compilation",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{OUTPUT_LIMIT, Policy, collect};
    use std::io::Read;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn private_budget_adapts_to_free_space_without_charging_a_warm_cache_twice() {
        let mib = 1024 * 1024;
        assert!(
            super::budget_for_space(
                super::PRIVATE_OUTPUT_LIMIT,
                0,
                super::DISK_RESERVE + 7 * mib
            )
            .is_err()
        );
        assert_eq!(
            super::budget_for_space(
                super::PRIVATE_OUTPUT_LIMIT,
                64 * mib,
                super::DISK_RESERVE + 256 * mib
            )
            .unwrap(),
            320 * mib
        );
        assert_eq!(
            super::budget_for_space(super::PRIVATE_OUTPUT_LIMIT, 64 * mib, 16 * 1024 * mib)
                .unwrap(),
            super::PRIVATE_OUTPUT_LIMIT
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn resident_monitor_counts_our_group_and_ignores_sparse_virtual_size() {
        let stat =
            "42 (compiler (title)) R 1 42 42 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 34359738368 8192 0";
        assert_eq!(
            super::resident_bytes_in_group(stat, 42).unwrap(),
            8192 * 4096
        );
        assert_eq!(super::resident_bytes_in_group(stat, 43).unwrap(), 0);
        assert!(super::resident_bytes_in_group("bad stat", 42).is_err());
    }

    #[test]
    fn combined_stdout_and_stderr_never_exceed_capture_budget() {
        let total = Arc::new(AtomicUsize::new(0));
        let overflow = Arc::new(AtomicBool::new(false));
        let stdout = collect(
            std::io::repeat(b'x').take(OUTPUT_LIMIT as u64 - 3),
            &total,
            &overflow,
        );
        let stderr = collect(std::io::repeat(b'y').take(20), &total, &overflow);
        assert_eq!(stdout.len() + stderr.len(), OUTPUT_LIMIT);
        assert_eq!(stderr, b"yyy");
        assert!(overflow.load(Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_output_hardlink_cannot_grant_source_write_access() {
        let fixture = tempfile::tempdir().unwrap();
        let build = fixture.path().join(".texe/environments/test/build");
        std::fs::create_dir_all(&build).unwrap();
        let source = fixture.path().join("source.tex");
        std::fs::write(&source, "research source").unwrap();
        std::fs::hard_link(&source, build.join("output.tex")).unwrap();
        assert!(Policy::new(fixture.path(), &[build], &[], &[]).is_err());
        assert_eq!(std::fs::read_to_string(source).unwrap(), "research source");
    }

    #[test]
    fn portable_fontconfig_uses_only_pinned_font_directories() {
        let fixture = tempfile::tempdir().unwrap();
        let output = fixture.path().join(".texe/environments/test/build/output");
        std::fs::create_dir_all(&output).unwrap();
        let fonts = fixture.path().join("locked & fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        let environment = super::font_environment(fixture.path(), &output, &[fonts]).unwrap();
        let config = std::fs::read_to_string(output.join(".fontconfig.conf")).unwrap();
        assert!(config.contains("locked &amp; fonts"));
        assert!(!config.contains("<include"));
        assert!(!config.contains("/etc/fonts"));
        assert_eq!(environment[0].0, "FONTCONFIG_FILE");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn system_read_rules_do_not_regrant_ambient_fonts() {
        let rules = super::system_read_paths().unwrap();
        for fonts in [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/usr/local/lib/X11/fonts",
            "/opt/fonts",
        ] {
            if let Ok(fonts) = std::fs::canonicalize(fonts) {
                assert!(
                    !rules
                        .iter()
                        .any(|(path, directory_only)| !directory_only && fonts.starts_with(path)),
                    "{}",
                    fonts.display()
                );
            }
        }
        assert!(rules.iter().any(|(path, directory_only)| !directory_only
            && std::path::Path::new("/usr/bin").starts_with(path)));
    }
}
