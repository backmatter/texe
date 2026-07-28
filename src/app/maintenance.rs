use std::path::Path;

use crate::app::output::{human_bytes, print_json};
use crate::app::project::{ProjectContext, load_project};
use crate::clean::{self, CleanOptions, CleanReport};
use crate::{TexeError, ux};

pub(super) fn run_clean(
    project: Option<&Path>,
    options: CleanOptions,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let mut report = CleanReport {
        schema: "texe.clean-report/v1".to_string(),
        project: Vec::new(),
        caches: Vec::new(),
        freed_bytes: 0,
    };
    let sweeping_caches = options.caches || options.all;
    let context = if sweeping_caches {
        load_optional_project(project)?
    } else {
        Some(load_project(project)?)
    };
    if let Some(context) = context {
        clean::clean_project(&context.root, &context.manifest, &mut report)?;
    }
    if sweeping_caches {
        clean::clean_caches(options, &mut report)?;
    }
    if presentation.json {
        return print_json(&report);
    }
    if presentation.quiet {
        return Ok(());
    }
    for path in &report.project {
        println!("removed {}", path.display());
    }
    for path in &report.caches {
        println!("removed {}", path.display());
    }
    if report.project.is_empty() && report.caches.is_empty() {
        println!("nothing to remove");
    } else {
        println!("freed {}", human_bytes(report.freed_bytes));
    }
    if !report.project.is_empty() {
        println!("kept texe.lock and the published artifact");
    }
    Ok(())
}

pub(super) fn run_clean_dry_run(
    project: Option<&Path>,
    options: CleanOptions,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let mut report = CleanReport {
        schema: "texe.clean-dry-run/v1".to_string(),
        project: Vec::new(),
        caches: Vec::new(),
        freed_bytes: 0,
    };
    let sweeping_caches = options.caches || options.all;
    let context = if sweeping_caches {
        load_optional_project(project)?
    } else {
        Some(load_project(project)?)
    };
    if let Some(context) = context {
        clean::measure_project(&context.root, &context.manifest, &mut report)?;
    }
    if sweeping_caches {
        clean::measure_caches(options, &mut report)?;
    }
    if presentation.json {
        return print_json(&report);
    }
    if presentation.quiet {
        return Ok(());
    }
    if report.project.is_empty() && report.caches.is_empty() {
        println!("nothing would be removed");
        return Ok(());
    }
    for path in &report.project {
        println!("would remove {}", path.display());
    }
    for path in &report.caches {
        println!("would remove {}", path.display());
    }
    println!("would free about {}", human_bytes(report.freed_bytes));
    println!("nothing was removed");
    Ok(())
}

pub(super) fn run_storage(
    project: Option<&Path>,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    let context = load_optional_project(project)?;
    let report = clean::storage_report(
        context
            .as_ref()
            .map(|context| (context.root.as_path(), &context.manifest)),
    )?;
    if presentation.json {
        return print_json(&report);
    }
    if presentation.quiet {
        return Ok(());
    }
    println!("Storage report (nothing was removed)");
    for entry in &report.project {
        println!(
            "  project {:>9}  {} — {}",
            human_bytes(entry.bytes),
            entry.path.display(),
            entry.purpose
        );
    }
    for entry in &report.shared {
        println!(
            "  shared  {:>9}  {} — {}",
            human_bytes(entry.bytes),
            entry.path.display(),
            entry.purpose
        );
    }
    if report.project.is_empty() && report.shared.is_empty() {
        println!("  no texe-managed build or cache data found");
    }
    println!("total: {}", human_bytes(report.total_bytes));
    println!("remove project build data: `texe clean`");
    println!("remove unused shared data too: `texe clean --caches`");
    Ok(())
}

fn load_optional_project(project: Option<&Path>) -> Result<Option<ProjectContext>, TexeError> {
    match load_project(project) {
        Ok(context) => Ok(Some(context)),
        // Machine-wide reports and cache sweeps work outside a project, but an
        // explicit project path or a discovered malformed manifest must never
        // be silently discarded.
        Err(TexeError::Manifest(message))
            if project.is_none() && message.contains("could not find texe.toml") =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::load_optional_project;

    #[test]
    fn explicit_invalid_projects_are_not_treated_as_absent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let missing = directory.path().join("missing");
        assert!(load_optional_project(Some(&missing)).is_err());

        fs::write(directory.path().join("texe.toml"), "not valid TOML = [")
            .expect("malformed manifest");
        assert!(load_optional_project(Some(directory.path())).is_err());
    }
}
