// The named-but-unparsed variants (Route, WeightedMesh, PointList, SpellList, Path,
// AbilityList, WeaponTrace, BumpMap, Blur, UiMenu, UiElementGroup) exist so CLIP_WARN and
// the loader's rejected-chunk lists can name a chunk instead of printing an unknown code.
// Their names follow vekien/xi-model-viewer ui/js/dat/inspect.js SECTION_TYPE_NAMES;
// ffxi-dat deliberately ships no parser for them yet.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Terminate = 0x00,
    Rmp = 0x01,
    Generator = 0x05,
    Route = 0x06,
    Scheduler = 0x07,
    Tim = 0x09,
    KeyFrame = 0x19,
    Mzb = 0x1C,
    D3m = 0x1F,
    Img = 0x20,
    SpriteSheet = 0x21,
    WeightedMesh = 0x25,
    Bone = 0x29,
    VertexOs2 = 0x2A,
    AnimMo2 = 0x2B,
    Mmb = 0x2E,
    Weather = 0x2F,
    UiMenu = 0x30,
    UiElementGroup = 0x31,
    Rid = 0x36,
    Sep = 0x3D,
    PointList = 0x3E,
    Cib = 0x45,
    SpellList = 0x49,
    Path = 0x4A,
    AbilityList = 0x53,
    WeaponTrace = 0x54,
    BumpMap = 0x5D,
    Blur = 0x5E,
}

impl ChunkKind {
    pub fn from_u8(k: u8) -> Option<Self> {
        Some(match k {
            0x00 => Self::Terminate,
            0x01 => Self::Rmp,
            0x05 => Self::Generator,
            0x06 => Self::Route,
            0x07 => Self::Scheduler,
            0x09 => Self::Tim,
            0x19 => Self::KeyFrame,
            0x1C => Self::Mzb,
            0x1F => Self::D3m,
            0x20 => Self::Img,
            0x21 => Self::SpriteSheet,
            0x25 => Self::WeightedMesh,
            0x29 => Self::Bone,
            0x2A => Self::VertexOs2,
            0x2B => Self::AnimMo2,
            0x2E => Self::Mmb,
            0x2F => Self::Weather,
            0x30 => Self::UiMenu,
            0x31 => Self::UiElementGroup,
            0x36 => Self::Rid,
            0x3D => Self::Sep,
            0x3E => Self::PointList,
            0x45 => Self::Cib,
            0x49 => Self::SpellList,
            0x4A => Self::Path,
            0x53 => Self::AbilityList,
            0x54 => Self::WeaponTrace,
            0x5D => Self::BumpMap,
            0x5E => Self::Blur,
            _ => return None,
        })
    }

    pub fn label(k: u8) -> &'static str {
        match k {
            0x00 => "Terminate",
            0x01 => "Rmp",
            0x05 => "Generator",
            0x06 => "Route",
            0x07 => "Scheduler",
            0x09 => "Tim",
            0x19 => "KeyFrame",
            0x1C => "Mzb",
            0x1F => "D3m",
            0x20 => "Img",
            0x21 => "SpriteSheet",
            0x25 => "WeightedMesh",
            0x29 => "Bone",
            0x2A => "VertexOs2",
            0x2B => "AnimMo2",
            0x2E => "Mmb",
            0x2F => "Weather",
            0x30 => "UiMenu",
            0x31 => "UiElementGroup",
            0x36 => "Rid",
            0x3D => "Sep",
            0x3E => "PointList",
            0x45 => "Cib",
            0x49 => "SpellList",
            0x4A => "Path",
            0x53 => "AbilityList",
            0x54 => "WeaponTrace",
            0x5D => "BumpMap",
            0x5E => "Blur",
            _ => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_roundtrip() {
        for raw in [0x01u8, 0x06, 0x09, 0x20, 0x25, 0x2A, 0x2B, 0x3E, 0x45, 0x54] {
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
