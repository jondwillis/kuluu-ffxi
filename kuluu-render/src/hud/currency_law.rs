//! The Currencies windows' law, straight out of the client.
//!
//! Statcom2 rows **Currencies** (item id 6) and **Currencies 2** (item id 10) open the
//! `menu    evitem` window in mode 1 / mode 2 (FFXiMain.dll retail-2026-09 `.text 0x22A620`;
//! Key Items is the same window at mode 0 — vault main_menu.md §59 addendum 22.2/27, bead
//! kuluu-o9qt7.35). Each list is built by a fill loop over a fixed descriptor array:
//! - descriptors `.rdata 0x3862C8` (mode 1) and `.rdata 0x3865E0` (mode 2), stride 8:
//!   `{dword currency_id, dword packed}`; the LOW half of `packed` is a GetText **table 12**
//!   text id (`mov ax, word [slot*8 + 0x103862CC]` at `.text 0x229FA7`, through helper
//!   `.text 0x152EF0`) and the HIGH half is an icon glyph id — kuluu menus render no icons, so
//!   only the label id matters here. The label STRINGS come from the DAT table-12 catalog at
//!   runtime (DAT-first law); these tables carry only the ids the DLL's `.rdata` defines.
//! - display order is the descriptor array order and deliberately jumps around the id space
//!   (mode-1 slot 11 holds currency `0x5A`), so rows are never derived from ids.
//! - value: kind byte `[currency_id + .text 0x10229C38]` (mode 1, 99 B) or
//!   `[currency_id + .text 0x10229E90]` (mode 2, 97 B) → getter `.text 0x229B58[kind]` /
//!   `.text 0x229E28[kind]`. Every getter but mode-1 kind 41 reads one packet field:
//!   `pkt[mult*id + disp]` (width 1/2/4, packet-relative offsets INCLUDING the 4-byte header),
//!   optionally shifted right then mask-ANDed. Kind 41 splices two fields:
//!   `((byte pkt+0xDC & 0xF) << 5) | (dword pkt+0xD8 >>> 27)` — a 9-bit slice spanning the
//!   dword boundary, part of a five-field plan-stored bitfamily LSB declares identically as
//!   `bloodshed/umbrage/ritualistic/tutelary/primacy_plans_stored : 9`
//!   (vendor/server/src/map/packets/s2c/0x113_currencies_1.h).
//! - Nothing is cached client-side: every row select re-sends the header-only c2s request
//!   (`.text 0xFD520` / `.text 0xFD550`) and the one-shot reply handler folds s2c 0x113/0x118
//!   into the list (vault §59 addendum 27 = findings C82-1..C82-3).

/// Which `evitem` mode a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrencyMode {
    /// Statcom2 row id 6 → c2s 0x10F ↔ s2c 0x113.
    One,
    /// Statcom2 row id 10 → c2s 0x115 ↔ s2c 0x118.
    Two,
}

impl CurrencyMode {
    pub const ALL: [Self; 2] = [Self::One, Self::Two];

    /// The wire discriminant carried by `AgentCommand`/`AgentEvent` (0 = mode 1).
    pub fn as_wire(self) -> u8 {
        match self {
            Self::One => 0,
            Self::Two => 1,
        }
    }

    pub fn from_wire(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::One),
            1 => Some(Self::Two),
            _ => None,
        }
    }

    /// Descriptor rows `(currency id, GetText table-12 label text id)` in DISPLAY order.
    pub fn rows(self) -> &'static [(u8, u16)] {
        match self {
            Self::One => MODE1_ROWS,
            Self::Two => MODE2_ROWS,
        }
    }

    fn kind_by_id(self) -> &'static [u8] {
        match self {
            Self::One => MODE1_KIND_BY_ID,
            Self::Two => MODE2_KIND_BY_ID,
        }
    }

    fn getters(self) -> &'static [Getter] {
        match self {
            Self::One => &MODE1_GETTERS,
            Self::Two => &MODE2_GETTERS,
        }
    }
}

