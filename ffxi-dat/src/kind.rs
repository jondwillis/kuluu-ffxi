//! Chunk-kind enum + name lookup.
//!
//! The `kind` field in an FFXI chunk header (lower 7 bits of the value
//! word) selects which decoder to use. Names sourced from two GPL-3
//! references that agree on the values:
//!   - LSB/FFXI-NavMesh-Builder `Common/dat/Types/ResourceType.cs`
//!   - galkareeve `mapViewer/FFXIMesh.cpp` dispatch switch
//!
//! See `chunk.rs` for how this value is extracted from a chunk header.

/// Well-known chunk kinds we care about. Numeric values are stable.
/// Anything not listed here is parsed as an opaque chunk.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Terminate = 0x00,
    Rmp = 0x01,        // file-header marker (mot_, 0hm_, npc_, ...)
    Scheduler = 0x07,  // event/scheduler records (sdam, sp00, ...)
    Tim = 0x09,        // texture (legacy TIM)
    Mzb = 0x1C,        // zone static mesh (collision)
    Img = 0x20,        // image / texture container (D3S in some refs)
    Bone = 0x29,       // skeleton bones (Sk2)
    VertexOs2 = 0x2A,  // vertex data (OS2)
    AnimMo2 = 0x2B,    // animation keyframes (Mo2)
    Mmb = 0x2E,        // composite entity model (decryption + inline vertex/bone)
    Rid = 0x36,        // resource id (file metadata)
    // TODO(atmosphere): identify and add variants for FFXI's
    // per-zone visual data. The viewer's `ZoneAtmosphereProvider`
    // (ffxi-viewer-core::atmosphere) is ready to consume:
    //   * Sky-dome / skybox cubemap (separate sky DATs keyed on zone id)
    //   * Per-zone ambient color & fog parameters
    //   * Indoor light emitters (likely an MZB placement subtype, not
    //     a top-level chunk — see `parse_placements`)
    // None of these chunk types are confirmed in this parser yet;
    // they need reverse-engineering against POLUtils / Windower notes
    // or empirical inspection of the DAT chunk streams.
}

impl ChunkKind {
    /// Try to identify a kind value. Returns None for unknown kinds —
    /// caller continues with the raw byte (lax handling).
    pub fn from_u8(k: u8) -> Option<Self> {
        Some(match k {
            0x00 => Self::Terminate,
            0x01 => Self::Rmp,
            0x07 => Self::Scheduler,
            0x09 => Self::Tim,
            0x1C => Self::Mzb,
            0x20 => Self::Img,
            0x29 => Self::Bone,
            0x2A => Self::VertexOs2,
            0x2B => Self::AnimMo2,
            0x2E => Self::Mmb,
            0x36 => Self::Rid,
            _ => return None,
        })
    }

    /// Human-readable name for any kind byte, including ones we don't
    /// have a variant for.
    pub fn label(k: u8) -> &'static str {
        match k {
            0x00 => "Terminate",
            0x01 => "Rmp",
            0x07 => "Scheduler",
            0x09 => "Tim",
            0x1C => "Mzb",
            0x20 => "Img",
            0x29 => "Bone",
            0x2A => "VertexOs2",
            0x2B => "AnimMo2",
            0x2E => "Mmb",
            0x36 => "Rid",
            _ => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_roundtrip() {
        for raw in [0x01u8, 0x09, 0x20, 0x2A, 0x2B, 0x2E] {
            assert_eq!(ChunkKind::from_u8(raw).unwrap() as u8, raw);
        }
    }

    #[test]
    fn label_covers_known_kinds() {
        assert_eq!(ChunkKind::label(0x2E), "Mmb");
        assert_eq!(ChunkKind::label(0x2B), "AnimMo2");
        assert_eq!(ChunkKind::label(0xFF), "unknown");
    }
}
