//! A session with a PlayOnline patch server. Host/port/tag derivation from
//! PlayOnlineViewer/viewer/com/app.dll (viewer 1.18.15e, SHA-256 7ba99828...):
//! FUN_101b68d4 formats `pc%03d%s.pol.com`, FUN_1029c3a8 adds the title index
//! to 53000, FUN_1027b731 picks the platform tag. Verified live on
//! pc001.pol.com:53001.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::lz;
use crate::polp::{self, Tags, VersionReply};

pub const TITLE_FFXI: u32 = 1;
pub const PLATFORM_WINDOWS: [u8; 4] = *b"W20\0";
const HOST_DOMAIN: &str = "pol.com";
const HOST_PREFIX: &str = "pc";
const PORT_BASE: u16 = 53000;
const IO_TIMEOUT: Duration = Duration::from_secs(120);
/// First body byte of the manifest reply; anything else is a refusal.
const MANIFEST_STATUS_OK: u8 = 2;

pub mod direct_encoding {
    pub const STORED: u8 = 1;
    pub const ZLIB: u8 = 3;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub tags: Tags,
}

impl Server {
    pub fn for_title(title: u32) -> Self {
        let mut title_tag = [0u8; 4];
        title_tag.copy_from_slice(format!("{title:04}").as_bytes());
        Self {
            host: format!("{HOST_PREFIX}{title:03}.{HOST_DOMAIN}"),
            port: PORT_BASE + title as u16,
            tags: Tags {
                platform: PLATFORM_WINDOWS,
                title: title_tag,
            },
        }
    }
}

pub struct Session {
    tags: Tags,
    stream: TcpStream,
}

impl Session {
    pub fn connect(server: &Server) -> Result<Self, String> {
        let stream = TcpStream::connect((server.host.as_str(), server.port))
            .map_err(|e| format!("connecting to {}:{}: {e}", server.host, server.port))?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
        Ok(Self {
            tags: server.tags,
            stream,
        })
    }

    fn send(&mut self, frame: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(frame)
            .map_err(|e| format!("sending to patch server: {e}"))
    }

    fn recv(&mut self) -> Result<Vec<u8>, String> {
        let mut head = [0u8; 4];
        self.stream
            .read_exact(&mut head)
            .map_err(|e| format!("reading from patch server: {e}"))?;
        let total = polp::declared_len(&head);
        if !(polp::HEADER_LEN..=polp::MAX_STREAM_LEN).contains(&total) {
            return Err(format!("patch server announced a {total}-byte frame"));
        }
        let mut frame = vec![0u8; total];
        frame[..4].copy_from_slice(&head);
        self.stream
            .read_exact(&mut frame[4..])
            .map_err(|e| format!("reading a {total}-byte frame: {e}"))?;
        Ok(frame)
    }

    fn exchange(&mut self, request: &[u8], expect: u32) -> Result<Vec<u8>, String> {
        self.send(request)?;
        let frame = self.recv()?;
        let parsed = polp::parse(&frame)?;
        if parsed.kind == polp::kind::REJECT {
            return Err(
                "patch server rejected the request (wrong platform/title tags?)".to_string(),
            );
        }
        if parsed.kind != expect {
            return Err(format!(
                "patch server answered with frame type {} (expected {expect})",
                parsed.kind
            ));
        }
        Ok(frame)
    }

    pub fn version_check(&mut self, local_version: &[u8]) -> Result<VersionReply, String> {
        let frame = self.exchange(
            &polp::version(&self.tags, local_version),
            polp::kind::VERSION_REPLY,
        )?;
        polp::parse_version_reply(&frame[polp::HEADER_LEN..])
    }

    /// The hello reply is the whole manifest, `.slc`-coded, behind a status byte.
    pub fn manifest(&mut self) -> Result<Vec<u8>, String> {
        let frame = self.exchange(&polp::hello(&self.tags), polp::kind::MANIFEST)?;
        let body = &frame[polp::HEADER_LEN..];
        match body.first() {
            Some(&MANIFEST_STATUS_OK) => lz::decode_direct(&body[1..]),
            Some(other) => Err(format!("manifest reply status {other}")),
            None => Err("empty manifest reply".to_string()),
        }
    }

    /// Fetch `len` bytes of a server path in `CHUNK_LEN` requests, reporting
    /// the running byte count.
    pub fn fetch(
        &mut self,
        path: &str,
        len: u64,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Vec<u8>, String> {
        let mut data = Vec::with_capacity(len as usize);
        while (data.len() as u64) < len {
            let offset = data.len() as u32;
            let want = ((len - data.len() as u64) as u32).min(polp::CHUNK_LEN);
            let frame = self.exchange(
                &polp::file_request(&self.tags, path, offset, want),
                polp::kind::FILE_CHUNK,
            )?;
            let (got_offset, chunk) = polp::file_chunk(&frame[polp::HEADER_LEN..])?;
            if got_offset != offset || chunk.is_empty() {
                return Err(format!(
                    "{path}: asked for offset {offset}, server sent {} bytes at {got_offset}",
                    chunk.len()
                ));
            }
            data.extend_from_slice(chunk);
            progress(data.len() as u64);
        }
        Ok(data)
    }
}

/// A `Direct` download is a flag byte and then the file stored, zlib-deflated,
/// or `.slc`-coded (app.dll FUN_1029cd5a case 0x5df).
pub fn decode_direct_payload(data: &[u8]) -> Result<Vec<u8>, String> {
    let (&flag, payload) = data.split_first().ok_or("empty file download")?;
    match flag {
        direct_encoding::STORED => Ok(payload.to_vec()),
        direct_encoding::ZLIB => {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(payload)
                .read_to_end(&mut out)
                .map_err(|e| format!("zlib: {e}"))?;
            Ok(out)
        }
        _ => lz::decode_direct(payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffxi_is_title_one_on_pc001() {
        let s = Server::for_title(TITLE_FFXI);
        assert_eq!(s.host, "pc001.pol.com");
        assert_eq!(s.port, 53001);
        assert_eq!(s.tags.platform, *b"W20\0");
        assert_eq!(s.tags.title, *b"0001");
    }

    #[test]
    fn direct_payload_dispatches_on_the_flag_byte() {
        assert_eq!(decode_direct_payload(&[1, b'a', b'b']).unwrap(), b"ab");
        let mut z = Vec::new();
        {
            use std::io::Write as _;
            let mut e = flate2::write::ZlibEncoder::new(&mut z, flate2::Compression::default());
            e.write_all(b"zipped").unwrap();
            e.finish().unwrap();
        }
        let mut data = vec![3];
        data.extend_from_slice(&z);
        assert_eq!(decode_direct_payload(&data).unwrap(), b"zipped");
        let mut coded = vec![2];
        coded.extend_from_slice(&9u32.to_le_bytes());
        coded.extend_from_slice(&[b'q' << 1, 0]);
        assert_eq!(decode_direct_payload(&coded).unwrap(), b"q");
        assert!(decode_direct_payload(&[]).is_err());
    }
}
