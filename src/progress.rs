//! Human build progress and local timing history.
//!
//! Timing observations are deliberately derived, project-local state. They do
//! not enter `texe.lock`, the build fingerprint, or any engine environment.
//! The estimates can therefore improve the interactive experience without
//! changing what a build resolves or the bytes it produces.

use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HISTORY_SCHEMA: &str = "texe.timing-history/v1";
const MAX_SAMPLES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhaseKind {
    Toolchain,
    Package,
    Format,
    EngineDiscovery,
    EngineFinal,
    Bibliography,
    Index,
}

impl PhaseKind {
    const fn key(self) -> &'static str {
        match self {
            Self::Toolchain => "toolchain",
            Self::Package => "packages",
            Self::Format => "format",
            Self::EngineDiscovery => "engine-discovery",
            Self::EngineFinal => "engine-final",
            Self::Bibliography => "bibliography",
            Self::Index => "index",
        }
    }

    const fn is_engine(self) -> bool {
        matches!(self, Self::EngineDiscovery | Self::EngineFinal)
    }
}

mod history;
mod pqty;
mod render;

pub(crate) use history::format_duration;
use history::{
    TimingHistory, TimingSample, comparable_samples, estimate_remaining, median, millis,
    read_history, remaining_suffix, timing_summary, useful_estimate, write_history,
};
use pqty::{
    DownloadCategory, DownloadMetrics, PQTY_PROGRESS_SCHEMA, PqtyProgressEvent,
    handle_download_event,
};
use render::{LiveProgress, PlainProgress, emit_plain};

struct ProgressInner {
    pub(super) started: Instant,
    pub(super) engine: String,
    pub(super) frozen: bool,
    pub(super) incremental: bool,
    pub(super) max_passes: usize,
    pub(super) history: TimingHistory,
    pub(super) phase_millis: BTreeMap<String, u64>,
    pub(super) engine_millis: Vec<u64>,
    pub(super) estimate_announced: bool,
    pub(super) downloads: BTreeMap<DownloadCategory, DownloadMetrics>,
    pub(super) live: Option<LiveProgress>,
    pub(super) plain: Option<PlainProgress>,
}

#[derive(Clone)]
pub(crate) struct Progress {
    inner: Arc<Mutex<ProgressInner>>,
    history_path: PathBuf,
    interactive: bool,
    enabled: bool,
    verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressLayout {
    Standalone,
    Embedded,
}

impl Progress {
    pub(crate) fn new(
        history_path: PathBuf,
        engine: &str,
        frozen: bool,
        max_passes: usize,
        enabled: bool,
        verbose: bool,
        layout: ProgressLayout,
    ) -> Self {
        let history = read_history(&history_path);
        let interactive = enabled && !verbose && std::io::stderr().is_terminal();
        let plain = enabled && !verbose && !interactive;
        Self {
            inner: Arc::new(Mutex::new(ProgressInner {
                started: Instant::now(),
                engine: engine.to_string(),
                frozen,
                incremental: false,
                max_passes,
                history,
                phase_millis: BTreeMap::new(),
                engine_millis: Vec::new(),
                estimate_announced: false,
                downloads: BTreeMap::new(),
                live: interactive
                    .then(|| LiveProgress::new(matches!(layout, ProgressLayout::Embedded))),
                plain: plain.then(PlainProgress::default),
            })),
            history_path,
            interactive,
            enabled,
            verbose,
        }
    }

    pub(crate) fn begin(&self, incremental: bool) {
        let mut inner = self.inner.lock().expect("progress mutex");
        // Start after manifest discovery, but before toolchain preparation, so
        // the estimate covers every potentially slow part of this invocation.
        inner.started = Instant::now();
        // First builds and incremental builds have radically different timing
        // profiles even when they share an engine and project directory.
        inner.incremental = incremental;
        let comparable = comparable_samples(&inner);
        if comparable.is_empty() {
            return;
        }
        let comparable_count = comparable.len();
        let total = median(
            comparable
                .iter()
                .map(|sample| sample.total_millis)
                .collect(),
        );
        let lower = total.saturating_mul(4) / 5;
        let upper = total.saturating_mul(13) / 10;
        inner.estimate_announced = true;
        if self.enabled && self.verbose && useful_estimate(lower, upper) {
            eprintln!(
                "texe: if a rebuild is needed, estimated build time {}–{} \
                 (from {} comparable local build{})",
                format_duration(Duration::from_millis(lower)),
                format_duration(Duration::from_millis(upper)),
                comparable_count,
                if comparable_count == 1 { "" } else { "s" }
            );
        }
    }

