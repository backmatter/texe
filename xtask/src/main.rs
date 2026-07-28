//! Repository-maintenance commands for texe.
#![allow(missing_docs)]

mod command;
mod package;
mod pqty;
mod render;
mod snapshot;
mod verify;

use std::error::Error;
use std::io;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug, Parser)]
#[command(
    bin_name = "cargo xtask",
    version,
    about = "Repository-maintenance commands for texe"
)]
struct Cli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Debug, Subcommand)]
enum XtaskCommand {
    /// Run repository and integration verification.
    Verify(VerifyArgs),
    /// Maintain the pinned pqty command suite.
    Pqty {
        #[command(subcommand)]
        command: PqtyCommand,
    },
    /// Build release and Debian packages.
    Package {
        #[command(subcommand)]
        command: PackageCommand,
    },
    /// Render package-manager metadata.
    Render {
        #[command(subcommand)]
        command: RenderCommand,
    },
    /// Generate a pinned TeX Live snapshot recipe.
    Snapshot(SnapshotArgs),
}

#[derive(Debug, Args)]
struct VerifyArgs {
    /// Run one verification case.
    #[arg(value_enum)]
    case: Option<VerifyCase>,
    /// Use an existing texe/pqty/pqty-fls suite for the platform case.
    #[arg(long, value_name = "DIR", requires = "case")]
    suite_bin: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum VerifyCase {
    Platform,
    Managed,
    Luatex,
    Common,
    Bibliography,
    Index,
    Local,
}

impl VerifyCase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Managed => "managed",
            Self::Luatex => "luatex",
            Self::Common => "common",
            Self::Bibliography => "bibliography",
            Self::Index => "index",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Subcommand)]
enum PqtyCommand {
    /// Verify the pqty checkout against suite.lock.toml.
    Check {
        /// pqty checkout; defaults to PQTY_REPO or ../pqty.
        checkout: Option<PathBuf>,
    },
    /// Update suite.lock.toml from a clean pqty checkout.
    Update {
        /// Git tag or commit to pin.
        reference: String,
        /// pqty checkout; defaults to PQTY_REPO or ../pqty.
        checkout: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum PackageCommand {
    /// Build the portable command-suite archive.
    Suite {
        /// Archive destination.
        output: Option<PathBuf>,
    },
    /// Build the Linux x86-64 Debian package.
    Deb {
        /// Directory containing texe, pqty, and pqty-fls.
        #[arg(long, value_name = "DIR")]
        suite_bin: PathBuf,
        /// Installer destination.
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
        /// Release version in X.Y.Z form.
        #[arg(long, value_parser = parse_version)]
        version: String,
    },
}

#[derive(Debug, Subcommand)]
enum RenderCommand {
    /// Render the Homebrew formula.
    Homebrew(RenderArgs),
    /// Render the WinGet manifests.
    Winget(RenderArgs),
}

#[derive(Debug, Args)]
struct RenderArgs {
    /// Release version in X.Y.Z form.
    #[arg(long, value_parser = parse_version)]
    version: String,
    /// SHA-256 digest of the release archive.
    #[arg(long, value_parser = parse_sha256)]
    sha256: String,
    /// Rendered file or directory destination.
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct SnapshotArgs {
    /// Snapshot identifier.
    #[arg(long)]
    snapshot: String,
    /// Dated TeX Live archive base URL.
    #[arg(long)]
    base: String,
    /// Biber version recorded in the recipe.
    #[arg(long, default_value = "2.21")]
    biber_version: String,
    /// Managed-component recipe recorded for Biber.
    #[arg(long, default_value = "par-bootstrap-v1")]
    biber_component_recipe: String,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        XtaskCommand::Verify(options) => match options.case {
            None => verify::all(),
            Some(case) => {
                if options.suite_bin.is_some() && case != VerifyCase::Platform {
                    return Err(message("--suite-bin is only valid for the platform case"));
                }
                verify::case(case.as_str(), options.suite_bin.as_deref())
            }
        },
        XtaskCommand::Pqty { command } => match command {
            PqtyCommand::Check { checkout } => pqty::verify(checkout.as_deref()),
            PqtyCommand::Update {
                reference,
                checkout,
            } => pqty::update(&reference, checkout.as_deref()),
        },
        XtaskCommand::Package { command } => match command {
            PackageCommand::Suite { output } => package::suite(output.as_deref()),
            PackageCommand::Deb {
                suite_bin,
                output,
                version,
            } => package::deb(&suite_bin, &output, &version),
        },
        XtaskCommand::Render { command } => match command {
            RenderCommand::Homebrew(options) => {
                render::homebrew(&options.version, &options.sha256, &options.output)
            }
            RenderCommand::Winget(options) => {
                render::winget(&options.version, &options.sha256, &options.output)
            }
        },
        XtaskCommand::Snapshot(options) => snapshot::generate(
            &options.snapshot,
            &options.base,
            &options.biber_version,
            &options.biber_component_recipe,
        ),
    }
}

fn parse_version(value: &str) -> std::result::Result<String, String> {
    let valid = value.split('.').count() == 3
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if valid {
        Ok(value.to_string())
    } else {
        Err("version must be X.Y.Z".to_string())
    }
}

fn parse_sha256(value: &str) -> std::result::Result<String, String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value.to_string())
    } else {
        Err("SHA-256 must be a 64-character hexadecimal digest".to_string())
    }
}

fn message(text: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(text.into()))
}
