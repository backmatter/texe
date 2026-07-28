use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::TexeError;
use crate::build::BuildWarning;

pub(super) fn collect_warnings(log_path: &Path) -> Vec<BuildWarning> {
    let Ok(log) = fs::read_to_string(log_path) else {
        return Vec::new();
    };
    let mut warnings = BTreeSet::new();
    for line in log.lines().map(str::trim) {
        let lower = line.to_ascii_lowercase();
        let kind = if lower.contains("citation") && lower.contains("undefined") {
            Some("unresolved-citation")
        } else if lower.contains("reference") && lower.contains("undefined") {
            Some("unresolved-reference")
        } else if lower.starts_with("missing character:") {
            Some("missing-character")
        } else if lower.starts_with("overfull") || lower.starts_with("underfull") {
            Some("layout")
        } else if lower.contains("warning:") {
            Some("latex")
        } else {
            None
        };
        if let Some(kind) = kind {
            warnings.insert((warning_rank(kind), kind.to_string(), line.to_string()));
        }
    }
    warnings
        .into_iter()
        .map(|(_, kind, message)| BuildWarning { kind, message })
        .collect()
}

pub(super) const fn warning_rank(kind: &str) -> u8 {
    match kind.as_bytes() {
        b"unresolved-citation" => 0,
        b"unresolved-reference" => 1,
        b"missing-character" => 2,
        b"latex" => 3,
        _ => 4,
    }
}

pub(super) fn remove_if_file(path: &Path) -> Result<(), TexeError> {
    if path.is_file() {
        fs::remove_file(path).map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

pub(super) trait ErrorContext {
    fn context(self, message: &str) -> Self;
}

impl ErrorContext for TexeError {
    fn context(self, message: &str) -> Self {
        TexeError::Build(format!("{message}: {self}"))
    }
}
