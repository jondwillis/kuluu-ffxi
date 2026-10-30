use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use ffxi_install::{Cancel, Lane, Progress, CANCELLED};

use super::dat_setup::{DatSetupForm, DatSetupUiDirty, ACCENT_COLOR, MUTED_COLOR, OK_COLOR};
use crate::ffxi_client::{self, SetupOptions};

pub(super) enum JobKind {
    Setup(SetupOptions),
    Update { root: PathBuf, verify: bool },
}

enum JobEvent {
    Progress(Progress),
    Done(Result<PathBuf, String>),
}

/// Lines kept under the lanes; enough to read what just went past without
/// turning the panel into a log viewer.
pub(super) const LOG_LINES: usize = 5;
/// A lane with no event for this long says so rather than looking identical to
/// one that is simply slow: `curl --retry` reconnects silently, and a wedged
/// transfer is otherwise indistinguishable from a healthy 40-minute one.
const STALL_AFTER: Duration = Duration::from_secs(20);
/// Smoothing for the transfer rate. Raw per-poll deltas swing far too hard to
/// read, and the ETA built from them is unusable.
const RATE_SMOOTHING: f64 = 0.15;
/// Ignore rate samples shorter than this; a tiny dt divides into a wild rate.
const RATE_MIN_SAMPLE: Duration = Duration::from_millis(400);
/// Width of the sliding block that marks an unknown-length phase, and how long
/// one sweep takes. Distinct from a 0% bar, which means no progress yet.
const INDETERMINATE_WIDTH: f32 = 22.0;
const INDETERMINATE_PERIOD: f32 = 1.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LaneStatus {
    Pending,
    Active,
    Done,
}

pub(super) struct LaneState {
    pub status: LaneStatus,
    pub headline: String,
    pub detail: String,
    /// `None` while the stage is running but uncountable, which renders as an
    /// indeterminate bar rather than as zero progress.
    pub fraction: Option<f32>,
    last_event: Instant,
}

impl LaneState {
    fn new() -> Self {
        Self {
            status: LaneStatus::Pending,
            headline: String::new(),
            detail: String::new(),
            fraction: None,
            last_event: Instant::now(),
        }
    }

    fn stalled(&self) -> bool {
        self.status == LaneStatus::Active && self.last_event.elapsed() >= STALL_AFTER
    }
}

/// Exponentially smoothed transfer rate, fed by whichever stage is moving
/// bytes; the launcher shows it because on a multi-GB fetch a rate and an ETA
/// are the only signals that separate slow from wedged.
#[derive(Default)]
struct RateMeter {
    last: Option<(Instant, u64)>,
    bytes_per_sec: Option<f64>,
}

impl RateMeter {
    fn sample(&mut self, done: u64) {
        let now = Instant::now();
        let Some((prev_at, prev_done)) = self.last else {
            self.last = Some((now, done));
            return;
        };
        let dt = now.duration_since(prev_at);
        if dt < RATE_MIN_SAMPLE {
            return;
        }
        self.last = Some((now, done));
        let Some(gained) = done.checked_sub(prev_done) else {
            return;
        };
        let instant = gained as f64 / dt.as_secs_f64();
        self.bytes_per_sec = Some(match self.bytes_per_sec {
            Some(avg) => avg + (instant - avg) * RATE_SMOOTHING,
            None => instant,
        });
    }

    /// `rate - eta` for the remaining bytes, empty until a rate is known.
    fn summary(&self, done: u64, total: u64) -> String {
        let Some(bps) = self.bytes_per_sec.filter(|b| *b > 1.0) else {
            return String::new();
        };
        let mut out = format!(" - {}/s", fmt_bytes(bps as u64));
        if let Some(left) = total.checked_sub(done).filter(|_| total > 0) {
            out.push_str(&format!(
                " - about {} left",
                fmt_duration(Duration::from_secs_f64(left as f64 / bps))
            ));
        }
        out
    }
}

