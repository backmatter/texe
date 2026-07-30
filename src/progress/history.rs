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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) package_plan: Option<PackagePlanSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackagePlanSample {
    pub(super) items_total: usize,
    pub(super) items_cached: usize,
    pub(super) bytes_total: u64,
    pub(super) bytes_cached: u64,
    pub(super) bytes_to_download: u64,
}

impl PackagePlanSample {
    /// pqty omits a plan when the content store already satisfies the exact
    /// lock. A completed install with no plan is therefore a distinct,
    /// zero-network observation rather than missing timing information.
    pub(super) const fn no_download_plan() -> Self {
        Self {
            items_total: 0,
            items_cached: 0,
            bytes_total: 0,
            bytes_cached: 0,
            bytes_to_download: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RemainingEstimate {
    pub(super) lower_millis: u64,
    pub(super) upper_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TotalEstimate {
    pub(super) lower_millis: u64,
    pub(super) upper_millis: u64,
    pub(super) rough: bool,
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
    let total = estimate_total(inner, now)?;
    if total.upper_millis <= elapsed {
        return None;
    }
    Some(RemainingEstimate {
        lower_millis: total.lower_millis.saturating_sub(elapsed),
        upper_millis: total.upper_millis.saturating_sub(elapsed),
    })
}

pub(super) fn estimate_total(inner: &ProgressInner, now: Instant) -> Option<TotalEstimate> {
    let comparable = comparable_samples(inner);
    let historical = if comparable.is_empty() {
        None
    } else {
        let package_plan = inner.package_plan();
        let (lower_total, upper_total, rough) = package_plan
            .and_then(|plan| package_aware_total_range(&comparable, plan))
            .unwrap_or_else(|| {
                let total = median(
                    comparable
                        .iter()
                        .map(|sample| sample.total_millis)
                        .collect(),
                );
                let (lower, upper) = historical_total_range(total, inner.incremental);
                (lower, upper, false)
            });
        Some(TotalEstimate {
            lower_millis: lower_total,
            upper_millis: upper_total,
            rough,
        })
    };

    let observed = if inner.engine_millis.is_empty() {
        None
    } else {
        let per_pass = median(inner.engine_millis.clone());
        if per_pass < 10_000 {
            None
        } else {
            let completed = inner.engine_millis.len();
            let plausible_total_passes = inner.max_passes + usize::from(!inner.frozen) * 4;
            let high_passes = plausible_total_passes.saturating_sub(completed).max(1);
            let elapsed = millis(now.saturating_duration_since(inner.started));
            let lower = if inner
                .phase_millis
                .get("engine-final")
                .is_some_and(|millis| *millis > 0)
            {
                // A final pass may itself have stabilized the document.
                elapsed
            } else {
                // Discovery cannot publish the PDF; at least one final pass
                // still follows.
                elapsed.saturating_add(per_pass)
            };
            Some(TotalEstimate {
                lower_millis: lower,
                upper_millis: elapsed.saturating_add(per_pass.saturating_mul(high_passes as u64)),
                rough: true,
            })
        }
    };

    match (historical, observed) {
        (Some(historical), Some(observed)) => {
            let lower = historical.lower_millis.max(observed.lower_millis);
            let upper = historical.upper_millis.min(observed.upper_millis);
            Some(if lower < upper {
                TotalEstimate {
                    lower_millis: lower,
                    upper_millis: upper,
                    rough: true,
                }
            } else {
                // Conflicting evidence should widen the estimate instead of
                // presenting a precise range that cannot contain both.
                TotalEstimate {
                    lower_millis: historical.lower_millis.min(observed.lower_millis),
                    upper_millis: historical.upper_millis.max(observed.upper_millis),
                    rough: true,
                }
            })
        }
        (Some(estimate), None) | (None, Some(estimate)) => Some(estimate),
        (None, None) => None,
    }
}

pub(super) fn live_total_status(inner: &ProgressInner, now: Instant) -> String {
    let Some(estimate) = estimate_total(inner, now) else {
        return "total estimate after first LaTeX pass".to_string();
    };
    if !useful_estimate(estimate.lower_millis, estimate.upper_millis) {
        return "total estimate after first LaTeX pass".to_string();
    }
    format!(
        "{} total {}–{}",
        if estimate.rough { "rough" } else { "estimated" },
        format_duration(Duration::from_millis(estimate.lower_millis)),
        format_duration(Duration::from_millis(estimate.upper_millis))
    )
}

pub(super) fn live_remaining_status(inner: &ProgressInner, now: Instant) -> String {
    let elapsed = millis(now.saturating_duration_since(inner.started));
    let Some(estimate) = estimate_total(inner, now) else {
        return "estimating time left".to_string();
    };
    let lower = estimate.lower_millis.saturating_sub(elapsed);
    let upper = estimate.upper_millis.saturating_sub(elapsed);
    if upper < 2_000 {
        return "finishing".to_string();
    }
    if lower < 1_000 {
        return format!(
            "up to {} left",
            format_duration(Duration::from_millis(upper))
        );
    }
    if upper.saturating_sub(lower) < 1_000 {
        return format!(
            "about {} left",
            format_duration(Duration::from_millis(upper))
        );
    }
    format!(
        "about {}–{} left",
        format_duration(Duration::from_millis(lower)),
        format_duration(Duration::from_millis(upper))
    )
}

pub(super) const fn historical_total_range(total: u64, incremental: bool) -> (u64, u64) {
    if incremental {
        (total.saturating_mul(4) / 5, total.saturating_mul(13) / 10)
    } else {
        // A project-cold build can reuse every shared download or follow a
        // shared-cache cleanup. Both have the same project state but very
        // different package/component time, so keep this first-build range
        // deliberately broad.
        (total / 2, total.saturating_mul(2))
    }
}

fn package_aware_total_range(
    samples: &[&TimingSample],
    current: PackagePlanSample,
) -> Option<(u64, u64, bool)> {
    let phased = samples
        .iter()
        .filter_map(|sample| {
            sample
                .phase_millis
                .get("packages")
                .copied()
                .map(|packages| (*sample, packages))
        })
        .collect::<Vec<_>>();
    if phased.is_empty() {
        return None;
    }

    let non_package = phased
        .iter()
        .map(|(sample, packages)| sample.total_millis.saturating_sub(*packages))
        .collect::<Vec<_>>();
    let matching_packages = phased
        .iter()
        .filter_map(|(sample, packages)| {
            sample
                .package_plan
                .filter(|plan| similar_package_work(*plan, current))
                .map(|_| *packages)
        })
        .collect::<Vec<_>>();
    let matched = !matching_packages.is_empty();
    let package_values = if matched {
        matching_packages
    } else {
        phased
            .iter()
            .map(|(_, packages)| *packages)
            .collect::<Vec<_>>()
    };

    let (non_package_lower, non_package_upper) = guarded_range(non_package, 4, 5, 13, 10)?;
    let (package_lower, package_upper) = if matched {
        guarded_range(package_values, 4, 5, 13, 10)?
    } else {
        // Legacy observations still reveal how variable package preparation
        // has been, but not which cache state produced each duration.
        guarded_range(package_values, 3, 5, 8, 5)?
    };
    Some((
        non_package_lower.saturating_add(package_lower),
        non_package_upper.saturating_add(package_upper),
        !matched,
    ))
}

fn guarded_range(
    values: Vec<u64>,
    lower_numerator: u64,
    lower_denominator: u64,
    upper_numerator: u64,
    upper_denominator: u64,
) -> Option<(u64, u64)> {
    let minimum = values.iter().copied().min()?;
    let maximum = values.iter().copied().max()?;
    let center = median(values);
    Some((
        minimum.min(center.saturating_mul(lower_numerator) / lower_denominator),
        maximum.max(center.saturating_mul(upper_numerator) / upper_denominator),
    ))
}

fn similar_package_work(left: PackagePlanSample, right: PackagePlanSample) -> bool {
    match (left.bytes_to_download, right.bytes_to_download) {
        (0, 0) => similar_work_count(left.items_total, right.items_total),
        (0, _) | (_, 0) => false,
        (left, right) => left.max(right) <= left.min(right).saturating_mul(4),
    }
}

fn similar_work_count(left: usize, right: usize) -> bool {
    match (left, right) {
        (0, 0) => true,
        (0, _) | (_, 0) => false,
        (left, right) => left.max(right) <= left.min(right).saturating_mul(4),
    }
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
