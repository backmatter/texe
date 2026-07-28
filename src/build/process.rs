use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, Read as _};
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;

use crate::TexeError;

pub(crate) fn checked_output(
    tool: &Path,
    arguments: &[OsString],
    cwd: &Path,
    environment: &[(OsString, OsString)],
) -> Result<Output, TexeError> {
    let output = raw_output(tool, arguments, cwd, environment)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(TexeError::Process {
            tool: tool.to_path_buf(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub(crate) fn raw_output(
    tool: &Path,
    arguments: &[OsString],
    cwd: &Path,
    environment: &[(OsString, OsString)],
) -> Result<Output, TexeError> {
    output_with_removed_environment(tool, arguments, cwd, environment, |_| false)
}

fn output_with_removed_environment(
    tool: &Path,
    arguments: &[OsString],
    cwd: &Path,
    environment: &[(OsString, OsString)],
    remove: impl Fn(&OsStr) -> bool,
) -> Result<Output, TexeError> {
    let mut command = Command::new(tool);
    command.args(arguments).current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        if remove(&name) {
            command.env_remove(name);
        }
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().map_err(|source| TexeError::Spawn {
        tool: tool.to_path_buf(),
        source,
    })
}

pub(crate) fn raw_output_streaming(
    tool: &Path,
    arguments: &[OsString],
    cwd: &Path,
    environment: &[(OsString, OsString)],
    mut consume_stderr_line: impl FnMut(&str) -> bool,
) -> Result<Output, TexeError> {
    let mut command = Command::new(tool);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|source| TexeError::Spawn {
        tool: tool.to_path_buf(),
        source,
    })?;
    let stdout = child
        .stdout
        .take()
        .expect("stdout was configured as a pipe");
    let stderr = child
        .stderr
        .take()
        .expect("stderr was configured as a pipe");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::BufReader::new(stdout).read_to_end(&mut bytes);
        (result, bytes)
    });

    let mut captured_stderr = Vec::new();
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = stderr
            .read_until(b'\n', &mut line)
            .map_err(|source| TexeError::Io {
                path: tool.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        if !consume_stderr_line(text.trim_end_matches(['\r', '\n'])) {
            captured_stderr.extend_from_slice(&line);
        }
    }
    let status = child.wait().map_err(|source| TexeError::Io {
        path: tool.to_path_buf(),
        source,
    })?;
    let (stdout_result, stdout) = stdout_reader.join().map_err(|_| {
        TexeError::Build(format!(
            "stdout reader for {} terminated unexpectedly",
            tool.display()
        ))
    })?;
    stdout_result.map_err(|source| TexeError::Io {
        path: tool.to_path_buf(),
        source,
    })?;
    Ok(Output {
        status,
        stdout,
        stderr: captured_stderr,
    })
}

pub(crate) fn raw_engine_output(
    tool: &Path,
    arguments: &[OsString],
    cwd: &Path,
    environment: &[(OsString, OsString)],
) -> Result<Output, TexeError> {
    output_with_removed_environment(tool, arguments, cwd, environment, is_engine_search_variable)
}

/// The official Biber binaries are self-contained PAR executables. Keep their
/// internal loader state and Perl module search independent of the caller,
/// just as managed TeX runs ignore inherited Kpathsea searches.
pub(crate) fn raw_bundled_biber_output(
    tool: &Path,
    arguments: &[OsString],
    cwd: &Path,
    environment: &[(OsString, OsString)],
) -> Result<Output, TexeError> {
    output_with_removed_environment(tool, arguments, cwd, environment, |name| {
        is_engine_search_variable(name) || is_standalone_perl_variable(name)
    })
}

pub(super) fn search_path_from(roots: &[&Path], working_directory: &Path) -> OsString {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let value = roots
        .iter()
        .map(|root| {
            if *root == Path::new(".") {
                ".".to_string()
            } else {
                let path = engine_path_from(root, working_directory);
                let path = path.to_string_lossy();
                format!("{path}//")
            }
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string());
    OsString::from(value)
}

/// TeX Live accepts forward slashes on every supported platform, while some
/// Kpathsea command-line inputs interpret native Windows backslashes
/// inconsistently.
pub(super) fn engine_path(path: &Path) -> OsString {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy();
        let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            format!("//{unc}")
        } else if let Some(local) = path.strip_prefix(r"\\?\") {
            local.to_string()
        } else {
            path.into_owned()
        };
        OsString::from(path.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().to_os_string()
    }
}

/// Keep Windows search variables free of non-ASCII ancestor names. Kpathsea's
/// Windows environment parser is not Unicode-safe, while relative paths are
/// resolved from the Unicode-aware process working directory.
pub(super) fn engine_path_from(path: &Path, working_directory: &Path) -> OsString {
    #[cfg(windows)]
    {
        if let Some(relative) = relative_path(path, working_directory) {
            return engine_path(&relative);
        }
    }
    #[cfg(not(windows))]
    let _ = working_directory;
    engine_path(path)
}

#[cfg(windows)]
fn relative_path(path: &Path, working_directory: &Path) -> Option<PathBuf> {
    if path.is_relative() {
        return Some(path.to_path_buf());
    }
    if !working_directory.is_absolute() {
        return None;
    }

    let mut ancestor = working_directory;
    let mut relative = PathBuf::new();
    loop {
        if let Ok(suffix) = path.strip_prefix(ancestor) {
            if !suffix.as_os_str().is_empty() {
                if relative.as_os_str().is_empty() {
                    relative.push(".");
                }
                relative.push(suffix);
            }
            return Some(if relative.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                relative
            });
        }
        ancestor = ancestor.parent()?;
        relative.push("..");
    }
}

/// Kpathsea gives a program-qualified search variable precedence over the
/// generic one. Set both so a locked search path remains authoritative when
/// the engine selects a format-specific program name.
pub(super) fn engine_input_environment(
    program: &str,
    search: OsString,
) -> [(OsString, OsString); 4] {
    [
        (OsString::from("TEXINPUTS"), search.clone()),
        (
            OsString::from(format!("TEXINPUTS_{program}")),
            search.clone(),
        ),
        (OsString::from("LUAINPUTS"), search.clone()),
        (OsString::from(format!("LUAINPUTS_{program}")), search),
    ]
}

/// A managed run sees only the managed binaries. Adding the host's `/usr/bin`
/// here would reintroduce exactly the ambient dependency the managed provider
/// exists to remove.
pub(super) fn managed_path(binary_dir: &Path) -> OsString {
    binary_dir.as_os_str().to_os_string()
}

/// Shell escape is an explicit reproducibility opt-out, so in that mode the
/// project may reach host commands after the pinned managed binaries.
pub(super) fn shell_escape_path(binary_dir: &Path) -> OsString {
    let paths = std::iter::once(binary_dir.to_path_buf()).chain(
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>()),
    );
    std::env::join_paths(paths).unwrap_or_else(|_| managed_path(binary_dir))
}

