//! Native unpack of Square Enix's full FFXI installer: no Wine, no
//! `FFXISetup.exe`. The CDN ships five RAR5 volumes (part1 is a WinRAR SFX
//! stub in front of the first volume) holding WiX MSIs plus their CAB
//! payloads. Volumes are downloaded in order and each cabinet is decompressed
//! as soon as the volume that completes it lands, so the LZX decode overlaps
//! the download; the MSI File/Directory tables (last volume) then place the
//! staged members under `SquareEnix/`. Progress goes to a caller-supplied
//! sink so a CLI and the launcher UI render the same events.

pub mod lz;
pub mod manifest;
pub mod patch_client;
pub mod polp;
pub mod report;
pub mod update;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

pub const CDN_BASE: &str = "https://gdl.square-enix.com/ffxi/download";
pub const VOLUME_COUNT: usize = 5;
const MSI_DIRS: [&str; 2] = ["PlayOnline", "FINAL_FANTASY_XI"];
/// The install root every MSI directory chain passes through; everything
/// above it (`Program Files`, `PlayOnline`) is the installer's choice, not
/// the client's layout.
const SQUARE_ENIX_DIR: &str = "SquareEnix";
const STAGING_DIR: &str = ".staging";
const MSI_TABLE_FILE: &str = "File";
const MSI_TABLE_COMPONENT: &str = "Component";
const MSI_TABLE_DIRECTORY: &str = "Directory";
const MSI_TABLE_MEDIA: &str = "Media";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    VolumeCached {
        index: usize,
    },
    VolumeDownloading {
        index: usize,
        url: String,
    },
    VolumeReady {
        index: usize,
        complete_members: usize,
    },
    MemberExtracting {
        name: String,
    },
    MemberExtracted {
        name: String,
        millis: u128,
    },
    CabDecoding {
        name: String,
        files: usize,
    },
    CabProgress {
        name: String,
        done: usize,
        files: usize,
    },
    CabDecoded {
        name: String,
        new_files: usize,
        millis: u128,
    },
    FilesOutsideInstallIgnored {
        msi: String,
        count: usize,
    },
    MsiPlaced {
        msi: String,
        files: usize,
    },
    Finished {
        files: usize,
        target_root: PathBuf,
    },
    UpdateVersion {
        local: Option<String>,
        server: String,
        release_unix: u32,
    },
    UpdateScanning {
        done: usize,
        total: usize,
    },
    UpdatePlanned {
        files: usize,
        current: usize,
        to_fetch: usize,
        bytes: u64,
    },
    UpdateFile {
        index: usize,
        count: usize,
        path: String,
        bytes: u64,
    },
    UpdateBytes {
        done: u64,
        total: u64,
    },
    UpdateFinished {
        version: String,
        fetched: usize,
        bytes: u64,
        root: PathBuf,
    },
}

pub type Reporter = dyn Fn(Progress) + Send + Sync;

pub struct Region {
    pub tag: &'static str,
    pub sub: &'static str,
}

pub fn region(name: &str) -> Result<Region, String> {
    match name {
        "us" => Ok(Region {
            tag: "FFXIFullSetup_US",
            sub: "us",
        }),
        "eu" => Ok(Region {
            tag: "FFXIFullSetup_EU",
            sub: "eu",
        }),
        other => Err(format!("unknown --region `{other}` (use us or eu)")),
    }
}

pub fn volume_name(tag: &str, index: usize) -> String {
    if index == 1 {
        format!("{tag}.part1.exe")
    } else {
        format!("{tag}.part{index}.rar")
    }
}

// --- RAR5 header scan (std only): which members each volume completes ---
// Format: rarlab.com/technote.htm "RAR 5.0 archive format". Every header is
// `crc32 u32, size vint, type vint, flags vint, [extra vint], [data vint], ...`;
// file headers (type 2) carry `file_flags, unpacked, attrs, [mtime], [crc],
// compression, host, name_len, name`. Header flag 0x10 = continues in the
// next volume.

const RAR5_SIGNATURE: &[u8] = b"Rar!\x1a\x07\x01\x00";
const RAR5_HEADER_FILE: u64 = 2;
const RAR5_HEADER_END: u64 = 5;
const RAR5_HFLAG_EXTRA: u64 = 0x1;
const RAR5_HFLAG_DATA: u64 = 0x2;
const RAR5_HFLAG_SPLIT_AFTER: u64 = 0x10;
const RAR5_FFLAG_MTIME: u64 = 0x2;
const RAR5_FFLAG_CRC: u64 = 0x4;
/// An SFX stub precedes the signature in part1; the stub here is ~300 KiB.
const SFX_SCAN_LIMIT: usize = 8 << 20;

