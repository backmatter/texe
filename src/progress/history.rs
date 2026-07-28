use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::progress::{HISTORY_SCHEMA, ProgressInner};
use crate::{TexeError, atomic};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TimingHistory {
    pub(super) schema: String,
    #[serde(default)]
    pub(super) samples: Vec<TimingSample>,
}

impl Default for TimingHistory {
    fn default() -> Self {
        Self {
            schema: HISTORY_SCHEMA.to_string(),
            samples: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TimingSample {
    pub(super) engine: String,
    pub(super) frozen: bool,
    #[serde(default)]
    pub(super) incremental: bool,
    pub(super) total_millis: u64,
    pub(super) phase_millis: BTreeMap<String, u64>,
    pub(super) engine_passes: usize,
    pub(super) bibliography_runs: usize,
    pub(super) index_runs: usize,
    pub(super) convergence_rounds: usize,
    pub(super) environment_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RemainingEstimate {
    pub(super) lower_millis: u64,
    pub(super) upper_millis: u64,
}

pub(super) fn remaining_suffix(inner: &ProgressInner, now: Instant) -> String {
    let elapsed = now.saturating_duration_since(inner.started);
    estimate_remaining(inner, now).map_or_else(
        || format!(" · elapsed {}", format_duration(elapsed)),
        |estimate| {
            if !useful_estimate(estimate.lower_millis, estimate.upper_millis) {
                return format!(" · elapsed {}", format_duration(elapsed));
            }
            format!(
                " · elapsed {} · estimated {}–{} remaining",
                format_duration(elapsed),
                format_duration(Duration::from_millis(estimate.lower_millis)),
                format_duration(Duration::from_millis(estimate.upper_millis))
            )
        },
    )
}

pub(super) const fn useful_estimate(lower_millis: u64, upper_millis: u64) -> bool {
    upper_millis >= 2_000 && upper_millis.saturating_sub(lower_millis) >= 1_000
}

pub(super) fn estimate_remaining(inner: &ProgressInner, now: Instant) -> Option<RemainingEstimate> {
    let elapsed = millis(now.saturating_duration_since(inner.started));
    let comparable = comparable_samples(inner);
    if !comparable.is_empty() {
        let total = median(
            comparable
                .iter()
                .map(|sample| sample.total_millis)
                .collect(),
        );
        let lower_total = total.saturating_mul(4) / 5;
        let upper_total = total.saturating_mul(13) / 10;
        if upper_total <= elapsed {
            return None;
        }
        return Some(RemainingEstimate {
            lower_millis: lower_total.saturating_sub(elapsed),
            upper_millis: upper_total.saturating_sub(elapsed),
        });
    }

    if inner.engine_millis.is_empty() {
        return None;
    }
    let per_pass = median(inner.engine_millis.clone());
    if per_pass < 10_000 {
        return None;
    }
    let completed = inner.engine_millis.len();
    let plausible_total_passes = inner.max_passes + usize::from(!inner.frozen) * 4;
    let high_passes = plausible_total_passes.saturating_sub(completed).max(1);
    Some(RemainingEstimate {
        lower_millis: per_pass,
        upper_millis: per_pass.saturating_mul(high_passes as u64),
    })
}

pub(super) fn comparable_samples(inner: &ProgressInner) -> Vec<&TimingSample> {
    inner
        .history
        .samples
        .iter()
        .rev()
        .filter(|sample| {
            sample.engine == inner.engine
                && sample.frozen == inner.frozen
                && sample.incremental == inner.incremental
        })
        .take(5)
        .collect()
}

pub(super) fn timing_summary(total: u64, phases: &BTreeMap<String, u64>) -> String {
    let mut fields = vec![format!(
        "build timing: total {}",
        format_duration(Duration::from_millis(total))
    )];
    for (key, label) in [
        ("toolchain", "toolchain"),
        ("packages", "packages"),
        ("format", "format"),
        ("engine-discovery", "discovery engines"),
        ("engine-final", "final engines"),
        ("bibliography", "bibliography"),
        ("index", "indexes"),
    ] {
        if let Some(millis) = phases.get(key).copied().filter(|millis| *millis > 0) {
            fields.push(format!(
                "{label} {}",
                format_duration(Duration::from_millis(millis))
            ));
        }
    }
    fields.join(" · ")
}

pub(super) fn median(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or_default()
}

pub(super) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds == 0 {
        return "<1s".to_string();
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {remainder:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes:02}m")
}

pub(super) fn read_history(path: &Path) -> TimingHistory {
    let Ok(bytes) = fs::read(path) else {
        return TimingHistory::default();
    };
    let Ok(history) = serde_json::from_slice::<TimingHistory>(&bytes) else {
        return TimingHistory::default();
    };
    if history.schema == HISTORY_SCHEMA {
        history
    } else {
        TimingHistory::default()
    }
}

pub(super) fn write_history(path: &Path, history: &TimingHistory) -> Result<(), TexeError> {
    let mut bytes = serde_json::to_vec_pretty(history).map_err(|source| TexeError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    atomic::write(path, &bytes)
}
