use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::progress::history::{format_duration, millis};
use crate::{human_bytes, human_count};

pub(super) const PQTY_PROGRESS_SCHEMA: &str = "pqty.progress/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DownloadCategory {
    Registry,
    Packages,
}

impl DownloadCategory {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Registry => "Registry Snapshot",
            Self::Packages => "packages",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum PqtyProgressEvent {
    #[serde(rename = "download-plan")]
    Plan {
        schema: String,
        category: DownloadCategory,
        items_total: usize,
        items_cached: usize,
        #[serde(default)]
        bytes_total: Option<u64>,
        #[serde(default)]
        bytes_cached: Option<u64>,
        #[serde(default)]
        bytes_to_download: Option<u64>,
    },
    #[serde(rename = "download-start")]
    Start {
        schema: String,
        category: DownloadCategory,
        item: String,
        url: String,
        attempt: usize,
        #[serde(default)]
        bytes_total: Option<u64>,
    },
    #[serde(rename = "download-progress")]
    Progress {
        schema: String,
        category: DownloadCategory,
        item: String,
        bytes_received: u64,
        #[serde(default)]
        bytes_total: Option<u64>,
        elapsed_millis: u64,
    },
    #[serde(rename = "download-complete")]
    Complete {
        schema: String,
        category: DownloadCategory,
        item: String,
        bytes_received: u64,
        #[serde(default)]
        bytes_total: Option<u64>,
        elapsed_millis: u64,
    },
}

impl PqtyProgressEvent {
    pub(super) fn schema(&self) -> &str {
        match self {
            Self::Plan { schema, .. }
            | Self::Start { schema, .. }
            | Self::Progress { schema, .. }
            | Self::Complete { schema, .. } => schema,
        }
    }

