//! Round-trip integration tests.
//!
//! Real `.spw`/`.bgw` fixtures aren't checked in (they're SE-licensed
//! content). Instead we synthesize valid headers + ADPCM bodies in
//! memory, decode them, and verify properties hold. A second test
//! (gated on `FFXI_INSTALL`) decodes a real install asset end-to-end.

use std::path::PathBuf;

use ffxi_audio::{decode_bytes, decode_file, find_audio, parse_any, AudioKind, SampleFormat};

fn build_synthetic_spw_adpcm() -> Vec<u8> {
    let block_size: u32 = 28;
    let sample_blocks: u32 = 4;
    let channels: u8 = 1;
    let mut buf = vec![0u8; 48];
    buf[..6].copy_from_slice(b"SeWave");
    // format = ADPCM (0)
    buf[0x14..0x18].copy_from_slice(&sample_blocks.to_le_bytes());
    // loop_start = 0
    buf[0x1C..0x20].copy_from_slice(&22000u32.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&50u32.to_le_bytes());
    buf[0x2A] = channels;
    buf[0x2B] = block_size as u8;

    // Append `sample_blocks` of all-silent ADPCM (header byte = 0x00,
    // followed by block_size/2 zero bytes).
    let block_bytes = 1 + (block_size / 2) as usize;
    for _ in 0..sample_blocks {
        let mut block = vec![0u8; block_bytes];
        block[0] = 0x00; // filter 0, scale 12
        buf.extend_from_slice(&block);
    }
    buf
}

#[test]
fn synthetic_spw_decodes_to_silence() {
    let bytes = build_synthetic_spw_adpcm();
    let (h, off) = parse_any(&bytes).unwrap();
    assert_eq!(h.format, SampleFormat::Adpcm);
    assert_eq!(h.channels, 1);
    assert_eq!(h.block_size, 28);
    assert_eq!(h.sample_blocks, 4);
    assert_eq!(h.sample_rate, 22050.0);
    assert_eq!(off, 48);

    let decoded = decode_bytes(&bytes).unwrap();
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.frames(), 28 * 4);
    assert!(decoded.samples.iter().all(|s| *s == 0.0));
    assert!(decoded.loop_start_sample.is_none());
}

#[test]
fn synthetic_bgw_stereo_silence_round_trip() {
    let block_size: u32 = 28;
    let sample_blocks: u32 = 4;
    let channels: u8 = 2;
    // Loop wire value 3 → vgmstream conversion `(3 - 1) * 28 = 56`,
    // i.e. the audio loops back to the start of block index 2. This
    // synthetic test explicitly exercises the `(N - 1) * block_size`
    // formula introduced when the decoder was corrected against
    // vgmstream's `bgw.c::loop_start_sample`. Picking a non-edge
    // value (3, not 1 or 2) makes the conversion visible in the
    // assertion below.
    let loop_start_wire: u32 = 3;
    let expected_loop_sample: u32 = (loop_start_wire - 1) * block_size; // 56
    let mut buf = vec![0u8; 48];
    buf[..8].copy_from_slice(b"BGMStrea");
    buf[8..12].copy_from_slice(b"m\0\0\0");
    buf[0x18..0x1C].copy_from_slice(&sample_blocks.to_le_bytes());
    buf[0x1C..0x20].copy_from_slice(&loop_start_wire.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&44100u32.to_le_bytes());
    buf[0x2E] = channels;
    buf[0x2F] = block_size as u8;

    let block_bytes_per_channel = 1 + (block_size / 2) as usize;
    for _ in 0..sample_blocks {
        for _ in 0..channels {
            let mut block = vec![0u8; block_bytes_per_channel];
            block[0] = 0x00;
            buf.extend_from_slice(&block);
        }
    }

    let decoded = decode_bytes(&buf).unwrap();
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.frames(), (block_size * sample_blocks) as usize);
    // Interleaved L,R pairs — both zero.
    assert!(decoded.samples.iter().all(|s| *s == 0.0));
    assert_eq!(decoded.loop_start_sample, Some(expected_loop_sample));
}

#[test]
fn spw_no_loop_sentinel_is_handled() {
    // Real .spw files use `loop_start = 0xFFFFFFFF` (signed -1) as
    // the "no loop" sentinel. Earlier code multiplied that by
    // block_size as u32 and panicked on overflow. Regression test.
    let block_size: u32 = 28;
    let sample_blocks: u32 = 2;
    let channels: u8 = 1;
    let mut buf = vec![0u8; 48];
    buf[..6].copy_from_slice(b"SeWave");
    buf[0x14..0x18].copy_from_slice(&sample_blocks.to_le_bytes());
    // u32::MAX = signed -1 → no loop.
    buf[0x18..0x1C].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    buf[0x1C..0x20].copy_from_slice(&22000u32.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&50u32.to_le_bytes());
    buf[0x2A] = channels;
    buf[0x2B] = block_size as u8;

    let block_bytes = 1 + (block_size / 2) as usize;
    for _ in 0..sample_blocks {
        let mut block = vec![0u8; block_bytes];
        block[0] = 0x00;
        buf.extend_from_slice(&block);
    }

    let decoded = decode_bytes(&buf).expect("should decode without panic");
    assert!(decoded.loop_start_sample.is_none());
    assert_eq!(decoded.frames(), (block_size * sample_blocks) as usize);
}

/// Decode a real asset from a local install if `FFXI_INSTALL` is set.
/// Smoke test that catches header-layout / block-arithmetic
/// regressions on real data without checking in copyrighted bytes.
#[test]
fn real_install_smoke() {
    let Ok(install) = std::env::var("FFXI_INSTALL") else {
        eprintln!("skipping: FFXI_INSTALL not set");
        return;
    };
    let install = PathBuf::from(install);

    // Try music 101 first, then fall back to anything 0..200.
    let mut found: Option<(AudioKind, u32, _)> = None;
    for id in [101u32, 1, 2, 3, 100] {
        if let Some(p) = find_audio(&install, AudioKind::Bgm, id) {
            let decoded = decode_file(&p).expect("decode_file");
            found = Some((AudioKind::Bgm, id, decoded));
            break;
        }
    }
    let (_, _, decoded) = found.expect("no BGM found under FFXI_INSTALL");
    assert!(decoded.channels >= 1 && decoded.channels <= 2);
    assert!(decoded.sample_rate >= 8000.0 && decoded.sample_rate <= 48000.0);
    assert!(decoded.frames() > 100); // at least a few blocks of samples
}