/// Mode-1 descriptors: `.rdata 0x3862C8`, 99 entries (the fill callback pushes `0x63` rows).
pub const MODE1_ROWS: &[(u8, u16)] = &[
    (0x0, 136),
    (0x1, 137),
    (0x2, 138),
    (0x3, 139),
    (0x4, 140),
    (0x5, 141),
    (0x6, 142),
    (0x7, 143),
    (0x8, 144),
    (0x9, 145),
    (0xA, 146),
    (0x5A, 473),
    (0x5B, 474),
    (0x5C, 475),
    (0x5D, 476),
    (0x5E, 477),
    (0x5F, 478),
    (0x60, 479),
    (0x61, 480),
    (0xB, 147),
    (0xC, 148),
    (0xD, 149),
    (0xE, 150),
    (0xF, 151),
    (0x10, 152),
    (0x11, 153),
    (0x12, 154),
    (0x13, 155),
    (0x14, 156),
    (0x15, 157),
    (0x16, 158),
    (0x17, 159),
    (0x18, 160),
    (0x19, 161),
    (0x1A, 162),
    (0x1B, 163),
    (0x1C, 164),
    (0x46, 213),
    (0x1D, 165),
    (0x1E, 166),
    (0x1F, 167),
    (0x20, 168),
    (0x21, 169),
    (0x22, 170),
    (0x23, 171),
    (0x24, 172),
    (0x25, 173),
    (0x40, 206),
    (0x26, 174),
    (0x3F, 205),
    (0x27, 175),
    (0x28, 176),
    (0x29, 177),
    (0x2A, 178),
    (0x62, 484),
    (0x47, 214),
    (0x58, 467),
    (0x2B, 179),
    (0x2C, 180),
    (0x2D, 181),
    (0x2E, 182),
    (0x2F, 183),
    (0x30, 184),
    (0x31, 185),
    (0x32, 186),
    (0x41, 207),
    (0x33, 187),
    (0x34, 188),
    (0x35, 189),
    (0x48, 215),
    (0x49, 216),
    (0x4A, 217),
    (0x4B, 218),
    (0x4C, 219),
    (0x36, 190),
    (0x42, 208),
    (0x37, 197),
    (0x38, 198),
    (0x39, 199),
    (0x3A, 200),
    (0x3B, 201),
    (0x3C, 202),
    (0x3D, 203),
    (0x3E, 204),
    (0x43, 210),
    (0x44, 211),
    (0x45, 212),
    (0x4D, 443),
    (0x4E, 457),
    (0x4F, 458),
    (0x50, 459),
    (0x51, 460),
    (0x52, 461),
    (0x53, 462),
    (0x54, 463),
    (0x55, 464),
    (0x56, 465),
    (0x57, 466),
    (0x59, 472),
];

