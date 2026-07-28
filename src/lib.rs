//! Create, configure, and build reproducible LaTeX projects with texe.
//!
//! The command-line application is texe's primary interface. This crate also
//! exposes the configuration and initialization primitives used by the CLI.
//! Versioned command and JSON schema contracts are the stable
//! process-integration boundary. The Rust API follows Cargo's `SemVer` rules;
//! while the major version is zero, minor releases may contain breaking Rust
//! API changes.

#![deny(missing_docs)]

mod app;
mod atomic;
mod build;
mod clean;
mod cli;
mod config;
mod diagnostics;
mod error;
mod guard;
mod integrations;
mod lockfile;
mod package;
mod progress;
mod state;
mod toolchain;
mod ux;
mod viewer;
mod watch;

pub(crate) use app::{human_bytes, human_count};
pub use config::{
    BibliographyConfig, GeneratedInput, IndexConfig, InitOutcome, InitRequest, InitSettings,
    InputConfig, MANIFEST_NAME, PROJECT_SCHEMA, PackagesConfig, ProjectConfig, ProjectManifest,
    StarterDocument, StarterTemplate, ToolchainConfig, configure_init, discover_manifest,
    init_project, init_project_with_starter,
};
pub use error::TexeError;

/// Run the texe command-line application and map failures to stable exit codes.
pub fn main_entry() {
    app::main_entry();
}
