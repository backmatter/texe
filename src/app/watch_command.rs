use std::path::{Path, PathBuf};

use crate::app::build_command::{BuildOutcome, execute_build, present_build_report};
use crate::app::output::print_json_line;
use crate::app::project::load_project;
use crate::build::{self, BuildOptions};
use crate::watch::WatchSignal;
use crate::{TexeError, ux, viewer, watch};
use std::sync::mpsc::{Receiver, TryRecvError};

pub(super) fn run_watch(
    project: Option<&Path>,
    options: BuildOptions,
    verify_toolchain: bool,
    presentation: ux::Presentation,
    accept_downloads: bool,
    view: bool,
) -> Result<(), TexeError> {
    build::shipout::keep_engine_warm();
    let (signal_sender, signals) = std::sync::mpsc::channel();
    let stop_sender = signal_sender.clone();
    ctrlc::set_handler(move || {
        crate::compiler_sandbox::cancel_active();
        let _ = stop_sender.send(WatchSignal::Stop);
    })
    .map_err(|error| TexeError::Build(format!("could not listen for Ctrl-C: {error}")))?;

    let initial = load_project(project)?;
    let root = initial.root;
    let mut manifest = initial.manifest;
    let mut pdf = published_pdf(&root, &manifest.project.entry);
    let (viewer, viewer_opened) = if view {
        let viewer = viewer::Viewer::start(&pdf)?;
        let opened = viewer.open_browser()?;
        (Some(viewer), opened)
    } else {
        (None, false)
    };
    if presentation.json {
        let viewer_url = viewer.as_ref().map(viewer::Viewer::url);
        print_json_line(&watch_started_event(
            &root,
            &pdf,
            viewer_url.as_deref(),
            viewer_opened,
        ))?;
    } else if !presentation.quiet {
        eprintln!("Watching {} for changes", root.display());
        if let Some(viewer) = &viewer {
            announce_viewer(viewer, &pdf, viewer_opened);
        }
    }

    let _watcher = watch::watch_files(&root, signal_sender)?;
    let mut build_number = 1_u64;
    let mut baseline = watch::ProjectSnapshot::capture(&root, &manifest)?;
    if let Some(viewer) = &viewer {
        viewer.notify_building();
    }
    announce_watch_build(build_number, &[], presentation)?;
    let initial_result = execute_build(
        project,
        options,
        verify_toolchain,
        presentation,
        accept_downloads,
        false,
    );
    if watch_interrupted(&signals)? {
        return finish_watch(presentation);
    }
    present_watch_attempt(
        build_number,
        initial_result,
        &pdf,
        viewer.as_ref(),
        presentation,
    )?;

    loop {
        // Compare before blocking: a save during that build is already
        // pending, and its events may have been drained with the others.
        let observed = watch::ProjectSnapshot::capture(&root, &manifest)?;
        let changed_paths = baseline.changes_since(&observed);
        if changed_paths.is_empty() {
            if wait_for_file_event(&signals)? {
                return finish_watch(presentation);
            }
            continue;
        }
        if let Ok(context) = load_project(project) {
            manifest = context.manifest;
            let next_pdf = published_pdf(&root, &manifest.project.entry);
            if next_pdf != pdf {
                pdf = next_pdf;
                if let Some(viewer) = &viewer {
                    viewer.set_pdf(&pdf);
                }
            }
        }
        build_number += 1;
        baseline = watch::ProjectSnapshot::capture(&root, &manifest)?;
        if let Some(viewer) = &viewer {
            viewer.notify_building();
        }
        announce_watch_build(build_number, &changed_paths, presentation)?;
        let result = execute_build(
            project,
            options,
            verify_toolchain,
            presentation,
            true,
            false,
        );
        if watch_interrupted(&signals)? {
            return finish_watch(presentation);
        }
        present_watch_attempt(build_number, result, &pdf, viewer.as_ref(), presentation)?;
    }
}

/// Block until the filesystem reports activity. Returns whether to stop.
fn wait_for_file_event(signals: &Receiver<WatchSignal>) -> Result<bool, TexeError> {
    match signals.recv() {
        Ok(WatchSignal::Stop) => Ok(true),
        // One save is many events; a snapshot answers for all of them.
        Ok(WatchSignal::FilesChanged) => watch_interrupted(signals),
        Err(_) => Err(TexeError::Build(
            "watch event source stopped unexpectedly".to_string(),
        )),
    }
}

/// Drain queued signals, reporting whether one asked to stop.
fn watch_interrupted(signals: &Receiver<WatchSignal>) -> Result<bool, TexeError> {
    loop {
        match signals.try_recv() {
            Ok(WatchSignal::Stop) => return Ok(true),
            Ok(WatchSignal::FilesChanged) => {}
            Err(TryRecvError::Empty) => return Ok(false),
            Err(TryRecvError::Disconnected) => {
                return Err(TexeError::Build(
                    "watch event source stopped unexpectedly".to_string(),
                ));
            }
        }
    }
}

