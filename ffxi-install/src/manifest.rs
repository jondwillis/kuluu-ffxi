//! The patch server's file manifest, the same text the viewer keeps as
//! `patch.cfg`. Format from PlayOnlineViewer/viewer/com/polcore.dll (viewer
//! 1.18.15e, SHA-256 73b1864b...): FUN_1004011e tokenises, FUN_10040060 maps
//! the keywords, FUN_10038fed fills the per-version fields, and the worker
//! case 0xf of FUN_1003e241 computes the signature every line is checked
//! against.

use std::fs;
use std::io::Read;
use std::path::Path;

use md5::{Digest, Md5};

pub const MANIFEST_FILE: &str = "patch.cfg";
const KEYWORD_FILE: &str = "file";
const KEYWORD_END: &str = "end";
const DIRECT_SUFFIX: &str = ".slc";
const INDIRECT_SUFFIX: &str = ".olc";

/// What a manifest line asserts about a file's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileSig {
    pub len: u64,
    /// Sum of the bytes as signed 8-bit values, wrapping at 32 bits.
    pub byte_sum: i32,
    /// First four bytes of the MD5 digest, little-endian.
    pub md5_word: i32,
}

impl FileSig {
    pub fn of(data: &[u8]) -> Self {
        let mut sum = 0i32;
        for &b in data {
            sum = sum.wrapping_add(b as i8 as i32);
        }
        let d = Md5::digest(data);
        Self {
            len: data.len() as u64,
            byte_sum: sum,
            md5_word: i32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        }
    }

