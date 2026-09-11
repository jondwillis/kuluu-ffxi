use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ffxi_install::Progress;

use super::dat_setup::{DatSetupForm, DatSetupUiDirty};
use crate::ffxi_client::{self, SetupOptions};

pub(super) enum JobKind {
    Setup(SetupOptions),
    Update { root: PathBuf, verify: bool },
}

enum JobEvent {
    Progress(Progress),
    Done(Result<PathBuf, String>),
}

/// A download/patch running on its own thread; present only while it runs.
#[derive(Resource)]
pub(super) struct ClientJob {
    pub title: String,
    pub phase: String,
    pub detail: String,
    /// Progress of the current phase when it is countable.
    pub fraction: Option<f32>,
    rx: Mutex<Receiver<JobEvent>>,
}

#[derive(Component)]
pub(super) struct JobPhaseText;

#[derive(Component)]
pub(super) struct JobDetailText;

#[derive(Component)]
pub(super) struct JobBarFill;

const THREAD_NAME: &str = "ffxi-client-job";

pub(super) fn start(commands: &mut Commands, kind: JobKind) {
    let title = match &kind {
        JobKind::Setup(opts) => format!("Getting the official client as `{}`", opts.name),
        JobKind::Update { root, .. } => format!("Updating {}", root.display()),
    };
    let (tx, rx) = mpsc::channel();
    let report_tx = Arc::new(Mutex::new(tx.clone()));
    let spawned = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || {
            let report = move |p: Progress| {
                if let Ok(t) = report_tx.lock() {
                    t.send(JobEvent::Progress(p)).ok();
                }
            };
            let result = match kind {
                JobKind::Setup(opts) => ffxi_client::setup(&opts, &report).map(|o| o.root),
                JobKind::Update { root, verify } => {
                    ffxi_client::update(&root, verify, &report).map(|_| root)
                }
            };
            tx.send(JobEvent::Done(result)).ok();
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "ffxi-client job thread failed to start");
        return;
    }
    commands.insert_resource(ClientJob {
        title,
        phase: "Starting...".to_string(),
        detail: String::new(),
        fraction: None,
        rx: Mutex::new(rx),
    });
}

fn mb(bytes: u64) -> u64 {
    bytes / 1_000_000
}

fn fraction(p: &Progress) -> Option<f32> {
    use Progress::*;
    let ratio = |done: u64, total: u64| (total > 0).then(|| done as f32 / total as f32);
    match p {
        CabProgress { done, files, .. } => ratio(*done as u64, *files as u64),
        UpdateScanning { done, total } => ratio(*done as u64, *total as u64),
        UpdateBytes { done, total } => ratio(*done, *total),
        UpdateFinished { .. } | Finished { .. } => Some(1.0),
        VolumeDownloading { .. } | UpdateVersion { .. } | UpdatePlanned { .. } => Some(0.0),
        _ => None,
    }
}

/// (phase, detail): a phase replaces the headline, a detail the line under it.
fn lines(p: &Progress) -> (Option<String>, String) {
    use Progress::*;
    let volumes = ffxi_install::VOLUME_COUNT;
    match p {
        VolumeCached { index } => (
            Some(format!(
                "Installer volume {index}/{volumes} already downloaded"
            )),
            String::new(),
        ),
        VolumeDownloading { index, url } => (
            Some(format!("Downloading installer volume {index}/{volumes}")),
            url.clone(),
        ),
        VolumeReady { index, .. } => (
            Some(format!("Installer volume {index}/{volumes} ready")),
            String::new(),
        ),
        MemberExtracting { name } => (None, format!("Extracting {name}")),
        MemberExtracted { name, .. } => (None, format!("Extracted {name}")),
        CabDecoding { name, files } => (Some(format!("Decoding {name}")), format!("{files} files")),
        CabProgress { name, done, files } => (
            Some(format!("Decoding {name}")),
            format!("{done} / {files} files"),
        ),
        CabDecoded {
            name, new_files, ..
        } => (None, format!("Decoded {name}: {new_files} new files")),
        FilesOutsideInstallIgnored { .. } => (None, String::new()),
        MsiPlaced { msi, files } => (None, format!("Placed {files} files from {msi}")),
        Finished { files, .. } => (
            Some(format!("Unpacked {files} files; now patching")),
            String::new(),
        ),
        UpdateVersion { local, server, .. } => (
            Some(format!("Server version {server}")),
            format!(
                "local version {}",
                local.as_deref().unwrap_or("none (base image)")
            ),
        ),
        UpdateScanning { done, total } => (
            Some("Checking local files".to_string()),
            format!("{done} / {total}"),
        ),
        UpdatePlanned {
            to_fetch, bytes, ..
        } => (
            Some(format!("Fetching {to_fetch} files ({} MB)", mb(*bytes))),
            String::new(),
        ),
        UpdateFile {
            index, count, path, ..
        } => (None, format!("[{}/{count}] {path}", index + 1)),
        UpdateBytes { done, total } => (None, format!("{} / {} MB", mb(*done), mb(*total))),
        UpdateFinished {
            version, fetched, ..
        } => (
            Some(format!("Updated to {version}")),
            format!("{fetched} files fetched"),
        ),
    }
}

pub(super) fn poll_system(
    mut commands: Commands,
    job: Option<ResMut<ClientJob>>,
    mut form: ResMut<DatSetupForm>,
    mut dirty: ResMut<DatSetupUiDirty>,
    mut phase_q: Query<&mut Text, (With<JobPhaseText>, Without<JobDetailText>)>,
    mut detail_q: Query<&mut Text, With<JobDetailText>>,
    mut bar_q: Query<&mut Node, With<JobBarFill>>,
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
            JobEvent::Progress(p) => {
                let (phase, detail) = lines(&p);
                if let Some(phase) = phase {
                    job.phase = phase;
                }
                if let Some(f) = fraction(&p) {
                    job.fraction = Some(f);
                }
                job.detail = detail;
            }
            JobEvent::Done(result) => {
                done = Some(result);
                break;
            }
        }
    }
    for mut t in phase_q.iter_mut() {
        if t.0 != job.phase {
            t.0 = job.phase.clone();
        }
    }
    for mut t in detail_q.iter_mut() {
        if t.0 != job.detail {
            t.0 = job.detail.clone();
        }
    }
    for mut n in bar_q.iter_mut() {
        n.width = Val::Percent(job.fraction.unwrap_or(0.0) * 100.0);
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
        Err(e) => Err(e),
    });
    dirty.0 = true;
}
