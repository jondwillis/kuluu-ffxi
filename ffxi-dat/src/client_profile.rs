use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

pub const FFXIMAIN_DLL: &str = "FFXiMain.dll";

/// The item DAT whose block ids tell the two known layouts apart.
const ITEM_LAYOUT_PROBE_DAT: &str = "ROM/118/106.DAT";

/// PlayOnline's per-file patch history. Every applied version update appears
/// as a `YYYYMMDD_n`-style stamp (`3` prefixed; e.g. `30230905_0` is the
/// 2023-09-05 update) at the start of a line, so the largest stamp is the
/// version this install was last patched to.
const PATCH_CFG: &str = "patch.cfg";

/// Per-byte obfuscation shared by every item DAT. POLUtils
/// Wiki/FFXIDATFileEncryption.wiki: a fixed rotate for item data.
pub(crate) const ITEM_BYTE_SHIFT: u32 = 5;

/// Retail item DAT block layouts, in release order. Each variant is a whole
/// on-disk format, so parsers dispatch on it rather than on a build date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemBlockLayout {
    /// 0xC00-byte blocks: every retail build up to the September 2026 update,
    /// and therefore every HorizonXI client. POLUtils Item.cs.
    Legacy,
    /// 0x1400-byte blocks with two extra bytes after `flags` and, for
    /// equipment, two after `races`. Windower/ResourceExtractor commit 51bef17
    /// ResourceParser.cs ParseItems (2026-09-10).
    Retail2026,
}

impl ItemBlockLayout {
    pub const ALL: [ItemBlockLayout; 2] = [ItemBlockLayout::Legacy, ItemBlockLayout::Retail2026];

    pub const fn stride(self) -> usize {
        match self {
            ItemBlockLayout::Legacy => 0xC00,
            ItemBlockLayout::Retail2026 => 0x1400,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            ItemBlockLayout::Legacy => "legacy-0xC00",
            ItemBlockLayout::Retail2026 => "retail-2026-0x1400",
        }
    }

    /// Which stride makes the first two blocks carry consecutive ids. Works on
    /// a file prefix, so callers only need the first `0x1400 + 4` bytes.
    pub fn detect(head: &[u8]) -> Option<ItemBlockLayout> {
        let id_at = |off: usize| -> Option<u32> {
            let b = head.get(off..off + 4)?;
            Some(u32::from_le_bytes([
                b[0].rotate_right(ITEM_BYTE_SHIFT),
                b[1].rotate_right(ITEM_BYTE_SHIFT),
                b[2].rotate_right(ITEM_BYTE_SHIFT),
                b[3].rotate_right(ITEM_BYTE_SHIFT),
            ]))
        };
        let base = id_at(0)?;
        Self::ALL
            .into_iter()
            .find(|layout| id_at(layout.stride()) == Some(base + 1))
    }

    pub fn probe_file(path: &Path) -> Option<ItemBlockLayout> {
        let mut f = std::fs::File::open(path).ok()?;
        let len = f.metadata().ok()?.len() as usize;
        let want = ItemBlockLayout::Retail2026.stride() + 4;
        let mut head = vec![0u8; want.min(len)];
        f.seek(SeekFrom::Start(0)).ok()?;
        f.read_exact(&mut head).ok()?;
        Self::detect(&head)
    }
}

/// A client build this tree has been verified against. Add a row when a new
/// install is measured; disassembly citations name the row's `name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownClient {
    pub name: &'static str,
    pub ffximain_sha256: &'static str,
    /// `None` for an install PlayOnline Viewer has never patched: SE's base
    /// image ships without `patch.cfg`.
    pub patch_version: Option<&'static str>,
    pub item_layout: ItemBlockLayout,
    /// Square Enix's own lineage, which the PlayOnline patch server can bring
    /// forward. A private server's pinned client is not, and patching it
    /// toward retail breaks it.
    pub retail: bool,
}

pub const KNOWN_CLIENTS: &[KnownClient] = &[
    KnownClient {
        name: "horizonxi-2023",
        ffximain_sha256: "f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c",
        patch_version: Some("30230905_0"),
        item_layout: ItemBlockLayout::Legacy,
        retail: false,
    },
    // FFXIFullSetup_US from gdl.square-enix.com (CDN Last-Modified 2019-05-10),
    // unpacked by ffxi-install; the unpatched starting point of every retail
    // install.
    KnownClient {
        name: "retail-2019-base",
        ffximain_sha256: "3da0a1e0dc897294880c0a4bf9ea0e9c580786b2d05698290e588c761a802835",
        patch_version: None,
        item_layout: ItemBlockLayout::Legacy,
        retail: true,
    },
    // retail-2019-base patched by `cargo xtask ffxi-client update` (ffxi-install's
    // PlayOnline patch client) to the server's 2026-09-04 release.
    KnownClient {
        name: "retail-2026-09",
        ffximain_sha256: "f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4",
        patch_version: Some("30260904_1"),
        item_layout: ItemBlockLayout::Retail2026,
        retail: true,
    },
];

/// What an install actually is, measured from its files. `known` is `Some`
/// only when the DLL hash matches a [`KNOWN_CLIENTS`] row; everything else is
/// probed so an unmeasured build still gets the right parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProfile {
    pub known: Option<&'static KnownClient>,
    pub ffximain_sha256: Option<String>,
    pub ffximain_len: Option<u64>,
    pub patch_version: Option<String>,
    pub item_layout: Option<ItemBlockLayout>,
}

