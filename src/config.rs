use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Schema identifier required by every texe project manifest.
pub const PROJECT_SCHEMA: &str = "texe.project/v1";
/// File name used for a texe project manifest.
pub const MANIFEST_NAME: &str = "texe.toml";
const DEFAULT_ENTRY: &str = "main.tex";
const DEFAULT_ENGINE: &str = "pdflatex";
const MANAGED_ENGINES: &[&str] = &["pdflatex", "lualatex"];
const MAX_GENERATED_INPUTS: usize = 128;
const MAX_GENERATED_INPUT_BYTES: usize = 1024 * 1024;
const MAX_GENERATED_INPUT_TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// User-supplied choices used to initialize an existing project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitRequest {
    /// Optional main TeX source path.
    pub entry: Option<PathBuf>,
    /// Optional TeX engine name.
    pub engine: Option<String>,
    /// Whether texe may prompt for missing choices.
    pub interactive: bool,
    /// Whether texe should accept defaults without prompting.
    pub accept_defaults: bool,
}

/// Fully resolved settings for project initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitSettings {
    /// Main TeX source path.
    pub entry: PathBuf,
    /// TeX engine selected for the project.
    pub engine: String,
}

/// Initial content to put in a newly created TeX document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StarterTemplate {
    /// A minimal compilable article.
    Basic,
    /// An empty document for callers that provide their own content.
    #[default]
    Empty,
}

/// Metadata and template choices for a new starter document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StarterDocument {
    /// Document title.
    pub title: String,
    /// Document author.
    pub author: String,
    /// Starter content template.
    pub template: StarterTemplate,
}

/// Files produced when a project is initialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    /// Path to the newly written project manifest.
    pub manifest: PathBuf,
    /// Every file created during initialization.
    pub created_files: Vec<PathBuf>,
}

/// Complete, versioned representation of `texe.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    /// Manifest schema identifier. This must equal [`PROJECT_SCHEMA`].
    pub schema: String,
    /// Project paths and generated inputs.
    pub project: ProjectConfig,
    /// Managed TeX runtime configuration.
    pub toolchain: ToolchainConfig,
    /// Extra project-owned input roots.
    #[serde(default)]
    pub inputs: InputConfig,
    /// Bibliography tool configuration.
    #[serde(default)]
    pub bibliography: BibliographyConfig,
    /// Index tool configuration.
    #[serde(default)]
    pub index: IndexConfig,
    /// Package-resolution and materialization configuration.
    #[serde(default)]
    pub packages: PackagesConfig,
}

/// Project-owned paths and generated inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Main TeX source path, relative to the project manifest.
    pub entry: PathBuf,
    /// Private build directory, relative to the project manifest.
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
    /// Small, declarative inputs that upstream build scripts would otherwise
    /// generate. They are materialized only inside texe's private build roots.
    #[serde(default)]
    pub generated: Vec<GeneratedInput>,
}

/// A small declarative file materialized in texe's private build root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedInput {
    /// Destination path relative to the private generated-input root.
    pub path: PathBuf,
    /// UTF-8 file contents.
    pub content: String,
}

/// Additional project-owned TeX input roots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Additional project-owned roots searched recursively for TeX inputs.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

/// Managed TeX runtime and execution policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainConfig {
    /// Toolchain provider; the initial schema supports `managed`.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// TeX engine, such as `pdflatex` or `lualatex`.
    pub engine: String,
    /// Managed runtime channel.
    #[serde(default = "default_channel")]
    pub channel: String,
    /// Recorder adapter used to trace package inputs.
    #[serde(default = "default_adapter")]
    pub adapter: String,
    /// `kpsewhich` executable or override.
    #[serde(default = "default_kpsewhich")]
    pub kpsewhich: String,
    /// Maximum TeX convergence passes.
    #[serde(default = "default_max_passes")]
    pub max_passes: usize,
    /// Permit the TeX engine to execute arbitrary project-selected commands.
    /// This is deliberately opt-in because those commands are not pinned by
    /// texe.lock.
    #[serde(default)]
    pub shell_escape: bool,
    /// Permit project or host overrides for pqty, recorder, bibliography, and
    /// index commands while using the managed provider. These commands sit
    /// outside the managed runtime and disable the no-op build cache.
    #[serde(default)]
    pub allow_unmanaged_commands: bool,
}