#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub continues: bool,
}

fn vint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let c = *b.get(*i)?;
        *i += 1;
        v |= u64::from(c & 0x7f) << shift;
        shift += 7;
        if c & 0x80 == 0 {
            return Some(v);
        }
    }
}

fn find_signature(f: &mut fs::File) -> Result<u64, String> {
    let mut head = vec![0u8; SFX_SCAN_LIMIT];
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let n = f.read(&mut head).map_err(|e| e.to_string())?;
    head.truncate(n);
    head.windows(RAR5_SIGNATURE.len())
        .position(|w| w == RAR5_SIGNATURE)
        .map(|p| p as u64)
        .ok_or_else(|| "no RAR5 signature in the first 8 MiB".to_string())
}

/// The file members whose headers appear in this volume, in archive order.
pub fn scan_volume(path: &Path) -> Result<Vec<Member>, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut pos = find_signature(&mut f)? + RAR5_SIGNATURE.len() as u64;
    let mut members = Vec::new();
    loop {
        f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        let mut probe = [0u8; 16];
        let n = f.read(&mut probe).map_err(|e| e.to_string())?;
        if n < 8 {
            break;
        }
        let mut i = 4;
        let Some(hsize) = vint(&probe[..n], &mut i) else {
            break;
        };
        let body_start = i;
        let mut hdr = vec![0u8; body_start + hsize as usize];
        f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
        let mut i = body_start;
        let htype = vint(&hdr, &mut i).ok_or("truncated header")?;
        let hflags = vint(&hdr, &mut i).ok_or("truncated header")?;
        if hflags & RAR5_HFLAG_EXTRA != 0 {
            vint(&hdr, &mut i).ok_or("truncated header")?;
        }
        let data = if hflags & RAR5_HFLAG_DATA != 0 {
            vint(&hdr, &mut i).ok_or("truncated header")?
        } else {
            0
        };
        if htype == RAR5_HEADER_FILE {
            let fflags = vint(&hdr, &mut i).ok_or("truncated file header")?;
            vint(&hdr, &mut i).ok_or("truncated file header")?;
            vint(&hdr, &mut i).ok_or("truncated file header")?;
            if fflags & RAR5_FFLAG_MTIME != 0 {
                i += 4;
            }
            if fflags & RAR5_FFLAG_CRC != 0 {
                i += 4;
            }
            vint(&hdr, &mut i).ok_or("truncated file header")?;
            vint(&hdr, &mut i).ok_or("truncated file header")?;
            let name_len = vint(&hdr, &mut i).ok_or("truncated file header")? as usize;
            let name = hdr
                .get(i..i + name_len)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .ok_or("truncated file name")?;
            members.push(Member {
                name,
                continues: hflags & RAR5_HFLAG_SPLIT_AFTER != 0,
            });
        }
        if htype == RAR5_HEADER_END {
            break;
        }
        pos += hdr.len() as u64 + data;
    }
    Ok(members)
}

// --- download ---

pub fn is_complete_download(url: &str, dest: &Path) -> bool {
    let Ok(meta) = fs::metadata(dest) else {
        return false;
    };
    let Ok(out) = Command::new("curl").args(["-sIL", url]).output() else {
        return false;
    };
    let head = String::from_utf8_lossy(&out.stdout);
    head.lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .filter_map(|(_, v)| v.trim().parse::<u64>().ok())
        .next_back()
        .is_some_and(|len| len == meta.len())
}

fn curl(url: &str, dest: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["-L", "--fail", "--retry", "3", "-C", "-", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("running curl: {e}"))?;
    if !status.success() {
        return Err(format!("curl failed for {url} ({status})"));
    }
    Ok(())
}

// --- RAR member extraction ---

