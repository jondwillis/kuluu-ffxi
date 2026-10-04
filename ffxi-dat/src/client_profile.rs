use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

pub const FFXIMAIN_DLL: &str = "FFXiMain.dll";

/// The item DAT whose block ids tell the two known layouts apart.
const ITEM_LAYOUT_PROBE_FILE_ID: u32 = crate::item_dat::ITEM_DAT_GENERAL;

/// PlayOnline's per-file patch history. Every applied version update appears
/// as a `YYYYMMDD_n`-style stamp (`3` prefixed; e.g. `30230905_0` is the
/// 2023-09-05 update) at the start of a line, so the largest stamp is the
/// version this install was last patched to.
pub const PATCH_CFG: &str = "patch.cfg";

/// PlayOnline's per-file content ledger, which SE's installer keeps and ours does not write. One
/// `<checksum>:<size>:<path>` line per tracked file, closed by a bare `::`. Its 22-character checksum
/// is an encoding this tree has not reversed, so the ledger is read for presence only.
pub const PATCH_LEDGER_FILE: &str = "patch.txt";

/// The same ledger frozen when the base image was installed. Measured on a retail-2026-09 tree: its
/// 61,601 entries all appear in [`PATCH_LEDGER_FILE`] with identical checksum and size, which
/// carries 1,828 more - hence probe prefers the patch ledger.
pub const BASE_LEDGER_FILE: &str = "file.txt";

/// Which content ledger an install carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLedger {
    PatchTxt,
    FileTxt,
}

impl ContentLedger {
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::PatchTxt => PATCH_LEDGER_FILE,
            Self::FileTxt => BASE_LEDGER_FILE,
        }
    }
}

/// A content ledger read off disk: which file it came from and how many entries it lists. Patching
/// an install adds entries, so callers assert the source, never an exact count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentLedgerProbed {
    pub source: ContentLedger,
    pub entries: u32,
}

impl ContentLedgerProbed {
    /// The install's content ledger, preferring the patch ledger. `None` when it carries neither.
    pub fn probe(root: &Path) -> Option<Self> {
        let read = |name: &'static str, source| {
            let text = std::fs::read_to_string(root.join(name)).ok()?;
            Some(Self {
                source,
                entries: count_ledger_entries(&text),
            })
        };
        read(PATCH_LEDGER_FILE, ContentLedger::PatchTxt)
            .or_else(|| read(BASE_LEDGER_FILE, ContentLedger::FileTxt))
    }
}

/// How many ledger entries a file lists. Blank lines and the bare `::` terminator are not
/// entries; SE writes these files with CRLF, so trailing whitespace is trimmed before matching.
fn count_ledger_entries(ledger: &str) -> u32 {
    ledger
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let Some((checksum, rest)) = line.split_once(':') else {
                return false;
            };
            let Some((size, path)) = rest.split_once(':') else {
                return false;
            };
            !checksum.is_empty() && !path.is_empty() && size.parse::<u64>().is_ok()
        })
        .count() as u32
}

/// What the install a [`KnownClient`] row was measured on carries for SE's content ledger.
/// `Unmeasured` is reserved for builds whose tree nobody has examined here; it asserts nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerExpectation {
    Unmeasured,
    Absent,
    Present(ContentLedger),
}

/// Per-byte obfuscation shared by every item DAT. POLUtils
/// Wiki/FFXIDATFileEncryption.wiki: a fixed rotate for item data.
pub const ITEM_BYTE_SHIFT: u32 = 5;

/// Retail item DAT block layouts, in release order. Each variant is a whole
/// on-disk format, so parsers dispatch on it rather than on a build date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemBlockLayout {
    /// 0xC00-byte blocks: [`KNOWN_CLIENTS`] horizonxi-2023 and
    /// retail-2019-base. POLUtils Item.cs.
    Legacy,
    /// 0x1400-byte blocks. Relative to `Legacy`: a reserved u16 follows
    /// `flags`, so stack/type/resource/targets and every tail shift by 2; an
    /// equipment block inserts a second reserved u16 after `races`, so `jobs`
    /// and the rest of the equipment tail shift by 4; a general or currency
    /// block gains a trailing reserved u16 before its string table; a usable
    /// block's last tail field narrows from u32 to u16. The icon offset is
    /// unchanged and the extra 0x800 bytes are trailing pad. Measured on every
    /// block of every item DAT of [`KNOWN_CLIENTS`] horizonxi-2023 (0xC00) and
    /// retail-2026-09 (0x1400); Windower/ResourceExtractor ResourceParser.cs
    /// `ParseItems` is the secondary hypothesis source.
    Retail2026,
}