pub fn latest_patch_version(patch_cfg: &str) -> Option<String> {
    patch_cfg
        .lines()
        .filter_map(|line| line.split_ascii_whitespace().next())
        .filter(|tok| is_patch_stamp(tok))
        .max()
        .map(str::to_owned)
}

fn is_patch_stamp(tok: &str) -> bool {
    let Some((date, seq)) = tok.split_once('_') else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && !seq.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit())
}

impl ClientProfile {
    pub fn probe(root: &Path) -> ClientProfile {
        let dll = root.join(FFXIMAIN_DLL);
        let (ffximain_sha256, ffximain_len) = match hash_file(&dll) {
            Some((hash, len)) => (Some(hash), Some(len)),
            None => (None, None),
        };
        let known = ffximain_sha256
            .as_deref()
            .and_then(|hash| KNOWN_CLIENTS.iter().find(|k| k.ffximain_sha256 == hash));
        let item_layout = ItemBlockLayout::probe_file(&root.join(ITEM_LAYOUT_PROBE_DAT))
            .or(known.map(|k| k.item_layout));
        let patch_version = std::fs::read_to_string(root.join(PATCH_CFG))
            .ok()
            .and_then(|cfg| latest_patch_version(&cfg));
        ClientProfile {
            known,
            ffximain_sha256,
            ffximain_len,
            patch_version,
            item_layout,
        }
    }

    pub fn name(&self) -> &str {
        self.known.map(|k| k.name).unwrap_or("unknown")
    }

    pub fn is_known(&self) -> bool {
        self.known.is_some()
    }
}

impl fmt::Display for ClientProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())?;
        if let Some(hash) = &self.ffximain_sha256 {
            write!(f, " ffximain={}", &hash[..12])?;
        } else {
            write!(f, " ffximain=missing")?;
        }
        match &self.patch_version {
            Some(v) => write!(f, " patch={v}")?,
            None => write!(f, " patch=unknown")?,
        }
        match self.item_layout {
            Some(layout) => write!(f, " items={}", layout.name()),
            None => write!(f, " items=unprobed"),
        }
    }
}

fn hash_file(path: &Path) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut len = 0u64;
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some((hex, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_ids(stride: usize, base: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; stride * 2 + 4];
        for (block, id) in [(0usize, base), (1, base + 1)] {
            let off = block * stride;
            for (i, b) in id.to_le_bytes().iter().enumerate() {
                bytes[off + i] = b.rotate_left(ITEM_BYTE_SHIFT);
            }
        }
        bytes
    }

    #[test]
    fn detect_picks_the_stride_whose_second_block_is_consecutive() {
        for layout in ItemBlockLayout::ALL {
            let head = encoded_ids(layout.stride(), 0x2800);
            assert_eq!(ItemBlockLayout::detect(&head), Some(layout));
        }
    }

    #[test]
    fn detect_rejects_a_stride_that_lands_between_blocks() {
        let head = encoded_ids(0x1000, 0);
        assert_eq!(ItemBlockLayout::detect(&head), None);
        assert_eq!(ItemBlockLayout::detect(&[]), None);
    }

    #[test]
    fn known_client_hashes_are_lowercase_sha256_hex() {
        for k in KNOWN_CLIENTS {
            assert_eq!(k.ffximain_sha256.len(), 64, "{}", k.name);
            assert!(
                k.ffximain_sha256
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{}",
                k.name
            );
        }
    }

    #[test]
    fn latest_patch_version_is_the_largest_leading_stamp() {
        let cfg =
            "file patch.txt {\n30020917_0 1 2 3 x\n30230905_0 1 2 3 y\n30230801_1 1 2 3 z\n}\n\
                   file ROM/0/0.DAT {\n30210706_0 5 6 7 w\n}\n";
        assert_eq!(latest_patch_version(cfg).as_deref(), Some("30230905_0"));
        assert_eq!(latest_patch_version("file x {\n}\n"), None);
    }

    /// The `retail` named target, when a developer has downloaded and updated
    /// it; skips otherwise.
    #[test]
    fn retail_target_is_a_known_client() {
        let targets = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(crate::archive::TARGETS_DIR);
        let root = crate::archive::target_install_dir(&targets, "retail");
        if !root.join(FFXIMAIN_DLL).is_file() {
            return;
        }
        let profile = ClientProfile::probe(&root);
        assert!(
            profile.is_known(),
            "retail target is not in KNOWN_CLIENTS: {profile}"
        );
        let known = profile.known.unwrap();
        assert_eq!(profile.item_layout, Some(known.item_layout), "{profile}");
        assert_eq!(
            profile.patch_version.as_deref(),
            known.patch_version,
            "{profile}"
        );
    }

    #[test]
    fn vendored_install_is_a_known_client() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let profile = ClientProfile::probe(root.root());
        assert_eq!(profile.item_layout, Some(ItemBlockLayout::Legacy));
        assert_eq!(
            profile.patch_version.as_deref(),
            profile.known.and_then(|k| k.patch_version),
            "{profile}"
        );
        assert!(
            profile.is_known(),
            "vendored install is not in KNOWN_CLIENTS: {profile}"
        );
    }
}