    pub(super) const fn category(&self) -> DownloadCategory {
        match self {
            Self::Plan { category, .. }
            | Self::Start { category, .. }
            | Self::Progress { category, .. }
            | Self::Complete { category, .. } => *category,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct DownloadMetrics {
    pub(super) planned_bytes: Option<u64>,
    pub(super) completed_bytes: u64,
    active_bytes: u64,
    active_item: Option<String>,
    started: Option<Instant>,
    last_rendered: Option<Instant>,
}

pub(super) fn handle_download_event(
    metrics: &mut DownloadMetrics,
    event: &PqtyProgressEvent,
    interactive: bool,
) -> Option<String> {
    match event {
        PqtyProgressEvent::Plan { .. } => Some(handle_plan(metrics, event)),
        PqtyProgressEvent::Start { .. } => handle_start(metrics, event),
        PqtyProgressEvent::Progress { .. } => handle_progress(metrics, event, interactive),
        PqtyProgressEvent::Complete { .. } => handle_complete(metrics, event, interactive),
    }
}

fn handle_plan(metrics: &mut DownloadMetrics, event: &PqtyProgressEvent) -> String {
    let PqtyProgressEvent::Plan {
        category,
        items_total,
        items_cached,
        bytes_total,
        bytes_cached,
        bytes_to_download,
        ..
    } = event
    else {
        unreachable!("plan handler received another progress event");
    };
    if metrics
        .planned_bytes
        .is_some_and(|planned| metrics.completed_bytes >= planned)
        && metrics.active_item.is_none()
    {
        *metrics = DownloadMetrics::default();
    }
    metrics.planned_bytes = match (metrics.planned_bytes, bytes_to_download) {
        (Some(previous), Some(additional)) => Some(previous.saturating_add(*additional)),
        (None, Some(download)) if metrics.completed_bytes == 0 => Some(*download),
        _ => None,
    };
    download_plan_message(
        *category,
        *items_total,
        *items_cached,
        *bytes_total,
        *bytes_cached,
        *bytes_to_download,
    )
}

fn handle_start(metrics: &mut DownloadMetrics, event: &PqtyProgressEvent) -> Option<String> {
    let PqtyProgressEvent::Start {
        category,
        item,
        url,
        attempt,
        bytes_total,
        ..
    } = event
    else {
        unreachable!("start handler received another progress event");
    };
    metrics.started.get_or_insert_with(Instant::now);
    metrics.active_bytes = 0;
    metrics.active_item = Some(item.clone());
    if *attempt > 1 {
        Some(format!(
            "retrying {} download for {item} from {url} (attempt {attempt})",
            category.label()
        ))
    } else if metrics.planned_bytes.is_none() {
        Some(format!(
            "downloading {} {item}{}",
            category.label(),
            bytes_total.map_or_else(String::new, |bytes| {
                format!(" ({})", human_bytes(bytes))
            })
        ))
    } else {
        None
    }
}

fn handle_progress(
    metrics: &mut DownloadMetrics,
    event: &PqtyProgressEvent,
    interactive: bool,
) -> Option<String> {
    let PqtyProgressEvent::Progress {
        category,
        item,
        bytes_received,
        bytes_total,
        elapsed_millis,
        ..
    } = event
    else {
        unreachable!("progress handler received another progress event");
    };
    metrics.active_bytes = *bytes_received;
    metrics.active_item = Some(item.clone());
    let interval = download_render_interval(interactive);
    if metrics
        .last_rendered
        .is_some_and(|last| last.elapsed() < interval)
    {
        return None;
    }
    metrics.last_rendered = Some(Instant::now());
    Some(download_progress_message(
        "downloading",
        *category,
        item,
        metrics,
        *bytes_total,
        *elapsed_millis,
    ))
}

fn handle_complete(
    metrics: &mut DownloadMetrics,
    event: &PqtyProgressEvent,
    interactive: bool,
) -> Option<String> {
    let PqtyProgressEvent::Complete {
        category,
        item,
        bytes_received,
        bytes_total,
        elapsed_millis,
        ..
    } = event
    else {
        unreachable!("complete handler received another progress event");
    };
    metrics.completed_bytes = metrics
        .completed_bytes
        .saturating_add(bytes_total.unwrap_or(*bytes_received));
    metrics.active_bytes = 0;
    metrics.active_item = None;
    let all_complete = metrics
        .planned_bytes
        .is_some_and(|planned| metrics.completed_bytes >= planned);
    if all_complete {
        let elapsed = metrics
            .started
            .map_or(Duration::ZERO, |start| start.elapsed());
        let rate = metrics.completed_bytes.saturating_mul(1_000) / millis(elapsed).max(1);
        Some(format!(
            "{} download complete: {} in {} · {}/s",
            category.label(),
            human_bytes(metrics.completed_bytes),
            format_duration(elapsed),
            human_bytes(rate)
        ))
    } else if metrics.planned_bytes.is_some()
        && metrics
            .last_rendered
            .is_none_or(|last| last.elapsed() >= download_render_interval(interactive))
    {
        metrics.last_rendered = Some(Instant::now());
        Some(download_progress_message(
            &format!("downloaded {item};"),
            *category,
            item,
            metrics,
            None,
            *elapsed_millis,
        ))
    } else if *elapsed_millis >= 2_000 {
        Some(format!(
            "downloaded {item}: {} in {}",
            human_bytes(*bytes_received),
            format_duration(Duration::from_millis(*elapsed_millis))
        ))
    } else {
        None
    }
}

pub(super) fn download_plan_message(
    category: DownloadCategory,
    items_total: usize,
    items_cached: usize,
    bytes_total: Option<u64>,
    bytes_cached: Option<u64>,
    bytes_to_download: Option<u64>,
) -> String {
    match (bytes_total, bytes_cached, bytes_to_download) {
        (Some(total), Some(_), Some(0)) => format!(
            "{}: all {}, {}, already cached",
            category.label(),
            human_count(items_total, "item", "items"),
            human_bytes(total)
        ),
        (Some(total), Some(cached), Some(download)) => format!(
            "{} download plan: {} across {}; {} of {} cached",
            category.label(),
            human_bytes(download),
            human_count(items_total.saturating_sub(items_cached), "item", "items"),
            human_bytes(cached),
            human_bytes(total)
        ),
        _ => format!(
            "{} download started; the server did not declare a size",
            category.label()
        ),
    }
}

fn download_progress_message(
    lead: &str,
    category: DownloadCategory,
    item: &str,
    metrics: &DownloadMetrics,
    item_total: Option<u64>,
    item_elapsed_millis: u64,
) -> String {
    let transferred = metrics.completed_bytes.saturating_add(metrics.active_bytes);
    let (rate, remaining) = metrics.started.map_or((0, None), |started| {
        let elapsed = started.elapsed();
        let rate = transferred.saturating_mul(1_000) / millis(elapsed).max(1);
        let remaining = metrics.planned_bytes.and_then(|total| {
            let minimum_sample = total.div_ceil(10).min(1024 * 1024);
            (rate > 0 && elapsed >= Duration::from_secs(3) && transferred >= minimum_sample)
                .then(|| Duration::from_secs(total.saturating_sub(transferred) / rate))
        });
        (rate, remaining)
    });
    let aggregate = metrics.planned_bytes.map_or_else(
        || human_bytes(transferred),
        |total| {
            let percent = transferred.saturating_mul(100) / total.max(1);
            format!(
                "{}/{} ({percent}%)",
                human_bytes(transferred),
                human_bytes(total)
            )
        },
    );
    let item_progress = item_total.map_or_else(String::new, |total| {
        format!(
            " · {item} {}/{}",
            human_bytes(metrics.active_bytes),
            human_bytes(total)
        )
    });
    let remaining = remaining.map_or_else(String::new, |remaining| {
        format!(" · about {} remaining", format_duration(remaining))
    });
    let current_rate = if rate == 0 {
        metrics.active_bytes.saturating_mul(1_000) / item_elapsed_millis.max(1)
    } else {
        rate
    };
    format!(
        "{lead} {}: {aggregate}{item_progress} · {}/s{remaining}",
        category.label(),
        human_bytes(current_rate)
    )
}

pub(super) const fn download_render_interval(interactive: bool) -> Duration {
    if interactive {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(10)
    }
}