    pub(crate) fn announce_rebuild(&self) {
        let should_announce = {
            let mut inner = self.inner.lock().expect("progress mutex");
            if inner.estimate_announced {
                false
            } else {
                inner.estimate_announced = true;
                true
            }
        };
        if should_announce && self.enabled && self.verbose {
            eprintln!(
                "texe: no comparable local timing yet; a broad estimate will appear \
                 if the first engine pass takes at least 10s"
            );
        }
    }

    pub(crate) fn phase<T, E>(
        &self,
        kind: PhaseKind,
        label: impl Into<String>,
        action: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let mut guard = self.start(kind, label.into());
        let result = action();
        guard.succeeded = result.is_ok();
        drop(guard);
        result
    }

    pub(crate) fn handle_pqty_line(&self, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<PqtyProgressEvent>(line) else {
            return false;
        };
        if event.schema() != PQTY_PROGRESS_SCHEMA {
            return false;
        }
        let message = {
            let mut inner = self.inner.lock().expect("progress mutex");
            handle_download_event(
                inner.downloads.entry(event.category()).or_default(),
                &event,
                self.interactive,
            )
        };
        if self.enabled
            && let Some(message) = message
        {
            if self.interactive {
                if let Some(live) = self.inner.lock().expect("progress mutex").live.as_ref() {
                    live.show_download(&message);
                }
            } else {
                eprintln!("texe: {message}");
            }
        }
        true
    }

    fn start(&self, kind: PhaseKind, label: String) -> PhaseGuard {
        let suffix = {
            let inner = self.inner.lock().expect("progress mutex");
            remaining_suffix(&inner, Instant::now())
        };
        if self.interactive {
            if let Some(live) = self.inner.lock().expect("progress mutex").live.as_mut() {
                live.update(kind, &label);
            }
        } else if self.enabled && self.verbose {
            eprintln!("texe: {label}{suffix}");
        } else if self.enabled {
            let messages = self
                .inner
                .lock()
                .expect("progress mutex")
                .plain
                .as_mut()
                .map(|plain| plain.update(kind))
                .unwrap_or_default();
            emit_plain(messages);
        }

        PhaseGuard {
            progress: self.clone(),
            kind,
            label,
            started: Instant::now(),
            succeeded: false,
        }
    }

    fn record_phase(&self, kind: PhaseKind, label: &str, elapsed: Duration, succeeded: bool) {
        let millis = millis(elapsed);
        let mut first_engine_estimate = None;
        {
            let mut inner = self.inner.lock().expect("progress mutex");
            *inner
                .phase_millis
                .entry(kind.key().to_string())
                .or_default() += millis;
            if kind.is_engine() {
                inner.engine_millis.push(millis);
                if inner.engine_millis.len() == 1
                    && comparable_samples(&inner).is_empty()
                    && millis >= 10_000
                {
                    first_engine_estimate = estimate_remaining(&inner, Instant::now());
                }
            }
        }
        if self.enabled && self.verbose && elapsed >= Duration::from_secs(2) {
            eprintln!(
                "texe: {} {label} {} {}",
                if succeeded { "completed" } else { "failed" },
                if succeeded { "in" } else { "after" },
                format_duration(elapsed)
            );
        }
        if self.enabled
            && let Some(estimate) = first_engine_estimate
        {
            if self.interactive {
                if let Some(bar) = self
                    .inner
                    .lock()
                    .expect("progress mutex")
                    .live
                    .as_ref()
                    .and_then(|live| live.bar.as_ref())
                {
                    bar.set_message(format!(
                        "Building PDF · about {}–{} remaining",
                        format_duration(Duration::from_millis(estimate.lower_millis)),
                        format_duration(Duration::from_millis(estimate.upper_millis))
                    ));
                }
            } else if self.verbose {
                eprintln!(
                    "texe: first engine pass suggests about {}–{} remaining \
                     (low confidence; this narrows after a comparable local build)",
                    format_duration(Duration::from_millis(estimate.lower_millis)),
                    format_duration(Duration::from_millis(estimate.upper_millis))
                );
            }
        }
    }

