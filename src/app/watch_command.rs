use std::path::{Path, PathBuf};

use crate::app::build_command::{BuildOutcome, execute_build, present_build_report};
use crate::app::output::print_json_line;
use crate::app::project::load_project;
use crate::build::{self, BuildOptions};
use crate::{TexeError, ux, viewer, watch};

pub(super) fn run_watch(
    project: Option<&Path>,
    options: BuildOptions,
    verify_toolchain: bool,
    presentation: ux::Presentation,
    poll_ms: u64,
    accept_downloads: bool,
    view: bool,
) -> Result<(), TexeError> {
    let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop_sender.send(());
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

    let mut build_number = 1_u64;
    announce_watch_build(build_number, &[], presentation)?;
    let initial_result = execute_build(
        project,
        options,
        verify_toolchain,
        presentation,
        accept_downloads,
        false,
    );
    if watch_interrupted(&stop_receiver)? {
        return finish_watch(presentation);
    }
    present_watch_attempt(
        build_number,
        initial_result,
        &pdf,
        viewer.as_ref(),
        presentation,
    )?;

    let mut snapshot = watch::ProjectSnapshot::capture(&root, &manifest)?;
    let poll = std::time::Duration::from_millis(poll_ms);
    loop {
        if wait_for_watch_tick(&stop_receiver, poll)? {
            return finish_watch(presentation);
        }
        let observed = watch::ProjectSnapshot::capture(&root, &manifest)?;
        if observed == snapshot {
            continue;
        }
        // Editors commonly replace a file through several rapid rename/write
        // operations. Require two equal observations before parsing/building.
        let mut settled = observed;
        for _ in 0..8 {
            if wait_for_watch_tick(&stop_receiver, std::time::Duration::from_millis(75))? {
                return finish_watch(presentation);
            }
            let next = watch::ProjectSnapshot::capture(&root, &manifest)?;
            if next == settled {
                break;
            }
            settled = next;
        }
        let changes = snapshot.changes_since(&settled);
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
        announce_watch_build(build_number, &changes, presentation)?;
        let result = execute_build(
            project,
            options,
            verify_toolchain,
            presentation,
            true,
            false,
        );
        if watch_interrupted(&stop_receiver)? {
            return finish_watch(presentation);
        }
        present_watch_attempt(build_number, result, &pdf, viewer.as_ref(), presentation)?;
        // Capture texe.lock and any other project-root outputs exactly as the
        // completed attempt left them, preventing self-triggered rebuilds.
        snapshot = watch::ProjectSnapshot::capture(&root, &manifest)?;
    }
}

fn wait_for_watch_tick(
    stop: &std::sync::mpsc::Receiver<()>,
    duration: std::time::Duration,
) -> Result<bool, TexeError> {
    match stop.recv_timeout(duration) {
        Ok(()) => Ok(true),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(false),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(TexeError::Build(
            "watch interrupt handler stopped unexpectedly".to_string(),
        )),
    }
}

fn watch_interrupted(stop: &std::sync::mpsc::Receiver<()>) -> Result<bool, TexeError> {
    match stop.try_recv() {
        Ok(()) => Ok(true),
        Err(std::sync::mpsc::TryRecvError::Empty) => Ok(false),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(TexeError::Build(
            "watch interrupt handler stopped unexpectedly".to_string(),
        )),
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
            .file_stem()
            .unwrap_or_else(|| std::ffi::OsStr::new("main")),
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