fn extract_members(
    part1: &Path,
    wanted: &[String],
    out_dir: &Path,
    report: &Reporter,
) -> Result<(), String> {
    let mut remaining: Vec<&String> = wanted.iter().collect();
    let mut archive = unrar::Archive::new(part1)
        .open_for_processing()
        .map_err(|e| format!("opening {}: {e}", part1.display()))?;
    while !remaining.is_empty() {
        let Some(header) = archive.read_header().map_err(|e| e.to_string())? else {
            break;
        };
        let name = header.entry().filename.to_string_lossy().into_owned();
        archive = if let Some(pos) = remaining.iter().position(|w| **w == name) {
            remaining.remove(pos);
            report(Progress::MemberExtracting { name: name.clone() });
            let t = Instant::now();
            let next = header
                .extract_with_base(out_dir)
                .map_err(|e| format!("extracting {name}: {e}"))?;
            report(Progress::MemberExtracted {
                name: name.clone(),
                millis: t.elapsed().as_millis(),
            });
            next
        } else {
            header.skip().map_err(|e| format!("skipping {name}: {e}"))?
        };
    }
    if !remaining.is_empty() {
        return Err(format!("archive has no member(s) {remaining:?}"));
    }
    Ok(())
}

// --- CAB decode into the staging area, keyed by member name ---

/// How many progress events a cabinet decode emits, spread over its members.
const CAB_PROGRESS_STEPS: usize = 20;