/// Pins every clock the engine can observe: `SOURCE_DATE_EPOCH` fixes the PDF
/// `/CreationDate`, `/ModDate`, and `/ID`, while `FORCE_SOURCE_DATE` also fixes
/// `\today` and friends so the typeset content matches.
pub(super) fn source_date_environment(epoch: &str) -> [(OsString, OsString); 2] {
    [
        (
            OsString::from("SOURCE_DATE_EPOCH"),
            OsString::from(epoch.to_string()),
        ),
        (OsString::from("FORCE_SOURCE_DATE"), OsString::from("1")),
    ]
}

fn is_engine_search_variable(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with("TEX")
            || name.starts_with("LUA")
            || name.starts_with("FONTCONFIG")
            || name == "OSFONTDIR"
    })
}

fn is_standalone_perl_variable(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with("PAR_") || name.starts_with("PERL"))
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    #[cfg(windows)]
    use crate::build::process::engine_path_from;
    use crate::build::process::{
        engine_input_environment, engine_path, is_engine_search_variable,
        is_standalone_perl_variable, managed_path, shell_escape_path,
    };

    #[test]
    fn managed_path_exposes_no_host_binaries() {
        let value = managed_path(Path::new("/data/texe/toolchains/runtime/bin/x86_64-linux"));
        let value = value.to_string_lossy();
        assert_eq!(value, "/data/texe/toolchains/runtime/bin/x86_64-linux");
        assert!(!value.contains("/usr/bin"));
    }

    #[test]
    fn shell_escape_path_places_the_pinned_tools_first() {
        let value = shell_escape_path(Path::new("/managed/bin"));
        assert_eq!(
            std::env::split_paths(&value).next().as_deref(),
            Some(Path::new("/managed/bin"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn kpathsea_search_paths_use_windows_separators_and_portable_slashes() {
        assert_eq!(
            crate::build::process::search_path_from(
                &[
                    Path::new("."),
                    Path::new(r"C:\project\.texe\build\output"),
                    Path::new(r"C:\project\.texe\texmf"),
                    Path::new(r"D:\runtime\texmf-dist"),
                ],
                Path::new(r"C:\project"),
            ),
            ".;./.texe/build/output//;./.texe/texmf//;D:/runtime/texmf-dist//"
        );
    }

    #[cfg(windows)]
    #[test]
    fn kpathsea_paths_can_escape_to_project_siblings_without_repeating_unicode_ancestors() {
        let project = Path::new(r"\\?\C:\Users\Ada\Research Δ Results");
        let output = project.join(r".texe\build\output\chapters");
        assert_eq!(
            engine_path_from(&project.join(".texe/texmf"), &output),
            "../../../texmf"
        );
        assert_eq!(engine_path_from(project, &output), "../../../..");
    }

    #[test]
    fn managed_engine_clears_inherited_tex_lua_and_font_searches() {
        for name in [
            "TEXINPUTS",
            "TEXMFHOME",
            "LUAINPUTS",
            "LUA_PATH",
            "LUA_CPATH",
            "FONTCONFIG_FILE",
            "FONTCONFIG_PATH",
            "OSFONTDIR",
        ] {
            assert!(is_engine_search_variable(OsStr::new(name)), "{name}");
        }
        assert!(!is_engine_search_variable(OsStr::new("PATH")));
        assert!(!is_engine_search_variable(OsStr::new("SOURCE_DATE_EPOCH")));
    }

    #[test]
    fn bundled_biber_clears_inherited_par_and_perl_loader_state() {
        for name in [
            "PAR_INITIALIZED",
            "PAR_TEMP",
            "PAR_GLOBAL_TMPDIR",
            "PERL5LIB",
            "PERLLIB",
            "PERL5OPT",
            "PERLIO",
        ] {
            assert!(is_standalone_perl_variable(OsStr::new(name)), "{name}");
        }
        assert!(!is_standalone_perl_variable(OsStr::new("PATH")));
        assert!(!is_standalone_perl_variable(OsStr::new("BIBINPUTS")));
    }

    #[test]
    fn engine_inputs_override_generic_and_program_specific_searches() {
        assert_eq!(
            engine_input_environment("pdflatex", OsString::from("/locked//")),
            [
                ("TEXINPUTS", "/locked//"),
                ("TEXINPUTS_pdflatex", "/locked//"),
                ("LUAINPUTS", "/locked//"),
                ("LUAINPUTS_pdflatex", "/locked//"),
            ]
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        );
    }

    #[test]
    fn engine_paths_use_the_platforms_kpathsea_spelling() {
        let path = engine_path(Path::new(r"C:\project\texmf\pdflatex.ini"));
        if cfg!(windows) {
            assert_eq!(path, "C:/project/texmf/pdflatex.ini");
            assert_eq!(
                engine_path(Path::new(r"\\?\C:\project\texmf\pdflatex.ini")),
                "C:/project/texmf/pdflatex.ini"
            );
            assert_eq!(
                engine_path(Path::new(r"\\?\UNC\server\share\texmf\pdflatex.ini")),
                "//server/share/texmf/pdflatex.ini"
            );
        } else {
            assert_eq!(path, r"C:\project\texmf\pdflatex.ini");
        }
    }
}
