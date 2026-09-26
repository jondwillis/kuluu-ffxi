//! Per-opcode metadata for the event VM, re-expressed (as our own committed
//! table) from atom0s/XiEvents `OpCodes/*.md` — a studied `research/` reference,
//! not a build input.
//!
//! - `size`: byte width to advance `ExecPointer` past an opcode the VM does not
//!   handle explicitly (only valid when `!jumps`).
//! - `jumps`: the opcode sets `ExecPointer` non-linearly (a real jump/branch);
//!   the VM must not skip an unimplemented one by size — it would desync.
//! - `sets_ret`: the opcode yields (breaks the exec loop this tick).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpMeta {
    pub size: u8,
    pub jumps: bool,
    pub sets_ret: bool,
    pub valid: bool,
}

/// Indexed by opcode value (0x00..=0xD9); every opcode in that range is defined.
pub const OPCODE_META: &[OpMeta] = &[
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0000
    OpMeta {
        size: 3,
        jumps: true,
        sets_ret: false,
        valid: true,
    }, // 0x0001
    OpMeta {
        size: 8,
        jumps: true,
        sets_ret: false,
        valid: true,
    }, // 0x0002
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0003
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0004
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0005
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0006
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0007
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0008
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0009
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x000A
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x000B
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x000C
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x000D
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x000E
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x000F
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0010
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0011
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0012
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0013
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0014
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0015
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0016
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0017
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0018
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0019
    OpMeta {
        size: 3,
        jumps: true,
        sets_ret: true,
        valid: true,
    }, // 0x001A
    OpMeta {
        size: 1,
        jumps: true,
        sets_ret: true,
        valid: true,
    }, // 0x001B
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x001C
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x001D
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x001E
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x001F
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0020
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0021
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0022
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0023
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0024
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0025
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0026
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0027
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0028
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0029
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x002A
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x002B
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x002C
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x002D
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x002E
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x002F
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0030
    OpMeta {
        size: 10,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0031
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0032
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0033
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0034
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0035
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0036
    OpMeta {
        size: 9,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0037
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0038
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0039
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x003A
    OpMeta {
        size: 11,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x003B
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x003C
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x003D
    OpMeta {
        size: 7,
        jumps: true,
        sets_ret: false,
        valid: true,
    }, // 0x003E
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x003F
    OpMeta {
        size: 9,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0040
    OpMeta {
        size: 9,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0041
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0042
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0043
    OpMeta {
        size: 5,
        jumps: true,
        sets_ret: false,
        valid: true,
    }, // 0x0044
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0045
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0046
    OpMeta {
        size: 10,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0047
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0048
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0049
    OpMeta {
        size: 9,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x004A
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x004B
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x004C
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x004D
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x004E
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x004F
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0050
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0051
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0052
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0053
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0054
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0055
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0056
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0057
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0058
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0059
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x005A
    OpMeta {
        // research/XiEvents/OpCodes/0x005B.md "Pseudo Code",144 — dispatched with param3=0,
        // so every ExecPointer path advances 15 (the +2 is param3-gated and unused
        // by 0x5B/0x66). atom0s's size table ambiguously lists "15, 17"; the
        // param3=0 call site is authoritative. Confirm against a captured event
        // stream if one containing 0x5B/0x66 becomes available.
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x005B
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x005C
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x005D
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x005E
    OpMeta {
        size: 18,
        jumps: true,
        sets_ret: true,
        valid: true,
    }, // 0x005F
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0060
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0061
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0062
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0063
    OpMeta {
        size: 11,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0064
    OpMeta {
        size: 11,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0065
    OpMeta {
        // See 0x005B: 0x0066 dispatches to the same helper with param3=0, so it
        // advances 15 too (research/XiEvents/OpCodes/0x0066.md "Pseudo Code").
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0066
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0067
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0068
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0069
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x006A
    OpMeta {
        size: 9,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x006B
    OpMeta {
        size: 9,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x006C
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x006D
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x006E
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x006F
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0070
    // Widest case is 0x20's 16, which the doc's `OpCode Size` header row omits.
    OpMeta {
        size: 16,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0071
    OpMeta {
        size: 10,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0072
    OpMeta {
        size: 11,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0073
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0074
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0075
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0076
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0077
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0078
    OpMeta {
        size: 12,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0079
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x007A
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x007B
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x007C
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x007D
    OpMeta {
        size: 18,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x007E
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x007F
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0080
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0081
    OpMeta {
        size: 7,
        jumps: true,
        sets_ret: false,
        valid: true,
    }, // 0x0082
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0083
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0084
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0085
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0086
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0087
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0088
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0089
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x008A
    OpMeta {
        size: 25,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x008B
    OpMeta {
        size: 14,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x008C
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x008D
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x008E
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x008F
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0090
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0091
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0092
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0093
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0094
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0095
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0096
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x0097
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0098
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x0099
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x009A
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x009B
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x009C
    // Widest case is 0x08's 23; `jumps` stays set because case 0x07 is a real
    // jump, so an unhandled 0x9D must never be skipped by this size.
    OpMeta {
        size: 23,
        jumps: true,
        sets_ret: false,
        valid: true,
    }, // 0x009D
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x009E
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x009F
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00A0
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A1
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A2
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A3
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A4
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A5
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00A6
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00A7
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A8
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00A9
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00AA
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00AB
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00AC
    OpMeta {
        size: 12,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00AD
    OpMeta {
        size: 10,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00AE
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00AF
    OpMeta {
        size: 12,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00B0
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00B1
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00B2
    OpMeta {
        size: 18,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00B3
    OpMeta {
        size: 20,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00B4
    OpMeta {
        size: 4,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00B5
    OpMeta {
        size: 20,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00B6
    OpMeta {
        size: 10,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00B7
    OpMeta {
        size: 27,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00B8
    OpMeta {
        size: 8,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00B9
    OpMeta {
        size: 13,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00BA
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00BB
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00BC
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00BD
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00BE
    OpMeta {
        size: 10,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00BF
    OpMeta {
        size: 3,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C0
    OpMeta {
        size: 5,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00C1
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C2
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C3
    OpMeta {
        size: 11,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C4
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C5
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00C6
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C7
    OpMeta {
        size: 7,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C8
    OpMeta {
        size: 1,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00C9
    // 0x00CA/0x00CB: deprecated, no retail handler (research/XiEvents/OpCodes/0x00CA.md, 0x00CB.md). No
    // event authors them; the VM stops on either.
    OpMeta {
        size: 0,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00CA
    OpMeta {
        size: 0,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00CB
    OpMeta {
        size: 14,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00CC
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00CD
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00CE
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00CF
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D0
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00D1
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D2
    OpMeta {
        size: 6,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D3
    OpMeta {
        size: 12,
        jumps: true,
        sets_ret: true,
        valid: true,
    }, // 0x00D4
    OpMeta {
        size: 17,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D5
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: true,
        valid: true,
    }, // 0x00D6
    OpMeta {
        size: 15,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D7
    OpMeta {
        size: 12,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D8
    OpMeta {
        size: 2,
        jumps: false,
        sets_ret: false,
        valid: true,
    }, // 0x00D9
];

/// Width of a *variable*-width opcode, selected by its sub byte
/// (`EventData[ExecPointer + 1]`).
///
/// [`OpMeta::size`] carries one number per opcode, so for these it holds the
/// widest case. Advancing by that where the data encodes a narrower one walks
/// `ExecPointer` into the middle of the next instruction and the VM then
/// executes operand bytes as opcodes — silently, because the skip looks
/// successful. `None` means the opcode is fixed-width, or that its width is not
/// modelled yet and the caller should fall back to `OpMeta::size`.
///
/// Rules transcribed from the retail pseudo-code *body* in atom0s/XiEvents
/// `OpCodes/*.md` — never its `OpCode Size` header row, which is incomplete
/// (0x0071.md lists `2,4,6,8,10` and omits its case 0x20's 16). Only opcodes
/// whose dispatch is decided by the sub byte alone appear here: 0x9D cases
/// 0x07 (a jump) and 0x0C have widths entangled with a runtime value, so they
/// are left to the wider VM work rather than guessed at.
pub fn sub_size(op: u8, sub: u8) -> Option<u8> {
    match op {
        // 0x0046.md: sub 2 reads a work offset (4); every other path, including
        // the render-flag-gated fall-through, advances 2.
        OP_DEFCAMERA => Some(if sub == 2 { 4 } else { 2 }),
        // 0x0072.md: mode 0 (the forecast read) is 4 bytes, mode 1 (the value
        // copy) is 6; an unknown mode retail spins on, so no width is encoded.
        OP_GETWEATHER => match sub {
            0 => Some(4),
            1 => Some(6),
            _ => None,
        },
        // 0x0079.md: sub 1 is lookatone with a trailing work offset (12); sub 2
        // and the zero path are both 10.
        OP_LOOKAT => Some(if sub == 1 { 12 } else { 10 }),
        // 0x005C.md: the low band 0x00-0x07 sets a song id (4); the 0x80-0x87
        // band adds a volume (6), as do 0xA0/0xA1. Any other sub returns without
        // touching ExecPointer at all — a hang, so authored data cannot contain
        // one; fall back rather than encode a width retail never advances by.
        OP_MUSIC => match sub {
            0x00..=0x07 => Some(4),
            0x80..=0x87 | 0xA0 | 0xA1 => Some(6),
            _ => None,
        },
        // 0x00C2.md: sub 1 writes a party mask (4), sub 2 a validity flag (6),
        // anything else advances 2.
        OP_MOGHOUSE_VISIT => Some(match sub {
            1 => 4,
            2 => 6,
            _ => 2,
        }),
        // 0x007E.md: the mount/dismount opcode, whose width is its case. Cases
        // 3 and 6 are wider because they carry the chocobo's CustomProperties;
        // 7 carries a mount id. Retail computes width 0 for any other case and
        // then advances by it — a hang, so authored data cannot hold one.
        OP_CHOCOBO => match sub {
            0..=2 | 4 | 5 | 8 => Some(6),
            3 => Some(16),
            6 => Some(18),
            7 => Some(8),
            _ => None,
        },
        // 0x00B6.md: the entity-look family. Cases 0x0B/0x0D/0x0E carry a whole
        // Look struct, 0x0F a model size, 0x14/0x15 an actor lookup. Case 0x10
        // advances 2 only once the player entity has finished loading; this VM
        // models no load state, so it takes the loaded path.
        OP_LOOKSET => match sub {
            0x00..=0x0A | 0x0C | 0x0F | 0x11 => Some(4),
            0x0B => Some(20),
            0x0D => Some(14),
            0x0E => Some(16),
            0x10 | 0x12 | 0x13 => Some(2),
            0x14 | 0x15 => Some(6),
            _ => None,
        },
        // 0x0047.md: case 0 sends the position tag (10); case 1 polls until the
        // server acknowledges it (2). The VM models the round-trip as a pending
        // tag held on the case-1 poll, so these widths are only the fallbacks.
        OP_EVENTPOSSET => match sub {
            0 => Some(10),
            1 => Some(2),
            _ => None,
        },
        // 0x0075.md: case 0 opens an indoor sub-region (4), case 1 polls (2),
        // case 2 is `ExecPointer -= 6` then `+= 8` on the sub-region change
        // succeeding — net +2.
        OP_LOADROOM => match sub {
            0 => Some(4),
            1 | 2 => Some(2),
            _ => None,
        },
        // 0x00CC.md: the item/search info windows.
        OP_ITEMINFO => match sub {
            0x00 | 0x01 | 0x03 => Some(10),
            0x02 => Some(14),
            0x10 => Some(6),
            0x11 | 0x20 => Some(4),
            _ => None,
        },
        // 0x0059.md: entity turn/move speed. The odd 7 is case 5's trailing
        // flag byte, the 6 case 6's actor lookup with no work operand.
        OP_ENTITYSPEED => match sub {
            0 | 2 | 7 => Some(4),
            1 | 3 | 4 | 8 => Some(8),
            5 => Some(7),
            6 => Some(6),
            _ => None,
        },
        // 0x00D4.md: cases 0 and 2 each run the 0x24 query helper (+1, then
        // the helper's +7) and case 1 is a flat +8; case 3 is +6; cases 4/5
        // are +12. An unknown case parks, so no width is encoded for it.
        OP_MAP_QUERY => match sub {
            0..=2 => Some(8),
            3 => Some(6),
            4 | 5 => Some(12),
            _ => None,
        },
        // 0x00B3.md: the ranking-board cases — 0/3/4/6/7/9 are 4, case 1 is 14,
        // case 5 is 18, cases 2/8 are 2, and retail's default arm advances 2
        // for any other sub.
        OP_RANKING => match sub {
            1 => Some(14),
            5 => Some(18),
            0 | 3 | 4 | 6 | 7 | 9 => Some(4),
            _ => Some(2),
        },
        // 0x00A7.md: case 0 arms the await and sends the tag (2), case 1
        // writes the result (4); retail spins on any other sub, so no width is
        // encoded for it.
        OP_A7_WAIT => match sub {
            0 => Some(2),
            1 => Some(4),
            _ => None,
        },
        // 0x00A6.md: cases 0/1 are the request and its poll (2 each), case 2
        // writes the answered MapNum (4); retail spins on any other case, so
        // no width is encoded for it.
        OP_A6_SUBMAP => match sub {
            0 | 1 => Some(2),
            2 => Some(4),
            _ => None,
        },
        // 0x00B2.md: mode 0 is the timed wait (4), mode 1 the delivery-mode
        // request (2); retail returns without advancing on any other mode, so
        // no width is encoded for it.
        OP_B2_DELIVERY => match sub {
            0 => Some(4),
            1 => Some(2),
            _ => None,
        },
        // 0x0087.md, 0x0088.md: the world-pass send cases (0/2) and the poll
        // (1) are all 2 bytes; retail spins on any other case, so no width is
        // encoded for it.
        OP_FRIENDPASS_87 | OP_FRIENDPASS_88 => match sub {
            0..=2 => Some(2),
            _ => None,
        },
        // 0x008C.md: case 0 is 8, the poll 2, case 2 is 12, cases 3/4 are
        // 10, case 5 is 14; retail spins on any other case, so no width is
        // encoded for it.
        OP_RECIPE => match sub {
            0 => Some(8),
            1 => Some(2),
            2 => Some(12),
            3 | 4 => Some(10),
            5 => Some(14),
            _ => None,
        },
        // 0x001F.md: case 0 sets the goal position (8); case 1 re-runs each
        // frame while the entity walks and advances 2 on arrival. No frame
        // clock here, so case 1 arrives immediately.
        OP_MOVE => match sub {
            0 => Some(8),
            1 => Some(2),
            _ => None,
        },
        // 0x005A.md CodeMOVE2: the uncalibrated twin of 0x1F — case 0 stores
        // the goal in ReqStack (8); case 1 re-runs each frame while the entity
        // walks and advances 2 on arrival.
        OP_CODE_MOVE2 => match sub {
            0 => Some(8),
            1 => Some(2),
            _ => None,
        },
        // 0x00B4.md: the event window family. Cases 0x00/0x13 carry a 16-byte
        // string, 0x14 five work operands, 0x05/0x06 a trailing key byte.
        OP_WINDOW => match sub {
            0x00 | 0x13 => Some(20),
            0x01 | 0x02 | 0x04 | 0x0F | 0x10 | 0x11 | 0x12 => Some(6),
            0x03 | 0x08 | 0x0B | 0x0D | 0x0E | 0x15 => Some(2),
            0x05 | 0x06 => Some(3),
            0x07 | 0x09 | 0x0A | 0x0C => Some(4),
            0x14 => Some(12),
            _ => None,
        },
        // 0x0071.md: the event menu family. 0x11 falls through to 0x13 in the
        // C switch; case 0x20 reads seven work operands.
        OP_MENU => match sub {
            0x00 | 0x01 | 0x02 | 0x21 | 0x51 | 0x53 => Some(2),
            0x03 | 0x10 | 0x11 | 0x13 | 0x30 | 0x31 | 0x40 | 0x50 | 0x52 | 0x55 => Some(4),
            0x12 | 0x32 => Some(6),
            0x41 => Some(8),
            0x54 => Some(10),
            0x20 => Some(16),
            _ => None,
        },
        // 0x00AB.md: render-flag toggles on the event entity; 0x1B/0x1C take an
        // actor lookup instead.
        OP_RENDERFLAG => match sub {
            0x00..=0x10 | 0x12 | 0x13 | 0x19 | 0x1A => Some(2),
            0x11 | 0x14..=0x18 => Some(4),
            0x1B | 0x1C => Some(6),
            _ => None,
        },
        // 0x007A.md: event-VM reset / ExtData sharing. Case 2 advances in retail
        // only when the target actor resolves AND owns an event pointer; this VM
        // resolves no actors, so advancing 6 is a deliberate divergence — the
        // faithful alternative is a spin the host cannot break.
        OP_REQRESET => match sub {
            0 | 2 | 5 => Some(6),
            1 => Some(7),
            3 => Some(2),
            4 => Some(8),
            _ => None,
        },
        // 0x00B5.md: sub 0 renames the event entity (4); any other sub sets
        // RetFlag without advancing, so authored data cannot hold one.
        OP_NAMESET => match sub {
            0 => Some(4),
            _ => None,
        },
        // 0x005F.md: each case steps one byte then delegates to another opcode
        // handler, so the widths are 1 + the delegate's: 0xC1's 5, 0x5B's 15/17,
        // 0x53's 13.
        OP_SUBSCHED => match sub {
            0 | 1 => Some(2),
            2 => Some(6),
            3 | 4 => Some(16),
            5 | 6 => Some(18),
            7 => Some(14),
            _ => None,
        },
        // 0x00AC.md: entity status/render-flag writes; cases 2-4 take an actor
        // lookup instead of the event entity.
        OP_STATUSSET => match sub {
            0 | 1 => Some(4),
            2 | 3 => Some(6),
            4 => Some(8),
            _ => None,
        },
        // 0x00B7.md: the event-entity work/name family. Case 0 writes a work
        // slot behind a two-actor lookup and advances 10 whether or not the
        // lookup resolves; cases 1-4 advance 8 on every path. Retail spins on
        // any other sub, so no width is encoded for it.
        OP_B7 => match sub {
            0x00 => Some(10),
            0x01..=0x04 => Some(8),
            _ => None,
        },
        // 0x00D8.md: the EventDir writes. Case 0 copies the entity's last
        // position and case 4 writes all three components (12); cases 1-3
        // write one component each (8); an unlisted sub falls through to the
        // trailing +6. The actor-lookup failure path also advances 6, so the
        // success-path widths are the faithful choice, the same divergence the
        // 0x7A case-2 arm takes.
        OP_D8 => match sub {
            0x00 => Some(6),
            0x01..=0x03 => Some(8),
            0x04 => Some(12),
            _ => Some(6),
        },
        // 0x0031.md SMOVE: case 0 stores the goal and moves (10); case 1
        // re-runs each frame while the entity walks and advances 2 on arrival,
        // the same poll shape as 0x1F/0x5A case 1.
        OP_SMOVE => match sub {
            0 => Some(10),
            1 => Some(2),
            _ => None,
        },
        // 0x00AE.md: the weather / name-color / entity-link family. Every case
        // advances by a fixed amount on both its failure and success paths:
        // the weather set (0) and the pointer clear (6) by 6, the name-color
        // (1, 2) and Mou4 (3, 4) cases by 8, and the pointer copy (5) and
        // EnvironmentAreaId (7, 8) cases by 10. Retail spins on any other sub,
        // so no width is encoded for it.
        OP_AE => match sub {
            0x00 | 0x06 => Some(6),
            0x01..=0x04 => Some(8),
            0x05 | 0x07 | 0x08 => Some(10),
            _ => None,
        },
        // 0x009D.md: the string/indirect-work family. The cases that consult
        // `PTR_Ptr_Work_Zone` (0x0D, 0x0E) take the branch for an unpopulated
        // slot, and the string-compare cases (0x08, 0x09) take their
        // fall-through, because this VM models neither store. Case 0x07 is a
        // real jump and 0x0C's width depends on a runtime value, so both are
        // absent and keep stopping the VM.
        OP_STRINGOPS => match sub {
            0x00 | 0x01 | 0x03 | 0x04 | 0x05 | 0x06 | 0x0E => Some(8),
            0x02 => Some(6),
            0x08 => Some(23),
            0x09 => Some(9),
            0x0A | 0x0B | 0x0D | 0x0F | 0x10 => Some(10),
            _ => None,
        },
        _ => None,
    }
}

/// Sub-cases of the window/menu families that retail *polls*: they set `RetFlag`
/// and return with `ExecPointer` untouched until the player has typed a value,
/// picked an entry, or the server has answered, and only then advance while
/// storing the answer into a work slot. Nothing in this VM can satisfy them — it
/// has no text-input, linkshell-menu or search-window surface — so advancing by
/// [`sub_size`] would run the script on as if the player had answered, with the
/// destination work slot holding 0. The VM stops on them instead, which the host
/// already handles by ending the event rather than rendering a wrong frame
/// (research/XiEvents/OpCodes/0x0071.md, 0x00CC.md, 0x00B4.md).
pub fn is_input_wait(op: u8, sub: u8) -> bool {
    match op {
        // 0x01/0x02 poll the password window and the server's reply; 0x11/0x13/
        // 0x31 the numeric/string input buffer; 0x41 the Linkshell Concierge
        // choice. Excludes 0x53 and 0xB4/0x15: those hold only while their UI
        // object EXISTS and advance when it is absent (0x0071.md case 0x53,
        // 0x00B4.md case 0x15), and this VM never creates one, so retail's own
        // answer here is the unconditional advance.
        OP_MENU => matches!(sub, 0x01 | 0x02 | 0x11 | 0x13 | 0x31 | 0x41),
        // 0x11 polls the search-menu zone window opened by case 0x10 and stores
        // the picked zone.
        OP_ITEMINFO => sub == 0x11,
        _ => false,
    }
}

const OP_DEFCAMERA: u8 = 0x46;
const OP_GETWEATHER: u8 = 0x72;
const OP_LOOKAT: u8 = 0x79;
const OP_MUSIC: u8 = 0x5C;
const OP_MOGHOUSE_VISIT: u8 = 0xC2;
const OP_CHOCOBO: u8 = 0x7E;
pub(crate) const OP_LOOKSET: u8 = 0xB6;
pub(crate) const OP_EVENTPOSSET: u8 = 0x47;
pub(crate) const OP_LOADROOM: u8 = 0x75;
pub(crate) const OP_ITEMINFO: u8 = 0xCC;
pub(crate) const OP_ENTITYSPEED: u8 = 0x59;
pub(crate) const OP_MOVE: u8 = 0x1F;
pub(crate) const OP_CODE_MOVE2: u8 = 0x5A;
pub(crate) const OP_WINDOW: u8 = 0xB4;
pub(crate) const OP_MENU: u8 = 0x71;
pub(crate) const OP_RENDERFLAG: u8 = 0xAB;
pub(crate) const OP_REQRESET: u8 = 0x7A;
pub(crate) const OP_NAMESET: u8 = 0xB5;
pub(crate) const OP_SUBSCHED: u8 = 0x5F;
pub(crate) const OP_STRINGOPS: u8 = 0x9D;
const OP_AE: u8 = 0xAE;
const OP_B7: u8 = 0xB7;
const OP_D8: u8 = 0xD8;
const OP_SMOVE: u8 = 0x31;
pub(crate) const OP_STATUSSET: u8 = 0xAC;
pub(crate) const OP_MAP_QUERY: u8 = 0xD4;
pub(crate) const OP_RANKING: u8 = 0xB3;
pub(crate) const OP_A7_WAIT: u8 = 0xA7;
pub(crate) const OP_A6_SUBMAP: u8 = 0xA6;
pub(crate) const OP_B2_DELIVERY: u8 = 0xB2;
pub(crate) const OP_FRIENDPASS_87: u8 = 0x87;
pub(crate) const OP_FRIENDPASS_88: u8 = 0x88;
pub(crate) const OP_RECIPE: u8 = 0x8C;

#[cfg(test)]
mod tests {
    use super::*;

    // Widths transcribed from the retail pseudo-code in atom0s/XiEvents. Each of
    // these opcodes has a single `OpMeta::size` holding its WIDEST case, so a
    // regression that drops sub_size would silently advance too far and start
    // executing operand bytes as opcodes.
    #[test]
    fn variable_width_opcodes_pick_their_width_from_the_sub_byte() {
        // 0x0046.md — only sub 2 is the 4-byte work-offset form.
        assert_eq!(sub_size(0x46, 2), Some(4));
        for sub in [0u8, 1, 3, 0xFF] {
            assert_eq!(sub_size(0x46, sub), Some(2), "0x46 sub {sub}");
        }
        // 0x0079.md — only sub 1 carries the trailing work offset.
        assert_eq!(sub_size(0x79, 1), Some(12));
        for sub in [0u8, 2, 9, 0xFF] {
            assert_eq!(sub_size(0x79, sub), Some(10), "0x79 sub {sub}");
        }
        // 0x005C.md — two documented bands, and a hang everywhere else.
        assert_eq!(sub_size(0x5C, 0x00), Some(4));
        assert_eq!(sub_size(0x5C, 0x07), Some(4));
        assert_eq!(sub_size(0x5C, 0x80), Some(6));
        assert_eq!(sub_size(0x5C, 0x87), Some(6));
        assert_eq!(sub_size(0x5C, 0xA1), Some(6));
        assert_eq!(sub_size(0x5C, 0x40), None, "undocumented sub falls back");
        // 0x00C2.md
        assert_eq!(sub_size(0xC2, 1), Some(4));
        assert_eq!(sub_size(0xC2, 2), Some(6));
        assert_eq!(sub_size(0xC2, 7), Some(2));
        // 0x007E.md — the mount opcode's width is its case. The two wide ones
        // carry the chocobo's CustomProperties, so mistaking them for the
        // common 6 lands mid-operand.
        for sub in [0u8, 1, 2, 4, 5, 8] {
            assert_eq!(sub_size(0x7E, sub), Some(6), "0x7E sub {sub}");
        }
        assert_eq!(sub_size(0x7E, 3), Some(16));
        assert_eq!(sub_size(0x7E, 6), Some(18));
        assert_eq!(sub_size(0x7E, 7), Some(8));
        assert_eq!(sub_size(0x7E, 9), None, "undocumented case has no width");
        // 0x00B6.md — the look family; the wide cases carry a whole Look struct.
        assert_eq!(sub_size(0xB6, 0x0A), Some(4));
        assert_eq!(sub_size(0xB6, 0x0B), Some(20));
        assert_eq!(sub_size(0xB6, 0x0D), Some(14));
        assert_eq!(sub_size(0xB6, 0x0E), Some(16));
        assert_eq!(sub_size(0xB6, 0x10), Some(2));
        assert_eq!(sub_size(0xB6, 0x15), Some(6));
        assert_eq!(sub_size(0xB6, 0x16), None);
        // 0x0047.md
        assert_eq!(sub_size(0x47, 0), Some(10));
        assert_eq!(sub_size(0x47, 1), Some(2));
        assert_eq!(sub_size(0x47, 2), None);
        // 0x00D4.md — cases 0/1/2 are 8, case 3 is 6, cases 4/5 are 12.
        for sub in [0u8, 1, 2] {
            assert_eq!(sub_size(0xD4, sub), Some(8), "0xD4 sub {sub}");
        }
        assert_eq!(sub_size(0xD4, 3), Some(6));
        assert_eq!(sub_size(0xD4, 4), Some(12));
        assert_eq!(sub_size(0xD4, 5), Some(12));
        assert_eq!(sub_size(0xD4, 6), None, "undocumented case has no width");
        // 0x00B3.md — the ranking-board widths; unlike 0xD4, an undocumented
        // sub still advances 2 (retail's default arm).
        for sub in [0u8, 3, 4, 6, 7, 9] {
            assert_eq!(sub_size(0xB3, sub), Some(4), "0xB3 sub {sub}");
        }
        assert_eq!(sub_size(0xB3, 1), Some(14));
        assert_eq!(sub_size(0xB3, 5), Some(18));
        for sub in [2u8, 8, 0x0A, 0xFF] {
            assert_eq!(sub_size(0xB3, sub), Some(2), "0xB3 sub {sub}");
        }
        // 0x00A7.md — the server-answer wait; an undocumented sub spins.
        assert_eq!(sub_size(0xA7, 0), Some(2));
        assert_eq!(sub_size(0xA7, 1), Some(4));
        assert_eq!(sub_size(0xA7, 2), None, "undocumented sub spins");
        // 0x00A6.md — the sub-map request; an undocumented case spins.
        assert_eq!(sub_size(0xA6, 0), Some(2));
        assert_eq!(sub_size(0xA6, 1), Some(2));
        assert_eq!(sub_size(0xA6, 2), Some(4));
        assert_eq!(sub_size(0xA6, 3), None, "undocumented case spins");
        // 0x00B2.md — the delivery box; an undocumented mode spins.
        assert_eq!(sub_size(0xB2, 0), Some(4));
        assert_eq!(sub_size(0xB2, 1), Some(2));
        assert_eq!(sub_size(0xB2, 2), None, "undocumented mode spins");
        // 0x0087.md, 0x0088.md — the world pass; an undocumented case spins.
        for op in [0x87u8, 0x88] {
            for sub in 0..=2u8 {
                assert_eq!(sub_size(op, sub), Some(2), "0x{op:02X} sub {sub}");
            }
            assert_eq!(sub_size(op, 3), None, "undocumented case spins");
        }
        // 0x008C.md — the crafting support; an undocumented case spins.
        assert_eq!(sub_size(0x8C, 0), Some(8));
        assert_eq!(sub_size(0x8C, 1), Some(2));
        assert_eq!(sub_size(0x8C, 2), Some(12));
        assert_eq!(sub_size(0x8C, 3), Some(10));
        assert_eq!(sub_size(0x8C, 4), Some(10));
        assert_eq!(sub_size(0x8C, 5), Some(14));
        assert_eq!(sub_size(0x8C, 6), None, "undocumented case spins");
        // 0x0075.md — case 2's -6/+8 pair nets +2.
        assert_eq!(sub_size(0x75, 0), Some(4));
        assert_eq!(sub_size(0x75, 1), Some(2));
        assert_eq!(sub_size(0x75, 2), Some(2));
        assert_eq!(sub_size(0x75, 3), None);
        // 0x00CC.md
        assert_eq!(sub_size(0xCC, 0x00), Some(10));
        assert_eq!(sub_size(0xCC, 0x02), Some(14));
        assert_eq!(sub_size(0xCC, 0x10), Some(6));
        assert_eq!(sub_size(0xCC, 0x11), Some(4));
        assert_eq!(sub_size(0xCC, 0x20), Some(4));
        assert_eq!(sub_size(0xCC, 0x21), None);
        // 0x0059.md — the two odd widths are the ones a fixed 8 would overrun.
        assert_eq!(sub_size(0x59, 0), Some(4));
        assert_eq!(sub_size(0x59, 1), Some(8));
        assert_eq!(sub_size(0x59, 5), Some(7));
        assert_eq!(sub_size(0x59, 6), Some(6));
        assert_eq!(sub_size(0x59, 9), None);
        // 0x001F.md
        assert_eq!(sub_size(0x1F, 0), Some(8));
        assert_eq!(sub_size(0x1F, 1), Some(2));
        assert_eq!(sub_size(0x1F, 2), None);
        // 0x005A.md — CodeMOVE2 carries the same widths as its calibrated twin.
        assert_eq!(sub_size(0x5A, 0), Some(8));
        assert_eq!(sub_size(0x5A, 1), Some(2));
        assert_eq!(sub_size(0x5A, 2), None);
        // 0x00B4.md
        assert_eq!(sub_size(0xB4, 0x00), Some(20));
        assert_eq!(sub_size(0xB4, 0x05), Some(3));
        assert_eq!(sub_size(0xB4, 0x13), Some(20));
        assert_eq!(sub_size(0xB4, 0x14), Some(12));
        assert_eq!(sub_size(0xB4, 0x15), Some(2));
        assert_eq!(sub_size(0xB4, 0x16), None);
        // 0x0071.md — 0x11 falls through to 0x13 in the C switch, and 0x20 is
        // the 16-wide case the doc's size header omits.
        assert_eq!(sub_size(0x71, 0x11), Some(4));
        assert_eq!(sub_size(0x71, 0x13), Some(4));
        assert_eq!(sub_size(0x71, 0x20), Some(16));
        assert_eq!(sub_size(0x71, 0x54), Some(10));
        assert_eq!(sub_size(0x71, 0x56), None);
        // 0x00AB.md
        assert_eq!(sub_size(0xAB, 0x11), Some(4));
        assert_eq!(sub_size(0xAB, 0x14), Some(4));
        assert_eq!(sub_size(0xAB, 0x19), Some(2));
        assert_eq!(sub_size(0xAB, 0x1B), Some(6));
        assert_eq!(sub_size(0xAB, 0x1D), None);
        // 0x007A.md
        assert_eq!(sub_size(0x7A, 0), Some(6));
        assert_eq!(sub_size(0x7A, 1), Some(7));
        assert_eq!(sub_size(0x7A, 3), Some(2));
        assert_eq!(sub_size(0x7A, 4), Some(8));
        assert_eq!(sub_size(0x7A, 6), None);
        // 0x00B5.md — only sub 0 advances at all.
        assert_eq!(sub_size(0xB5, 0), Some(4));
        assert_eq!(sub_size(0xB5, 1), None);
        // 0x005F.md — 1 byte plus the delegated opcode's own advance.
        assert_eq!(sub_size(0x5F, 0), Some(2));
        assert_eq!(sub_size(0x5F, 2), Some(6));
        assert_eq!(sub_size(0x5F, 3), Some(16));
        assert_eq!(sub_size(0x5F, 5), Some(18));
        assert_eq!(sub_size(0x5F, 7), Some(14));
        assert_eq!(sub_size(0x5F, 8), None);
        // 0x009D.md — 0x07 is a real jump and 0x0C's width is runtime-decided.
        assert_eq!(sub_size(0x9D, 0x00), Some(8));
        assert_eq!(sub_size(0x9D, 0x02), Some(6));
        assert_eq!(sub_size(0x9D, 0x07), None);
        assert_eq!(sub_size(0x9D, 0x08), Some(23));
        assert_eq!(sub_size(0x9D, 0x09), Some(9));
        assert_eq!(sub_size(0x9D, 0x0A), Some(10));
        assert_eq!(sub_size(0x9D, 0x0C), None);
        assert_eq!(sub_size(0x9D, 0x0E), Some(8));
        assert_eq!(sub_size(0x9D, 0x11), None);
        // 0x00AC.md
        assert_eq!(sub_size(0xAC, 0), Some(4));
        assert_eq!(sub_size(0xAC, 2), Some(6));
        assert_eq!(sub_size(0xAC, 4), Some(8));
        assert_eq!(sub_size(0xAC, 5), None);
    }

    // The polls retail holds ExecPointer on until the player or the server
    // answers, against the cases that open those windows and advance at once.
    #[test]
    fn input_waits_cover_the_polls_and_not_the_openers() {
        // 0x0071.md
        for sub in [0x01u8, 0x02, 0x11, 0x13, 0x31, 0x41] {
            assert!(is_input_wait(0x71, sub), "0x71 sub {sub:#04X} polls");
        }
        for sub in [
            0x00u8, 0x03, 0x10, 0x12, 0x20, 0x21, 0x30, 0x32, 0x40, 0x50, 0x54,
        ] {
            assert!(!is_input_wait(0x71, sub), "0x71 sub {sub:#04X} advances");
        }
        // 0x00CC.md
        assert!(is_input_wait(0xCC, 0x11));
        assert!(!is_input_wait(0xCC, 0x10));
        assert!(!is_input_wait(0xCC, 0x02));
        // Object-existence polls, not input waits: retail advances when the
        // object is absent, and this VM never creates one.
        assert!(!is_input_wait(0x71, 0x53));
        assert!(!is_input_wait(0xB4, 0x15));
        assert!(!is_input_wait(0xB4, 0x14));
        // A family with no poll case at all.
        assert!(!is_input_wait(0xB6, 0x11));
    }

    // Every width sub_size can return must be <= the table's fixed size, which is
    // the widest documented case. A value above it would mean the table is wrong,
    // not the dispatch.
    #[test]
    fn sub_widths_never_exceed_the_tables_widest_case() {
        for op in [
            0x46u8, 0x79, 0x5C, 0xC2, 0x7E, 0xB6, 0x47, 0x75, 0xCC, 0x59, 0x1F, 0xB4, 0x71, 0xAB,
            0x7A, 0xB5, 0x5F, 0x9D, 0xAC,
        ] {
            let fixed = OPCODE_META[op as usize].size;
            for sub in 0..=u8::MAX {
                if let Some(w) = sub_size(op, sub) {
                    assert!(w <= fixed, "op {op:#04X} sub {sub}: {w} > {fixed}");
                    assert!(w >= 2, "op {op:#04X} sub {sub}: {w} cannot advance");
                }
            }
        }
    }
}
