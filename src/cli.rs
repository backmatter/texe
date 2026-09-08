use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::StarterTemplate;

#[derive(Debug, Parser)]
#[command(
    version,
    long_version = concat!(
        env!("CARGO_PKG_VERSION"),
        "\ntarget: ",
        env!("TEXE_BUILD_TARGET"),
        "\ncommand suite: pqty ",
        env!("TEXE_PQTY_VERSION"),
        " (",
        env!("TEXE_PQTY_CAPABILITIES"),
        ")"
    ),
    about = "Create, build, and work on a LaTeX paper"
)]
pub(crate) struct Cli {
    /// Emit the command result or error as versioned JSON.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Suppress progress and successful human output.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub(crate) quiet: bool,
    /// Include additional paths and reproducibility details.
    #[arg(short, long, global = true, conflicts_with = "quiet")]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TemplateChoice {
    /// A structured scientific article with examples and a bibliography.
    Basic,
    /// The minimum compilable paper with a title and author.
    Empty,
}

impl From<TemplateChoice> for StarterTemplate {
    fn from(value: TemplateChoice) -> Self {
        match value {
            TemplateChoice::Basic => Self::Basic,
            TemplateChoice::Empty => Self::Empty,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Hand a LaTeX Workshop build to the active VS Code companion.
    #[command(hide = true)]
    EditorBuild {
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Initialize a texe project.
    Init {
        /// Directory to initialize.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Root LaTeX source. Detected or prompted for when omitted.
        #[arg(long)]
        entry: Option<PathBuf>,
        /// Engine command. Prompted for on a terminal when omitted.
        #[arg(long)]
        engine: Option<String>,
        /// Paper title used only when a new entry file is created.
        #[arg(long)]
        title: Option<String>,
        /// Author used only when a new entry file is created.
        #[arg(long)]
        author: Option<String>,
        /// Starter used only when a new entry file is created.
        #[arg(long, value_enum)]
        template: Option<TemplateChoice>,
        /// Accept detected/default choices without prompting.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Initialize Git and add only texe-derived outputs to .gitignore.
        #[arg(long)]
        git: bool,
        /// Configure VS Code, install missing LaTeX extensions, and open the project.
        #[arg(long)]
        vscode: bool,
    },
    /// Check and adopt an existing paper for texe and VS Code.
    Adopt {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        entry: Option<PathBuf>,
        #[arg(long)]
        engine: Option<String>,
        /// Inspect compatibility without writing files or downloading anything.
        #[arg(long)]
        check: bool,
        #[arg(short = 'y', long)]
        yes: bool,
        #[arg(long)]
        no_build: bool,
        #[arg(long)]
        no_editor: bool,
        /// Accept the editor setting conflicts shown by the compatibility check.
        #[arg(long, conflicts_with = "no_editor")]
        replace_conflicts: bool,
    },
    /// Validate the project, tools, engine, and runtime roots.
    Doctor {
        /// Project directory or texe.toml. Searches ancestors when omitted.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Rehash every installed managed runtime and component file now,
        /// rather than on the recorded verification interval.
        #[arg(long)]
        verify_toolchain: bool,
        /// Do not access the network; require populated local caches.
        #[arg(long)]
        offline: bool,
        /// Accept a disclosed first-use download without prompting.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Remove derived build state, and optionally the shared managed caches.
    Clean {
        /// Project directory or texe.toml. Searches ancestors when omitted.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Also remove managed runtime, format, component, and download entries
        /// that no current recipe needs.
        #[arg(long)]
        caches: bool,
        /// Remove all shared managed data, including package and editor caches.
        /// Required data is recreated on the next build or editor setup.
        #[arg(long)]
        all: bool,
        /// Show exactly what would be removed without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Resolve packages and build the project.
    Build {
        /// Project directory or texe.toml. Searches ancestors when omitted.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Require the existing package lock and forbid convergence.
        #[arg(long)]
        frozen: bool,
        /// Build even when nothing has changed since the last build.
        #[arg(long)]
        force: bool,
        /// Rehash every installed managed runtime and component file now,
        /// rather than on the recorded verification interval.
        #[arg(long)]
        verify_toolchain: bool,
        /// Do not access the network; require populated local caches.
        #[arg(long)]
        offline: bool,
        /// Accept a disclosed first-build download without prompting.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Rebuild whenever project inputs change.
    Watch {
        /// Project directory or texe.toml. Searches ancestors when omitted.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Require the existing package lock and forbid convergence.
        #[arg(long)]
        frozen: bool,
        /// Rehash every installed managed runtime and component file now,
        /// rather than on the recorded verification interval.
        #[arg(long)]
        verify_toolchain: bool,
        /// Do not access the network; require populated local caches.
        #[arg(long)]
        offline: bool,
        /// Accept a disclosed first-build download without prompting.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Filesystem polling interval in milliseconds.
        #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(50..=60_000))]
        poll_ms: u64,
        /// Wait for this many milliseconds without input changes before rebuilding.
        #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(50..=60_000))]
        debounce_ms: u64,
        /// Open a loopback-only browser viewer and refresh it after successful builds.
        #[arg(long)]
        view: bool,
    },
    /// Set up or remove texe's project-local VS Code and LaTeX integration.
    Editor {
        /// Project directory or texe.toml. Searches ancestors when omitted.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Undo texe-owned settings while preserving later user edits.
        #[arg(long)]
        remove: bool,
        /// Print authoritative project paths without changing anything.
        #[arg(long, conflicts_with_all = ["remove", "preview", "configure_only", "replace_conflicts"])]
        inspect: bool,
        /// Preview the merged settings and conflicting keys without writing.
        #[arg(long, conflicts_with_all = ["remove", "configure_only", "replace_conflicts"])]
        preview: bool,
        /// Update settings without installing extensions or launching VS Code.
        #[arg(long, conflicts_with = "remove")]
        configure_only: bool,
        /// Accept replacement of the conflicting texe integration values.
        #[arg(long, conflicts_with = "remove")]
        replace_conflicts: bool,
    },
    /// Show project and shared managed storage without removing anything.
    Storage {
        /// Project directory or texe.toml. Searches ancestors when omitted.
        #[arg(long)]
        project: Option<PathBuf>,
    },
}
