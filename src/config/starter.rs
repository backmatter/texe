use crate::config::discovery::validate_engine;
use crate::config::validation::validate_relative_path;
use crate::config::{
    InitOutcome, MANAGED_ENGINES, MANIFEST_NAME, PROJECT_SCHEMA, StarterDocument, StarterTemplate,
};

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::TexeError;

/// Initialize a manifest and, when absent, a starter document.
///
/// The manifest is never overwritten.
///
/// # Errors
///
/// Returns an error for invalid paths, an existing manifest, or a filesystem
/// failure.
pub fn init_project(
    directory: &Path,
    entry: &Path,
    engine: &str,
) -> Result<(PathBuf, bool), TexeError> {
    let outcome = init_project_with_starter(directory, entry, engine, &StarterDocument::default())?;
    let created_entry = outcome.created_files.iter().any(|path| path == entry);
    Ok((outcome.manifest, created_entry))
}

/// Initialize a manifest and render the selected starter when the entry is
/// absent.
///
/// Existing entry files are never modified. The manifest and every starter
/// file created by a failed attempt are removed before the error is returned.
///
/// # Errors
///
/// Returns an error for invalid paths, an existing manifest, a starter-file
/// collision, or a filesystem failure.
pub fn init_project_with_starter(
    directory: &Path,
    entry: &Path,
    engine: &str,
    starter: &StarterDocument,
) -> Result<InitOutcome, TexeError> {
    validate_relative_path("entry", entry)?;
    let engine = validate_engine(engine)?;
    fs::create_dir_all(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let manifest_path = directory.join(MANIFEST_NAME);
    let entry_value = toml::Value::String(entry.to_string_lossy().replace('\\', "/")).to_string();
    let uses_managed_recipe = MANAGED_ENGINES.contains(&engine.as_str());
    let engine_value = toml::Value::String(engine).to_string();
    let provider = if uses_managed_recipe {
        String::new()
    } else {
        "provider = \"system\"\n".to_string()
    };
    let manifest = format!(
        r#"schema = "{PROJECT_SCHEMA}"

[project]
entry = {entry_value}

[toolchain]
{provider}engine = {engine_value}
"#
    );
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&manifest_path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                TexeError::Manifest(format!(
                    "{} already exists; refusing to overwrite it",
                    manifest_path.display()
                ))
            } else {
                TexeError::Io {
                    path: manifest_path.clone(),
                    source,
                }
            }
        })?;
    if let Err(source) = file.write_all(manifest.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(&manifest_path);
        return Err(TexeError::Io {
            path: manifest_path,
            source,
        });
    }

    let entry_path = directory.join(entry);
    if entry_path.exists() {
        return Ok(InitOutcome {
            manifest: manifest_path,
            created_files: Vec::new(),
        });
    }

    let created_files = match create_starter_files(directory, entry, starter) {
        Ok(created) => created,
        Err(error) => {
            let _ = fs::remove_file(&manifest_path);
            return Err(error);
        }
    };

    Ok(InitOutcome {
        manifest: manifest_path,
        created_files,
    })
}

fn create_starter_files(
    directory: &Path,
    entry: &Path,
    starter: &StarterDocument,
) -> Result<Vec<PathBuf>, TexeError> {
    let rendered = render_starter(entry, starter);
    if let Some(conflict) = rendered
        .iter()
        .map(|(path, _)| directory.join(path))
        .find(|path| path.exists())
    {
        return Err(TexeError::Manifest(format!(
            "{} already exists; refusing to overwrite it",
            conflict.display()
        )));
    }

    let mut created_files = Vec::new();
    let result = (|| {
        for (relative, contents) in &rendered {
            let path = directory.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| TexeError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let mut output = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|source| TexeError::Io {
                    path: path.clone(),
                    source,
                })?;
            created_files.push(relative.clone());
            output
                .write_all(contents.as_bytes())
                .map_err(|source| TexeError::Io { path, source })?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        for relative in &created_files {
            let _ = fs::remove_file(directory.join(relative));
        }
        return Err(error);
    }
    Ok(created_files)
}

fn render_starter(entry: &Path, starter: &StarterDocument) -> Vec<(PathBuf, String)> {
    let title = latex_text(if starter.title.trim().is_empty() {
        "Untitled Paper"
    } else {
        &starter.title
    });
    let author = latex_text(&starter.author);
    let mut source = format!(
        "\\documentclass{{article}}\n\n\\title{{{title}}}\n\\author{{{author}}}\n\\date{{\\today}}\n"
    );
    let mut files = Vec::new();
    match starter.template {
        StarterTemplate::Basic => {
            source.push_str(BASIC_PAPER_BODY);
            files.push((
                PathBuf::from("references.bib"),
                BASIC_BIBLIOGRAPHY.to_string(),
            ));
        }
        StarterTemplate::Empty => source.push_str(EMPTY_PAPER_BODY),
    }
    files.insert(0, (entry.to_path_buf(), source));
    files
}

fn latex_text(value: &str) -> String {
    let mut escaped = String::new();
    let mut previous_was_space = true;
    for character in value.chars() {
        if character.is_whitespace() {
            if !previous_was_space {
                escaped.push(' ');
                previous_was_space = true;
            }
            continue;
        }
        previous_was_space = false;
        match character {
            '\\' => escaped.push_str("\\textbackslash{}"),
            '{' => escaped.push_str("\\{"),
            '}' => escaped.push_str("\\}"),
            '%' => escaped.push_str("\\%"),
            '$' => escaped.push_str("\\$"),
            '#' => escaped.push_str("\\#"),
            '_' => escaped.push_str("\\_"),
            '&' => escaped.push_str("\\&"),
            '~' => escaped.push_str("\\textasciitilde{}"),
            '^' => escaped.push_str("\\textasciicircum{}"),
            _ => escaped.push(character),
        }
    }
    if previous_was_space {
        escaped.pop();
    }
    escaped
}

const EMPTY_PAPER_BODY: &str = r"

\begin{document}
\maketitle

\end{document}
";

const BASIC_PAPER_BODY: &str = r"

\begin{document}
\maketitle

\begin{abstract}
% Summarize the question, method, and main result.
Write the abstract here.
\end{abstract}

\section{Introduction}
% Explain the research question and why it matters.
Introduce the paper and cite relevant work, for example \cite{example}.

\section{Methods}
% Describe the data, materials, and analysis.
A numbered equation can be referenced later:
\begin{equation}
  E = mc^2
  \label{eq:example}
\end{equation}

\section{Results}
Equation~\ref{eq:example} is an example cross-reference.

\begin{table}[ht]
  \centering
  \begin{tabular}{lr}
    Measurement & Value \\
    Example     & 1.00
  \end{tabular}
  \caption{Replace this example with a result.}
  \label{tab:example}
\end{table}

\begin{figure}[ht]
  \centering
  \fbox{\rule{0pt}{35mm}\rule{0.7\linewidth}{0pt}}
  \caption{Replace this placeholder with a figure.}
  \label{fig:example}
\end{figure}

\section{Discussion}
% Interpret the results, limitations, and implications.
Discuss the findings here.

\section{Conclusion}
% State the main conclusion without repeating the abstract.
Write the conclusion here.

\bibliographystyle{plain}
\bibliography{references}

\end{document}
";

const BASIC_BIBLIOGRAPHY: &str = r"@article{example,
  author  = {Ada Researcher},
  title   = {Replace This Example Reference},
  journal = {Journal of Examples},
  year    = {2026},
}
";
