use std::fs;
use std::path::Path;

use crate::TexeError;
use crate::build::EngineRun;
use crate::toolchain::ResolvedToolchain;

pub(super) fn engine_failure(
    project_root: &Path,
    entry: &Path,
    toolchain: &ResolvedToolchain,
    run: &EngineRun,
) -> TexeError {
    let log = fs::read_to_string(&run.log_path).unwrap_or_default();
    let process_stderr = String::from_utf8_lossy(&run.stderr);
    let process_stdout = String::from_utf8_lossy(&run.stdout);
    let _ = toolchain;
    TexeError::Diagnostic(Box::new(crate::diagnostics::from_engine_log(
        project_root,
        entry,
        &run.log_path,
        &log,
        &process_stdout,
        &process_stderr,
    )))
}

pub(super) fn package_error_with_engine_context(
    error: TexeError,
    project_root: &Path,
    entry: &Path,
    toolchain: &ResolvedToolchain,
    run: &EngineRun,
) -> TexeError {
    if run.status.success() {
        error
    } else {
        match engine_failure(project_root, entry, toolchain, run) {
            TexeError::Diagnostic(diagnostic) => TexeError::Diagnostic(diagnostic),
            diagnostic => TexeError::Build(format!("{error}; {diagnostic}")),
        }
    }
}

#[cfg(test)]
pub(super) fn engine_failure_detail(
    log: &str,
    process_stdout: &str,
    process_stderr: &str,
) -> String {
    let excerpt = engine_log_excerpt(log);
    if excerpt.is_empty() {
        return format!("{}\n{}", process_stdout.trim(), process_stderr.trim());
    }
    if process_stderr.trim().is_empty() {
        return excerpt;
    }
    let stderr = process_stderr
        .lines()
        .rev()
        .take(LOG_EXCERPT_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!("{excerpt}\n\nengine process stderr:\n{}", stderr.trim())
}

/// The number of log lines an engine failure is allowed to print.
#[cfg(test)]
pub(super) const LOG_EXCERPT_LINES: usize = 30;

/// Pick the part of an engine log a reader needs.
///
/// A TeX log ends with a page-and-a-half of memory statistics — string counts,
/// font words, stack positions — that describe a run nobody is debugging. Under
/// `-halt-on-error` the line that matters is the last `!` diagnostic and the
/// source context under it, which the raw tail pushes off the top of the
/// message. Drop the statistics, keep the diagnostics that follow them, and
/// show the last error with its context.
#[cfg(test)]
pub(super) fn engine_log_excerpt(log: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_statistics = false;
    for line in log.lines() {
        if line.starts_with("Here is how much of TeX's memory you used") {
            in_statistics = true;
            continue;
        }
        // The fatal-error verdict and any output-file note are printed after
        // the statistics block and are worth keeping.
        if in_statistics {
            if !(line.starts_with('!') || line.starts_with("Output written")) {
                continue;
            }
            in_statistics = false;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    // `!  ==> Fatal error occurred` is the verdict, not the cause; anchoring on
    // it would hide the diagnostic that produced it.
    let anchor = lines
        .iter()
        .rposition(|line| line.starts_with('!') && !line.starts_with("!  ==>"));
    let excerpt = match anchor {
        // Show the failing diagnostic and the source context printed under it.
        Some(error) => &lines[error..],
        None => &lines[lines.len().saturating_sub(LOG_EXCERPT_LINES)..],
    };
    excerpt
        .iter()
        .take(LOG_EXCERPT_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}