/// Mode-2 descriptors: `.rdata 0x3865E0`, 97 entries (`push 0x61`).
pub const MODE2_ROWS: &[(u8, u16)] = &[
    (0x0, 191),
    (0x1, 209),
    (0x2, 192),
    (0x3, 193),
    (0x4, 194),
    (0x5, 195),
    (0x6, 445),
    (0x7, 350),
    (0x8, 351),
    (0x9, 352),
    (0xA, 353),
    (0xB, 354),
    (0xC, 355),
    (0xD, 356),
    (0xE, 357),
    (0xF, 358),
    (0x10, 359),
    (0x11, 360),
    (0x12, 361),
    (0x13, 362),
    (0x14, 363),
    (0x15, 364),
    (0x16, 365),
    (0x17, 366),
    (0x18, 367),
    (0x19, 368),
    (0x1A, 369),
    (0x1B, 370),
    (0x1C, 371),
    (0x1D, 372),
    (0x1E, 373),
    (0x1F, 374),
    (0x20, 375),
    (0x21, 376),
    (0x22, 377),
    (0x23, 378),
    (0x24, 379),
    (0x25, 380),
    (0x26, 381),
    (0x27, 382),
    (0x28, 383),
    (0x29, 384),
    (0x2A, 385),
    (0x2B, 386),
    (0x2C, 387),
    (0x2D, 388),
    (0x2E, 389),
    (0x2F, 390),
    (0x30, 391),
    (0x31, 392),
    (0x32, 393),
    (0x33, 394),
    (0x37, 196),
    (0x38, 454),
    (0x39, 455),
    (0x34, 468),
    (0x35, 469),
    (0x36, 470),
    (0x3A, 471),
    (0x3E, 485),
    (0x3F, 486),
    (0x40, 487),
    (0x41, 488),
    (0x42, 489),
    (0x43, 490),
    (0x44, 491),
    (0x45, 492),
    (0x46, 493),
    (0x47, 494),
    (0x48, 495),
    (0x49, 496),
    (0x4A, 497),
    (0x4B, 498),
    (0x4C, 499),
    (0x4D, 500),
    (0x4E, 501),
    (0x4F, 502),
    (0x50, 503),
    (0x51, 504),
    (0x52, 505),
    (0x53, 506),
    (0x54, 507),
    (0x55, 508),
    (0x56, 509),
    (0x57, 510),
    (0x3B, 481),
    (0x3C, 482),
    (0x3D, 483),
    (0x58, 511),
    (0x59, 512),
    (0x5A, 513),
    (0x5B, 514),
    (0x5C, 519),
    (0x5D, 518),
    (0x5E, 522),
    (0x5F, 523),
    (0x60, 524),
];

/// Kind byte per currency id — FFXiMain.dll retail-2026-09 `.text 0x10229C38` (mode 1) /
/// `.text 0x10229E90` (mode 2). The fill loop reads `[id + table]` and exits above id `0x62`
/// (mode 1) / `0x60` (mode 2), matching these tables' lengths to the descriptor counts.
const MODE1_KIND_BY_ID: &[u8] = &[
    0, 0, 0, 1, 1, 1, 1, 1, 2, 3, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 8, 9,
    10, 10, 10, 11, 12, 12, 12, 13, 14, 14, 15, 16, 17, 18, 19, 19, 19, 19, 19, 20, 21, 22, 23, 24,
    25, 26, 27, 28, 28, 28, 28, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44,
    44, 44, 44, 44, 44, 44, 44, 44, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55,
];

const MODE2_KIND_BY_ID: &[u8] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 8, 9, 10, 11, 12, 13, 14,
    15, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
];

/// One field read: little-endian `pkt[mult*id + disp]`, packet-relative offsets (the 4-byte
/// header counted), then optional shr / mask-AND on the same register. Kind-indexed by the two
/// getter jump tables; kinds beyond a mode's table are never reached (checked by test).
#[derive(Debug, Clone, Copy)]
struct Getter {
    width: u8,
    mult: u8,
    disp: i32,
    shift: Option<u8>,
    mask: Option<u32>,
}

const fn getter(width: u8, mult: u8, disp: i32) -> Getter {
    Getter {
        width,
        mult,
        disp,
        shift: None,
        mask: None,
    }
}

/// The mode-1 kind whose arm splices two packet fields (`.text 0x229A33`); the evaluator
/// special-cases it by number because no [`Getter`] shape can express it.
const MODE1_PLANS_SPLICE_KIND: u8 = 41;

