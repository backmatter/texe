use std::path::PathBuf;

use crate::{diagnostics, ux};

/// Error returned by texe's public library entry points.
#[derive(Debug, thiserror::Error)]
pub enum TexeError {
    /// Invalid command-line input or unsupported requested behavior.
    #[error("{0}")]
    Usage(String),
    /// A filesystem operation failed at a known path.
    #[error("I/O error at {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A TOML file could not be parsed.
    #[error("invalid TOML at {path}: {source}")]
    Toml {
        /// Path to the invalid TOML file.
        path: PathBuf,
        /// TOML parser error.
        #[source]
        source: toml::de::Error,
    },
    /// A JSON file could not be parsed.
    #[error("invalid JSON at {path}: {source}")]
    Json {
        /// Path to the invalid JSON file.
        path: PathBuf,
        /// JSON parser error.
        #[source]
        source: serde_json::Error,
    },
    /// The project manifest is invalid or missing.
    #[error("invalid project manifest: {0}")]
    Manifest(String),
    /// Interactive project setup could not finish.
    #[error("interactive setup failed: {0}")]
    Prompt(String),
    /// A required external executable was not found.
    #[error("required tool is not available: {0}")]
    ToolNotFound(String),
    /// An external process could not be started.
    #[error("could not start {tool}: {source}")]
    Spawn {
        /// Executable that could not be started.
        tool: PathBuf,
        /// Underlying process-spawn error.
        #[source]
        source: std::io::Error,
    },
    /// A download or other network operation failed.
    #[error("{message}")]
    Network {
        /// User-facing summary, including the attempted source when available.
        message: String,
        /// Structured error returned by the network operation.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// An external process returned an unsuccessful status.
    #[error(
        "{tool} failed with status {status:?}{detail}",
        detail = if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )]
    Process {
        /// Executable that failed.
        tool: PathBuf,
        /// Process exit code, or `None` if it exited without one.
        status: Option<i32>,
        /// Captured standard error.
        stderr: String,
    },
    /// Managed toolchain provisioning or validation failed.
    #[error("toolchain error: {0}")]
    Toolchain(String),
    /// The document build failed.
    #[error("build error: {0}")]
    Build(String),
    /// A structured diagnostic already suitable for user-facing rendering.
    #[error("{0}")]
    Diagnostic(Box<diagnostics::Diagnostic>),
}

impl TexeError {
    pub(crate) fn category(&self) -> ux::ErrorCategory {
        match self {
            Self::Manifest(_) | Self::Toml { .. } | Self::Json { .. } => ux::ErrorCategory::Project,
            Self::Prompt(message) if message == "cancelled" => ux::ErrorCategory::Cancelled,
            Self::Usage(_) | Self::Prompt(_) => ux::ErrorCategory::Usage,
            Self::Network { .. } => ux::ErrorCategory::Network,
            Self::ToolNotFound(_) | Self::Toolchain(_) => ux::ErrorCategory::Tool,
            Self::Build(_) | Self::Process { .. } | Self::Diagnostic(_) => ux::ErrorCategory::Build,
            Self::Io { .. } | Self::Spawn { .. } => ux::ErrorCategory::System,
        }
    }

    pub(crate) fn action(&self) -> Option<&'static str> {
        match self {
            Self::Usage(_) => Some("correct the command using `texe --help`, then try again"),
            Self::Manifest(message) if message.contains("could not find texe.toml") => {
                Some("run `texe` to create a paper, or `texe init` in an existing LaTeX folder")
            }
            Self::Manifest(_) | Self::Toml { .. } => {
                Some("fix the named project setting, then run the command again")
            }
            Self::Prompt(message) if message == "cancelled" => {
                Some("nothing was changed; run `texe` again when you are ready")
            }
            Self::Prompt(_) => Some("run the command again, or pass explicit flags"),
            Self::ToolNotFound(_) => Some(
                "reinstall the complete texe command suite so texe, pqty, and pqty-fls are together",
            ),
            Self::Network { .. } => Some(
                "check the connection and retry, or use `texe build --offline` with a populated cache",
            ),
            Self::Toolchain(_) => Some("run `texe doctor --verbose` for the failing tool and path"),
            Self::Build(_) | Self::Process { .. } => Some(
                "fix the first source error shown above, then build again; use `--verbose` for the full engine detail",
            ),
            Self::Diagnostic(_) => None,
            Self::Io { .. } | Self::Json { .. } | Self::Spawn { .. } => {
                Some("check the named path and permissions, then try again")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::TexeError;
    use crate::ux::ErrorCategory;

    #[test]
    fn only_structured_network_errors_use_the_network_category() {
        let network = TexeError::Network {
            message: "request failed".to_string(),
            source: Box::new(std::io::Error::other("connection failed")),
        };
        assert_eq!(network.category(), ErrorCategory::Network);

        let integrity =
            TexeError::Toolchain("downloaded mirror response failed SHA-512".to_string());
        assert_eq!(integrity.category(), ErrorCategory::Tool);

        let build = TexeError::Build("connection diagram is invalid".to_string());
        assert_eq!(build.category(), ErrorCategory::Build);
    }
}
