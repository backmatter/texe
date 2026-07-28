//! Optional, project-local integrations.
//!
//! A missing external application is guidance, not a failed paper. Each editor
//! adapter owns only its project-local configuration and removal behavior.

mod git;
mod vscode;

pub(crate) use git::setup_git;
pub(crate) use vscode::{open_vscode, remove_vscode, setup_vscode};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IntegrationReport {
    pub(crate) messages: Vec<String>,
}
