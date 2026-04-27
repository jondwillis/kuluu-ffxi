#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Terminate = 0x00,
    Rmp = 0x01,
    Generator = 0x05,
    Scheduler = 0x07,
    Tim = 0x09,
    KeyFrame = 0x19,
    Mzb = 0x1C,
    D3m = 0x1F,
    Img = 0x20,
    SpriteSheet = 0x21,
    Bone = 0x29,
    VertexOs2 = 0x2A,
    AnimMo2 = 0x2B,
    Mmb = 0x2E,
    Weather = 0x2F,
    Rid = 0x36,
    Sep = 0x3D,
    Cib = 0x45,
}

impl ChunkKind {
    pub fn from_u8(k: u8) -> Option<Self> {
        Some(match k {
            0x00 => Self::Terminate,
            0x01 => Self::Rmp,
            0x05 => Self::Generator,
            0x07 => Self::Scheduler,
            0x09 => Self::Tim,
            0x19 => Self::KeyFrame,
            0x1C => Self::Mzb,
            0x1F => Self::D3m,
            0x20 => Self::Img,
            0x21 => Self::SpriteSheet,
            0x29 => Self::Bone,
            0x2A => Self::VertexOs2,
            0x2B => Self::AnimMo2,
            0x2E => Self::Mmb,
            0x2F => Self::Weather,
            0x36 => Self::Rid,
            0x3D => Self::Sep,
            0x45 => Self::Cib,
            _ => return None,
        })
    }

    pub fn label(k: u8) -> &'static str {
        match k {
            0x00 => "Terminate",
            0x01 => "Rmp",
            0x05 => "Generator",
            0x07 => "Scheduler",
            0x09 => "Tim",
            0x19 => "KeyFrame",
            0x1C => "Mzb",
            0x1F => "D3m",
            0x20 => "Img",
            0x21 => "SpriteSheet",
            0x29 => "Bone",
            0x2A => "VertexOs2",
            0x2B => "AnimMo2",
            0x2E => "Mmb",
            0x2F => "Weather",
            0x36 => "Rid",
            0x3D => "Sep",
            0x45 => "Cib",
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