    pub(crate) fn finish(
        &self,
        engine_passes: usize,
        bibliography_runs: usize,
        index_runs: usize,
        convergence_rounds: usize,
        environment_fingerprint: &str,
    ) {
        let (mut history, sample, summary) = {
            let inner = self.inner.lock().expect("progress mutex");
            let total = millis(inner.started.elapsed());
            let sample = TimingSample {
                engine: inner.engine.clone(),
                frozen: inner.frozen,
                incremental: inner.incremental,
                total_millis: total,
                phase_millis: inner.phase_millis.clone(),
                engine_passes,
                bibliography_runs,
                index_runs,
                convergence_rounds,
                environment_fingerprint: environment_fingerprint.to_string(),
            };
            let summary = timing_summary(total, &sample.phase_millis);
            (inner.history.clone(), sample, summary)
        };

        if self.enabled && self.verbose {
            eprintln!("texe: {summary}");
        }
        history.samples.push(sample);
        if history.samples.len() > MAX_SAMPLES {
            history
                .samples
                .drain(..history.samples.len().saturating_sub(MAX_SAMPLES));
        }
        if let Err(error) = write_history(&self.history_path, &history)
            && self.enabled
        {
            eprintln!("texe: warning: could not save timing history: {error}");
        }
    }

    pub(crate) const fn is_live(&self) -> bool {
        self.interactive
    }

    pub(crate) fn complete(&self, message: &str) {
        let mut inner = self.inner.lock().expect("progress mutex");
        if let Some(live) = inner.live.as_mut() {
            live.complete(message);
        } else if let Some(plain) = inner.plain.as_mut() {
            emit_plain(plain.complete());
        }
    }

    pub(crate) fn fail(&self, message: &str) {
        let mut inner = self.inner.lock().expect("progress mutex");
        if let Some(live) = inner.live.as_mut() {
            live.fail(message);
        } else if let Some(plain) = inner.plain.as_mut() {
            emit_plain(plain.fail());
        }
    }
}

struct PhaseGuard {
    progress: Progress,
    kind: PhaseKind,
    label: String,
    pub(super) started: Instant,
    succeeded: bool,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        self.progress.record_phase(
            self.kind,
            &self.label,
            self.started.elapsed(),
            self.succeeded,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{Duration, Instant};

    use crate::progress::history::{
        RemainingEstimate, TimingHistory, TimingSample, estimate_remaining, format_duration,
        read_history, useful_estimate, write_history,
    };
    use crate::progress::pqty::{DownloadCategory, download_plan_message};
    use crate::progress::render::PlainProgress;
    use crate::progress::{HISTORY_SCHEMA, PhaseKind, Progress, ProgressInner, ProgressLayout};

    fn sample(engine: &str, frozen: bool, total_millis: u64) -> TimingSample {
        TimingSample {
            engine: engine.to_string(),
            frozen,
            incremental: false,
            total_millis,
            phase_millis: BTreeMap::new(),
            engine_passes: 4,
            bibliography_runs: 0,
            index_runs: 0,
            convergence_rounds: 0,
            environment_fingerprint: "sha256:test".to_string(),
        }
    }

    #[test]
    fn formats_customer_facing_durations() {
        assert_eq!(format_duration(Duration::from_millis(400)), "<1s");
        assert_eq!(format_duration(Duration::from_secs(9)), "9s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(format_duration(Duration::from_secs(62 * 60)), "1h 02m");
    }

    #[test]
    fn comparable_history_estimates_a_range_around_the_median() {
        let now = Instant::now();
        let mut unrelated_incremental = sample("pdflatex", false, 5_000);
        unrelated_incremental.incremental = true;
        let inner = ProgressInner {
            started: now
                .checked_sub(Duration::from_secs(2 * 60))
                .expect("representable earlier instant"),
            engine: "pdflatex".to_string(),
            frozen: false,
            incremental: false,
            max_passes: 8,
            history: TimingHistory {
                schema: HISTORY_SCHEMA.to_string(),
                samples: vec![
                    sample("pdflatex", false, 900_000),
                    sample("pdflatex", false, 1_000_000),
                    sample("pdflatex", false, 1_100_000),
                    sample("pdflatex", true, 5_000_000),
                    unrelated_incremental,
                ],
            },
            phase_millis: BTreeMap::new(),
            engine_millis: Vec::new(),
            estimate_announced: false,
            downloads: BTreeMap::new(),
            live: None,
            plain: None,
        };
        assert_eq!(
            estimate_remaining(&inner, now),
            Some(RemainingEstimate {
                lower_millis: 680_000,
                upper_millis: 1_180_000,
            })
        );
    }

    #[test]
    fn first_long_engine_pass_produces_a_conservative_range() {
        let now = Instant::now();
        let inner = ProgressInner {
            started: now
                .checked_sub(Duration::from_secs(130))
                .expect("representable earlier instant"),
            engine: "pdflatex".to_string(),
            frozen: false,
            incremental: false,
            max_passes: 8,
            history: TimingHistory::default(),
            phase_millis: BTreeMap::new(),
            engine_millis: vec![120_000],
            estimate_announced: false,
            downloads: BTreeMap::new(),
            live: None,
            plain: None,
        };
        assert_eq!(
            estimate_remaining(&inner, now),
            Some(RemainingEstimate {
                lower_millis: 120_000,
                upper_millis: 1_320_000,
            })
        );
    }

    #[test]
    fn timing_history_round_trips_and_rejects_another_schema() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("timings.json");
        let history = TimingHistory {
            schema: HISTORY_SCHEMA.to_string(),
            samples: vec![sample("lualatex", false, 42_000)],
        };
        write_history(&path, &history).expect("write history");
        assert_eq!(read_history(&path).samples.len(), 1);

        fs::write(
            &path,
            br#"{"schema":"texe.timing-history/v999","samples":[]}"#,
        )
        .expect("replace history");
        assert!(read_history(&path).samples.is_empty());
    }

    #[test]
    fn consumes_closed_pqty_progress_and_tracks_the_download_plan() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let progress = Progress::new(
            directory.path().join("timings.json"),
            "pdflatex",
            false,
            5,
            true,
            false,
            ProgressLayout::Standalone,
        );
        assert!(progress.handle_pqty_line(
            r#"{"schema":"pqty.progress/v1","event":"download-plan","category":"packages","items_total":2,"items_cached":1,"bytes_total":9000,"bytes_cached":4000,"bytes_to_download":5000}"#
        ));
        assert!(progress.handle_pqty_line(
            r#"{"schema":"pqty.progress/v1","event":"download-start","category":"packages","item":"latex","url":"https://example.invalid/latex.tar.xz","attempt":1,"bytes_total":5000}"#
        ));
        assert!(progress.handle_pqty_line(
            r#"{"schema":"pqty.progress/v1","event":"download-complete","category":"packages","item":"latex","bytes_received":5000,"bytes_total":5000,"elapsed_millis":1000}"#
        ));

        let inner = progress.inner.lock().expect("progress mutex");
        let packages = inner
            .downloads
            .get(&DownloadCategory::Packages)
            .expect("package download metrics");
        assert_eq!(packages.planned_bytes, Some(5_000));
        assert_eq!(packages.completed_bytes, 5_000);
        drop(inner);

        assert!(!progress.handle_pqty_line(
            r#"{"schema":"pqty.progress/v1","event":"download-plan","category":"packages","items_total":0,"items_cached":0,"unexpected":true}"#
        ));
        assert!(!progress.handle_pqty_line(
            r#"{"schema":"pqty.progress/v2","event":"download-plan","category":"packages","items_total":0,"items_cached":0}"#
        ));
    }

