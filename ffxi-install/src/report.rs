//! Terminal rendering of [`Progress`], shared by every CLI that drives this
//! crate so their output cannot drift.

use std::io::Write;

use crate::Progress;

pub fn print_progress(p: Progress) {
    use Progress::*;
    match p {
        VolumeCached { index } => println!("volume {index}: already downloaded"),
        VolumeDownloading { index, url } => println!("volume {index}: downloading {url}"),
        VolumeReady {
            index,
            complete_members,
        } => println!("volume {index}: ready, {complete_members} member(s) complete"),
        MemberExtracting { name } => println!("  extracting {name} ..."),
        MemberExtracted { name, millis } => println!("  extracted {name} ({millis} ms)"),
        CabDecoding { name, files } => println!("  decoding {name}: {files} file(s)"),
        CabProgress { name, done, files } => println!("  decoding {name}: {done}/{files}"),
        CabDecoded {
            name,
            new_files,
            millis,
        } => println!("  decoded {name}: {new_files} new file(s) ({millis} ms)"),
        FilesOutsideInstallIgnored { msi, count } => {
            println!("  {msi}: {count} file(s) outside SquareEnix/ ignored")
        }
        MsiPlaced { msi, files } => println!("placed {files} file(s) from {msi}"),
        Finished { files, target_root } => {
            println!("unpacked {files} file(s) into {}", target_root.display())
        }
        UpdateVersion {
            local,
            server,
            release_unix,
        } => println!(
            "server version {server} (released {}); local {}",
            unix_date(release_unix),
            local.as_deref().unwrap_or("unversioned")
        ),
        UpdateScanning { done, total } => println!("checking local files: {done}/{total}"),
        UpdatePlanned {
            files,
            current,
            to_fetch,
            bytes,
        } => println!(
            "{files} file(s) in manifest: {current} current, {to_fetch} to fetch ({} MB)",
            bytes / 1_000_000
        ),
        UpdateFile {
            index,
            count,
            path,
            bytes,
        } => println!("[{}/{count}] {path} ({bytes} bytes)", index + 1),
        UpdateBytes { done, total } => {
            print!("\r  {} / {} MB", done / 1_000_000, total / 1_000_000);
            std::io::stdout().flush().ok();
        }
        UpdateFinished {
            version,
            fetched,
            bytes,
            root,
        } => println!(
            "\nupdated {} to {version}: {fetched} file(s), {} MB",
            root.display(),
            bytes / 1_000_000
        ),
    }
}

/// `YYYY-MM-DD` of a unix timestamp; the patch server's release time only
/// needs day precision.
pub fn unix_date(secs: u32) -> String {
    const DAY: u64 = 86_400;
    let is_leap = |y: u64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = secs as u64 / DAY;
    let (mut y, mut rem) = (1970u64, days);
    loop {
        let len = if is_leap(y) { 366 } else { 365 };
        if rem < len {
            break;
        }
        rem -= len;
        y += 1;
    }
    let months = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0;
    while rem >= months[m] {
        rem -= months[m];
        m += 1;
    }
    format!("{y}-{:02}-{:02}", m + 1, rem + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_date_known_points() {
        assert_eq!(unix_date(0), "1970-01-01");
        assert_eq!(unix_date(951_782_400), "2000-02-29");
        assert_eq!(unix_date(1_788_518_731), "2026-09-04");
    }
}
