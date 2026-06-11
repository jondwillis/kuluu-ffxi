//! `DatId` — 4-character resource identifier (port of XIM `DatId`).
//!
//! XIM stores the id as a 4-char String built from a chunk's FourCC name.
//! We keep the same 4 raw bytes and expose the same query helpers the
//! runtime needs to resolve parameterized animations (`idl?`, `run?`, ...).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DatId(pub [u8; 4]);

impl DatId {
    pub fn from_name(name: &[u8; 4]) -> Self {
        DatId(*name)
    }

    /// Build from a string, padding/truncating to exactly 4 bytes.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        let mut id = [0u8; 4];
        for (i, b) in s.bytes().take(4).enumerate() {
            id[i] = b;
        }
        DatId(id)
    }

    pub fn as_str(&self) -> String {
        self.0.iter().map(|&b| b as char).collect()
    }

    /// 4th character (XIM `finalChar`).
    pub fn final_char(&self) -> char {
        self.0[3] as char
    }

    /// 4th character parsed as a single decimal digit (XIM `finalDigit`).
    pub fn final_digit(&self) -> Option<u32> {
        (self.0[3] as char).to_digit(10)
    }

    /// XIM `isParameterized`: id ends with the literal '?'.
    pub fn is_parameterized(&self) -> bool {
        self.0[3] == b'?'
    }

    /// XIM `parameterizedMatch`: if `other` is parameterized, compare the
    /// first 3 chars; otherwise require exact equality.
    pub fn parameterized_match(&self, other: &DatId) -> bool {
        if other.is_parameterized() {
            self.0[0..3] == other.0[0..3]
        } else {
            self.0 == other.0
        }
    }

    pub fn starts_with(&self, prefix: &str) -> bool {
        let p = prefix.as_bytes();
        p.len() <= 4 && self.0[..p.len()] == *p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameterized_match_prefix() {
        let idl0 = DatId::from_str("idl0");
        assert!(idl0.parameterized_match(&DatId::from_str("idl?")));
        assert!(!idl0.parameterized_match(&DatId::from_str("run?")));
    }

    #[test]
    fn parameterized_match_exact_when_not_parameterized() {
        let idl0 = DatId::from_str("idl0");
        assert!(idl0.parameterized_match(&DatId::from_str("idl0")));
        assert!(!idl0.parameterized_match(&DatId::from_str("idl1")));
    }

    #[test]
    fn final_digit_and_char() {
        assert_eq!(DatId::from_str("idl7").final_digit(), Some(7));
        assert_eq!(DatId::from_str("idl?").final_digit(), None);
        assert_eq!(DatId::from_str("idl3").final_char(), '3');
    }

    #[test]
    fn from_name_and_starts_with() {
        let id = DatId::from_name(b"run0");
        assert_eq!(id.as_str(), "run0");
        assert!(id.starts_with("run"));
        assert!(!id.starts_with("idl"));
    }
}
