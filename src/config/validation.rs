use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::TexeError;
use crate::config::{
    GeneratedInput, LINK_MODES, MAX_GENERATED_INPUT_BYTES, MAX_GENERATED_INPUT_TOTAL_BYTES,
    MAX_GENERATED_INPUTS, PROJECT_SCHEMA, ProjectManifest,
};

impl ProjectManifest {
    /// Load and validate a project manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, contains invalid TOML,
    /// uses an unsupported schema, or violates manifest invariants.
    pub fn load(path: &Path) -> Result<Self, TexeError> {
        let text = fs::read_to_string(path).map_err(|source| TexeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let manifest: Self = toml::from_str(&text).map_err(|source| TexeError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate schema, command, path, and pass-count invariants.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported schema, empty command, unsafe path,
    /// or invalid pass count.
    pub fn validate(&self) -> Result<(), TexeError> {
        if self.schema != PROJECT_SCHEMA {
            return Err(TexeError::Manifest(format!(
                "unsupported manifest schema {}; expected {PROJECT_SCHEMA}",
                self.schema
            )));
        }
        validate_portable_project_path("project.entry", &self.project.entry)?;
        validate_derived_path("project.build_dir", &self.project.build_dir)?;
        validate_derived_path("packages.lock", &self.packages.lock)?;
        validate_derived_path("packages.texmf", &self.packages.texmf)?;
        if let Some(store) = &self.packages.store {
            validate_relative_path("packages.store", store)?;
        }
        validate_derived_paths_do_not_overlap(self)?;
        validate_generated_inputs(&self.project.generated)?;
        validate_input_roots("inputs.roots", &self.inputs.roots)?;
        validate_bibliography_roots(&self.bibliography.roots)?;
        for (name, value) in [
            ("toolchain.provider", self.toolchain.provider.as_str()),
            ("toolchain.engine", self.toolchain.engine.as_str()),
            ("toolchain.channel", self.toolchain.channel.as_str()),
            ("toolchain.adapter", self.toolchain.adapter.as_str()),
            ("toolchain.kpsewhich", self.toolchain.kpsewhich.as_str()),
            ("packages.manager", self.packages.manager.as_str()),
            (
                "packages.trace_adapter",
                self.packages.trace_adapter.as_str(),
            ),
            ("bibliography.bibtex", self.bibliography.bibtex.as_str()),
            ("bibliography.biber", self.bibliography.biber.as_str()),
            ("index.makeindex", self.index.makeindex.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(TexeError::Manifest(format!("{name} cannot be empty")));
            }
        }
        if !(2..=20).contains(&self.toolchain.max_passes) {
            return Err(TexeError::Manifest(
                "toolchain.max_passes must be between 2 and 20".to_string(),
            ));
        }
        if !LINK_MODES.contains(&self.packages.link.as_str()) {
            return Err(TexeError::Manifest(format!(
                "packages.link must be one of {}; got `{}`",
                LINK_MODES.join(", "),
                self.packages.link
            )));
        }
        if self.toolchain.provider == "managed"
            && self.uses_unmanaged_commands()
            && !self.toolchain.allow_unmanaged_commands
        {
            return Err(TexeError::Manifest(
                "managed command overrides require `toolchain.allow_unmanaged_commands = true`; \
                 they can execute host or project software and disable the no-op build cache"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

fn validate_derived_path(name: &str, path: &Path) -> Result<(), TexeError> {
    validate_relative_path(name, path)?;
    let mut components = path.components();
    let private_root = components
        .next()
        .is_some_and(|component| component.as_os_str() == ".texe");
    if !private_root || components.next().is_none() {
        return Err(TexeError::Manifest(format!(
            "{name} must be below texe's private .texe directory"
        )));
    }
    Ok(())
}

fn validate_derived_paths_do_not_overlap(manifest: &ProjectManifest) -> Result<(), TexeError> {
    let derived = [
        ("project.build_dir", manifest.project.build_dir.as_path()),
        ("packages.lock", manifest.packages.lock.as_path()),
        ("packages.texmf", manifest.packages.texmf.as_path()),
    ];
    for (index, (left_name, left)) in derived.iter().enumerate() {
        for (right_name, right) in &derived[index + 1..] {
            if paths_overlap(left, right) {
                return Err(TexeError::Manifest(format!(
                    "{left_name} and {right_name} cannot overlap"
                )));
            }
        }
        if let Some(store) = manifest.packages.store.as_deref()
            && paths_overlap(left, store)
        {
            return Err(TexeError::Manifest(format!(
                "{left_name} and packages.store cannot overlap"
            )));
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn validate_generated_inputs(inputs: &[GeneratedInput]) -> Result<(), TexeError> {
    if inputs.len() > MAX_GENERATED_INPUTS {
        return Err(TexeError::Manifest(format!(
            "project.generated cannot contain more than {MAX_GENERATED_INPUTS} files"
        )));
    }
    let mut paths = BTreeSet::new();
    let mut total_bytes = 0usize;
    for (index, input) in inputs.iter().enumerate() {
        let name = format!("project.generated[{index}].path");
        validate_portable_project_path(&name, &input.path)?;
        if !paths.insert(input.path.clone()) {
            return Err(TexeError::Manifest(format!(
                "project.generated contains duplicate path {}",
                input.path.display()
            )));
        }
        if input.content.len() > MAX_GENERATED_INPUT_BYTES {
            return Err(TexeError::Manifest(format!(
                "project.generated input {} exceeds {MAX_GENERATED_INPUT_BYTES} bytes",
                input.path.display()
            )));
        }
        total_bytes = total_bytes.saturating_add(input.content.len());
    }
    if total_bytes > MAX_GENERATED_INPUT_TOTAL_BYTES {
        return Err(TexeError::Manifest(format!(
            "project.generated content exceeds {MAX_GENERATED_INPUT_TOTAL_BYTES} bytes in total"
        )));
    }
    Ok(())
}

fn validate_bibliography_roots(roots: &[PathBuf]) -> Result<(), TexeError> {
    validate_input_roots("bibliography.roots", roots)
}

fn validate_input_roots(name: &str, roots: &[PathBuf]) -> Result<(), TexeError> {
    let mut paths = BTreeSet::new();
    for (index, root) in roots.iter().enumerate() {
        validate_portable_project_path(&format!("{name}[{index}]"), root)?;
        if !paths.insert(root.clone()) {
            return Err(TexeError::Manifest(format!(
                "{name} contains duplicate path {}",
                root.display()
            )));
        }
    }
    Ok(())
}

fn validate_portable_project_path(name: &str, path: &Path) -> Result<(), TexeError> {
    validate_relative_path(name, path)?;
    if path
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".texe")
    {
        return Err(TexeError::Manifest(format!(
            "{name} cannot target texe's private .texe directory"
        )));
    }
    Ok(())
}

pub(super) fn validate_relative_path(name: &str, path: &Path) -> Result<(), TexeError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(TexeError::Manifest(format!(
            "{name} must be a non-empty project-relative path"
        )));
    }
    let Some(text) = path.to_str() else {
        return Err(TexeError::Manifest(format!("{name} must be valid UTF-8")));
    };
    let bytes = text.as_bytes();
    let drive_letter = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if drive_letter
        || text.contains('\\')
        || text.contains("//")
        || text.ends_with('/')
        || text.chars().any(char::is_control)
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(TexeError::Manifest(format!(
            "{name} must contain only portable project-relative path components"
        )));
    }
    Ok(())
}