/// A download/patch running on its own thread; present only while it runs.
#[derive(Resource)]
pub(super) struct ClientJob {
    pub title: String,
    lanes: [LaneState; Lane::ALL.len()],
    pub log: VecDeque<String>,
    pub cancel: Cancel,
    pub cancelling: bool,
    rate: RateMeter,
    rx: Mutex<Receiver<JobEvent>>,
}

impl ClientJob {
    pub fn lane(&self, lane: Lane) -> &LaneState {
        &self.lanes[lane_index(lane)]
    }

    fn lane_mut(&mut self, lane: Lane) -> &mut LaneState {
        &mut self.lanes[lane_index(lane)]
    }
}

fn lane_index(lane: Lane) -> usize {
    Lane::ALL
        .iter()
        .position(|l| *l == lane)
        .unwrap_or_default()
}

pub(super) fn stage_color(status: LaneStatus) -> Color {
    match status {
        LaneStatus::Pending => MUTED_COLOR,
        LaneStatus::Active => ACCENT_COLOR,
        LaneStatus::Done => OK_COLOR,
    }
}

#[derive(Component)]
pub(super) struct JobStageLabel(pub Lane);

#[derive(Component)]
pub(super) struct JobLaneBar(pub Lane);

/// Every string this panel updates, keyed by where it belongs. One component
/// for all of them keeps the refresh to a single `Text` query; four separate
/// ones cannot be made pairwise disjoint and Bevy rejects the system.
#[derive(Component, Clone)]
pub(super) enum JobText {
    Headline(Lane),
    Detail(Lane),
    Log(usize),
    CancelButton,
}

const THREAD_NAME: &str = "ffxi-client-job";
pub(super) const CANCEL_LABEL: &str = "Cancel";
pub(super) const CANCELLING_LABEL: &str = "Cancelling...";

pub(super) fn start(commands: &mut Commands, kind: JobKind) {
    let title = match &kind {
        JobKind::Setup(opts) => format!("Getting the official client as `{}`", opts.name),
        JobKind::Update { root, .. } => format!("Updating {}", root.display()),
    };
    let (tx, rx) = mpsc::channel();
    let report_tx = Arc::new(Mutex::new(tx.clone()));
    let cancel = Cancel::default();
    let worker_cancel = cancel.clone();
    let spawned = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || {
            let report = move |p: Progress| {
                if let Ok(t) = report_tx.lock() {
                    t.send(JobEvent::Progress(p)).ok();
                }
            };
            let result = match kind {
                JobKind::Setup(opts) => {
                    ffxi_client::setup(&opts, &worker_cancel, &report).map(|o| o.root)
                }
                JobKind::Update { root, verify } => {
                    ffxi_client::update(&root, verify, &worker_cancel, &report).map(|_| root)
                }
            };
            tx.send(JobEvent::Done(result)).ok();
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "ffxi-client job thread failed to start");
        return;
    }
    let mut job = ClientJob {
        title,
        lanes: [LaneState::new(), LaneState::new(), LaneState::new()],
        log: VecDeque::new(),
        cancel,
        cancelling: false,
        rate: RateMeter::default(),
        rx: Mutex::new(rx),
    };
    job.lane_mut(Lane::Download).status = LaneStatus::Active;
    job.lane_mut(Lane::Download).headline = "Contacting the download server".to_string();
    commands.insert_resource(job);
}

fn fmt_bytes(bytes: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    const KB: u64 = 1_000;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{} MB", b / MB),
        b if b >= KB => format!("{} KB", b / KB),
        b => format!("{b} B"),
    }
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        s if s >= 3600 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn ratio(done: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| (done as f32 / total as f32).clamp(0.0, 1.0))
}

