//! PlayOnline patch-server framing (`POLP` frames on TCP). Reverse-engineered
//! from PlayOnlineViewer/viewer/com/polcore.dll (viewer 1.18.15e, SHA-256
//! 73b1864b...): FUN_1003968a / FUN_10039729 / FUN_10039825 build frames, the
//! socket worker at FUN_1003b156 validates them, FUN_100396f0 is the checksum.

use md5::{Digest, Md5};

pub const MAGIC: [u8; 4] = *b"POLP";
pub const HEADER_LEN: usize = 16;
/// Largest frame the viewer's socket worker accepts from the server; the
/// manifest reply is exempt because the client switches to raw reads for it.
pub const MAX_FRAME_LEN: usize = 0x16d00;
pub const MAX_STREAM_LEN: usize = 1 << 30;
/// Largest slice a file-chunk request may ask for (client-side cap).
pub const CHUNK_LEN: u32 = 0x10000;

pub mod kind {
    pub const HELLO: u32 = 1;
    pub const MANIFEST: u32 = 2;
    pub const FILE_REQUEST: u32 = 3;
    pub const FILE_CHUNK: u32 = 4;
    pub const REJECT: u32 = 5;
    pub const CLOSE: u32 = 6;
    pub const VERSION: u32 = 7;
    pub const VERSION_REPLY: u32 = 8;
}

/// The two 4-byte identifiers every client frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tags {
    pub platform: [u8; 4],
    pub title: [u8; 4],
}

impl Tags {
    fn bytes(&self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[..4].copy_from_slice(&self.platform);
        b[4..].copy_from_slice(&self.title);
        b
    }
}

pub fn checksum(after_checksum_field: &[u8]) -> [u8; 4] {
    let d = Md5::digest(after_checksum_field);
    [d[0], d[1], d[2], d[3]]
}

pub fn seal(kind: u32, body: &[u8]) -> Vec<u8> {
    let total = (HEADER_LEN + body.len()) as u32;
    let mut frame = Vec::with_capacity(total as usize);
    frame.extend_from_slice(&total.to_le_bytes());
    frame.extend_from_slice(&[0; 4]);
    frame.extend_from_slice(&MAGIC);
    frame.extend_from_slice(&kind.to_le_bytes());
    frame.extend_from_slice(body);
    let ck = checksum(&frame[8..]);
    frame[4..8].copy_from_slice(&ck);
    frame
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub kind: u32,
    pub body: &'a [u8],
}

pub fn declared_len(head: &[u8; 4]) -> usize {
    u32::from_le_bytes(*head) as usize
}

pub fn parse(frame: &[u8]) -> Result<Frame<'_>, String> {
    if frame.len() < HEADER_LEN {
        return Err(format!("frame too short ({} bytes)", frame.len()));
    }
    let total = declared_len(&[frame[0], frame[1], frame[2], frame[3]]);
    if total != frame.len() {
        return Err(format!(
            "frame length {total} does not match the {} bytes received",
            frame.len()
        ));
    }
    if frame[8..12] != MAGIC {
        return Err(format!("bad frame magic {:02x?}", &frame[8..12]));
    }
    if checksum(&frame[8..]) != frame[4..8] {
        return Err("frame checksum mismatch".to_string());
    }
    Ok(Frame {
        kind: u32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]),
        body: &frame[HEADER_LEN..],
    })
}

pub fn hello(tags: &Tags) -> Vec<u8> {
    seal(kind::HELLO, &tags.bytes())
}

pub const VERSION_BLOCK_LEN: usize = 0x40;

pub fn version(tags: &Tags, local_version: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + VERSION_BLOCK_LEN);
    body.extend_from_slice(&tags.bytes());
    let mut block = [0u8; VERSION_BLOCK_LEN];
    let n = local_version.len().min(VERSION_BLOCK_LEN - 1);
    block[..n].copy_from_slice(&local_version[..n]);
    body.extend_from_slice(&block);
    seal(kind::VERSION, &body)
}

pub fn file_request(tags: &Tags, path: &str, offset: u32, len: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(20 + path.len() + 1);
    body.extend_from_slice(&offset.to_le_bytes());
    body.extend_from_slice(&len.to_le_bytes());
    body.extend_from_slice(&tags.bytes());
    body.extend_from_slice(&(path.len() as u32 + 1).to_le_bytes());
    body.extend_from_slice(path.as_bytes());
    body.push(0);
    seal(kind::FILE_REQUEST, &body)
}

/// Body of a `VERSION_REPLY` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReply {
    pub release_unix: u32,
    pub flags: u32,
    /// `empty` for a fresh install, `unknown` when the server does not
    /// recognise the block it was sent (the viewer aborts on it).
    pub label: String,
    pub client_addr: String,
    pub version: String,
}

pub const VERSION_LABEL_UNKNOWN: &str = "unknown";

