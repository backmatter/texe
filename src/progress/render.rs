use crate::progress::PhaseKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveStage {
    Toolchain,
    Packages,
    Requirements,
    Pdf,
}

impl LiveStage {
    pub(super) const fn from_phase(kind: PhaseKind, current: Option<Self>) -> Self {
        match kind {
            PhaseKind::Toolchain => Self::Toolchain,
            PhaseKind::Package | PhaseKind::Format => match current {
                Some(Self::Requirements) => Self::Requirements,
                Some(Self::Pdf) => Self::Pdf,
                _ => Self::Packages,
            },
            PhaseKind::EngineDiscovery => Self::Requirements,
            PhaseKind::EngineFinal | PhaseKind::Bibliography | PhaseKind::Index => Self::Pdf,
        }
    }

    pub(super) const fn completed(self) -> &'static str {
        match self {
            Self::Toolchain => "Prepared LaTeX environment",
            Self::Packages => "Prepared required packages",
            Self::Requirements => "Checked document requirements",
            Self::Pdf => "Built PDF",
        }
    }
}

pub(super) struct LiveProgress {
    stage: Option<LiveStage>,
    pub(super) bar: Option<cliclack::ProgressBar>,
    embedded: bool,
}

impl LiveProgress {
    pub(super) const fn new(embedded: bool) -> Self {
        Self {
            stage: None,
            bar: None,
            embedded,
        }
    }

    pub(super) fn update(&mut self, kind: PhaseKind, detail: &str) {
        let stage = LiveStage::from_phase(kind, self.stage);
        if self.stage != Some(stage) {
            self.finish_current();
            let bar = cliclack::spinner().with_template("{msg} · {elapsed_precise}");
            bar.start(active_stage_message(stage, kind, detail));
            self.stage = Some(stage);
            self.bar = Some(bar);
        } else if matches!(
            kind,
            PhaseKind::EngineFinal | PhaseKind::Bibliography | PhaseKind::Index
        ) && let Some(bar) = &self.bar
        {
            bar.set_message(active_stage_message(stage, kind, detail));
        }
    }

    pub(super) fn show_download(&self, message: &str) {
        if let Some(bar) = &self.bar {
            bar.set_message(message);
        }
    }

    pub(super) fn complete(&mut self, message: &str) {
        if let Some(bar) = self.bar.take() {
            bar.clear();
        }
        if self.embedded {
            let _ = cliclack::log::step(message);
        } else {
            let _ = cliclack::outro(message);
        }
        self.stage = None;
    }

    pub(super) fn fail(&mut self, message: &str) {
        if let Some(bar) = self.bar.take() {
            bar.clear();
        }
        if self.embedded {
            let _ = cliclack::log::error(message);
        } else {
            let _ = cliclack::outro_cancel(message);
        }
        self.stage = None;
    }

    fn finish_current(&mut self) {
        if let (Some(stage), Some(bar)) = (self.stage.take(), self.bar.take()) {
            bar.clear();
            let _ = cliclack::log::step(stage.completed());
        }
    }
}

#[derive(Default)]
pub(super) struct PlainProgress {
    stage: Option<LiveStage>,
}

impl PlainProgress {
    pub(super) fn update(&mut self, kind: PhaseKind) -> Vec<String> {
        let stage = LiveStage::from_phase(kind, self.stage);
        if self.stage == Some(stage) {
            return Vec::new();
        }
        let mut messages = self
            .stage
            .take()
            .map(|previous| previous.completed().to_string())
            .into_iter()
            .collect::<Vec<_>>();
        messages.push(active_stage_message(stage, kind, ""));
        self.stage = Some(stage);
        messages
    }

    pub(super) fn complete(&mut self) -> Vec<String> {
        self.stage
            .take()
            .map(|stage| vec![stage.completed().to_string()])
            .unwrap_or_default()
    }

    pub(super) fn fail(&mut self) -> Vec<String> {
        self.stage = None;
        vec!["Build stopped".to_string()]
    }
}

pub(super) fn emit_plain(messages: impl IntoIterator<Item = String>) {
    for message in messages {
        eprintln!("texe: {message}");
    }
}

fn active_stage_message(stage: LiveStage, kind: PhaseKind, detail: &str) -> String {
    match (stage, kind) {
        (LiveStage::Toolchain, _) => "Preparing LaTeX environment".to_string(),
        (LiveStage::Packages, _) => "Preparing required packages".to_string(),
        (LiveStage::Requirements, _) => "Checking document requirements".to_string(),
        (LiveStage::Pdf, PhaseKind::EngineFinal) if !detail.is_empty() => {
            format!("Building PDF · {}", detail.trim_start_matches("frozen "))
        }
        (LiveStage::Pdf, PhaseKind::Bibliography) => "Building bibliography".to_string(),
        (LiveStage::Pdf, PhaseKind::Index) => "Building index".to_string(),
        (LiveStage::Pdf, _) => "Building PDF".to_string(),
    }
}
