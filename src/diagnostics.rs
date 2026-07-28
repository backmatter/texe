//! Deterministic, local translation of TeX diagnostics.
//!
//! This catalog intentionally prefers a focused original diagnostic over an
//! uncertain rewrite. It never reads outside the project and never performs a
//! network request.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub(crate) schema: &'static str,
    pub(crate) family: &'static str,
    pub(crate) message: String,
    pub(crate) explanation: String,
    pub(crate) action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<SourceLocation>,
    pub(crate) original: String,
    pub(crate) log: PathBuf,
    pub(crate) previous_artifact: ArtifactStatus,
    #[serde(skip)]
    technical_details: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceLocation {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArtifactStatus {
    pub(crate) path: PathBuf,
    pub(crate) retained: bool,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_human(formatter, true)
    }
}

pub(crate) struct WatchDiagnostic<'a>(&'a Diagnostic);

impl fmt::Display for WatchDiagnostic<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt_human(formatter, false)
    }
}

impl Diagnostic {
    pub(crate) fn for_watch(&self) -> WatchDiagnostic<'_> {
        WatchDiagnostic(self)
    }

    fn fmt_human(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        include_artifact_status: bool,
    ) -> fmt::Result {
        if let Some(location) = &self.location {
            writeln!(
                formatter,
                "Build stopped in {} at line {}",
                location.file.display(),
                location.line
            )?;
        } else {
            writeln!(formatter, "Build stopped")?;
        }
        writeln!(formatter)?;
        writeln!(formatter, "{}", self.message)?;
        writeln!(formatter, "{}", self.explanation)?;
        if let Some(location) = &self.location
            && let Some(source) = &location.source
        {
            writeln!(formatter)?;
            writeln!(formatter, "{:>4} │ {}", location.line, source)?;
            let indentation = source.len() - source.trim_start().len();
            writeln!(
                formatter,
                "     │ {}{}",
                " ".repeat(indentation),
                "^".repeat(source.trim().chars().count().clamp(1, 72))
            )?;
        }
        writeln!(formatter)?;
        if include_artifact_status {
            if self.previous_artifact.retained {
                writeln!(
                    formatter,
                    "Build failed. The previous {} was kept.",
                    self.previous_artifact.path.display()
                )?;
            } else {
                writeln!(formatter, "Build failed. No previous PDF was available.")?;
            }
        }
        write!(
            formatter,
            "Technical details: {} (or run `texe build --verbose`)",
            self.log.display()
        )
    }
}