/// Mode-1 getters, kind-indexed (jump table `.text 0x229B58`, bodies `.text 0x2297F0..0x229B4C`).
/// `disp` is packet-relative: id-scaled families carry the byte offset common to their arms.
const MODE1_GETTERS: [Getter; 56] = [
    getter(4, 4, 4),  // k0  dword pkt+id*4+4 — conquest points family (LSB +0/+4/+8 body)
    getter(2, 2, 10), // k1  word  pkt+id*2+0xA — seal/crest stored family
    getter(2, 0, 0x1A), // k2  fixed word +0x1A
    getter(2, 0, 0x1C), // k3  fixed word +0x1C
    getter(2, 0, 0x1E), // k4  fixed word +0x1E
    getter(4, 4, -12), // k5  dword pkt+id*4-0xC — guild points family (negative disp)
    getter(4, 0, 68), // k6  dword +0x44
    byte_id(0x33),    // k7  byte pkt+id+0x33 — eight-byte indexed family begins here
    getter(4, 0, 80), // k8  dword +0x50
    getter(4, 0, 84), // k9  dword +0x54
    getter(2, 2, 0x1A), // k10 word pkt+id*2+0x1A — team-word family
    getter(4, 0, 96), // k11 dword +0x60
    byte_id(0x41),    // k12 byte pkt+id+0x41
    getter(4, 0, 104), // k13 dword +0x68
    getter(2, 2, 0x1E), // k14 word pkt+id*2+0x1E
    getter(4, 0, 112), // k15 dword +0x70
    getter(4, 0, 116), // k16 dword +0x74
    getter(4, 0, 120), // k17 dword +0x78
    getter(4, 0, 124), // k18 dword +0x7C
    getter(4, 4, -52), // k19 dword pkt+id*4-0x34 — second negative-disp family
    getter(4, 0, 148), // k20 dword +0x94
    getter(4, 0, 152), // k21 dword +0x98
    getter(4, 0, 156), // k22 dword +0x9C
    getter(4, 0, 160), // k23 dword +0xA0
    getter(4, 0, 164), // k24 dword +0xA4
    getter(4, 0, 172), // k25 dword +0xAC
    getter(4, 0, 176), // k26 dword +0xB0
    getter(4, 0, 180), // k27 dword +0xB4
    byte_id(0x7E),    // k28 byte pkt+id+0x7E — five-byte indexed family
    getter(1, 0, 0xBD), // k29 byte +0xBD
    getter(1, 0, 0xCC), // k30 byte +0xCC
    getter(1, 0, 0xBE), // k31 byte +0xBE
    getter(1, 0, 0xBF), // k32 byte +0xBF
    getter(4, 0, 0xC0), // k33 dword +0xC0
    getter(4, 0, 0xC4), // k34 dword +0xC4
    getter(4, 0, 0xC8), // k35 dword +0xC8
    getter(2, 0, 0x5E), // k36 word +0x5E
    getter(2, 0, 0xA8), // k37 word +0xA8
    masked4(0xD8, None), // k38 dword +0xD8 & 0x1FF — plan-stored bitfamily (LSB `:9` fields)
    masked4(0xD8, Some(9)), // k39 same dword >>9 & 0x1FF
    masked4(0xD8, Some(18)), // k40 same dword >>18 & 0x1FF
    getter(9, 0, 0),  // k41 SPLICE — .text 0x229A33, evaluated by kind, never through Getter
    masked_word_shr9(0xDC), // k42 word +0xDC >>4 & 0x1FF — bitfamily tail
    getter(4, 0, 228), // k43 dword +0xE4
    byte_id(0x80),    // k44 byte pkt+id+0x80 — ten-byte indexed family
    getter(2, 0, 0xE0), // k45 word +0xE0
    getter(2, 0, 0xAA), // k46 word +0xAA
    masked_dword_16(232), // k47 dword +0xE8 & 0xFFFF — narrow-dword family (k47/49/51/53)
    getter(2, 0, 0xEA), // k48 word +0xEA
    masked_dword_16(236), // k49 dword +0xEC & 0xFFFF
    getter(2, 0, 0xEE), // k50 word +0xEE
    masked_dword_16(240), // k51 dword +0xF0 & 0xFFFF
    getter(2, 0, 0xF2), // k52 word +0xF2
    masked_dword_16(244), // k53 dword +0xF4 & 0xFFFF
    getter(2, 0, 0xF6), // k54 word +0xF6 — the widest mode-1 reads; packet ≥ 0xFA bytes
    getter(2, 0, 0xF8), // k55 word +0xF8
];