/// The one-line record a step leaves behind in the scrollback; byte ticks and
/// other high-frequency events log nothing.
fn log_line(p: &Progress) -> Option<String> {
    use Progress::*;
    Some(match p {
        VolumeCached { index } => format!("volume {index}: already downloaded"),
        VolumeReady { index, .. } => format!("volume {index}: downloaded"),
        MemberExtracted { name, .. } => format!("extracted {name}"),
        CabDecoded {
            name, new_files, ..
        } => format!("decoded {name}: {new_files} new files"),
        MsiPlaced { msi, files } => format!("placed {files} files from {msi}"),
        FilesOutsideInstallIgnored { msi, count } => {
            format!("{msi}: {count} files outside the install ignored")
        }
        Finished { files, .. } => format!("unpacked {files} files"),
        UpdateVersion { local, server, .. } => format!(
            "patch server is at {server}; this install is at {}",
            local.as_deref().unwrap_or("the base image")
        ),
        UpdatePlanned {
            to_fetch, bytes, ..
        } => format!("{to_fetch} files to patch ({})", fmt_bytes(*bytes)),
        UpdateFinished {
            version, fetched, ..
        } => format!("patched to {version}: {fetched} files"),
        _ => return None,
    })
}

/// Applies one event to the lane that produced it. Lanes run concurrently, so
/// nothing here may write across lanes.
fn apply(job: &mut ClientJob, p: &Progress) {
    use Progress::*;
    let lane = p.lane();
    let rate = &mut job.rate;
    let state = job.lanes.get_mut(lane_index(lane)).expect("lane in range");
    state.last_event = Instant::now();
    if state.status == LaneStatus::Pending {
        state.status = LaneStatus::Active;
    }
    let volumes = ffxi_install::VOLUME_COUNT;
    match p {
        VolumeCached { index } => {
            state.headline = format!("Volume {index}/{volumes} already downloaded");
        }
        VolumeDownloading { index, .. } => {
            state.headline = format!("Downloading volume {index}/{volumes}");
        }
        DownloadBytes { done, total } => {
            rate.sample(*done);
            state.fraction = ratio(*done, *total);
            state.detail = match total {
                0 => format!("{} downloaded", fmt_bytes(*done)),
                total => format!(
                    "{} / {}{}",
                    fmt_bytes(*done),
                    fmt_bytes(*total),
                    rate.summary(*done, *total)
                ),
            };
        }
        VolumeReady { index, .. } => {
            state.headline = format!("Volume {index}/{volumes} downloaded");
            if *index == volumes {
                state.status = LaneStatus::Done;
                state.fraction = Some(1.0);
                state.detail.clear();
            }
        }
        MemberExtracting { name } => {
            state.headline = format!("Extracting {name}");
            state.fraction = None;
        }
        MemberExtracted { .. } => state.detail.clear(),
        CabDecoding { name, files } => {
            state.headline = format!("Decoding {name}");
            state.detail = format!("0 / {files} files");
            state.fraction = ratio(0, *files as u64);
        }
        CabProgress { name, done, files } => {
            state.headline = format!("Decoding {name}");
            state.detail = format!("{done} / {files} files");
            state.fraction = ratio(*done as u64, *files as u64);
        }
        CabDecoded { .. } | FilesOutsideInstallIgnored { .. } => state.detail.clear(),
        MsiPlaced { msi, files } => {
            state.headline = format!("Placing files from {msi}");
            state.detail = format!("{files} files");
            state.fraction = None;
        }
        Finished { files, .. } => {
            state.headline = format!("Unpacked {files} files");
            state.status = LaneStatus::Done;
            state.fraction = Some(1.0);
            state.detail.clear();
        }
        UpdateVersion { server, .. } => {
            state.headline = format!("Patching to {server}");
            state.fraction = None;
        }
        UpdateScanning { done, total } => {
            state.headline = "Checking local files".to_string();
            state.detail = format!("{done} / {total}");
            state.fraction = ratio(*done as u64, *total as u64);
        }
        UpdatePlanned {
            to_fetch, bytes, ..
        } => {
            state.headline = format!("Fetching {to_fetch} files ({})", fmt_bytes(*bytes));
            state.detail.clear();
            state.fraction = Some(0.0);
        }
        UpdateFile {
            index, count, path, ..
        } => {
            state.headline = format!("[{}/{count}] {path}", index + 1);
        }
        UpdateBytes { done, total } => {
            rate.sample(*done);
            state.fraction = ratio(*done, *total);
            state.detail = format!(
                "{} / {}{}",
                fmt_bytes(*done),
                fmt_bytes(*total),
                rate.summary(*done, *total)
            );
        }
        UpdateFinished { version, .. } => {
            state.headline = format!("Patched to {version}");
            state.status = LaneStatus::Done;
            state.fraction = Some(1.0);
            state.detail.clear();
        }
    }
    if let Some(line) = log_line(p) {
        job.log.push_back(line);
        while job.log.len() > LOG_LINES {
            job.log.pop_front();
        }
    }
}