/// Decoded value of the last byte of every real block on both measured
/// layouts; a stride that lands elsewhere reads a zero pad or icon byte.
pub const ITEM_BLOCK_TRAILER: u8 = 0xFF;

impl ItemBlockLayout {
    pub const ALL: [ItemBlockLayout; 2] = [ItemBlockLayout::Legacy, ItemBlockLayout::Retail2026];

    pub const fn stride(self) -> usize {
        match self {
            ItemBlockLayout::Legacy => 0xC00,
            ItemBlockLayout::Retail2026 => 0x1400,
        }
    }

    /// Bytes inserted after `flags`, before the rest of the common header.
    pub const fn header_shift(self) -> usize {
        match self {
            ItemBlockLayout::Legacy => 0,
            ItemBlockLayout::Retail2026 => 2,
        }
    }

    /// Bytes inserted after `races` in an equipment block, before `jobs`.
    pub const fn races_gap(self) -> usize {
        match self {
            ItemBlockLayout::Legacy => 0,
            ItemBlockLayout::Retail2026 => 2,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            ItemBlockLayout::Legacy => "legacy-0xC00",
            ItemBlockLayout::Retail2026 => "retail-2026-0x1400",
        }
    }

    /// Which stride ends the first block on [`ITEM_BLOCK_TRAILER`] and starts
    /// the second on the consecutive id, or on an all-zero id when the file
    /// holds a single real block (the currency DAT). Works on a file prefix,
    /// so callers only need the first `ItemBlockLayout::Retail2026.stride() + 4` bytes.
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
        Self::ALL.into_iter().find(|layout| {
            let stride = layout.stride();
            let trailer = head
                .get(stride - 1)
                .map(|b| b.rotate_right(ITEM_BYTE_SHIFT));
            let second = id_at(stride);
            trailer == Some(ITEM_BLOCK_TRAILER)
                && (second == Some(base.wrapping_add(1)) || second == Some(0))
        })
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

/// What a build's code section unpacks to. `FFXiMain.dll` ships `.text` with a
/// zero raw size and the code LZSS-packed in a `POL1` section, so a disassembly
/// citation can only be checked against the inflated image ([`crate::pol1`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnpackedText {
    /// SHA-256 of the unpacked bytes alone, with no PE wrapper around them.
    pub sha256: &'static str,
    /// `.text` VirtualSize, which is exactly how long the unpacked image is.
    pub virtual_size: u32,
    /// `AddressOfEntryPoint`: the POL1 unpacker stub, which runs before any
    /// game code and inflates `.text` in place.
    pub pol1_stub_rva: u32,
}

/// A client build this tree has been verified against. Add a row when a new
/// install is measured; disassembly citations name the row's `name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownClient {
    pub name: &'static str,
    pub ffximain_sha256: &'static str,
    /// `None` on a row whose `.text` nobody has unpacked, so no citation into
    /// its code can be pinned.
    pub unpacked_text: Option<UnpackedText>,
    /// `None` when the install carries no `patch.cfg` stamp: SE's base image
    /// ships without one, and a POL-patched install can lack it too. A tree SE patched itself
    /// records its history in [`PATCH_LEDGER_FILE`], which holds no stamp at all (measured on
    /// retail-2026-09: 63,428 ledger lines, zero `YYYYMMDD_n` tokens), so a stamped row can still
    /// probe to `None` without that being a parse failure.
    pub patch_version: Option<&'static str>,
    /// What the install this row was measured on carries for content ledgers. Stamp presence is
    /// not part of a build's identity here: an SE-installed tree and one patched by our own tool
    /// share one [`Self::ffximain_sha256`] and differ only in which of `patch.cfg` / `patch.txt`
    /// they own.
    pub ledger: LedgerExpectation,
    pub item_layout: ItemBlockLayout,
    /// Square Enix's own lineage, which the PlayOnline patch server can bring
    /// forward. A private server's pinned client is not, and patching it
    /// toward retail breaks it.
    pub retail: bool,
    pub offhand_model_rule: Option<OffhandModelRule>,
}

