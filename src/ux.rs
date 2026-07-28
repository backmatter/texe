//! Stable command-line presentation contracts.
//!
//! Workflow code receives terminal capabilities and presentation preferences
//! as values. This keeps terminal detection out of decisions and gives tests a
//! deterministic seam without pretending that redirected input is a terminal.

use std::fmt;
use std::io::IsTerminal as _;

use serde::Serialize;

use crate::TexeError;

pub(crate) const ERROR_SCHEMA: &str = "texe.error/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalCapabilities {
    pub(crate) stdin: bool,
    pub(crate) stdout: bool,
    pub(crate) stderr: bool,
}

impl TerminalCapabilities {
    pub(crate) fn detect() -> Self {
        Self {
            stdin: std::io::stdin().is_terminal(),
            stdout: std::io::stdout().is_terminal(),
            stderr: std::io::stderr().is_terminal(),
        }
    }

    pub(crate) const fn can_prompt(self) -> bool {
        self.stdin && self.stderr
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Presentation {
    pub(crate) json: bool,
    pub(crate) quiet: bool,
    pub(crate) verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ErrorCategory {
    Usage,
    Project,
    Tool,
    Network,
    Build,
    System,
    Cancelled,
}

impl ErrorCategory {
    pub(crate) const fn exit_code(self) -> u8 {
        match self {
            Self::Usage => 2,
            Self::Project => 3,
            Self::Tool => 4,
            Self::Network => 5,
            Self::Build => 6,
            Self::System => 7,
            Self::Cancelled => 130,
        }
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Usage => "usage",
            Self::Project => "project",
            Self::Tool => "tool",
            Self::Network => "network",
            Self::Build => "build",
            Self::System => "system",
            Self::Cancelled => "cancelled",
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    schema: &'static str,
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    category: ErrorCategory,
    code: u8,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic: Option<crate::diagnostics::Diagnostic>,
}

impl ErrorEnvelope {
    pub(crate) fn from_error(error: &TexeError) -> Self {
        let category = error.category();
        Self {
            schema: ERROR_SCHEMA,
            error: ErrorBody {
                category,
                code: category.exit_code(),
                message: error.to_string(),
                action: error.action().map(str::to_string),
                diagnostic: match error {
                    TexeError::Diagnostic(diagnostic) => Some((**diagnostic).clone()),
                    _ => None,
                },
            },
        }
    }
}

pub(crate) fn prompt<T>(result: std::io::Result<T>) -> Result<T, TexeError> {
    result.map_err(|error| {
        if error.kind() == std::io::ErrorKind::Interrupted {
            TexeError::Prompt("cancelled".to_string())
        } else {
            TexeError::Prompt(error.to_string())
        }
    })
}

pub(crate) fn present_error(error: &TexeError, presentation: Presentation) {
    if presentation.json {
        let envelope = ErrorEnvelope::from_error(error);
        match serde_json::to_string_pretty(&envelope) {
            Ok(json) => println!("{json}"),
            Err(_) => println!(
                r#"{{"schema":"{ERROR_SCHEMA}","error":{{"category":"system","code":7,"message":"could not serialize error"}}}}"#
            ),
        }
        return;
    }
    if error.category() == ErrorCategory::Cancelled {
        if !std::io::stderr().is_terminal() {
            eprintln!("Setup cancelled. Nothing was changed.");
        }
        return;
    }
    eprintln!("{}", human_error(error, presentation.verbose));
}

pub(crate) fn human_error(error: &TexeError, verbose: bool) -> String {
    if let TexeError::Diagnostic(diagnostic) = error {
        let mut output = format!("{diagnostic}\nnext: {}", diagnostic.action);
        if verbose && !diagnostic.technical_details().is_empty() {
            output.push_str("\n\nFull engine details:\n");
            output.push_str(diagnostic.technical_details());
        }
        return output;
    }
    let mut output = format!("texe: {}", friendly_message(error));
    if let Some(action) = error.action() {
        output.push_str("\nnext: ");
        output.push_str(action);
    }
    output
}

pub(crate) fn human_watch_error(error: &TexeError, verbose: bool) -> String {
    let TexeError::Diagnostic(diagnostic) = error else {
        return human_error(error, verbose);
    };
    let mut output = format!("{}\nnext: {}", diagnostic.for_watch(), diagnostic.action);
    if verbose && !diagnostic.technical_details().is_empty() {
        output.push_str("\n\nFull engine details:\n");
        output.push_str(diagnostic.technical_details());
    }
    output
}

fn friendly_message(error: &TexeError) -> String {
    match error {
        TexeError::Usage(message) => message
            .strip_prefix("error: ")
            .unwrap_or(message)
            .trim_end()
            .to_string(),
        TexeError::Manifest(message) => message
            .strip_prefix("could not find texe.toml from ")
            .map_or_else(
                || error.to_string(),
                |path| format!("no texe project was found from {path}"),
            ),
        _ => error.to_string(),
    }
}

#[cfg(test)]
pub(crate) fn normalize_transcript(transcript: &str, roots: &[&std::path::Path]) -> String {
    let mut normalized = transcript.replace("\r\n", "\n").replace('\r', "\n");
    for root in roots {
        normalized = normalized.replace(&root.display().to_string(), "<PROJECT>");
    }
    normalized
        .lines()
        .map(|line| {
            let line = strip_ansi(line);
            normalize_duration(&line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn strip_ansi(line: &str) -> String {
    let mut result = String::new();
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' && characters.peek() == Some(&'[') {
            characters.next();
            for code in characters.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(character);
        }
    }
    result
}

#[cfg(test)]
fn normalize_duration(line: &str) -> String {
    line.split_whitespace()
        .map(|word| {
            let trimmed = word
                .trim_matches(|character: char| matches!(character, ',' | ')' | '(' | '·' | ':'));
            if trimmed.ends_with("ms")
                && trimmed[..trimmed.len().saturating_sub(2)]
                    .chars()
                    .all(|character| character.is_ascii_digit())
                || trimmed.ends_with('s')
                    && trimmed[..trimmed.len().saturating_sub(1)]
                        .chars()
                        .all(|character| character.is_ascii_digit())
            {
                word.replace(trimmed, "<TIME>")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use crate::TexeError;
    use crate::ux::{ErrorCategory, human_error, normalize_transcript};
    use std::path::Path;

    #[test]
    fn categories_have_distinct_stable_exit_codes() {
        let categories = [
            ErrorCategory::Usage,
            ErrorCategory::Project,
            ErrorCategory::Tool,
            ErrorCategory::Network,
            ErrorCategory::Build,
            ErrorCategory::System,
            ErrorCategory::Cancelled,
        ];
        let mut codes = categories
            .map(ErrorCategory::exit_code)
            .into_iter()
            .collect::<Vec<_>>();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), categories.len());
    }

    #[test]
    fn transcript_normalizer_handles_paths_colors_endings_and_time() {
        let transcript = "\u{1b}[31mfailed\u{1b}[0m C:\\paper\\main.tex in 12s\r\n";
        assert_eq!(
            normalize_transcript(transcript, &[Path::new("C:\\paper")]),
            "failed <PROJECT>\\main.tex in <TIME>"
        );
    }

    #[test]
    fn human_errors_use_plain_headlines_while_categories_stay_structured() {
        let missing = TexeError::Manifest("could not find texe.toml from /work/paper".to_string());
        assert_eq!(
            human_error(&missing, false),
            "texe: no texe project was found from /work/paper\nnext: run `texe` to create a paper, or `texe init` in an existing LaTeX folder"
        );

        let usage = TexeError::Usage(
            "error: unrecognized subcommand 'oops'\n\nUsage: texe [COMMAND]".to_string(),
        );
        assert!(
            human_error(&usage, false)
                .starts_with("texe: unrecognized subcommand 'oops'\n\nUsage:")
        );
    }
}
