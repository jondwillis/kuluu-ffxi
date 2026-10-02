//! Bring an installed client to the patch server's current version the way
//! PlayOnline Viewer does, minus the viewer: version check, manifest, then
//! per file either the current `.slc`/zlib image or the chain of `.olc`
//! deltas, each result verified against its manifest signature. The patch
//! system's functions live in app.dll/polcore.dll
//! (research/XiPackets/patch/Reversing.md). Decision rule from
//! PlayOnlineViewer/viewer/com/polcore.dll (viewer 1.18.15e, SHA-256
//! 73b1864b...) FUN_1003219e case 0x44c.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

use crate::manifest::{self, FileHistory, FileSig, Manifest, MANIFEST_FILE};
use crate::patch_client::{self, Server, Session, TITLE_FFXI};
use crate::{Cancel, Progress, Reporter};

const TEMP_SUFFIX: &str = ".kuluu-update";
const SCAN_REPORT_EVERY: usize = 1000;
const FETCH_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Current,
    FetchDirect,
    /// Apply the deltas of `versions[from..]` in order onto the local file.
    ApplyDeltas {
        from: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlan {
    pub path: String,
    pub action: Action,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdatePlan {
    pub files: Vec<FilePlan>,
    pub current: usize,
    pub bytes: u64,
    pub lineage: Lineage,
}

/// How much of an install Square Enix's manifest recognises, counted while the
/// plan is built. Split because a retail install is internally consistent - its
/// executables and its data come from the same point in the lineage - and a
/// private server's client is not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lineage {
    pub code_present: usize,
    pub code_recognized: usize,
    pub content_present: usize,
    pub content_recognized: usize,
}

/// Percent of `present` local files matching some version in their own history.
fn rate(recognized: usize, present: usize) -> f64 {
    if present == 0 {
        return 0.0;
    }
    100.0 * recognized as f64 / present as f64
}

impl Lineage {
    pub fn code_rate(&self) -> f64 {
        rate(self.code_recognized, self.code_present)
    }

    pub fn content_rate(&self) -> f64 {
        rate(self.content_recognized, self.content_present)
    }

    /// Percentage points between what the install's code claims to be and what
    /// its data claims to be.
    pub fn disagreement(&self) -> f64 {
        (self.code_rate() - self.content_rate()).abs()
    }

    fn add(&mut self, is_code: bool, recognized: bool) {
        let (present, matched) = if is_code {
            (&mut self.code_present, &mut self.code_recognized)
        } else {
            (&mut self.content_present, &mut self.content_recognized)
        };
        *present += 1;
        *matched += usize::from(recognized);
    }
}

/// Manifest entries that are code rather than game data.
fn is_code(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".dll") || lower.ends_with(".exe")
}

pub fn decide(history: &FileHistory, local: Option<FileSig>) -> (Action, u64) {
    let last = history.versions.len() - 1;
    let direct = (Action::FetchDirect, history.current().direct_len);
    let Some(local) = local else {
        return direct;
    };
    let Some(matched) = history.versions.iter().rposition(|v| v.sig == local) else {
        return direct;
    };
    if matched == last {
        return (Action::Current, 0);
    }
    let later = &history.versions[matched + 1..];
    let mut delta_bytes = 0u64;
    for v in later {
        match &v.indirect {
            Some(d) => delta_bytes += d.len,
            None => return direct,
        }
    }
    if delta_bytes < history.current().direct_len {
        (Action::ApplyDeltas { from: matched + 1 }, delta_bytes)
    } else {
        direct
    }
}

/// How many manifest entries the workers claim per turn. The manifest runs to
/// tens of thousands of files, most of them small, so handing them out one at
/// a time spends more on the shared counter than on the hash.
const SCAN_BATCH: usize = 64;

/// Worker count for anything that fans out over the manifest.
fn workers(jobs: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(jobs)
        .max(1)
}