pub(crate) fn from_engine_log(
    project_root: &Path,
    entry: &Path,
    log_path: &Path,
    log: &str,
    process_stdout: &str,
    process_stderr: &str,
) -> Diagnostic {
    let original = primary_original(log, process_stdout, process_stderr);
    let evidence = format!("{original}\n{log}");
    let (family, message, explanation, action) = classify(&evidence, &original);
    let location = locate(log, entry).map(|(file, line)| {
        let safe_file = confined_file(project_root, &file).unwrap_or_else(|| entry.to_path_buf());
        let source = fs::read_to_string(project_root.join(&safe_file))
            .ok()
            .and_then(|contents| {
                contents
                    .lines()
                    .nth(line.saturating_sub(1))
                    .map(str::to_string)
            });
        SourceLocation {
            file: safe_file,
            line,
            source,
        }
    });
    let artifact = project_root.join(
        entry
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("main.tex")),
    );
    let artifact = artifact.with_extension("pdf");
    Diagnostic {
        schema: "texe.diagnostic/v1",
        family,
        message,
        explanation,
        action,
        location,
        original,
        log: relative_or_original(project_root, log_path),
        previous_artifact: ArtifactStatus {
            path: relative_or_original(project_root, &artifact),
            retained: artifact.is_file(),
        },
        technical_details: [log, process_stdout, process_stderr]
            .into_iter()
            .filter(|detail| !detail.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

impl Diagnostic {
    pub(crate) fn technical_details(&self) -> &str {
        &self.technical_details
    }
}

type Classification = (&'static str, String, String, String);

fn classify(evidence: &str, original: &str) -> Classification {
    let lower = evidence.to_ascii_lowercase();
    classify_syntax(evidence, &lower)
        .or_else(|| classify_dependencies(&lower))
        .or_else(|| classify_environment(&lower))
        .unwrap_or_else(|| {
            (
                "latex-error",
                "LaTeX reported an error.".to_string(),
                original.to_string(),
                "check the focused engine message and source line, then build again".to_string(),
            )
        })
}

fn classify_syntax(evidence: &str, lower: &str) -> Option<Classification> {
    if lower.contains("undefined control sequence") {
        return Some((
            "undefined-command",
            "LaTeX does not recognize a command near this line.".to_string(),
            command_hint(evidence).map_or_else(
                || {
                    "The command may be misspelled or provided by a package that is not loaded."
                        .to_string()
                },
                |command| {
                    format!("Check the spelling of `{command}` and the package that defines it.")
                },
            ),
            "correct the command or load its package, then build again".to_string(),
        ));
    }
    if lower.contains("missing }")
        || lower.contains("extra }")
        || lower.contains("runaway argument")
        || lower.contains("ended by \\end")
    {
        return Some((
            "braces-or-environment",
            "LaTeX found unbalanced braces or environments.".to_string(),
            "A `{` needs a matching `}`, and every `\\begin{...}` needs the same `\\end{...}`."
                .to_string(),
            "check the braces and begin/end pair around the highlighted line".to_string(),
        ));
    }
    if lower.contains("missing $ inserted")
        || lower.contains("misplaced alignment tab")
        || lower.contains("extra alignment tab")
        || lower.contains("math mode")
    {
        return Some((
            "math-or-alignment",
            "LaTeX found a math-mode or table-alignment mistake.".to_string(),
            "Math commands need math delimiters, and `&` may only separate columns in an alignment."
                .to_string(),
            "check `$...$`, equation delimiters, and `&` near this line".to_string(),
        ));
    }
    None
}

fn classify_dependencies(lower: &str) -> Option<Classification> {
    if (lower.contains("file `") || lower.contains("file '")) && lower.contains("not found")
        || lower.contains("i can't find file")
    {
        return Some((
            "missing-input",
            "LaTeX could not find a required file.".to_string(),
            "This may be a package, source file, image, font, or bibliography database."
                .to_string(),
            "check the filename and case; add project-owned directories to `[inputs].roots`"
                .to_string(),
        ));
    }
    if lower.contains("citation") && lower.contains("undefined")
        || lower.contains("there were undefined references")
    {
        return Some((
            "unresolved-reference",
            "A citation or cross-reference could not be resolved.".to_string(),
            "Check that the key exists and that the bibliography or label name matches exactly."
                .to_string(),
            "correct the citation/reference key, then build again".to_string(),
        ));
    }
    if lower.contains("unicode character")
        || lower.contains("inputenc error")
        || lower.contains("missing character")
    {
        return Some((
            "unicode-or-character",
            "The selected engine or font cannot typeset a character.".to_string(),
            "pdfLaTeX may need an encoding/package; LuaLaTeX may be a better fit for broad Unicode."
                .to_string(),
            "check the character and font, or select LuaLaTeX for Unicode-heavy documents"
                .to_string(),
        ));
    }
    if lower.contains("option clash for package") {
        return Some((
            "package-option-conflict",
            "The same package was loaded with conflicting options.".to_string(),
            "Pass its options once, before the package is first loaded.".to_string(),
            "combine or move the package options, then build again".to_string(),
        ));
    }
    None
}

fn classify_environment(lower: &str) -> Option<Classification> {
    if lower.contains("shell escape") || lower.contains("shell-escape") || lower.contains("write18")
    {
        return Some((
            "shell-escape-blocked",
            "This document asks LaTeX to run an external command.".to_string(),
            "texe keeps shell escape off because external commands are not pinned or isolated."
                .to_string(),
            "review the document; enable `toolchain.shell_escape` only if you trust the command"
                .to_string(),
        ));
    }
    if lower.contains("permission denied") {
        return Some((
            "permission",
            "The build could not read or write a required path.".to_string(),
            "The named file or directory is not writable by the current user.".to_string(),
            "check ownership and permissions on the named path".to_string(),
        ));
    }
    if lower.contains("no space left") || lower.contains("disk full") {
        return Some((
            "disk-space",
            "The computer ran out of storage during the build.".to_string(),
            "The previous PDF and verified cache entries were not replaced.".to_string(),
            "free storage, inspect it with `texe storage`, then build again".to_string(),
        ));
    }
    None
}

fn command_hint(evidence: &str) -> Option<String> {
    let line = evidence
        .lines()
        .find(|line| line.trim_start().starts_with("l."))?;
    let command_start = line.find('\\')?;
    let command = line[command_start..]
        .split(|character: char| {
            !(character.is_ascii_alphabetic() || matches!(character, '\\' | '@'))
        })
        .next()?;
    (command.len() > 1).then(|| command.to_string())
}

fn locate(log: &str, entry: &Path) -> Option<(PathBuf, usize)> {
    for line in log.lines() {
        if let Some(location) = file_line_location(line) {
            return Some(location);
        }
    }
    for line in log.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("l.") else {
            continue;
        };
        let digits = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if let Ok(number) = digits.parse() {
            return Some((entry.to_path_buf(), number));
        }
    }
    None
}

fn file_line_location(line: &str) -> Option<(PathBuf, usize)> {
    for (index, _) in line.match_indices(':') {
        let after = &line[index + 1..];
        let digits = after
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if digits.is_empty() || !after[digits.len()..].starts_with(':') {
            continue;
        }
        let file = line[..index].trim().trim_start_matches("./");
        if file.is_empty() {
            continue;
        }
        return digits
            .parse()
            .ok()
            .map(|number| (PathBuf::from(file), number));
    }
    None
}

fn primary_original(log: &str, stdout: &str, stderr: &str) -> String {
    if let Some(error) = log
        .lines()
        .find(|line| line.starts_with('!') && !line.starts_with("!  ==>"))
    {
        return error.trim_start_matches('!').trim().to_string();
    }
    for text in [stderr, stdout, log] {
        if let Some(line) = text.lines().find(|line| !line.trim().is_empty()) {
            return line.trim().to_string();
        }
    }
    "The engine stopped without a detailed message.".to_string()
}

fn confined_file(project_root: &Path, file: &Path) -> Option<PathBuf> {
    let relative = if file.is_absolute() {
        file.strip_prefix(project_root).ok()?
    } else {
        file
    };
    if relative
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        Some(relative.to_path_buf())
    } else {
        None
    }
}

fn relative_or_original(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::diagnostics::{Diagnostic, from_engine_log};

    fn diagnose(log: &str, source: &str) -> Diagnostic {
        let directory = tempfile::tempdir().expect("project");
        fs::write(directory.path().join("main.tex"), source).expect("source");
        let log_path = directory.path().join(".texe/build/output/main.log");
        from_engine_log(
            directory.path(),
            Path::new("main.tex"),
            &log_path,
            log,
            "",
            "",
        )
    }

    #[test]
    fn undefined_command_has_a_source_location_and_local_explanation() {
        let diagnostic = diagnose(
            "! Undefined control sequence.\nl.2 \\\\includegrphics{result.pdf}",
            "first\n\\includegrphics{result.pdf}\n",
        );
        assert_eq!(diagnostic.family, "undefined-command");
        assert_eq!(diagnostic.location.expect("location").line, 2);
        assert!(diagnostic.explanation.contains("\\includegrphics"));
    }

    #[test]
    fn a_nearby_unrelated_error_is_not_called_an_undefined_command() {
        let diagnostic = diagnose("! Missing $ inserted.\nl.4 value_1", "a\nb\nc\nvalue_1\n");
        assert_eq!(diagnostic.family, "math-or-alignment");
    }

    #[test]
    fn file_line_errors_support_nested_sources() {
        let diagnostic = diagnose(
            "./chapters/results.tex:42: LaTeX Error: Missing } inserted.",
            "",
        );
        let location = diagnostic.location.expect("location");
        assert_eq!(location.file, Path::new("chapters/results.tex"));
        assert_eq!(location.line, 42);
    }

    #[test]
    fn unknown_errors_preserve_the_original_message() {
        let diagnostic = diagnose("! An unusual engine failure.\nl.1 text", "text\n");
        assert_eq!(diagnostic.family, "latex-error");
        assert!(diagnostic.explanation.contains("unusual engine failure"));
    }

    #[test]
    fn every_catalog_family_has_positive_and_near_miss_coverage() {
        let cases = [
            (
                "undefined-command",
                "! Undefined control sequence.\nl.1 \\\\mispelled",
                "! Missing $ inserted.\nl.1 _",
            ),
            (
                "braces-or-environment",
                "! Missing } inserted.\nl.1 {text",
                "! Missing $ inserted.\nl.1 _",
            ),
            (
                "math-or-alignment",
                "! Misplaced alignment tab character &.\nl.1 A & B",
                "! Missing } inserted.\nl.1 {text",
            ),
            (
                "missing-input",
                "! LaTeX Error: File `chart.pdf' not found.",
                "! LaTeX Error: An unusual failure.",
            ),
            (
                "unresolved-reference",
                "LaTeX Warning: There were undefined references.",
                "LaTeX Warning: Reference output changed.",
            ),
            (
                "unicode-or-character",
                "! LaTeX Error: Unicode character α not set up.",
                "! LaTeX Error: Character count failed.",
            ),
            (
                "package-option-conflict",
                "! LaTeX Error: Option clash for package geometry.",
                "! LaTeX Error: Package geometry failed.",
            ),
            (
                "shell-escape-blocked",
                "! Package minted Error: You must invoke LaTeX with -shell-escape.",
                "! Package minted Error: Python executable not found.",
            ),
            (
                "permission",
                "main.tex:1: Permission denied",
                "main.tex:1: File is read-only metadata",
            ),
            (
                "disk-space",
                "write failed: No space left on device",
                "write failed: invalid data",
            ),
        ];

        for (family, positive, near_miss) in cases {
            assert_eq!(
                diagnose(positive, "source\n").family,
                family,
                "positive case for {family}"
            );
            assert_ne!(
                diagnose(near_miss, "source\n").family,
                family,
                "near miss for {family}"
            );
        }
    }
}
