//! Conservative static TeX command discovery for adoption, never execution.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Commands with their first braced argument, excluding comments and verbatim text.
pub(crate) fn commands(source: &str) -> Vec<(String, String)> {
    let source = super::uncommented_source(source);
    let mut rest = source.as_str();
    let mut result = Vec::new();
    while let Some(index) = rest.find('\\') {
        rest = &rest[index + 1..];
        let length = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        if length == 0 {
            rest = rest
                .get(rest.chars().next().map_or(0, char::len_utf8)..)
                .unwrap_or("");
            continue;
        }
        let name = &rest[..length];
        rest = &rest[length..];
        let name = if name == "write" && rest.trim_start().starts_with("18") {
            "write18"
        } else {
            name
        };
        if name == "verb" {
            rest = rest.strip_prefix('*').unwrap_or(rest);
            if let Some(delimiter) = rest.chars().next() {
                rest = &rest[delimiter.len_utf8()..];
                rest = rest
                    .find(delimiter)
                    .map_or("", |i| &rest[i + delimiter.len_utf8()..]);
            }
            continue;
        }
        rest = rest.trim_start();
        if rest.starts_with('[') {
            rest = rest.find(']').map_or("", |i| rest[i + 1..].trim_start());
        }
        let mut argument = String::new();
        if let Some(tail) = rest.strip_prefix('{')
            && let Some(end) = tail.find('}')
        {
            argument = tail[..end].trim().to_string();
            rest = &tail[end + 1..];
        }
        if name == "begin"
            && matches!(
                argument.as_str(),
                "verbatim" | "Verbatim" | "lstlisting" | "minted"
            )
        {
            let end = format!("\\end{{{argument}}}");
            rest = rest.find(&end).map_or("", |i| &rest[i + end.len()..]);
        }
        result.push((name.to_string(), argument));
    }
    result
}

/// Follow literal local includes and local package/class declarations only.
pub(crate) fn project_commands(root: &Path, entry: &Path) -> Vec<(String, String)> {
    let mut pending = vec![root.join(entry)];
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    let mut remaining = 8 * 1024 * 1024;
    while let Some(path) = pending.pop() {
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        if !path.starts_with(root) || !seen.insert(path.clone()) || seen.len() > 256 {
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > remaining {
            continue;
        }
        remaining -= metadata.len();
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let found = commands(&source);
        for (name, argument) in &found {
            let extension = match name.as_str() {
                "input" | "include" | "subfile" => "tex",
                "usepackage" | "RequirePackage" => "sty",
                "documentclass" | "LoadClass" => "cls",
                _ => continue,
            };
            for file in argument.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let mut file = PathBuf::from(file);
                if file.extension().is_none() {
                    file.set_extension(extension);
                }
                pending.push(root.join(&file));
                if let Some(parent) = path.parent() {
                    pending.push(parent.join(file));
                }
            }
        }
        result.extend(found);
    }
    result
}

pub(crate) fn has_package(commands: &[(String, String)], package: &str) -> bool {
    commands.iter().any(|(name, argument)| {
        matches!(name.as_str(), "usepackage" | "RequirePackage")
            && argument.split(',').any(|value| value.trim() == package)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declarations_ignore_prose_comments_and_verbatim() {
        let found = commands(
            r"The newly minted coin. % \usepackage{minted}
\verb|\usepackage{minted}|
\begin{verbatim}\usepackage{minted}\end{verbatim}
\usepackage [option] {fontspec, unicode-math}",
        );
        assert!(!has_package(&found, "minted"));
        assert!(has_package(&found, "fontspec"));
    }
    #[test]
    fn includes_and_local_classes_are_scanned_without_leaving_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(
            root.join("main.tex"),
            r"\documentclass{custom}\input{chapter}",
        )
        .unwrap();
        fs::write(root.join("custom.cls"), r"\RequirePackage{fontspec}").unwrap();
        fs::write(root.join("chapter.tex"), r"\input{main}\usepackage{minted}").unwrap();
        let found = project_commands(&root, Path::new("main.tex"));
        assert!(has_package(&found, "fontspec"));
        assert!(has_package(&found, "minted"));
        assert!(found.len() < 10);
    }
}