fn finish_watch(presentation: ux::Presentation) -> Result<(), TexeError> {
    if presentation.json {
        print_json_line(&watch_stopped_event())
    } else {
        if !presentation.quiet {
            eprintln!("Stopped watching.");
        }
        Ok(())
    }
}

fn announce_watch_build(
    build_number: u64,
    changes: &[PathBuf],
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    if presentation.json {
        return print_json_line(&watch_build_started_event(build_number, changes));
    }
    if presentation.quiet {
        return Ok(());
    }
    if changes.is_empty() {
        eprintln!("[{build_number}] Initial build");
    } else {
        let reason = changes
            .iter()
            .take(3)
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "[{build_number}] {reason} changed{}",
            if changes.len() > 3 {
                format!(" and {} more", changes.len() - 3)
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

fn present_watch_attempt(
    build_number: u64,
    result: Result<BuildOutcome, TexeError>,
    pdf: &Path,
    viewer: Option<&viewer::Viewer>,
    presentation: ux::Presentation,
) -> Result<(), TexeError> {
    match result {
        Ok(outcome) => {
            if presentation.json {
                print_json_line(&watch_build_succeeded_event(build_number, &outcome.report))?;
            } else {
                present_build_report(
                    &outcome.entry,
                    &outcome.report,
                    &outcome.progress,
                    presentation,
                )?;
            }
            if let Some(viewer) = viewer {
                viewer.notify_success();
            }
        }
        Err(error) => {
            if let Some(viewer) = viewer {
                viewer.notify_failure();
            }
            let previous_pdf = pdf.is_file();
            if presentation.json {
                print_json_line(&watch_build_failed_event(
                    build_number,
                    &error,
                    previous_pdf,
                ))?;
            } else {
                eprintln!("{}", ux::human_watch_error(&error, presentation.verbose));
                if previous_pdf {
                    eprintln!(
                        "[{build_number}] Previous {} kept; still watching",
                        pdf.file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .unwrap_or("PDF")
                    );
                } else {
                    eprintln!("[{build_number}] No PDF yet; still watching");
                }
            }
        }
    }
    Ok(())
}

pub(super) fn watch_started_event(
    project: &Path,
    pdf: &Path,
    viewer: Option<&str>,
    viewer_opened: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "texe.watch-event/v1",
        "event": "watch-started",
        "project": project,
        "pdf": pdf,
        "viewer": viewer,
        "viewer_opened": viewer_opened,
    })
}

pub(super) fn watch_build_started_event(
    build_number: u64,
    changes: &[PathBuf],
) -> serde_json::Value {
    serde_json::json!({
        "schema": "texe.watch-event/v1",
        "event": "build-started",
        "build": build_number,
        "reason": if changes.is_empty() { "initial" } else { "files-changed" },
        "changed": changes,
    })
}

pub(super) fn watch_build_succeeded_event(
    build_number: u64,
    report: &build::BuildReport,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "texe.watch-event/v1",
        "event": "build-succeeded",
        "build": build_number,
        "report": report,
    })
}

pub(super) fn watch_build_failed_event(
    build_number: u64,
    error: &TexeError,
    previous_pdf: bool,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "texe.watch-event/v1",
        "event": "build-failed",
        "build": build_number,
        "error": ux::ErrorEnvelope::from_error(error),
        "previous_pdf": previous_pdf,
        "watching": true,
    })
}

pub(super) fn watch_stopped_event() -> serde_json::Value {
    serde_json::json!({
        "schema": "texe.watch-event/v1",
        "event": "watch-stopped",
    })
}

fn announce_viewer(viewer: &viewer::Viewer, pdf: &Path, opened: bool) {
    let label = pdf
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("PDF");
    if opened {
        eprintln!("Viewing {label} at {}", viewer.url());
    } else {
        eprintln!("Open {label} in a browser at {}", viewer.url());
    }
}

fn published_pdf(root: &Path, entry: &Path) -> PathBuf {
    root.join(
        entry
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("main.tex")),
    )
    .with_extension("pdf")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::published_pdf;

    #[test]
    fn published_pdf_tracks_the_manifest_entry() {
        assert_eq!(
            published_pdf(Path::new("/paper"), Path::new("main.tex")),
            PathBuf::from("/paper/main.pdf")
        );
        assert_eq!(
            published_pdf(Path::new("/paper"), Path::new("sources/revised.tex")),
            PathBuf::from("/paper/revised.pdf")
        );
    }
}