fn unpack_cab(cab_path: &Path, staging: &Path, report: &Reporter) -> Result<usize, String> {
    let f = fs::File::open(cab_path).map_err(|e| format!("{}: {e}", cab_path.display()))?;
    let mut cabinet = cab::Cabinet::new(f)
        .map_err(|e| format!("{} is not a cabinet: {e}", cab_path.display()))?;
    // (folder index, folder-relative offset, size, name), in decode order: a
    // folder is one compression stream, so its members are read front to back.
    let mut entries: Vec<(usize, u64, u64, String)> = cabinet
        .folder_entries()
        .enumerate()
        .flat_map(|(fi, folder)| {
            folder.file_entries().map(move |e| {
                (
                    fi,
                    u64::from(e.uncompressed_offset()),
                    u64::from(e.uncompressed_size()),
                    e.name().to_string(),
                )
            })
        })
        .collect();
    entries.sort();
    fs::create_dir_all(staging).map_err(|e| e.to_string())?;
    let cab_name = file_name(cab_path);
    report(Progress::CabDecoding {
        name: cab_name.clone(),
        files: entries.len(),
    });
    let step = (entries.len() / CAB_PROGRESS_STEPS).max(1);
    let mut written = 0usize;
    let mut done = 0usize;
    let folder_count = cabinet.folder_entries().count();
    for fi in 0..folder_count {
        let folder_entries: Vec<&(usize, u64, u64, String)> =
            entries.iter().filter(|(f, ..)| *f == fi).collect();
        let staged =
            |size: u64, name: &str| fs::metadata(staging.join(name)).is_ok_and(|m| m.len() == size);
        let needs_decode = folder_entries
            .iter()
            .any(|(_, _, size, name)| !staged(*size, name));
        let mut reader = if needs_decode {
            Some(
                cabinet
                    .read_folder(fi)
                    .map_err(|e| format!("folder {fi} in {}: {e}", cab_path.display()))?,
            )
        } else {
            None
        };
        for (_, offset, size, name) in folder_entries {
            if done > 0 && done.is_multiple_of(step) {
                report(Progress::CabProgress {
                    name: cab_name.clone(),
                    done,
                    files: entries.len(),
                });
            }
            done += 1;
            if staged(*size, name) {
                continue;
            }
            let dest = staging.join(name);
            let reader = reader.as_mut().ok_or("folder reader missing")?;
            reader
                .seek_to_uncompressed_offset(*offset)
                .map_err(|e| format!("{name} in {}: {e}", cab_path.display()))?;
            let mut out =
                fs::File::create(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            let copied = std::io::copy(&mut reader.take(*size), &mut out)
                .map_err(|e| format!("{name}: {e}"))?;
            if copied != *size {
                return Err(format!(
                    "{name} in {}: expected {size} bytes, decoded {copied}",
                    cab_path.display()
                ));
            }
            written += 1;
        }
    }
    Ok(written)
}

// --- MSI-driven placement ---

struct MsiLayout {
    /// File key -> destination relative to the target root (`SquareEnix/...`).
    files: BTreeMap<String, PathBuf>,
    /// Embedded cabinet stream names (`#name` in the Media table).
    embedded_cabs: Vec<String>,
}

fn long_name(default_dir: &str) -> &str {
    default_dir
        .rsplit_once('|')
        .map_or(default_dir, |(_, long)| long)
}

fn read_msi_layout(msi_path: &Path, report: &Reporter) -> Result<MsiLayout, String> {
    let mut pkg = msi::open(msi_path).map_err(|e| format!("{}: {e}", msi_path.display()))?;
    let mut dirs: HashMap<String, (Option<String>, String)> = HashMap::new();
    for row in pkg
        .select_rows(msi::Select::table(MSI_TABLE_DIRECTORY))
        .map_err(|e| e.to_string())?
    {
        let key = row[0].as_str().unwrap_or_default().to_string();
        let parent = row[1].as_str().map(str::to_string);
        let name = long_name(row[2].as_str().unwrap_or_default()).to_string();
        dirs.insert(key, (parent, name));
    }
    let dir_path = |key: &str| -> Option<PathBuf> {
        let mut chain = Vec::new();
        let mut cur = key.to_string();
        while let Some((parent, name)) = dirs.get(&cur) {
            chain.push(name.clone());
            match parent {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        chain.reverse();
        let start = chain.iter().position(|n| n == SQUARE_ENIX_DIR)?;
        Some(chain[start..].iter().collect())
    };
    let mut components: HashMap<String, String> = HashMap::new();
    for row in pkg
        .select_rows(msi::Select::table(MSI_TABLE_COMPONENT))
        .map_err(|e| e.to_string())?
    {
        components.insert(
            row[0].as_str().unwrap_or_default().to_string(),
            row[2].as_str().unwrap_or_default().to_string(),
        );
    }
    let mut files = BTreeMap::new();
    let mut skipped = 0usize;
    for row in pkg
        .select_rows(msi::Select::table(MSI_TABLE_FILE))
        .map_err(|e| e.to_string())?
    {
        let key = row[0].as_str().unwrap_or_default().to_string();
        let component = row[1].as_str().unwrap_or_default();
        let name = long_name(row[2].as_str().unwrap_or_default()).to_string();
        let Some(dir) = components.get(component).and_then(|d| dir_path(d)) else {
            skipped += 1;
            continue;
        };
        files.insert(key, dir.join(name));
    }
    if skipped > 0 {
        report(Progress::FilesOutsideInstallIgnored {
            msi: file_name(msi_path),
            count: skipped,
        });
    }
    let mut embedded_cabs = Vec::new();
    for row in pkg
        .select_rows(msi::Select::table(MSI_TABLE_MEDIA))
        .map_err(|e| e.to_string())?
    {
        if let Some(cab) = row[3].as_str() {
            if let Some(stream) = cab.strip_prefix('#') {
                embedded_cabs.push(stream.to_string());
            }
        }
    }
    Ok(MsiLayout {
        files,
        embedded_cabs,
    })
}

fn extract_embedded_cab(msi_path: &Path, stream: &str, dest: &Path) -> Result<(), String> {
    let mut pkg = msi::open(msi_path).map_err(|e| format!("{}: {e}", msi_path.display()))?;
    let mut reader = pkg
        .read_stream(stream)
        .map_err(|e| format!("stream {stream} in {}: {e}", msi_path.display()))?;
    let mut out = fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
    Ok(())
}

fn place_files(layout: &MsiLayout, staging: &Path, target_root: &Path) -> Result<usize, String> {
    let mut placed = 0usize;
    let mut missing = Vec::new();
    for (key, rel) in &layout.files {
        let src = staging.join(key);
        if !src.is_file() {
            missing.push(key.clone());
            continue;
        }
        let dest = target_root.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        if fs::rename(&src, &dest).is_err() {
            fs::copy(&src, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            fs::remove_file(&src).ok();
        }
        placed += 1;
    }
    if !missing.is_empty() {
        return Err(format!(
            "{} member(s) named by the MSI were not in any cabinet, e.g. {:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        ));
    }
    Ok(placed)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

// --- the pipeline ---

enum Event {
    VolumeReady(usize),
    DownloadFailed(String),
    CabDone(Result<(PathBuf, usize), String>),
}

pub struct Plan<'a> {
    pub region: Region,
    /// Where the CDN volumes and extracted archives live (resumable cache).
    pub installer_dir: &'a Path,
    /// Receives `SquareEnix/FINAL FANTASY XI` and `SquareEnix/PlayOnlineViewer`.
    pub target_root: &'a Path,
}

pub fn download_and_unpack(plan: &Plan, report: &Reporter) -> Result<(), String> {
    let Plan {
        region,
        installer_dir,
        target_root,
    } = plan;
    fs::create_dir_all(installer_dir).map_err(|e| format!("{}: {e}", installer_dir.display()))?;
    fs::create_dir_all(target_root).map_err(|e| format!("{}: {e}", target_root.display()))?;
    let base = format!("{CDN_BASE}/{}", region.sub);
    let volumes: Vec<(String, PathBuf)> = (1..=VOLUME_COUNT)
        .map(|i| {
            let name = volume_name(region.tag, i);
            (format!("{base}/{name}"), installer_dir.join(name))
        })
        .collect();
    let part1 = volumes[0].1.clone();
    let payload_dir = installer_dir.join(region.tag);
    let staging = target_root.join(STAGING_DIR);

    let (tx, rx) = mpsc::channel::<Event>();
    thread::scope(|scope| -> Result<(), String> {
        let downloader = {
            let tx = tx.clone();
            let volumes = volumes.clone();
            scope.spawn(move || {
                for (i, (url, dest)) in volumes.iter().enumerate() {
                    let index = i + 1;
                    if is_complete_download(url, dest) {
                        report(Progress::VolumeCached { index });
                    } else {
                        report(Progress::VolumeDownloading {
                            index,
                            url: url.clone(),
                        });
                        if let Err(e) = curl(url, dest) {
                            let _ = tx.send(Event::DownloadFailed(e));
                            return;
                        }
                    }
                    if tx.send(Event::VolumeReady(index)).is_err() {
                        return;
                    }
                }
            })
        };
        let (cab_tx, cab_rx) = mpsc::channel::<PathBuf>();
        let cab_worker = {
            let tx = tx.clone();
            let staging = staging.clone();
            scope.spawn(move || {
                for cab in cab_rx {
                    let t = Instant::now();
                    let result = unpack_cab(&cab, &staging, report).map(|n| (cab.clone(), n));
                    if let Ok((_, n)) = &result {
                        report(Progress::CabDecoded {
                            name: file_name(&cab),
                            new_files: *n,
                            millis: t.elapsed().as_millis(),
                        });
                    }
                    if tx.send(Event::CabDone(result)).is_err() {
                        return;
                    }
                }
            })
        };
        drop(tx);

        let mut cabs_pending = 0usize;
        let mut volumes_done = 0usize;
        let mut failure: Option<String> = None;
        let mut msis: Vec<PathBuf> = Vec::new();
        for event in &rx {
            match event {
                Event::DownloadFailed(e) => {
                    failure = Some(e);
                    break;
                }
                Event::VolumeReady(index) => {
                    volumes_done = index;
                    let members = scan_volume(&volumes[index - 1].1)?;
                    let wanted: Vec<String> = members
                        .iter()
                        .filter(|m| !m.continues)
                        .map(|m| m.name.clone())
                        .collect();
                    report(Progress::VolumeReady {
                        index,
                        complete_members: wanted.len(),
                    });
                    let to_extract: Vec<String> = wanted
                        .iter()
                        .filter(|name| {
                            let p = installer_dir.join(name);
                            let lower = name.to_ascii_lowercase();
                            (lower.ends_with(".cab") || lower.ends_with(".msi"))
                                && !lower.contains("/redist/")
                                && !p.is_file()
                        })
                        .cloned()
                        .collect();
                    if !to_extract.is_empty() {
                        extract_members(&part1, &to_extract, installer_dir, report)?;
                    }
                    for name in &wanted {
                        let p = installer_dir.join(name);
                        let lower = name.to_ascii_lowercase();
                        if lower.contains("/redist/") || !p.is_file() {
                            continue;
                        }
                        if lower.ends_with(".cab") {
                            cabs_pending += 1;
                            cab_tx.send(p).map_err(|e| e.to_string())?;
                        } else if lower.ends_with(".msi") {
                            msis.push(p);
                        }
                    }
                    if index == VOLUME_COUNT {
                        drop(cab_tx);
                        break;
                    }
                }
                Event::CabDone(result) => {
                    cabs_pending -= 1;
                    result?;
                }
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        if volumes_done < VOLUME_COUNT {
            return Err("download ended before every volume arrived".into());
        }
        while cabs_pending > 0 {
            match rx.recv().map_err(|e| e.to_string())? {
                Event::CabDone(result) => {
                    cabs_pending -= 1;
                    result?;
                }
                Event::DownloadFailed(e) => return Err(e),
                Event::VolumeReady(_) => {}
            }
        }
        downloader
            .join()
            .map_err(|_| "downloader thread panicked")?;
        cab_worker.join().map_err(|_| "cabinet thread panicked")?;

        let mut total = 0usize;
        for msi_path in MSI_DIRS
            .iter()
            .filter_map(|d| msis.iter().find(|m| m.starts_with(payload_dir.join(d))))
        {
            let layout = read_msi_layout(msi_path, report)?;
            for stream in &layout.embedded_cabs {
                let cab = installer_dir.join(format!(
                    "{}.{stream}",
                    msi_path.file_stem().unwrap_or_default().to_string_lossy()
                ));
                if !cab.is_file() {
                    extract_embedded_cab(msi_path, stream, &cab)?;
                }
                let t = Instant::now();
                let n = unpack_cab(&cab, &staging, report)?;
                report(Progress::CabDecoded {
                    name: stream.clone(),
                    new_files: n,
                    millis: t.elapsed().as_millis(),
                });
            }
            let placed = place_files(&layout, &staging, target_root)?;
            report(Progress::MsiPlaced {
                msi: file_name(msi_path),
                files: placed,
            });
            total += placed;
        }
        if msis.len() < MSI_DIRS.len() {
            return Err(format!(
                "expected {} MSIs under {}, found {}",
                MSI_DIRS.len(),
                payload_dir.display(),
                msis.len()
            ));
        }
        let _ = fs::remove_dir(&staging);
        report(Progress::Finished {
            files: total,
            target_root: target_root.to_path_buf(),
        });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_name_takes_the_part_after_the_pipe() {
        assert_eq!(long_name("retdx9ia|FINAL FANTASY XI"), "FINAL FANTASY XI");
        assert_eq!(long_name("db"), "db");
    }

    #[test]
    fn vint_decodes_multi_byte_values() {
        let mut i = 0;
        assert_eq!(vint(&[0x80 | 0x05, 0x01], &mut i), Some(0x85));
        assert_eq!(i, 2);
        let mut i = 0;
        assert_eq!(vint(&[0x7f], &mut i), Some(0x7f));
        let mut i = 0;
        assert_eq!(vint(&[0x80], &mut i), None);
    }

    fn write_volume(dir: &Path, name: &str, sfx: bool, entries: &[(&str, bool)]) -> PathBuf {
        let mut bytes = Vec::new();
        if sfx {
            bytes.extend_from_slice(&[0x4d, 0x5a, 0, 0]);
        }
        bytes.extend_from_slice(RAR5_SIGNATURE);
        let push_header = |bytes: &mut Vec<u8>, body: &[u8]| {
            bytes.extend_from_slice(&[0, 0, 0, 0]);
            bytes.push(body.len() as u8);
            bytes.extend_from_slice(body);
        };
        push_header(&mut bytes, &[1, 0, 0]);
        for (name, continues) in entries {
            let hflags = RAR5_HFLAG_DATA
                | if *continues {
                    RAR5_HFLAG_SPLIT_AFTER
                } else {
                    0
                };
            let mut body = vec![RAR5_HEADER_FILE as u8, hflags as u8, 3, 0, 9, 0, 0, 0];
            body.push(name.len() as u8);
            body.extend_from_slice(name.as_bytes());
            push_header(&mut bytes, &body);
            bytes.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        }
        push_header(&mut bytes, &[RAR5_HEADER_END as u8, 0]);
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn scan_volume_reports_members_and_continuation() {
        let dir = std::env::temp_dir().join(format!("xtask-rar5-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let v1 = write_volume(
            &dir,
            "a.part1.exe",
            true,
            &[("setup.exe", false), ("big.cab", true)],
        );
        let v2 = write_volume(&dir, "a.part2.rar", false, &[("big.cab", false)]);
        let m1 = scan_volume(&v1).unwrap();
        assert_eq!(
            m1.iter()
                .map(|m| (m.name.as_str(), m.continues))
                .collect::<Vec<_>>(),
            vec![("setup.exe", false), ("big.cab", true)]
        );
        let m2 = scan_volume(&v2).unwrap();
        assert_eq!(
            m2.iter()
                .map(|m| (m.name.as_str(), m.continues))
                .collect::<Vec<_>>(),
            vec![("big.cab", false)]
        );
        fs::remove_dir_all(&dir).ok();
    }
}