/// Mode-2 getters (jump table `.text 0x229E28`, bodies `.text 0x229CD0..`). No splices; the
/// widest read is dword +0x9C so mode-2 packets run shorter than mode-1's.
const MODE2_GETTERS: [Getter; 26] = [
    getter(4, 0, 4),   // k0  dword +4 — first field after the packet header
    getter(1, 0, 0xA), // k1  byte +0xA
    getter(2, 0, 8),   // k2  word +8
    getter(4, 0, 12),  // k3  dword +0xC
    getter(2, 0, 16),  // k4  word +0x10
    getter(2, 0, 18),  // k5  word +0x12
    getter(2, 0, 72),  // k6  word +0x48
    byte_id(0x11),     // k7  byte pkt+id+0x11 — the 3×stone counters' indexed bytes (57 ids)
    getter(4, 0, 20),  // k8  dword +0x14
    getter(4, 0, 76),  // k9  dword +0x4C
    getter(2, 0, 74),  // k10 word +0x4A
    getter(4, 0, 80),  // k11 dword +0x50 — hallmark dword family (four consecutive)
    getter(4, 0, 84),  // k12 dword +0x54
    getter(4, 0, 88),  // k13 dword +0x58
    getter(4, 0, 92),  // k14 dword +0x5C
    getter(4, 0, 96),  // k15 dword +0x60
    byte_id(0x25),     // k16 byte pkt+id+0x25 — "set" byte family (crystals/spheres)
    getter(4, 0, 128), // k17 dword +0x80
    getter(1, 0, 0xB), // k18 byte +0xB
    getter(4, 0, 132), // k19 dword +0x84 — domain points pair
    getter(4, 0, 136), // k20 dword +0x88
    getter(4, 0, 140), // k21 dword +0x8C — segments family begins
    getter(4, 0, 144), // k22 dword +0x90
    getter(4, 0, 148), // k23 dword +0x94
    getter(4, 0, 152), // k24 dword +0x98
    getter(4, 0, 156), // k25 dword +0x9C — max mode-2 offset
];

/// Byte read indexed by the currency id itself (`[pkt + id + disp]`).
const fn byte_id(disp: i32) -> Getter {
    getter(1, 1, disp)
}

/// Dword at `disp`, optional shr, always &0x1FF — the plan-stored bitfamily mask law.
const fn masked4(disp: i32, shift: Option<u8>) -> Getter {
    let mut g = getter(4, 0, disp);
    g.shift = shift;
    g.mask = Some(0x1ff);
    g
}

/// Word at `disp` >>4 &0x1FF (mode-1 kind 42).
const fn masked_word_shr9(disp: i32) -> Getter {
    let mut g = getter(2, 0, disp);
    g.shift = Some(4);
    g.mask = Some(0x1ff);
    g
}

/// Dword read narrowed to its low half (mode-1 kinds 47/49/51/53).
const fn masked_dword_16(disp: i32) -> Getter {
    let mut g = getter(4, 0, disp);
    g.mask = Some(0xffff);
    g
}

/// Little-endian field read at a PACKET offset over `body` (the packet WITHOUT its 4-byte
/// header — kuluu hands body slices around, so the mapping is `pkt_off - 4`).
fn read_field(body: &[u8], pkt_off: usize, width: u8) -> Option<u32> {
    let b0 = pkt_off.checked_sub(4)?;
    let slice = body.get(b0..b0.checked_add(usize::from(width))?)?;
    Some(match width {
        1 => u32::from(slice[0]),
        2 => u16::from_le_bytes([slice[0], slice[1]]) as u32,
        4 => u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]),
        _ => return None,
    })
}