pub fn parse_version_reply(body: &[u8]) -> Result<VersionReply, String> {
    const LABEL_AT: usize = 8;
    const LEN_AT: usize = 8 + VERSION_BLOCK_LEN;
    const TEXT_AT: usize = LEN_AT + 4;
    if body.len() < TEXT_AT {
        return Err(format!(
            "version reply body too short ({} bytes)",
            body.len()
        ));
    }
    let u32_at =
        |at: usize| u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
    let mut strings = body[LABEL_AT..LEN_AT].split(|&b| b == 0);
    let label = String::from_utf8_lossy(strings.next().unwrap_or_default()).into_owned();
    let client_addr = String::from_utf8_lossy(strings.next().unwrap_or_default()).into_owned();
    let n = u32_at(LEN_AT) as usize;
    let text = body
        .get(TEXT_AT..TEXT_AT + n)
        .ok_or("version string truncated")?;
    let version =
        String::from_utf8_lossy(text.split(|&b| b == 0).next().unwrap_or_default()).into_owned();
    Ok(VersionReply {
        release_unix: u32_at(0),
        flags: u32_at(4),
        label,
        client_addr,
        version,
    })
}

/// Body of a `FILE_CHUNK` frame: `offset u32, len u32, extra u32, [extra
/// bytes: the echoed request path], data`.
pub fn file_chunk(body: &[u8]) -> Result<(u32, &[u8]), String> {
    if body.len() < 12 {
        return Err("file chunk body too short".to_string());
    }
    let offset = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
    let len = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
    let extra = u32::from_le_bytes([body[8], body[9], body[10], body[11]]) as usize;
    let start = 12 + extra;
    body.get(start..start + len)
        .map(|data| (offset, data))
        .ok_or_else(|| {
            format!(
                "file chunk claims {len} bytes after {extra} extra, body is {}",
                body.len()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS2_FFXI: Tags = Tags {
        platform: *b"PS2\0",
        title: *b"FFXI",
    };

    #[test]
    fn hello_matches_the_viewer_default_tags_vector() {
        let f = hello(&PS2_FFXI);
        assert_eq!(
            f,
            [
                0x18, 0, 0, 0, 0x62, 0x84, 0xa0, 0xa9, b'P', b'O', b'L', b'P', 1, 0, 0, 0, b'P',
                b'S', b'2', 0, b'F', b'F', b'X', b'I'
            ]
        );
    }

    #[test]
    fn parse_roundtrips_and_rejects_corruption() {
        let f = file_request(&PS2_FFXI, "ROM/0/0.DAT", 0x10000, CHUNK_LEN);
        let p = parse(&f).unwrap();
        assert_eq!(p.kind, kind::FILE_REQUEST);
        assert_eq!(&p.body[..8], &[0, 0, 1, 0, 0, 0, 1, 0]);
        assert_eq!(&p.body[16..20], &12u32.to_le_bytes());
        assert_eq!(&p.body[20..], b"ROM/0/0.DAT\0");
        let mut bad = f.clone();
        bad[20] ^= 1;
        assert!(parse(&bad).unwrap_err().contains("checksum"));
        let mut bad = f.clone();
        bad[9] = b'X';
        assert!(parse(&bad).unwrap_err().contains("magic"));
        assert!(parse(&f[..f.len() - 1]).is_err());
    }

    #[test]
    fn version_block_is_fixed_width_and_nul_terminated() {
        let f = version(&PS2_FFXI, b"30260904_1");
        assert_eq!(f.len(), HEADER_LEN + 8 + VERSION_BLOCK_LEN);
        assert_eq!(&f[24..34], b"30260904_1");
        assert!(f[34..].iter().all(|&b| b == 0));
        let long = vec![b'x'; 100];
        let f = version(&PS2_FFXI, &long);
        assert_eq!(f[24 + VERSION_BLOCK_LEN - 1], 0);
    }

    #[test]
    fn version_reply_layout() {
        let mut body = Vec::new();
        body.extend_from_slice(&1788518731u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        let mut label = [0u8; VERSION_BLOCK_LEN];
        label[..5].copy_from_slice(b"empty");
        label[6..21].copy_from_slice(b"124.150.156.107");
        body.extend_from_slice(&label);
        body.extend_from_slice(&11u32.to_le_bytes());
        body.extend_from_slice(b"30260904_1\0");
        let r = parse_version_reply(&body).unwrap();
        assert_eq!(r.release_unix, 1788518731);
        assert_eq!(r.label, "empty");
        assert_eq!(r.client_addr, "124.150.156.107");
        assert_eq!(r.version, "30260904_1");
    }

    #[test]
    fn file_chunk_skips_the_echoed_path() {
        let mut body = Vec::new();
        body.extend_from_slice(&65536u32.to_le_bytes());
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(b"a.slc\0");
        body.extend_from_slice(b"xyz");
        let (off, data) = file_chunk(&body).unwrap();
        assert_eq!(off, 65536);
        assert_eq!(data, b"xyz");
        assert!(file_chunk(&body[..body.len() - 1]).is_err());
    }
}