    pub fn of_path(path: &Path) -> Result<Self, String> {
        let mut f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut hasher = Md5::new();
        let mut sum = 0i32;
        let mut len = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f
                .read(&mut buf)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                sum = sum.wrapping_add(b as i8 as i32);
            }
            hasher.update(&buf[..n]);
            len += n as u64;
        }
        let d = hasher.finalize();
        Ok(Self {
            len,
            byte_sum: sum,
            md5_word: i32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub path: String,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub stamp: String,
    pub sig: FileSig,
    /// Server path of the whole file (`<stamp>/Direct/<path>.slc`).
    pub direct: String,
    pub direct_len: u64,
    /// Server path of the delta from the previous line, when one exists.
    pub indirect: Option<Delta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistory {
    pub path: String,
    /// Oldest first; the last line is the current version.
    pub versions: Vec<Version>,
}

impl FileHistory {
    pub fn current(&self) -> &Version {
        self.versions
            .last()
            .expect("parse keeps only histories with a version line")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub files: Vec<FileHistory>,
}

impl Manifest {
    pub fn latest_stamp(&self) -> Option<&str> {
        self.files
            .iter()
            .flat_map(|f| f.versions.iter().map(|v| v.stamp.as_str()))
            .max()
    }
}

fn parse_version_line(line: &str) -> Option<Version> {
    let mut it = line.splitn(5, ' ');
    let stamp = it.next()?;
    let len: u64 = it.next()?.parse().ok()?;
    let byte_sum: i32 = it.next()?.parse().ok()?;
    let md5_word: i32 = it.next()?.parse().ok()?;
    let rest = it.next()?;
    let (direct, rest) = split_path(rest, DIRECT_SUFFIX)?;
    let (direct_len, rest) = split_len(rest)?;
    let indirect = if rest.is_empty() {
        None
    } else {
        let (path, rest) = split_path(rest, INDIRECT_SUFFIX)?;
        let (len, rest) = split_len(rest)?;
        if !rest.is_empty() {
            return None;
        }
        Some(Delta {
            path: path.to_string(),
            len,
        })
    };
    Some(Version {
        stamp: stamp.to_string(),
        sig: FileSig {
            len,
            byte_sum,
            md5_word,
        },
        direct: direct.to_string(),
        direct_len,
        indirect,
    })
}

/// Paths may contain spaces, so a path token ends at its suffix followed by a
/// space.
fn split_path<'a>(rest: &'a str, suffix: &str) -> Option<(&'a str, &'a str)> {
    let marker = format!("{suffix} ");
    let end = rest.find(&marker)? + suffix.len();
    Some((&rest[..end], &rest[end + 1..]))
}

fn split_len(rest: &str) -> Option<(u64, &str)> {
    let (tok, rest) = match rest.find(' ') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    Some((tok.parse().ok()?, rest))
}

pub fn parse(text: &str) -> Result<Manifest, String> {
    let mut files = Vec::new();
    let mut open: Option<FileHistory> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let lineno = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        match &mut open {
            None => {
                if line == KEYWORD_END {
                    break;
                }
                let path = line
                    .strip_prefix(KEYWORD_FILE)
                    .and_then(|s| s.strip_prefix(' '))
                    .and_then(|s| s.strip_suffix(" {"))
                    .ok_or_else(|| {
                        format!("line {lineno}: expected `file <path> {{`, got {line:?}")
                    })?;
                open = Some(FileHistory {
                    path: path.to_string(),
                    versions: Vec::new(),
                });
            }
            Some(history) => {
                if line == "}" {
                    let history = open.take().unwrap();
                    if !history.versions.is_empty() {
                        files.push(history);
                    }
                } else {
                    let v = parse_version_line(line)
                        .ok_or_else(|| format!("line {lineno}: bad version line {line:?}"))?;
                    history.versions.push(v);
                }
            }
        }
    }
    if open.is_some() {
        return Err("manifest ends inside a `file` block".to_string());
    }
    Ok(Manifest { files })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_sig_matches_the_viewer_arithmetic() {
        let s = FileSig::of(b"hello world");
        assert_eq!(s.len, 11);
        assert_eq!(s.byte_sum, 1116);
        assert_eq!(s.md5_word, -1153714594);
        let all: Vec<u8> = (0..=255).collect();
        let s = FileSig::of(&all);
        assert_eq!(s.byte_sum, -128);
        assert_eq!(s.md5_word, -614086430);
    }

    #[test]
    fn of_path_equals_of_bytes() {
        let dir = std::env::temp_dir().join(format!("ffxi-install-sig-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(&p, &data).unwrap();
        assert_eq!(FileSig::of_path(&p).unwrap(), FileSig::of(&data));
        fs::remove_dir_all(&dir).ok();
    }

    const SAMPLE: &str = "file patch.txt {\n\
        30020917_0 833977 62439499 -1977655372 30020917_0/Direct/patch.txt.slc 372355\n\
        30210706_0 2882587 211117850 -1802613669 30210706_0/Direct/patch.txt.slc 1404313 30210706_0/Indirect/patch.txt.olc 1096290\n\
        }\n\
        \n\
        file Tools/FINAL FANTASY XI Config.chm {\n\
        30020917_0 301451 -56402 -579227117 30020917_0/Direct/Tools/FINAL FANTASY XI Config.chm.slc 293861\n\
        30210706_0 189885 6264 152620282 30210706_0/Direct/Tools/FINAL FANTASY XI Config.chm.slc 182506 30210706_0/Indirect/Tools/FINAL FANTASY XI Config.chm.olc 204184\n\
        }\n\
        file empty.dat {\n\
        }\n\
        end\n\
        file ignored/after/end {\n";

    #[test]
    fn parses_blocks_paths_with_spaces_and_stops_at_end() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.files[0].path, "patch.txt");
        assert_eq!(m.files[0].versions.len(), 2);
        assert_eq!(m.files[0].versions[0].indirect, None);
        let cur = m.files[0].current();
        assert_eq!(cur.stamp, "30210706_0");
        assert_eq!(
            cur.sig,
            FileSig {
                len: 2882587,
                byte_sum: 211117850,
                md5_word: -1802613669
            }
        );
        assert_eq!(cur.direct_len, 1404313);
        assert_eq!(
            cur.indirect,
            Some(Delta {
                path: "30210706_0/Indirect/patch.txt.olc".into(),
                len: 1096290
            })
        );
        let chm = &m.files[1];
        assert_eq!(chm.path, "Tools/FINAL FANTASY XI Config.chm");
        assert_eq!(
            chm.current().direct,
            "30210706_0/Direct/Tools/FINAL FANTASY XI Config.chm.slc"
        );
        assert_eq!(
            chm.current().indirect.as_ref().unwrap().path,
            "30210706_0/Indirect/Tools/FINAL FANTASY XI Config.chm.olc"
        );
        assert_eq!(m.latest_stamp(), Some("30210706_0"));
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse("file a {\nnot a version line\n}\n").is_err());
        assert!(parse("file a {\n").is_err());
        assert!(parse("30020917_0 1 2 3 x.slc 4\n").is_err());
        assert!(parse("file a {\n30020917_0 1 2 3 a.slc 4 a.olc 5 extra\n}\n").is_err());
    }
}
