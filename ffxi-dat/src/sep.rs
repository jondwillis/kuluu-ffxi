//! `Sep` chunk parser — kind `0x3D`. "Sound effect pointer".
//!
//! Each Sep chunk inside a DAT file points at one sound-effect file
//! (`.spw`) by numeric id. Animation/action DAT files contain
//! multiple Sep children alongside Scheduler timelines and
//! Generator declarations; when a Scheduler stage fires a
//! "sound-on-caster" / "sound-on-target" event (sub-types 0x0b /
//! 0x53), the event's 4-char id resolves to a Generator whose
//! Sound-type body references one of these Sep children by 4-char
//! `name` field.
//!
//! Wire layout (port of `vendor/lotus-ffxi/ffxi/dat/sep.cppm:26`):
//!
//! ```text
//! 0x00..0x08  reserved (8 bytes — possibly flags / per-channel
//!             mixer params; lotus ignores them and reads only the id)
//! 0x08..0x0C  u32 le  SE id  → resolves to
//!                              sound/win/se/seNNN/seNNNNNN.spw
//!                              (NNN = id/1000, NNNNNN = id padded
//!                              to 6, same as ffxi_audio::find_audio)
//! 0x0C..      remainder    (unparsed — observed sizes vary by
//!                          action; future work)
//! ```
//!
//! Lotus' implementation is also intentionally minimal (only `id` is
//! read); when more fields are needed (volume, pan, pitch, delay
//! within frame), they'll be added here.

use crate::{DatError, Result};

/// One Sep chunk's parsed payload. The chunk's 4-char `name` (from
/// the enclosing chunk header) is what Scheduler stages cite to
/// trigger this SE — keep it alongside the parsed body so callers
/// don't need a second lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sep {
    /// 4-char chunk identifier — the key Scheduler stages reference.
    pub name: [u8; 4],
    /// SE id → resolves to `sound/win/se/seNNN/seNNNNNN.spw`.
    pub se_id: u32,
}

impl Sep {
    /// Parse a Sep chunk body. `name` is the 4-char id from the
    /// enclosing DAT chunk header; `body` is the bytes *after* the
    /// 4-byte chunk header (i.e. what lotus calls the chunk's
    /// `buffer`).
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < 12 {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: 12,
                available: body.len(),
            });
        }
        let se_id = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
        Ok(Self { name, se_id })
    }

    /// `(directory, filename)` pair under `sound{,2..15}/win/se/`.
    /// Matches the layout `ffxi_audio::find_audio` searches.
    pub fn relative_path(&self) -> (String, String) {
        (
            format!("se{:03}", self.se_id / 1000),
            format!("se{:06}.spw", self.se_id),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_se_id_at_offset_8() {
        let mut body = vec![0u8; 16];
        body[8..12].copy_from_slice(&12345u32.to_le_bytes());
        let s = Sep::parse(*b"snd0", &body).unwrap();
        assert_eq!(s.se_id, 12345);
        assert_eq!(s.name, *b"snd0");
        assert_eq!(s.relative_path(), ("se012".to_string(), "se012345.spw".to_string()));
    }

    #[test]
    fn rejects_short_body() {
        let body = vec![0u8; 4];
        assert!(matches!(
            Sep::parse(*b"abcd", &body),
            Err(DatError::TruncatedChunk {
                needed: 12,
                available: 4,
                ..
            })
        ));
    }

    #[test]
    fn se_zero_resolves_to_se000() {
        let mut body = vec![0u8; 12];
        body[8..12].copy_from_slice(&0u32.to_le_bytes());
        let s = Sep::parse(*b"zero", &body).unwrap();
        assert_eq!(s.relative_path(), ("se000".to_string(), "se000000.spw".to_string()));
    }
}