    #[test]
    fn plain_progress_collapses_internal_phases_into_customer_stages() {
        let mut progress = PlainProgress::default();
        let mut transcript = progress.update(PhaseKind::Toolchain);
        transcript.extend(progress.update(PhaseKind::Package));
        assert!(progress.update(PhaseKind::Format).is_empty());
        transcript.extend(progress.update(PhaseKind::EngineDiscovery));
        assert!(progress.update(PhaseKind::Package).is_empty());
        transcript.extend(progress.update(PhaseKind::EngineFinal));
        transcript.extend(progress.complete());
        let expected = include_str!("../tests/fixtures/transcripts/build-redirected-stages.txt")
            .lines()
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(transcript.join("\n"), expected);
    }

    #[test]
    fn subsecond_timing_ranges_are_not_presented_as_estimates() {
        assert!(!useful_estimate(400, 900));
        assert!(!useful_estimate(1_500, 2_100));
        assert!(useful_estimate(2_000, 4_000));
    }

    #[test]
    fn translated_download_plans_use_natural_count_grammar() {
        assert_eq!(
            download_plan_message(
                DownloadCategory::Registry,
                1,
                1,
                Some(3 * 1024 * 1024),
                Some(3 * 1024 * 1024),
                Some(0),
            ),
            "Registry Snapshot: all 1 item, 3 MiB, already cached"
        );
        assert_eq!(
            download_plan_message(
                DownloadCategory::Packages,
                20,
                2,
                Some(5 * 1024 * 1024),
                Some(0),
                Some(5 * 1024 * 1024),
            ),
            "packages download plan: 5 MiB across 18 items; 0 B of 5 MiB cached"
        );
    }
}