/// Package resolution, lockfile, store, and materialization settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagesConfig {
    /// Package manager executable or override.
    #[serde(default = "default_manager")]
    pub manager: String,
    /// Recorder trace adapter executable or override.
    #[serde(default = "default_trace_adapter")]
    pub trace_adapter: String,
    /// Project package lockfile path.
    #[serde(default = "default_lock")]
    pub lock: PathBuf,
    /// Materialized project TEXMF root.
    #[serde(default = "default_texmf")]
    pub texmf: PathBuf,
    /// Optional shared package-store path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<PathBuf>,
    /// Whether the package manager may use network registries.
    #[serde(default = "default_remote")]
    pub remote: bool,
    /// How the materialized TEXMF tree references pqty's content-addressed
    /// store. Copying gives every project its own duplicate of every package
    /// file it uses; linking keeps one copy in the store.
    #[serde(default = "default_link")]
    pub link: String,
}

/// Bibliography tools and project-owned bibliography roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BibliographyConfig {
    /// BibTeX executable or override.
    #[serde(default = "default_bibtex")]
    pub bibtex: String,
    /// Biber executable or override.
    #[serde(default = "default_biber")]
    pub biber: String,
    /// Additional project-owned roots searched recursively for `.bib` and
    /// `.bst` files.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

/// Index-generation tool settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    /// `MakeIndex` executable or override.
    #[serde(default = "default_makeindex")]
    pub makeindex: String,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            makeindex: default_makeindex(),
        }
    }
}

impl Default for BibliographyConfig {
    fn default() -> Self {
        Self {
            bibtex: default_bibtex(),
            biber: default_biber(),
            roots: Vec::new(),
        }
    }
}

impl Default for PackagesConfig {
    fn default() -> Self {
        Self {
            manager: default_manager(),
            trace_adapter: default_trace_adapter(),
            lock: default_lock(),
            texmf: default_texmf(),
            store: None,
            remote: default_remote(),
            link: default_link(),
        }
    }
}

fn default_build_dir() -> PathBuf {
    PathBuf::from(".texe/build")
}

fn default_provider() -> String {
    "managed".to_string()
}

fn default_channel() -> String {
    "stable".to_string()
}

fn default_adapter() -> String {
    "kpathsea".to_string()
}

fn default_kpsewhich() -> String {
    "kpsewhich".to_string()
}

const fn default_max_passes() -> usize {
    5
}

fn default_manager() -> String {
    "pqty".to_string()
}

fn default_trace_adapter() -> String {
    "pqty-fls".to_string()
}

fn default_bibtex() -> String {
    "bibtex".to_string()
}

fn default_biber() -> String {
    "biber".to_string()
}

fn default_makeindex() -> String {
    "makeindex".to_string()
}

fn default_lock() -> PathBuf {
    PathBuf::from(".texe/state/pqty.lock")
}

fn default_texmf() -> PathBuf {
    PathBuf::from(".texe/texmf")
}

const fn default_remote() -> bool {
    true
}

/// Copy mode is pqty's supported installation contract.
fn default_link() -> String {
    "copy".to_string()
}

pub const LINK_MODES: &[&str] = &["copy", "experimental-symlink", "experimental-hardlink"];

impl ProjectManifest {
    pub(crate) fn uses_unmanaged_commands(&self) -> bool {
        self.packages.manager != default_manager()
            || self.packages.trace_adapter != default_trace_adapter()
            || self.bibliography.bibtex != default_bibtex()
            || self.bibliography.biber != default_biber()
            || self.index.makeindex != default_makeindex()
    }
}

mod discovery;
mod starter;
mod validation;