/// Scan the install against the manifest and build the update plan. The plan
/// runs in manifest order, so what a given install produces does not depend
/// on how the parallel scan interleaved.
pub fn plan(root: &Path, manifest: &Manifest, report: &Reporter) -> Result<UpdatePlan, String> {
    let total = manifest.files.len();
    let next = AtomicUsize::new(0);
    let scanned = AtomicUsize::new(0);
    let decided: Mutex<Vec<(usize, Action, u64)>> = Mutex::new(Vec::new());
    let lineage: Mutex<Lineage> = Mutex::new(Lineage::default());
    let failure: Mutex<Option<String>> = Mutex::new(None);

    report(Progress::UpdateScanning { done: 0, total });
    thread::scope(|scope| {
        for _ in 0..workers(total) {
            scope.spawn(|| {
                let mut mine = Vec::new();
                let mut seen = Lineage::default();
                loop {
                    let start = next.fetch_add(SCAN_BATCH, Ordering::Relaxed);
                    if start >= total || failure.lock().is_ok_and(|f| f.is_some()) {
                        break;
                    }
                    for i in start..(start + SCAN_BATCH).min(total) {
                        let history = &manifest.files[i];
                        let local_path = root.join(&history.path);
                        let local = if local_path.is_file() {
                            match FileSig::of_path(&local_path) {
                                Ok(sig) => Some(sig),
                                Err(e) => {
                                    *failure.lock().expect("scan failure lock") = Some(e);
                                    return;
                                }
                            }
                        } else {
                            None
                        };
                        if let Some(sig) = local {
                            let recognized = history.versions.iter().any(|v| v.sig == sig);
                            seen.add(is_code(&history.path), recognized);
                        }
                        let (action, bytes) = decide(history, local);
                        mine.push((i, action, bytes));
                    }
                    let done = scanned.fetch_add(SCAN_BATCH, Ordering::Relaxed) + SCAN_BATCH;
                    if done % SCAN_REPORT_EVERY < SCAN_BATCH {
                        report(Progress::UpdateScanning {
                            done: done.min(total),
                            total,
                        });
                    }
                }
                decided.lock().expect("scan result lock").append(&mut mine);
                let mut total = lineage.lock().expect("lineage lock");
                total.code_present += seen.code_present;
                total.code_recognized += seen.code_recognized;
                total.content_present += seen.content_present;
                total.content_recognized += seen.content_recognized;
            });
        }
    });
    if let Some(e) = failure.into_inner().expect("scan failure lock") {
        return Err(e);
    }
    report(Progress::UpdateScanning { done: total, total });

    let mut decided = decided.into_inner().expect("scan result lock");
    decided.sort_unstable_by_key(|(i, _, _)| *i);
    let mut plan = UpdatePlan {
        lineage: lineage.into_inner().expect("lineage lock"),
        ..Default::default()
    };
    for (i, action, bytes) in decided {
        if action == Action::Current {
            plan.current += 1;
        } else {
            plan.bytes += bytes;
            plan.files.push(FilePlan {
                path: manifest.files[i].path.clone(),
                action,
                bytes,
            });
        }
    }
    Ok(plan)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub version: String,
    pub fetched: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Re-verify every file even when the local manifest already carries
    /// the server's version stamp.
    pub force: bool,
}

pub fn local_version(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join(MANIFEST_FILE)).ok()?;
    manifest::parse(&text)
        .ok()?
        .latest_stamp()
        .map(str::to_owned)
}

fn write_atomically(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let tmp = PathBuf::from(format!("{}{TEMP_SUFFIX}", path.display()));
    fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{} -> {}: {e}", tmp.display(), path.display()))
}

struct Connection {
    server: Server,
    session: Session,
}

impl Connection {
    fn open(server: Server) -> Result<Self, String> {
        let session = Session::connect(&server)?;
        Ok(Self { server, session })
    }