/// One currency's value out of a folded raw report body (s2c 0x113/0x118 payload, header off).
/// `None` when the block is short of the field — retail has no padding fallback either: an
/// under-length report simply never reaches those getters.
pub fn currency_value(mode: CurrencyMode, id: u8, body: &[u8]) -> Option<u32> {
    let kind = *mode.kind_by_id().get(usize::from(id))?;
    if mode == CurrencyMode::One && kind == MODE1_PLANS_SPLICE_KIND {
        // .text 0x229A33: ((byte pkt+0xDC & 0xF) << 5) | (dword pkt+0xD8 >>> 27)
        let byte = read_field(body, 0xdc, 1)?;
        let dword = read_field(body, 0xd8, 4)?;
        return Some(((byte & 0xf) << 5) | (dword >> 27));
    }
    let g = mode.getters().get(usize::from(kind))?;
    let pkt_off = i64::from(g.disp) + i64::from(g.mult) * i64::from(id);
    if pkt_off < 4 {
        // A packet-relative field can never start inside the header.
        return None;
    }
    let v = read_field(body, usize::try_from(pkt_off).ok()?, g.width)?;
    let v = match g.shift {
        Some(s) => v >> u32::from(s),
        None => v,
    };
    Some(match g.mask {
        Some(m) => v & m,
        None => v,
    })
}

