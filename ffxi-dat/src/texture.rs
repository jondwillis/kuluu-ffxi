//! FFXI texture chunk parser (kind 0x20 = IMG container; inner format
//! is DXT3 with the `"3TXD"` magic, or DXT1 with a related variant).
//!
//! Layout of an IMG chunk body (empirically, file 7368 chunk 2 hm_b):
//!   offset 0..16       inner header — `0xa1` flag + 4-byte name "tim "
//!                      + padded ASCII variant ("hm_ba1_1...")
//!   offset 0x30..0x34  texture meta byte + 4-byte format magic
//!   offset 0x34..0x38  `"3TXD"` magic (= "DXT3" reversed). Other
//!                      observed variants: `"1TXD"` (DXT1).
//!   after magic        DDS-style DXT-compressed pixel data, optionally
//!                      preceded by width/height u16s.

use crate::Result;

/// FFXI texture pixel format (subset of the broader DDS surface formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    /// DXT1 / BC1 — 4 bpp, RGB(A) with 1-bit alpha.
    Dxt1,
    /// DXT3 / BC2 — 8 bpp, RGBA with explicit 4-bit alpha (FFXI default).
    Dxt3,
    /// Uncompressed 32-bit A8R8G8B8 (rare in FFXI).
    Argb32,
}

impl TexFormat {
    /// Match the FourCC magic FFXI uses for the format header.
    /// FFXI stores these reversed-endian — `"3TXD"` decrypted as ASCII
    /// = `"DXT3"` if you flip the byte order.
    pub fn from_magic(magic: &[u8]) -> Option<Self> {
        match magic {
            b"3TXD" => Some(Self::Dxt3),
            b"1TXD" => Some(Self::Dxt1),
            b"BGRA" | b"ARGB" => Some(Self::Argb32),
            _ => None,
        }
    }
}

/// Locate the `"NTXD"` magic in an IMG chunk body. Returns the byte
/// offset and the decoded format.
pub fn find_texture_format(body: &[u8]) -> Result<Option<(usize, TexFormat)>> {
    for (i, win) in body.windows(4).enumerate() {
        if let Some(fmt) = TexFormat::from_magic(win) {
            return Ok(Some((i, fmt)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dxt3_magic() {
        // 16 bytes padding then the "3TXD" magic.
        let mut buf = vec![0u8; 16];
        buf.extend_from_slice(b"3TXD");
        buf.extend_from_slice(&[0u8; 32]);
        let (off, fmt) = find_texture_format(&buf).unwrap().unwrap();
        assert_eq!(off, 16);
        assert_eq!(fmt, TexFormat::Dxt3);
    }

    #[test]
    fn detects_dxt1_magic() {
        let buf = b"1TXD".to_vec();
        let (off, fmt) = find_texture_format(&buf).unwrap().unwrap();
        assert_eq!(off, 0);
        assert_eq!(fmt, TexFormat::Dxt1);
    }

    #[test]
    fn no_magic_returns_none() {
        let buf = vec![0xFF; 32];
        assert!(find_texture_format(&buf).unwrap().is_none());
    }
}