    fn fetch(
        &mut self,
        path: &str,
        len: u64,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Vec<u8>, String> {
        let mut last_err = String::new();
        for attempt in 1..=FETCH_ATTEMPTS {
            match self.session.fetch(path, len, progress) {
                Ok(data) => return Ok(data),
                Err(e) => {
                    last_err = e;
                    if attempt < FETCH_ATTEMPTS {
                        self.session = Session::connect(&self.server)?;
                    }
                }
            }
        }
        Err(format!("{path}: {last_err}"))
    }
}

/// Produce the post-update bytes for one file. The progress callback reports
/// bytes cumulative within that file; a caller feeding a shared counter takes
/// the delta so it stays monotone while other connections report against the
/// same total.
fn produce(
    conn: &mut Connection,
    root: &Path,
    history: &FileHistory,
    action: &Action,
    progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, String> {
    let current = history.current();
    let data = match action {
        Action::Current => unreachable!("current files are not in the plan"),
        Action::FetchDirect => {
            let raw = conn.fetch(&current.direct, current.direct_len, progress)?;
            patch_client::decode_direct_payload(&raw)
                .map_err(|e| format!("{}: {e}", current.direct))?
        }
        Action::ApplyDeltas { from } => {
            let mut data =
                fs::read(root.join(&history.path)).map_err(|e| format!("{}: {e}", history.path))?;
            let mut done = 0u64;
            for v in &history.versions[*from..] {
                let delta = v
                    .indirect
                    .as_ref()
                    .expect("plan only chains versions with deltas");
                let base = done;
                let raw = conn.fetch(&delta.path, delta.len, &mut |n| progress(base + n))?;
                done += delta.len;
                data = crate::lz::apply_indirect(&raw, &data)
                    .map_err(|e| format!("{}: {e}", delta.path))?;
                if FileSig::of(&data) != v.sig {
                    return Err(format!(
                        "{}: delta {} produced the wrong content",
                        history.path, delta.path
                    ));
                }
            }
            data
        }
    };
    if FileSig::of(&data) != current.sig {
        return Err(format!(
            "{}: downloaded content does not match the manifest signature",
            history.path
        ));
    }
    Ok(data)
}

/// Connections held open against the patch server at once. Every request costs
/// a round trip (measured 240 ms for a small file against pc001.pol.com) and
/// most of the manifest's files fit in a single chunk, so the fetch is bound by
/// latency, not bandwidth, and a handful of connections is worth several times
/// the wall clock. Capped at what browsers allow themselves per host: the patch
/// server is Square Enix's, not ours to saturate.
const FETCH_CONNECTIONS: usize = 6;

/// Records the first failure; later ones are consequences of it.
fn record(failure: &Mutex<Option<String>>, e: String) {
    let mut slot = failure.lock().expect("fetch failure lock");
    slot.get_or_insert(e);
}

fn failed(failure: &Mutex<Option<String>>) -> bool {
    failure.lock().is_ok_and(|f| f.is_some())
}

/// Fetches every file in the plan over a pool of connections and returns the
/// bytes accounted for. Files are independent; only the delta chain within one
/// file is ordered, and `produce` keeps that chain on a single connection.
fn fetch_plan(
    root: &Path,
    plan: &UpdatePlan,
    by_path: &HashMap<&str, &FileHistory>,
    server: &Server,
    cancel: &Cancel,
    report: &Reporter,
) -> Result<u64, String> {
    let count = plan.files.len();
    let next = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let received = AtomicU64::new(0);
    let accounted = AtomicU64::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);

    thread::scope(|scope| {
        for _ in 0..FETCH_CONNECTIONS.min(count).max(1) {
            scope.spawn(|| {
                let mut conn = match Connection::open(server.clone()) {
                    Ok(conn) => conn,
                    Err(e) => return record(&failure, e),
                };
                loop {
                    if failed(&failure) {
                        return;
                    }
                    if cancel.is_cancelled() {
                        return record(&failure, crate::CANCELLED.to_string());
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= count {
                        return;
                    }
                    let item = &plan.files[index];
                    let history = by_path[item.path.as_str()];
                    let mut counted = 0u64;
                    let data = produce(&mut conn, root, history, &item.action, &mut |n| {
                        let delta = n.saturating_sub(counted);
                        counted = n;
                        report(Progress::UpdateBytes {
                            done: received.fetch_add(delta, Ordering::Relaxed) + delta,
                            total: plan.bytes,
                        });
                    });
                    let data = match data {
                        Ok(data) => data,
                        Err(e) => return record(&failure, e),
                    };
                    if let Err(e) = write_atomically(&root.join(&item.path), &data) {
                        return record(&failure, e);
                    }
                    accounted.fetch_add(item.bytes, Ordering::Relaxed);
                    report(Progress::UpdateFile {
                        index: completed.fetch_add(1, Ordering::Relaxed),
                        count,
                        path: item.path.clone(),
                        bytes: item.bytes,
                    });
                }
            });
        }
    });
    match failure.into_inner().expect("fetch failure lock") {
        Some(e) => Err(e),
        None => Ok(accounted.load(Ordering::Relaxed)),
    }
}

/// Percentage points of disagreement between an install's code and its data
/// that mark it as not Square Enix's. Measured against the manifest of
/// 2026-09-04: a retail install patched to current scores 100.0/100.0 (gap
/// 0.0); HorizonXI's client has retail-era data but three replaced executables
/// (84.2/97.2, gap 13.0); an Ashenbubs client has retail executables over
/// heavily modified data (100.0/41.9, gap 58.1). A retail install carrying a
/// few user-modified files moves one side by a point or two, well inside this.
const LINEAGE_DISAGREEMENT_LIMIT: f64 = 8.0;

/// Refuses an install whose code and data come from different lineages, which
/// is what a private server's client looks like from here: patching it toward
/// retail would overwrite exactly the files that make it work. Skipped for an
/// install PlayOnline left unpatched, since Square Enix's own base image
/// predates most of the manifest's history and so matches little of it - the
/// one case where a low score is expected and harmless.
fn refuse_foreign_lineage(root: &Path, plan: &UpdatePlan) -> Result<(), String> {
    if local_version(root).is_none() {
        return Ok(());
    }
    let lineage = &plan.lineage;
    if lineage.disagreement() <= LINEAGE_DISAGREEMENT_LIMIT {
        return Ok(());
    }
    Err(format!(
        "{} does not look like a Square Enix install: {}/{} of its executables and {}/{} of its \
         data files match a version the patch server knows ({:.1}% against {:.1}%). A private \
         server's client is patched away from retail, and updating it here would overwrite the \
         files that make it work.",
        root.display(),
        lineage.code_recognized,
        lineage.code_present,
        lineage.content_recognized,
        lineage.content_present,
        lineage.code_rate(),
        lineage.content_rate(),
    ))
}

pub fn run(
    root: &Path,
    options: Options,
    cancel: &Cancel,
    report: &Reporter,
) -> Result<Option<Outcome>, String> {
    let server = Server::for_title(TITLE_FFXI);
    let mut conn = Connection::open(server.clone())?;
    let reply = conn.session.version_check(&[])?;
    let local = local_version(root);
    report(Progress::UpdateVersion {
        local: local.clone(),
        server: reply.version.clone(),
        release_unix: reply.release_unix,
    });
    if !options.force && local.as_deref() == Some(reply.version.as_str()) {
        return Ok(None);
    }
    conn = Connection::open(server.clone())?;
    let text = conn.session.manifest()?;
    let manifest = manifest::parse(&String::from_utf8_lossy(&text))?;
    let plan = plan(root, &manifest, report)?;
    refuse_foreign_lineage(root, &plan)?;
    report(Progress::UpdatePlanned {
        files: manifest.files.len(),
        current: plan.current,
        to_fetch: plan.files.len(),
        bytes: plan.bytes,
    });
    let by_path: HashMap<&str, &FileHistory> = manifest
        .files
        .iter()
        .map(|h| (h.path.as_str(), h))
        .collect();
    let count = plan.files.len();
    let bytes = fetch_plan(root, &plan, &by_path, &server, cancel, report)?;
    write_atomically(&root.join(MANIFEST_FILE), &text)?;
    let outcome = Outcome {
        version: reply.version,
        fetched: count,
        bytes,
    };
    report(Progress::UpdateFinished {
        version: outcome.version.clone(),
        fetched: outcome.fetched,
        bytes: outcome.bytes,
        root: root.to_path_buf(),
    });
    Ok(Some(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Delta, Version};

    /// Rates measured against the 2026-09-04 manifest, whole-install.
    fn measured(code: (usize, usize), content: (usize, usize)) -> Lineage {
        Lineage {
            code_recognized: code.0,
            code_present: code.1,
            content_recognized: content.0,
            content_present: content.1,
        }
    }

    #[test]
    fn a_retail_install_agrees_with_itself() {
        let retail = measured((19, 19), (49_343, 49_343));
        assert_eq!(retail.disagreement(), 0.0);
        assert!(retail.disagreement() <= LINEAGE_DISAGREEMENT_LIMIT);
    }

    /// HorizonXI: retail-era data behind three replaced executables.
    #[test]
    fn replaced_executables_over_retail_data_are_refused() {
        let hxi = measured((16, 19), (47_099, 48_436));
        assert!(hxi.code_rate() < hxi.content_rate());
        assert!(
            hxi.disagreement() > LINEAGE_DISAGREEMENT_LIMIT,
            "gap {:.1}",
            hxi.disagreement()
        );
    }

    /// Ashenbubs: retail executables over heavily modified data.
    #[test]
    fn retail_executables_over_modified_data_are_refused() {
        let modded = measured((19, 19), (20_659, 49_343));
        assert!(modded.content_rate() < modded.code_rate());
        assert!(
            modded.disagreement() > LINEAGE_DISAGREEMENT_LIMIT,
            "gap {:.1}",
            modded.disagreement()
        );
    }

    /// A handful of user-modified data files must not read as a foreign client.
    #[test]
    fn a_few_modified_files_still_pass() {
        let modded = measured((19, 19), (49_243, 49_343));
        assert!(modded.disagreement() <= LINEAGE_DISAGREEMENT_LIMIT);
    }

    fn sig(n: i32) -> FileSig {
        FileSig {
            len: n as u64,
            byte_sum: n,
            md5_word: n,
        }
    }

    fn version(n: i32, direct_len: u64, delta: Option<u64>) -> Version {
        Version {
            stamp: format!("3026090{n}_0"),
            sig: sig(n),
            direct: format!("v{n}.slc"),
            direct_len,
            indirect: delta.map(|len| Delta {
                path: format!("v{n}.olc"),
                len,
            }),
        }
    }

    fn history(versions: Vec<Version>) -> FileHistory {
        FileHistory {
            path: "x.dat".into(),
            versions,
        }
    }

    #[test]
    fn decide_mirrors_the_viewer() {
        let h = history(vec![
            version(1, 100, None),
            version(2, 100, Some(10)),
            version(3, 100, Some(20)),
        ]);
        assert_eq!(decide(&h, None), (Action::FetchDirect, 100));
        assert_eq!(decide(&h, Some(sig(9))), (Action::FetchDirect, 100));
        assert_eq!(decide(&h, Some(sig(3))), (Action::Current, 0));
        assert_eq!(
            decide(&h, Some(sig(1))),
            (Action::ApplyDeltas { from: 1 }, 30)
        );
        assert_eq!(
            decide(&h, Some(sig(2))),
            (Action::ApplyDeltas { from: 2 }, 20)
        );
        let big = history(vec![version(1, 100, None), version(2, 25, Some(30))]);
        assert_eq!(decide(&big, Some(sig(1))), (Action::FetchDirect, 25));
        let gap = history(vec![
            version(1, 100, None),
            version(2, 100, None),
            version(3, 100, Some(1)),
        ]);
        assert_eq!(decide(&gap, Some(sig(1))), (Action::FetchDirect, 100));
    }

    #[test]
    fn decide_prefers_the_newest_matching_line() {
        let same = version(2, 100, Some(5));
        let mut dup = version(1, 100, None);
        dup.sig = same.sig;
        let h = history(vec![dup, same, version(3, 100, Some(5))]);
        assert_eq!(
            decide(&h, Some(sig(2))),
            (Action::ApplyDeltas { from: 2 }, 5)
        );
    }

    #[test]
    fn plan_scans_the_tree_and_counts_current_files() {
        let dir = std::env::temp_dir().join(format!("ffxi-install-plan-{}", std::process::id()));
        fs::create_dir_all(dir.join("ROM/1")).unwrap();
        fs::write(dir.join("ROM/1/a.DAT"), b"hello world").unwrap();
        let cur = Version {
            stamp: "30260904_1".into(),
            sig: FileSig::of(b"hello world"),
            direct: "d.slc".into(),
            direct_len: 7,
            indirect: None,
        };
        let m = Manifest {
            files: vec![
                FileHistory {
                    path: "ROM/1/a.DAT".into(),
                    versions: vec![cur.clone()],
                },
                FileHistory {
                    path: "ROM/1/missing.DAT".into(),
                    versions: vec![cur],
                },
            ],
        };
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let p = plan(&dir, &m, &move |e| sink.lock().unwrap().push(e)).unwrap();
        assert_eq!(p.current, 1);
        assert_eq!(p.files.len(), 1);
        assert_eq!(p.files[0].path, "ROM/1/missing.DAT");
        assert_eq!(p.bytes, 7);
        assert!(matches!(
            seen.lock().unwrap()[0],
            Progress::UpdateScanning { done: 0, total: 2 }
        ));
        fs::remove_dir_all(&dir).ok();
    }

    /// The scan fans out over batches and several workers; the plan it returns
    /// must still be in manifest order, or which file a resumed or reported
    /// index refers to depends on how the threads interleaved.
    #[test]
    fn a_fanned_out_scan_still_returns_the_plan_in_manifest_order() {
        let dir = std::env::temp_dir().join(format!("ffxi-install-order-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let count = SCAN_BATCH * 5 + 7;
        let files: Vec<FileHistory> = (0..count)
            .map(|i| FileHistory {
                path: format!("ROM/{i:05}.DAT"),
                versions: vec![Version {
                    stamp: "30260904_1".into(),
                    sig: sig(i as i32),
                    direct: format!("{i}.slc"),
                    direct_len: 7,
                    indirect: None,
                }],
            })
            .collect();
        let m = Manifest { files };
        let p = plan(&dir, &m, &|_| {}).unwrap();
        assert_eq!(p.files.len(), count);
        let paths: Vec<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
        let expected: Vec<&str> = m.files.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, expected);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn local_version_reads_the_manifest_stamp() {
        let dir = std::env::temp_dir().join(format!("ffxi-install-ver-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(local_version(&dir), None);
        fs::write(
            dir.join(MANIFEST_FILE),
            "file a {\n30210706_0 1 2 3 a.slc 4\n30260904_1 1 2 3 a.slc 4\n}\nend\n",
        )
        .unwrap();
        assert_eq!(local_version(&dir).as_deref(), Some("30260904_1"));
        fs::remove_dir_all(&dir).ok();
    }
}