// .agents/skills/retail-observe/references/2026-10-04-dam0-offhand-context.md Offhand appearance classification is build-scoped
const OFFHAND_EXCLUDED_MODEL_RANGES: [(u16, u16); 4] =
    [(0, 63), (117, 143), (471, 511), (640, 703)];
const RETAIL_2026_OFFHAND_EXCLUDED_MODEL_RANGE: (u16, u16) = (1180, 1195);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffhandModelRule {
    Horizon2023,
    Retail2026,
}

impl OffhandModelRule {
    pub fn qualifies(self, model: u16) -> bool {
        !OFFHAND_EXCLUDED_MODEL_RANGES
            .iter()
            .any(|&(first, last)| (first..=last).contains(&model))
            && (self != Self::Retail2026
                || !(RETAIL_2026_OFFHAND_EXCLUDED_MODEL_RANGE.0
                    ..=RETAIL_2026_OFFHAND_EXCLUDED_MODEL_RANGE.1)
                    .contains(&model))
    }
}

pub const KNOWN_CLIENTS: &[KnownClient] = &[
    KnownClient {
        name: "horizonxi-2023",
        ffximain_sha256: "f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c",
        unpacked_text: Some(UnpackedText {
            sha256: "f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527",
            virtual_size: 0x0032_30BE,
            pol1_stub_rva: 0x00BA_B4B0,
        }),
        patch_version: Some("30230905_0"),
        ledger: LedgerExpectation::Unmeasured,
        item_layout: ItemBlockLayout::Legacy,
        retail: false,
        offhand_model_rule: Some(OffhandModelRule::Horizon2023),
    },
    // FFXIFullSetup_US from gdl.square-enix.com (CDN Last-Modified 2019-05-10),
    // unpacked by ffxi-install (ffxi-install/src/lib.rs unpack_cab); the
    // unpatched starting point of every retail install.
    KnownClient {
        name: "retail-2019-base",
        ffximain_sha256: "3da0a1e0dc897294880c0a4bf9ea0e9c580786b2d05698290e588c761a802835",
        unpacked_text: None,
        patch_version: None,
        ledger: LedgerExpectation::Unmeasured,
        item_layout: ItemBlockLayout::Legacy,
        retail: true,
        offhand_model_rule: None,
    },
    // retail-2019-base patched to the server's 2026-09-04 release by the
    // PlayOnline patch client (ffxi-install/src/patch_client.rs), driven by
    // `cargo run -p kuluu -- install update`.
    KnownClient {
        name: "retail-2026-09",
        ffximain_sha256: "f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4",
        unpacked_text: Some(UnpackedText {
            sha256: "b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9",
            virtual_size: 0x0032_75EE,
            pol1_stub_rva: 0x00BE_4A50,
        }),
        patch_version: Some("30260904_1"),
        // Measured in place on an SE-installed retail-2026-09 tree (patch.txt, 63,428 entries, no
        // patch.cfg), so the stamp above describes this build's other lineage.
        ledger: LedgerExpectation::Present(ContentLedger::PatchTxt),
        item_layout: ItemBlockLayout::Retail2026,
        retail: true,
        offhand_model_rule: Some(OffhandModelRule::Retail2026),
    },
    // Square Enix retail PlayOnline install (polboot.exe + patch.txt, no
    // patch.cfg stamp): the developer's Phoenix bundle, measured in place.
    KnownClient {
        name: "phoenix-bundle",
        ffximain_sha256: "6f8844eb7f0380f30a3db2fc3c435e1145f5c450bdd0999133cc75c516ec3c3b",
        unpacked_text: None,
        patch_version: None,
        ledger: LedgerExpectation::Unmeasured,
        item_layout: ItemBlockLayout::Legacy,
        retail: true,
        offhand_model_rule: None,
    },
];

