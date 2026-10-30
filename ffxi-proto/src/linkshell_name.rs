//! Linkshell-name codec: the packed 6-bit encoding retail stores in a
//! linkshell item's exdata and ships in s2c 0x0C9 GENERAL `sComLinkName`, and which
//! the client writes back into c2s 0x0C4 `sComLinkName` when it creates a shell.
//!
//! Port of `DecodeStringLinkshell`/`EncodeStringLinkshell`
//! (vendor/server/src/common/utils.cpp) over [`unpack_bits_le`]/[`pack_bits_le`] —
//! i.e. LSB's LE bit codec, whose byte-reversed-window rule means these names are NOT
//! plain little-endian bit fields: reading them naively decodes "Kuluu" as garbage
//! (see the pin test below).

use crate::bitstream::{pack_bits_le, unpack_bits_le};

/// Bytes of `sComLinkName` in s2c 0x0C9 GENERAL
/// (vendor/server/src/map/packets/s2c/0x0c9_equip_inspect_general.h GP_SERV_COMMAND_EQUIP_INSPECT job).
pub const PACKED_LEN: usize = 16;

/// Bytes of `sComLinkName` in c2s 0x0C4
/// (vendor/server/src/map/packets/c2s/0x0c4_group_comlink_active.h GP_CLI_COMMAND_GROUP_COMLINK_ACTIVE).
pub const ACTIVE_PACKED_LEN: usize = 15;

const BITS_PER_CHAR: u32 = 6;

/// Encoder alphabet (utils.cpp EncodeStringLinkshell): 1..=26 lowercase, 27..=52 uppercase,
/// 53..=62 digits. 63 is the end marker written into the trailing bits.
const CODE_LOWER_BASE: u8 = 1;
const CODE_UPPER_BASE: u8 = 27;
const CODE_DIGIT_BASE: u8 = 53;
const CODE_END: u8 = 63;

/// `std::min<size_t>(20u, ...)` — the decoder never emits more than 20
/// characters regardless of buffer size (utils.cpp DecodeStringLinkshell length).
const MAX_CHARS: usize = 20;

/// Decode a packed linkshell name. Returns an empty string when no linkshell
/// is equipped (the server leaves the field zeroed).
pub fn decode(packed: &[u8]) -> String {
    let len = MAX_CHARS.min(packed.len() * 8 / BITS_PER_CHAR as usize);
    let mut out = String::with_capacity(len);
    for i in 0..len {
        let code = unpack_bits_le(packed, i * BITS_PER_CHAR as usize, BITS_PER_CHAR) as u8;
        match code {
            // A zero code only occurs in padding, and the encoder's partial end marker can leave
            // one bogus character in front of it — hence LSB dropping the previous character here
            // (utils.cpp DecodeStringLinkshell).
            0 => {
                out.pop();
                break;
            }
            CODE_END => break,
            c if c < CODE_UPPER_BASE => out.push((b'a' + c - CODE_LOWER_BASE) as char),
            c if c < CODE_DIGIT_BASE => out.push((b'A' + c - CODE_UPPER_BASE) as char),
            c if c < CODE_END => out.push((b'0' + c - CODE_DIGIT_BASE) as char),
            _ => break,
        }
    }
    out
}

/// `EncodeStringLinkshell` (utils.cpp): packs at most 20 characters into `out`, then writes the
/// end marker's leftover bits (`leftover == 8 || leftover == 2 ? 6 : leftover`). Both the s2c
/// 0x0C9 `[16]` field and the c2s 0x0C4 `[15]` field use this same routine, so callers pass their
/// own field width — 15 bytes holds exactly twenty 6-bit codes.
pub fn encode_into(name: &str, out: &mut [u8]) {
    let mut chars = 0usize;
    for (i, ch) in name.chars().take(MAX_CHARS).enumerate() {
        let code = match ch {
            '0'..='9' => ch as u8 - b'0' + CODE_DIGIT_BASE,
            'A'..='Z' => ch as u8 - b'A' + CODE_UPPER_BASE,
            'a'..='z' => ch as u8 - b'a' + CODE_LOWER_BASE,
            _ => 0,
        };
        pack_bits_le(
            out,
            u64::from(code),
            i * BITS_PER_CHAR as usize,
            BITS_PER_CHAR,
        );
        chars += 1;
    }
    let mut leftover = 8 - (chars * BITS_PER_CHAR as usize) % 8;
    if leftover == 8 || leftover == 2 {
        leftover = 6;
    }
    pack_bits_le(out, 0xFF, chars * BITS_PER_CHAR as usize, leftover as u32);
}

/// Convenience wrapper around [`encode_into`] for the s2c-shaped 16-byte field (test fixture use).
#[cfg(test)]
pub(crate) fn encode(name: &str) -> [u8; PACKED_LEN] {
    let mut buf = [0u8; PACKED_LEN];
    encode_into(name, &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_name_length() {
        // Length mod 4 decides how many end-marker bits the encoder can fit,
        // so all four residues must survive the round trip.
        for name in ["a", "ab", "abc", "abcd", "Kuluu", "TheLinkshell1234"] {
            assert_eq!(decode(&encode(name)), name, "round trip {name}");
        }
    }

    #[test]
    fn round_trips_in_the_fifteen_byte_active_field_too() {
        for name in ["a", "abcd", "Kuluu", "CowLLand2026"] {
            let mut buf = [0u8; ACTIVE_PACKED_LEN];
            encode_into(name, &mut buf);
            assert_eq!(decode(&buf), name, "round trip {name} in [15]");
        }
    }

    #[test]
    fn kuluu_packs_to_the_lsb_le_bytes() {
        // Independent emulation of utils.cpp packBitsLE over a 15-byte field: LSB's LE pair is not
        // naive bit packing, so this hex also pins the fix against a regression to plain-LE.
        let mut buf = [0u8; ACTIVE_PACKED_LEN];
        encode_into("Kuluu", &mut buf);
        assert_eq!(
            buf,
            [0x95, 0x53, 0x15, 0x57, 0xF0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,][..]
        );
    }

    #[test]
    fn mixed_case_and_digits_map_to_the_lsb_alphabet() {
        assert_eq!(decode(&encode("aZ09")), "aZ09");
    }

    #[test]
    fn no_linkshell_decodes_empty() {
        assert_eq!(decode(&[0u8; PACKED_LEN]), "");
    }

    #[test]
    fn decoding_stops_at_the_twenty_character_cap() {
        let long = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(decode(&encode(long)).len(), MAX_CHARS);
    }
}
