//! Bring an installed client to the patch server's current version the way
//! PlayOnline Viewer does, minus the viewer: version check, manifest, then
//! per file either the current `.slc`/zlib image or the chain of `.olc`
//! deltas, each result verified against its manifest signature. Decision
//! rule from PlayOnlineViewer/viewer/com/polcore.dll (viewer 1.18.15e,
//! SHA-256 73b1864b...) FUN_1003219e case 0x44c.

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{self, FileHistory, FileSig, Manifest, MANIFEST_FILE};
use crate::patch_client::{self, Server, Session, TITLE_FFXI};
use crate::{Progress, Reporter};

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

pub fn plan(root: &Path, manifest: &Manifest, report: &Reporter) -> Result<UpdatePlan, String> {
    let mut plan = UpdatePlan::default();
    let total = manifest.files.len();
    for (i, history) in manifest.files.iter().enumerate() {
        if i % SCAN_REPORT_EVERY == 0 {
            report(Progress::UpdateScanning { done: i, total });
        }
        let local_path = root.join(&history.path);
        let local = if local_path.is_file() {
            Some(FileSig::of_path(&local_path)?)
        } else {
            None
        };
        let (action, bytes) = decide(history, local);
        if action == Action::Current {
            plan.current += 1;
        } else {
            plan.bytes += bytes;
            plan.files.push(FilePlan {
                path: history.path.clone(),
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

pub fn run(root: &Path, options: Options, report: &Reporter) -> Result<Option<Outcome>, String> {
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
    conn = Connection::open(server)?;
    let text = conn.session.manifest()?;
    let manifest = manifest::parse(&String::from_utf8_lossy(&text))?;
    let plan = plan(root, &manifest, report)?;
    report(Progress::UpdatePlanned {
        files: manifest.files.len(),
        current: plan.current,
        to_fetch: plan.files.len(),
        bytes: plan.bytes,
    });
    let by_path: std::collections::HashMap<&str, &FileHistory> = manifest
        .files
        .iter()
        .map(|h| (h.path.as_str(), h))
        .collect();
    let count = plan.files.len();
    let mut bytes = 0u64;
    for (index, item) in plan.files.iter().enumerate() {
        report(Progress::UpdateFile {
            index,
            count,
            path: item.path.clone(),
            bytes: item.bytes,
        });
        let history = by_path[item.path.as_str()];
        let base = bytes;
        let total = plan.bytes;
        let data = produce(&mut conn, root, history, &item.action, &mut |n| {
            report(Progress::UpdateBytes {
                done: base + n,
                total,
            })
        })?;
        write_atomically(&root.join(&item.path), &data)?;
        bytes += item.bytes;
    }
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
