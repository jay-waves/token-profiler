use crate::{
    Result,
    model::{Event, Usage, field, now},
    storage,
    transcript::{self, Inventory, UsagePoint},
};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub(crate) struct State {
    pub enabled: bool,
    pub session: String,
    pub transcript: Option<PathBuf>,
    pub baseline: Option<Usage>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub started_ms: u128,
    #[serde(default)]
    pub stopped_ms: Option<u128>,
    #[serde(default)]
    pub baseline_ms: Option<u128>,
    #[serde(default)]
    pub last_hook_ms: Option<u128>,
    #[serde(default)]
    pub hook_count: u64,
    #[serde(default)]
    pub inventory: Option<Inventory>,
    #[serde(default)]
    pub cursor: u64,
    #[serde(default)]
    pub usage_points: Vec<UsagePoint>,
    #[serde(default)]
    pub counter_discontinuity: bool,
    #[serde(default)]
    pub usage_stale: bool,
    #[serde(default)]
    pub transcript_turn: String,
    #[serde(default)]
    pub pending_boundary: bool,
}
#[derive(Serialize)]
pub(crate) struct UsageInterval<'a> {
    pub turn: &'a str,
    pub timestamp: &'a str,
    pub usage: Option<Usage>,
}
impl State {
    pub(crate) fn new(session: String) -> Self {
        Self {
            enabled: true,
            session,
            transcript: None,
            baseline: None,
            warnings: vec!["New-content events start at activation; inventory and runtime counters have separate boundaries.".into()],
            started_ms: now(),
            stopped_ms: None,
            baseline_ms: None,
            last_hook_ms: None,
            hook_count: 0,
            inventory: None,
            cursor: 0,
            usage_points: vec![],
            counter_discontinuity: false,
            usage_stale: false,
            transcript_turn: String::new(),
            pending_boundary: false,
        }
    }
    pub(crate) fn demo() -> Self {
        let baseline = Usage {
            input: 0,
            cached: Some(0),
            output: 0,
            reasoning: Some(0),
        };
        let latest = Usage {
            input: 16000,
            cached: Some(8000),
            output: 1200,
            reasoning: Some(400),
        };
        Self {
            enabled: false,
            session: "demo".into(),
            transcript: None,
            baseline: Some(baseline),
            warnings: vec!["Synthetic demo data; not real session usage.".into()],
            started_ms: now(),
            stopped_ms: Some(now()),
            baseline_ms: Some(now()),
            last_hook_ms: Some(now()),
            hook_count: 3,
            inventory: Some(Inventory {
                observed_ms: now(),
                bytes: [
                    ("user_input".into(), 1200),
                    ("assistant_output".into(), 6000),
                    ("tool_result".into(), 18000),
                ]
                .into(),
                items: 12,
                compactions: 1,
                unsupported: 0,
            }),
            cursor: 0,
            usage_points: vec![UsagePoint {
                turn: "turn-2".into(),
                timestamp: String::new(),
                total: latest,
                break_before: false,
            }],
            counter_discontinuity: false,
            usage_stale: false,
            transcript_turn: String::new(),
            pending_boundary: false,
        }
    }
    pub(crate) fn attach(&mut self, path: &Path) -> Result<()> {
        let path = fs::canonicalize(path).context("Resolve transcript path")?;
        let scanned = transcript::scan(&path, 0, true, "")?;
        if scanned
            .session_id
            .as_deref()
            .is_some_and(|id| id != self.session)
        {
            bail!("Transcript session ID does not match the selected session");
        }
        self.transcript = Some(path);
        self.cursor = scanned.cursor;
        self.transcript_turn = scanned.last_turn;
        self.inventory = scanned.inventory;
        if let Some(inv) = &mut self.inventory {
            inv.observed_ms = now();
        }
        self.baseline = scanned.points.last().map(|p| p.total.clone());
        self.baseline_ms = self.baseline.as_ref().map(|_| now());
        self.usage_stale = scanned.usage_status == Some(false);
        if scanned.malformed > 0 {
            warn(
                self,
                "Malformed transcript records were skipped; inventory may be incomplete.",
            );
        }
        if self.baseline.is_none() {
            warn(
                self,
                "No runtime baseline at attachment; the first later counter will establish a delayed baseline.",
            );
        }
        Ok(())
    }
    pub(crate) fn connect(&mut self, explicit: Option<PathBuf>) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if let Some(path) = explicit {
            let path = fs::canonicalize(path).context("Resolve transcript path")?;
            if self
                .transcript
                .as_ref()
                .is_some_and(|current| current != &path)
            {
                bail!("Capture already uses a different transcript");
            }
            if self.transcript.is_none() {
                self.attach(&path)?;
            }
            return Ok(());
        }
        if self.transcript.is_none()
            && let Some(path) = transcript::discover(&self.session)
            && let Err(_error) = self.attach(&path)
        {
            warn(
                self,
                "Automatic transcript attachment failed; a later refresh will retry.",
            );
        }
        Ok(())
    }
    pub(crate) fn refresh(&mut self) {
        if !self.enabled {
            return;
        }
        let Some(path) = self.transcript.as_ref() else {
            return;
        };
        match transcript::scan(path, self.cursor, false, &self.transcript_turn) {
            Ok(mut scan) => {
                // Legacy states did not persist the cursor. Start from their last
                // known total instead of replaying old snapshots as new usage.
                if self.started_ms == 0 && self.cursor == 0 {
                    scan.points = scan.points.into_iter().last().into_iter().collect();
                    warn(
                        self,
                        "Legacy capture upgraded; earlier runtime intervals are unavailable.",
                    );
                }
                self.transcript_turn = scan.last_turn;
                if let Some(status) = scan.usage_status {
                    self.usage_stale = !status;
                }
                if scan.truncated {
                    self.counter_discontinuity = true;
                    self.pending_boundary = true;
                    warn(
                        self,
                        "Transcript was truncated; runtime deltas are unavailable across this boundary.",
                    );
                }
                self.cursor = scan.cursor;
                if scan.malformed > 0 {
                    warn(
                        self,
                        "Malformed transcript records were skipped; runtime coverage may be incomplete.",
                    );
                }
                if self.usage_stale {
                    warn(
                        self,
                        "Malformed cumulative usage data was skipped; runtime counters may be stale.",
                    );
                }
                // Keep optional-counter history within each uninterrupted segment.
                let mut previous = self.baseline.clone();
                for point in &self.usage_points {
                    previous = if point.break_before {
                        Some(point.total.clone())
                    } else {
                        Usage::high_water(previous.iter().chain(std::iter::once(&point.total)))
                    };
                }
                for mut point in scan.points {
                    let invalid_delta = previous
                        .as_ref()
                        .is_some_and(|last| point.total.delta(last).is_none());
                    point.break_before = self.pending_boundary || invalid_delta;
                    self.pending_boundary = false;
                    if point.break_before {
                        self.counter_discontinuity = true;
                        warn(
                            self,
                            "Runtime accounting has a gap; the full total is unknown, but valid intervals are retained.",
                        );
                    }
                    if self.baseline.is_none() {
                        self.baseline = Some(point.total.clone());
                        self.baseline_ms = Some(now());
                        warn(
                            self,
                            "Runtime accounting starts at a delayed baseline; earlier usage is excluded.",
                        );
                    } else if point.break_before || self.latest() != Some(&point.total) {
                        self.usage_points.push(point.clone());
                    }
                    previous = if point.break_before {
                        Some(point.total)
                    } else {
                        Usage::high_water(previous.iter().chain(std::iter::once(&point.total)))
                    };
                }
            }
            Err(_error) => {
                self.usage_stale = true;
                warn(
                    self,
                    "Transcript could not be read; runtime usage may be incomplete or stale.",
                );
            }
        }
    }
    pub(crate) fn latest(&self) -> Option<&Usage> {
        self.usage_points
            .last()
            .map(|p| &p.total)
            .or(self.baseline.as_ref())
    }
    pub(crate) fn usage(&self) -> Option<Usage> {
        if self.counter_discontinuity {
            return None;
        }
        self.latest()?.delta(self.baseline.as_ref()?)
    }
    pub(crate) fn intervals(&self) -> Vec<UsageInterval<'_>> {
        let mut previous = self.baseline.as_ref();
        self.usage_points
            .iter()
            .map(|point| {
                let usage = if point.break_before {
                    None
                } else {
                    previous.and_then(|p| point.total.delta(p))
                };
                previous = Some(&point.total);
                UsageInterval {
                    turn: &point.turn,
                    timestamp: &point.timestamp,
                    usage,
                }
            })
            .collect()
    }
    pub(crate) fn on_hook(&mut self, dir: &Path, v: &Value) -> Result<bool> {
        // Child-agent accounting is outside this capture; avoid mixing scopes.
        if !self.enabled || !field(v, "agent_id").is_empty() {
            return Ok(false);
        }
        if self.transcript.is_none()
            && !field(v, "transcript_path").is_empty()
            && let Err(_error) = self.attach(Path::new(field(v, "transcript_path")))
        {
            warn(
                self,
                "Hook transcript could not be attached; a later hook will retry.",
            );
        }
        self.hook_count += 1;
        self.last_hook_ms = Some(now());
        if storage::recover_event_tail(dir)? {
            warn(
                self,
                "An incomplete event log tail was discarded after an interrupted write.",
            );
        }
        let event = Event::from_hook(v);
        if event.as_ref().is_none_or(|e| e.source.is_none())
            && matches!(
                field(v, "hook_event_name"),
                "UserPromptSubmit" | "PreToolUse" | "PostToolUse"
            )
        {
            warn(
                self,
                "A hook event omitted its expected content payload; its source estimate is unknown.",
            );
        }
        if let Some(event) = &event {
            storage::append(dir, event)?;
        }
        let name = field(v, "hook_event_name");
        let finish = matches!(name, "Stop" | "Interrupt" | "SessionEnd");
        if finish {
            self.refresh();
        }
        if name == "SessionEnd" {
            self.enabled = false;
            self.stopped_ms = Some(now());
        }
        Ok(finish)
    }
}
pub(crate) fn warn(state: &mut State, message: &str) {
    if !state.warnings.iter().any(|w| w == message) {
        state.warnings.push(message.into());
    }
}
