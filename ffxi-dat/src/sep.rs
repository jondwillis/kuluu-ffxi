use crate::{DatError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sep {
    pub name: [u8; 4],

    pub se_id: u32,
}

impl Sep {
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
        assert_eq!(
            s.relative_path(),
            ("se012".to_string(), "se012345.spw".to_string())
        );
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
        assert_eq!(
            s.relative_path(),
            ("se000".to_string(), "se000000.spw".to_string())
        );
    }
}