/// The whole list for one mode in DISPLAY order: `(label text id, value)` per descriptor — the
/// label strings are resolved from the DAT table-12 catalog by the caller.
pub fn currency_rows(mode: CurrencyMode, body: &[u8]) -> Vec<(u16, Option<u32>)> {
    mode.rows()
        .iter()
        .map(|&(id, label_id)| (label_id, currency_value(mode, id, body)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_row_with_kind(mode: CurrencyMode, kind: u8) -> u8 {
        mode.rows()
            .iter()
            .find(|(id, _)| mode.kind_by_id()[usize::from(*id)] == kind)
            .map(|r| r.0)
            .unwrap()
    }

    #[test]
    fn descriptor_tables_have_retail_sizes_and_unique_ids() {
        assert_eq!(MODE1_ROWS.len(), 99); // fill pushes 0x63 rows (.text 0x229F80 law)
        assert_eq!(MODE2_ROWS.len(), 97); // push 0x61
        let mut ids: Vec<u8> = MODE1_ROWS.iter().map(|r| r.0).collect();
        let unique = ids
            .clone()
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(ids.len(), unique);
        ids.sort();
        assert_eq!(ids.last(), Some(&0x62)); // mode-1 fill exits above 0x62
        let mut ids2: Vec<u8> = MODE2_ROWS.iter().map(|r| r.0).collect();
        let unique2 = ids2
            .clone()
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(ids2.len(), unique2);
        ids2.sort();
        assert_eq!(ids2.last(), Some(&0x60));
    }

    #[test]
    fn kind_tables_cover_every_currency_id() {
        assert_eq!(MODE1_KIND_BY_ID.len(), 99);
        assert_eq!(MODE2_KIND_BY_ID.len(), 97);
        for &(id, _) in MODE1_ROWS {
            let k = MODE1_KIND_BY_ID[usize::from(id)];
            assert!(
                (k as usize) < MODE1_GETTERS.len(),
                "mode-1 id {id:#04X} kind {k}"
            );
        }
        for &(id, _) in MODE2_ROWS {
            let k = MODE2_KIND_BY_ID[usize::from(id)];
            assert!(
                (k as usize) < MODE2_GETTERS.len(),
                "mode-2 id {id:#04X} kind {k}"
            );
        }
    }

    #[test]
    fn indexed_families_read_id_scaled_offsets() {
        // Conquest points (mode-1 ids 0..2, kind 0 dword pkt+id*4+4): packet +4/+C for ids 0/2,
        // which land at body +0/+8 once the 4-byte header is accounted for.
        let mut body = vec![0u8; 0xF6];
        body[0x0..0x4].copy_from_slice(&123i32.to_le_bytes()); // packet +4 = body +0
        body[0x8..0xC].copy_from_slice(&(-7i32).to_le_bytes()); // id 2: pkt +C = body +8, negative kept unsigned
        assert_eq!(currency_value(CurrencyMode::One, 0, &body), Some(123));
        assert_eq!(
            currency_value(CurrencyMode::One, 2, &body),
            Some(u32::from_le_bytes((-7i32).to_le_bytes()))
        );
        // Kind-1 seals/crests: word pkt+id*2+0xA → id 3 at packet +0x10 = body +0xC.
        body[0xC..0xE].copy_from_slice(&42u16.to_le_bytes());
        assert_eq!(currency_value(CurrencyMode::One, 3, &body), Some(42));
        // Kind-5 guild points: dword pkt+id*4−0xC → first kind-5 id (11) at packet +0x20 = body
        // +0x1C after the header translation.
        let guild_first_id = first_row_with_kind(CurrencyMode::One, 5);
        assert_eq!(guild_first_id, 11); // the descriptor order pins which id owns this kind
        body[0x1C..0x20].copy_from_slice(&9_999u32.to_le_bytes());
        assert_eq!(
            currency_value(CurrencyMode::One, guild_first_id, &body),
            Some(9_999)
        );
    }

    #[test]
    fn plans_bitfield_family_matches_lsb_nine_bit_slices() {
        // Kinds 38–42 read the bitfamily LSB declares as five :9 fields — cross-check that each
        // slice of one crafted dword/byte pair matches plain (word >> shift) & 0x1FF math.
        let mut body = vec![0u8; 0xF6];
        // Bits [27,32) of the base dword (packet +0xD8) carry five splice bits; low ten are
        // ordinary nine-bit-slice territory.
        let dword0: u32 = (0b11010 << 27) | 0x1E5;
        body[0xD8 - 4..0xDC - 4].copy_from_slice(&dword0.to_le_bytes());
        let find = |kind| first_row_with_kind(CurrencyMode::One, kind);
        assert_eq!(
            currency_value(CurrencyMode::One, find(38), &body),
            Some(dword0 & 0x1FF)
        );
        assert_eq!(
            currency_value(CurrencyMode::One, find(39), &body),
            Some((dword0 >> 9) & 0x1FF)
        );
        assert_eq!(
            currency_value(CurrencyMode::One, find(40), &body),
            Some((dword0 >> 18) & 0x1FF)
        );
        // k42: word +0xDC >>4 & 0x1FF. Byte +0xDC = 0xA5, its high byte (+0xDD) stays 0, so the
        // word is 0x00A5 and the law gives (0xA5>>4)&0x1FF; the low nibble of that byte is also
        // consumed by k41 below.
        body[0xDC - 4] = 0xA5;
        assert_eq!(
            currency_value(CurrencyMode::One, find(42), &body),
            Some(0xA)
        );
        // k41 splice: ((byte+0xDC & 0xF)<<5) | (dword0 >>> 27).
        let expect = (0x5u32 << 5) | (dword0 >> 27);
        assert_eq!(
            currency_value(CurrencyMode::One, find(41), &body),
            Some(expect)
        );
    }

    #[test]
    fn short_block_yields_no_value_not_a_panic() {
        let body = [0u8; 4]; // far below every field offset (mode-1 k5/k19 have negative disps too)
        assert_eq!(currency_value(CurrencyMode::One, 0x62, &body), None);
        assert_eq!(currency_value(CurrencyMode::Two, 0x60, &body), None);
    }

    #[test]
    fn mode_two_bayld_is_first_dword() {
        let mut body = vec![0u8; 160]; // packet +4 → body +0 (kind 0 dword +4)
        body[0..4].copy_from_slice(&7_000_000i32.to_le_bytes());
        assert_eq!(
            currency_value(CurrencyMode::Two, 0x0, &body),
            Some(7_000_000)
        );
    }

    #[test]
    fn rows_follow_descriptor_order_not_id_order() {
        // Mode-1 slot 11 must be currency 0x5A (label id 473), never "id 11".
        let body = vec![0u8; 250];
        let rows = currency_rows(CurrencyMode::One, &body);
        assert_eq!(rows.len(), 99);
        assert_eq!(rows[11].0, 473);
    }

    #[test]
    fn wire_discriminants_roundtrip() {
        for m in CurrencyMode::ALL {
            assert_eq!(CurrencyMode::from_wire(m.as_wire()), Some(m));
        }
        assert_eq!(CurrencyMode::from_wire(2), None);
    }
}