pub use discovery::{configure_init, discover_manifest, resolve_manifest};
pub use starter::{init_project, init_project_with_starter};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::config::discovery::configure_init_with;
    use crate::config::{
        GeneratedInput, InitRequest, MANIFEST_NAME, ProjectManifest, StarterDocument,
        StarterTemplate, init_project, init_project_with_starter,
    };

    #[test]
    fn defaults_form_a_valid_manifest() {
        let manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
        )
        .expect("manifest parses");
        manifest.validate().expect("manifest validates");
        assert_eq!(manifest.project.build_dir, Path::new(".texe/build"));
        assert!(manifest.project.generated.is_empty());
        assert!(manifest.inputs.roots.is_empty());
        assert_eq!(manifest.toolchain.provider, "managed");
        assert_eq!(manifest.toolchain.channel, "stable");
        assert_eq!(manifest.bibliography.bibtex, "bibtex");
        assert_eq!(manifest.bibliography.biber, "biber");
        assert!(manifest.bibliography.roots.is_empty());
        assert_eq!(manifest.index.makeindex, "makeindex");
        assert!(!manifest.toolchain.shell_escape);
        assert!(!manifest.toolchain.allow_unmanaged_commands);
        assert_eq!(manifest.packages.manager, "pqty");
        assert_eq!(manifest.packages.lock, Path::new(".texe/state/pqty.lock"));
        assert!(manifest.packages.remote);
        assert_eq!(manifest.packages.link, "copy");
    }

    #[test]
    fn abbreviated_link_modes_are_rejected() {
        let manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
[packages]
link = "symlink"
"#,
        )
        .expect("manifest parses before semantic validation");
        let error = manifest.validate().expect_err("unsupported link mode");
        assert!(error.to_string().contains("experimental-symlink"));
    }

    #[test]
    fn paths_cannot_escape_project() {
        let mut manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
        )
        .expect("manifest parses");
        manifest.packages.texmf = PathBuf::from("../shared");
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn cleanable_paths_stay_in_non_overlapping_private_namespaces() {
        let mut manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
"#,
        )
        .expect("manifest parses");

        for unsafe_path in [".", ".texe", "main.tex", "./.texe/build"] {
            manifest.project.build_dir = PathBuf::from(unsafe_path);
            assert!(
                manifest.validate().is_err(),
                "accepted unsafe build directory {unsafe_path}"
            );
        }
        manifest.project.build_dir = PathBuf::from(".texe/build");
        manifest.packages.lock = PathBuf::from("main.tex");
        assert!(manifest.validate().is_err());

        manifest.packages.lock = PathBuf::from(".texe/build/pqty.lock");
        assert!(
            manifest.validate().is_err(),
            "derived paths may not contain one another"
        );
        manifest.packages.lock = PathBuf::from(".texe/state/pqty.lock");
        manifest.packages.store = Some(PathBuf::from(".texe/build/store"));
        assert!(
            manifest.validate().is_err(),
            "the persistent store may not overlap cleanable state"
        );
    }

    #[test]
    fn managed_command_overrides_require_an_explicit_opt_out() {
        let mut manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[toolchain]
engine = "pdflatex"
[packages]
manager = "tools/pqty"
"#,
        )
        .expect("manifest parses");
        assert!(manifest.validate().is_err());

        manifest.toolchain.allow_unmanaged_commands = true;
        manifest.validate().expect("explicit override validates");
        assert!(manifest.uses_unmanaged_commands());

        manifest.toolchain.provider = "system".to_string();
        manifest.toolchain.allow_unmanaged_commands = false;
        manifest
            .validate()
            .expect("the system provider is already an unmanaged boundary");
    }

    #[test]
    fn generated_inputs_and_search_roots_are_portable_and_unique() {
        let mut manifest: ProjectManifest = toml::from_str(
            r#"
schema = "texe.project/v1"
[project]
entry = "main.tex"
[[project.generated]]
path = "build/Version.tex"
content = "\\newcommand{\\Version}{locked}\n"
[toolchain]
engine = "pdflatex"
[inputs]
roots = ["styles"]
[bibliography]
roots = ["vendor/natbib"]
"#,
        )
        .expect("manifest parses");
        manifest.validate().expect("manifest validates");

        manifest.project.generated.push(GeneratedInput {
            path: PathBuf::from("build/Version.tex"),
            content: "duplicate".to_string(),
        });
        assert!(manifest.validate().is_err());
        manifest.project.generated.pop();

        manifest.inputs.roots = vec![PathBuf::from("styles"), PathBuf::from("styles")];
        assert!(manifest.validate().is_err());
        manifest.inputs.roots = vec![PathBuf::from("../shared")];
        assert!(manifest.validate().is_err());
        manifest.inputs.roots.clear();

        manifest.bibliography.roots = vec![PathBuf::from("../shared")];
        assert!(manifest.validate().is_err());
        manifest.bibliography.roots = vec![PathBuf::from(".texe/private")];
        assert!(manifest.validate().is_err());

        for unsafe_entry in [
            "../main.tex",
            "./main.tex",
            "C:/paper/main.tex",
            "bad\nname.tex",
            "paper//main.tex",
            "paper/",
        ] {
            manifest.bibliography.roots.clear();
            manifest.project.entry = PathBuf::from(unsafe_entry);
            assert!(
                manifest.validate().is_err(),
                "accepted non-portable entry {unsafe_entry:?}"
            );
        }
    }

    #[test]
    fn init_is_non_destructive() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (manifest, created_entry) =
            init_project(directory.path(), Path::new("main.tex"), "pdflatex")
                .expect("project initializes");
        assert!(manifest.is_file());
        assert!(created_entry);
        let source = fs::read_to_string(&manifest).expect("manifest can be read");
        assert!(!source.contains("provider"));
        assert!(!source.contains("[packages]"));
        ProjectManifest::load(&manifest).expect("minimal manifest is valid");
        assert!(init_project(directory.path(), Path::new("main.tex"), "pdflatex").is_err());
    }

    #[test]
    fn init_uses_system_provider_for_engines_without_a_managed_recipe() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (manifest, _) = init_project(directory.path(), Path::new("main.tex"), "xelatex")
            .expect("project initializes");
        let source = fs::read_to_string(manifest).expect("manifest can be read");
        assert!(source.contains("provider = \"system\""));
    }

    #[test]
    fn init_uses_managed_provider_for_lualatex() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (manifest, _) = init_project(directory.path(), Path::new("main.tex"), "lualatex")
            .expect("project initializes");
        let source = fs::read_to_string(&manifest).expect("manifest can be read");
        assert!(!source.contains("provider"));
        let manifest = ProjectManifest::load(&manifest).expect("manifest loads");
        assert_eq!(manifest.toolchain.engine, "lualatex");
        assert_eq!(manifest.toolchain.provider, "managed");
    }

    #[test]
    fn basic_starter_is_structured_and_escapes_paper_metadata() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let starter = StarterDocument {
            title: "Energy_Use & {Results} \\ 2026".to_string(),
            author: "Ada % Researcher".to_string(),
            template: StarterTemplate::Basic,
        };
        let outcome = init_project_with_starter(
            directory.path(),
            Path::new("main.tex"),
            "pdflatex",
            &starter,
        )
        .expect("project initializes");

        assert_eq!(
            outcome.created_files,
            [PathBuf::from("main.tex"), PathBuf::from("references.bib")]
        );
        ProjectManifest::load(&outcome.manifest).expect("manifest loads");
        let source = fs::read_to_string(directory.path().join("main.tex")).expect("starter source");
        assert!(source.contains("\\title{Energy\\_Use \\& \\{Results\\} \\textbackslash{} 2026}"));
        assert!(source.contains("\\author{Ada \\% Researcher}"));
        assert!(source.contains("\\begin{abstract}"));
        assert!(source.contains("\\section{Methods}"));
        assert!(source.contains("\\bibliography{references}"));
        assert!(directory.path().join("references.bib").is_file());
    }

    #[test]
    fn empty_starter_is_compilable_without_extra_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let starter = StarterDocument {
            title: "A Paper".to_string(),
            author: "A Scientist".to_string(),
            template: StarterTemplate::Empty,
        };
        let outcome = init_project_with_starter(
            directory.path(),
            Path::new("main.tex"),
            "pdflatex",
            &starter,
        )
        .expect("project initializes");

        assert_eq!(outcome.created_files, [PathBuf::from("main.tex")]);
        let source = fs::read_to_string(directory.path().join("main.tex")).expect("starter source");
        assert!(source.contains("\\title{A Paper}"));
        assert!(source.contains("\\author{A Scientist}"));
        assert!(source.contains("\\maketitle"));
        assert!(!directory.path().join("references.bib").exists());
    }

    #[test]
    fn starter_collision_rolls_back_the_manifest_and_created_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("references.bib"),
            b"user bibliography",
        )
        .expect("existing bibliography");
        let starter = StarterDocument {
            template: StarterTemplate::Basic,
            ..StarterDocument::default()
        };

        let error = init_project_with_starter(
            directory.path(),
            Path::new("main.tex"),
            "pdflatex",
            &starter,
        )
        .expect_err("starter collision must fail");

        assert!(error.to_string().contains("refusing to overwrite"));
        assert!(!directory.path().join(MANIFEST_NAME).exists());
        assert!(!directory.path().join("main.tex").exists());
        assert_eq!(
            fs::read(directory.path().join("references.bib")).expect("bibliography remains"),
            b"user bibliography"
        );
    }

    #[test]
    fn failed_starter_creation_rolls_back_the_manifest() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(directory.path().join("src"), b"not a directory").expect("blocking file");
        assert!(init_project(directory.path(), Path::new("src/main.tex"), "pdflatex").is_err());
        assert!(!directory.path().join(MANIFEST_NAME).exists());
    }

    #[test]
    fn init_detects_editor_engine_hint_without_an_engine_prompt() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("paper.tex"),
            "% !TeX program = xelatex\n\\documentclass{article}\n",
        )
        .expect("fixture can be written");
        let request = InitRequest {
            interactive: true,
            ..InitRequest::default()
        };
        let mut prompts = Vec::new();
        let settings = configure_init_with(
            directory.path(),
            &request,
            &mut |message, options, default| {
                prompts.push((message.to_string(), options.to_vec(), default));
                Ok(default)
            },
        )
        .expect("settings resolve");
        assert_eq!(settings.entry, Path::new("paper.tex"));
        assert_eq!(settings.engine, "xelatex");
        assert!(prompts.is_empty());
    }

    #[test]
    fn interactive_init_defaults_to_pdflatex_without_an_engine_prompt() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("paper.tex"),
            "\\documentclass{article}\n",
        )
        .expect("fixture can be written");
        let request = InitRequest {
            interactive: true,
            ..InitRequest::default()
        };
        let mut unexpected_prompt =
            |_: &str, _: &[String], _: usize| panic!("a single-entry project must not prompt");
        let settings = configure_init_with(directory.path(), &request, &mut unexpected_prompt)
            .expect("settings resolve");
        assert_eq!(settings.entry, Path::new("paper.tex"));
        assert_eq!(settings.engine, "pdflatex");
    }

    #[test]
    fn init_prompts_for_ambiguous_document_roots() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for name in ["article.tex", "slides.tex"] {
            fs::write(directory.path().join(name), "\\documentclass{article}\n")
                .expect("fixture can be written");
        }
        let request = InitRequest {
            interactive: true,
            engine: Some("pdflatex".to_string()),
            ..InitRequest::default()
        };
        let settings = configure_init_with(
            directory.path(),
            &request,
            &mut |message, options, default| {
                assert_eq!(message, "LaTeX entry");
                assert_eq!(options, ["article.tex", "slides.tex"]);
                assert_eq!(default, 0);
                Ok(1)
            },
        )
        .expect("settings resolve");
        assert_eq!(settings.entry, Path::new("slides.tex"));
        assert_eq!(settings.engine, "pdflatex");
    }

    #[test]
    fn non_interactive_init_requires_entry_when_ambiguous() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for name in ["article.tex", "slides.tex"] {
            fs::write(directory.path().join(name), "\\documentclass{article}\n")
                .expect("fixture can be written");
        }
        let mut unexpected_prompt = |_: &str, _: &[String], _: usize| {
            panic!("non-interactive initialization must not prompt")
        };
        let error = configure_init_with(
            directory.path(),
            &InitRequest::default(),
            &mut unexpected_prompt,
        )
        .expect_err("ambiguous entry should fail");
        assert!(error.to_string().contains("--entry"));

        let defaults = InitRequest {
            accept_defaults: true,
            ..InitRequest::default()
        };
        let settings = configure_init_with(directory.path(), &defaults, &mut unexpected_prompt)
            .expect("--yes accepts deterministic defaults");
        assert_eq!(settings.entry, Path::new("article.tex"));
    }
}