/// Fill geometry for one lane's bar: `(left, width)` in percent. An unknown
/// length sweeps a short block so it cannot be mistaken for a stalled 0%.
fn bar_geometry(state: &LaneState, seconds: f32) -> (f32, f32) {
    match state.fraction {
        Some(f) => (0.0, f * 100.0),
        None if state.status == LaneStatus::Active => {
            let phase = (seconds / INDETERMINATE_PERIOD).fract();
            let travel = 100.0 - INDETERMINATE_WIDTH;
            let triangle_wave = 1.0 - (2.0 * phase - 1.0).abs();
            (triangle_wave * travel, INDETERMINATE_WIDTH)
        }
        None => (0.0, 0.0),
    }
}

/// A stalled lane says so; otherwise the lane's own detail.
fn detail_line(state: &LaneState) -> String {
    if state.stalled() {
        return format!(
            "{} (no progress for {}s)",
            state.detail,
            STALL_AFTER.as_secs()
        );
    }
    state.detail.clone()
}

fn set_text(text: &mut Text, value: &str) {
    if text.0 != value {
        text.0.clear();
        text.0.push_str(value);
    }
}

pub(super) fn poll_system(
    mut commands: Commands,
    time: Res<Time>,
    job: Option<ResMut<ClientJob>>,
    mut form: ResMut<DatSetupForm>,
    mut dirty: ResMut<DatSetupUiDirty>,
    mut stage_q: Query<(&JobStageLabel, &mut TextColor)>,
    mut text_q: Query<(&JobText, &mut Text)>,
    mut bar_q: Query<(&JobLaneBar, &mut Node)>,
) {
    let Some(mut job) = job else {
        return;
    };
    let mut events = Vec::new();
    if let Ok(rx) = job.rx.lock() {
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
    }
    let mut done = None;
    for ev in events {
        match ev {
            JobEvent::Progress(p) => apply(&mut job, &p),
            JobEvent::Done(result) => {
                done = Some(result);
                break;
            }
        }
    }

    for (label, mut color) in stage_q.iter_mut() {
        color.0 = stage_color(job.lane(label.0).status);
    }
    for (slot, mut text) in text_q.iter_mut() {
        let value = match slot {
            JobText::Headline(lane) => job.lane(*lane).headline.clone(),
            JobText::Detail(lane) => detail_line(job.lane(*lane)),
            JobText::Log(index) => job.log.get(*index).cloned().unwrap_or_default(),
            JobText::CancelButton => if job.cancelling {
                CANCELLING_LABEL
            } else {
                CANCEL_LABEL
            }
            .to_string(),
        };
        set_text(&mut text, &value);
    }
    let seconds = time.elapsed_secs();
    for (lane, mut node) in bar_q.iter_mut() {
        let (left, width) = bar_geometry(job.lane(lane.0), seconds);
        node.left = Val::Percent(left);
        node.width = Val::Percent(width);
    }

    let Some(result) = done else {
        return;
    };
    commands.remove_resource::<ClientJob>();
    form.installs = ffxi_client::installs();
    form.feedback = Some(match result {
        Ok(root) => {
            let summary = ffxi_client::describe(&root);
            form.path = root.display().to_string();
            Ok(format!("Ready: {summary}. Press Continue to use it."))
        }
        Err(e) if e == CANCELLED => Ok(
            "Stopped. Whatever finished downloading is kept, so starting again resumes."
                .to_string(),
        ),
        Err(e) => Err(e),
    });
    dirty.0 = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indeterminate_bar_is_distinct_from_zero_progress() {
        let mut state = LaneState::new();
        state.status = LaneStatus::Active;
        state.fraction = None;
        let (_, width) = bar_geometry(&state, 0.5);
        assert_eq!(width, INDETERMINATE_WIDTH);

        state.fraction = Some(0.0);
        assert_eq!(bar_geometry(&state, 0.5), (0.0, 0.0));
    }

    #[test]
    fn indeterminate_block_stays_inside_the_track() {
        let mut state = LaneState::new();
        state.status = LaneStatus::Active;
        state.fraction = None;
        for step in 0..64 {
            let (left, width) = bar_geometry(&state, step as f32 * 0.1);
            assert!(left >= 0.0 && left + width <= 100.0, "left {left}");
        }
    }

    fn job_for_test() -> ClientJob {
        let (_tx, rx) = mpsc::channel();
        ClientJob {
            title: String::new(),
            lanes: [LaneState::new(), LaneState::new(), LaneState::new()],
            log: VecDeque::new(),
            cancel: Cancel::default(),
            cancelling: false,
            rate: RateMeter::default(),
            rx: Mutex::new(rx),
        }
    }

    #[test]
    fn unpack_events_never_touch_the_download_lane() {
        let mut job = job_for_test();
        apply(
            &mut job,
            &Progress::DownloadBytes {
                done: 3_000_000_000,
                total: 7_200_000_000,
            },
        );
        let before = job.lane(Lane::Download).fraction;
        assert!(before.is_some_and(|f| f > 0.4));

        apply(
            &mut job,
            &Progress::CabProgress {
                name: "data1.cab".into(),
                done: 1,
                files: 400,
            },
        );
        assert_eq!(job.lane(Lane::Download).fraction, before);
        assert!(job.lane(Lane::Unpack).headline.contains("data1.cab"));
        assert!(!job.lane(Lane::Download).headline.contains("data1.cab"));
    }

    /// A downloaded volume must not read as zero progress, which is exactly
    /// what the merged bar showed at "volume 2/5 ready".
    #[test]
    fn a_finished_volume_leaves_the_download_bar_advanced() {
        let mut job = job_for_test();
        apply(
            &mut job,
            &Progress::VolumeDownloading {
                index: 2,
                url: "https://example.invalid/vol2".into(),
            },
        );
        apply(
            &mut job,
            &Progress::DownloadBytes {
                done: 2_880_000_000,
                total: 7_200_000_000,
            },
        );
        apply(
            &mut job,
            &Progress::VolumeReady {
                index: 2,
                complete_members: 3,
            },
        );
        assert!(job.lane(Lane::Download).fraction.is_some_and(|f| f > 0.35));
        assert_eq!(job.lane(Lane::Download).status, LaneStatus::Active);
    }

    #[test]
    fn the_last_volume_completes_the_download_lane() {
        let mut job = job_for_test();
        apply(
            &mut job,
            &Progress::VolumeReady {
                index: ffxi_install::VOLUME_COUNT,
                complete_members: 1,
            },
        );
        assert_eq!(job.lane(Lane::Download).status, LaneStatus::Done);
        assert_eq!(job.lane(Lane::Download).fraction, Some(1.0));
    }

    #[test]
    fn scrollback_keeps_the_most_recent_lines() {
        let mut job = job_for_test();
        for index in 1..=LOG_LINES + 2 {
            apply(&mut job, &Progress::VolumeCached { index });
        }
        assert_eq!(job.log.len(), LOG_LINES);
        assert!(job.log.back().is_some_and(|l| l.contains("volume 7")));
    }

    #[test]
    fn rate_summary_reports_a_rate_and_an_eta() {
        let meter = RateMeter {
            bytes_per_sec: Some(10_000_000.0),
            ..Default::default()
        };
        let summary = meter.summary(1_000_000_000, 7_000_000_000);
        assert!(summary.contains("10 MB/s"), "{summary}");
        assert!(summary.contains("10m left"), "{summary}");
    }
}