/// What an install actually is, measured from its files. `known` is `Some`
/// only when the DLL hash matches a [`KNOWN_CLIENTS`] row; everything else is
/// probed so an unmeasured build still gets the right parsers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientProfile {
    pub known: Option<&'static KnownClient>,
    pub ffximain_sha256: Option<String>,
    pub ffximain_len: Option<u64>,
    pub patch_version: Option<String>,
    /// Whether the install carries `patch.cfg`, which is what makes a stamp knowable at all.
    pub patch_cfg_present: bool,
    /// SE's per-file content ledger if the install has one. For an SE-installed tree, which owns
    /// no `patch.cfg`, this is the only on-disk identity fact beyond the DLL hash.
    pub content_ledger: Option<ContentLedgerProbed>,
    pub item_layout: Option<ItemBlockLayout>,
}

/// The install's latest patch stamp alone, for callers that need the version
/// string the client puts on the wire without hashing FFXiMain.dll.
pub fn patch_version_at(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join(PATCH_CFG))
        .ok()
        .and_then(|cfg| latest_patch_version(&cfg))
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
    /// For a caller holding only an install path - the installer and launcher
    /// run before any VTABLE/FTABLE is loaded, and on a tree that may still be
    /// unpacking - so the layout probe reads the era ROM path of the
    /// general-item DAT rather than resolving its file id.
    pub fn probe(root: &Path) -> ClientProfile {
        Self::probe_with(
            root,
            &root.join(crate::item_dat::ITEM_DAT_GENERAL_ERA_ROM_PATH),
        )
    }

    /// Overlay-aware: [`crate::DatRoot::open`] calls this once its tables are
    /// loaded, so the layout is read from whichever file the install places the
    /// general-item DAT at - the same file [`crate::item_dat`] parses.
    pub fn probe_in(root: &crate::DatRoot) -> ClientProfile {
        match root.resolve(ITEM_LAYOUT_PROBE_FILE_ID) {
            Ok(loc) => Self::probe_with(root.root(), &loc.path_under(root)),
            Err(_) => Self::probe(root.root()),
        }
    }

    fn probe_with(root: &Path, item_layout_dat: &Path) -> ClientProfile {
        let dll = root.join(FFXIMAIN_DLL);
        let (ffximain_sha256, ffximain_len) = match hash_file(&dll) {
            Some((hash, len)) => (Some(hash), Some(len)),
            None => (None, None),
        };
        let known = ffximain_sha256
            .as_deref()
            .and_then(|hash| KNOWN_CLIENTS.iter().find(|k| k.ffximain_sha256 == hash));
        let item_layout =
            ItemBlockLayout::probe_file(item_layout_dat).or(known.map(|k| k.item_layout));
        let patch_version = patch_version_at(root);
        let patch_cfg_present = root.join(PATCH_CFG).is_file();
        let content_ledger = ContentLedgerProbed::probe(root);
        ClientProfile {
            known,
            ffximain_sha256,
            ffximain_len,
            patch_version,
            patch_cfg_present,
            content_ledger,
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
            None if self.patch_cfg_present => write!(f, " patch=cfg-unreadable")?,
            None => write!(f, " patch=none")?,
        }
        match self.content_ledger {
            Some(ledger) => write!(
                f,
                " ledger={}({})",
                ledger.source.file_name(),
                ledger.entries
            )?,
            None => write!(f, " ledger=none")?,
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

    // .agents/skills/retail-observe/references/2026-10-04-dam0-offhand-context.md Offhand appearance classification is build-scoped
    #[test]
    fn known_offhand_rules_preserve_the_measured_build_difference() {
        const OFFHAND_BUILD_DIFFERENCE_PINNED_MODEL: u16 = 1190;
        for client in KNOWN_CLIENTS {
            if let Some(rule) = client.offhand_model_rule {
                assert_eq!(
                    rule.qualifies(OFFHAND_BUILD_DIFFERENCE_PINNED_MODEL),
                    !client.retail,
                    "{} must retain its measured offhand rule",
                    client.name
                );
                assert!(!rule.qualifies(0), "no offhand model must not qualify");
            }
        }
    }

    fn encoded_ids(stride: usize, base: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; stride * 2 + 4];
        for (block, id) in [(0usize, base), (1, base + 1)] {
            let off = block * stride;
            for (i, b) in id.to_le_bytes().iter().enumerate() {
                bytes[off + i] = b.rotate_left(ITEM_BYTE_SHIFT);
            }
            bytes[off + stride - 1] = ITEM_BLOCK_TRAILER.rotate_left(ITEM_BYTE_SHIFT);
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
        let hex = |hash: &str, name: &str| {
            assert_eq!(hash.len(), Sha256::output_size() * 2, "{name}");
            assert!(
                hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{name}"
            );
        };
        for k in KNOWN_CLIENTS {
            hex(k.ffximain_sha256, k.name);
            if let Some(text) = k.unpacked_text {
                hex(text.sha256, k.name);
                assert_ne!(text.virtual_size, 0, "{}", k.name);
                assert_ne!(text.pol1_stub_rva, 0, "{}", k.name);
            }
        }
    }

    #[test]
    fn ledger_entries_ignore_crlf_blanks_and_the_terminator() {
        let ledger = "zE@hAtxgipi@VtcqejniQt:631040:ROM/32/13.DAT\r\n\
                      u_95SatcqHHiXE5UZ81zIt:2901584:FFXiMain.dll\r\n\
                      \r\n::";
        assert_eq!(count_ledger_entries(ledger), 2);
        assert_eq!(count_ledger_entries(""), 0);
    }

    #[test]
    fn a_ledger_line_is_never_read_as_a_patch_stamp() {
        assert_eq!(
            latest_patch_version("zE@hAtxgipi@VtcqejniQt:631040:ROM/32/13.DAT"),
            None
        );
    }

    #[test]
    fn latest_patch_version_is_the_largest_leading_stamp() {
        let cfg =
            "file patch.txt {\n30020917_0 1 2 3 x\n30230905_0 1 2 3 y\n30230801_1 1 2 3 z\n}\n\
                   file ROM/0/0.DAT {\n30210706_0 5 6 7 w\n}\n";
        assert_eq!(latest_patch_version(cfg).as_deref(), Some("30230905_0"));
        assert_eq!(latest_patch_version("file x {\n}\n"), None);
    }

    /// The install registered as `retail`, when a developer has downloaded
    /// and updated it; skips otherwise.
    #[test]
    fn retail_install_is_a_known_client() {
        let Some(root) = crate::install::named("retail") else {
            return;
        };
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
        assert!(
            profile.is_known(),
            "vendored install is not in KNOWN_CLIENTS: {profile}"
        );
        assert_eq!(
            profile.item_layout,
            profile.known.map(|k| k.item_layout),
            "{profile}"
        );
        // Absent `patch.cfg` means the install keeps its history in the ledger and knows no stamp;
        // present, a failed read is the scanner regression this catches.
        if profile.patch_cfg_present {
            assert_eq!(
                profile.patch_version.as_deref(),
                profile.known.and_then(|k| k.patch_version),
                "{profile}"
            );
        } else {
            assert_eq!(
                profile.patch_version, None,
                "stamp with no patch.cfg: {profile}"
            );
        }
        match profile.known.map(|k| k.ledger) {
            None | Some(LedgerExpectation::Unmeasured) => {}
            Some(LedgerExpectation::Absent) => assert!(
                profile.content_ledger.is_none(),
                "row says no ledger, probed {:?}: {profile}",
                profile.content_ledger
            ),
            Some(LedgerExpectation::Present(expected)) => {
                let ledger = profile.content_ledger.unwrap_or_else(|| {
                    panic!("expected a {} ledger: {profile}", expected.file_name())
                });
                assert_eq!(ledger.source, expected, "{profile}");
                assert_ne!(ledger.entries, 0, "{profile}");
            }
        }
    }

    #[test]
    fn every_item_dat_probes_to_the_profile_layout() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let profile = ClientProfile::probe_in(&root);
        for file_id in crate::item_dat::ITEM_DAT_FILE_IDS {
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let path = loc.path_under(&root);
            if !path.is_file() {
                continue;
            }
            assert_eq!(
                ItemBlockLayout::probe_file(&path),
                profile.item_layout,
                "file id {file_id}: {profile}"
            );
        }
    }
}
