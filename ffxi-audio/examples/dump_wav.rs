//! CLI: decode a `.bgw` or `.spw` from a local FFXI install and
//! write it out as a 16-bit PCM WAV for sanity-listening.
//!
//! ```text
//! cargo run -p ffxi-audio --example dump_wav -- \
//!     "<install>/SquareEnix/FINAL FANTASY XI" bgm 101 /tmp/m101.wav
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use ffxi_audio::{decode_file, find_audio, AudioKind};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let install: PathBuf = match args.next() {
        Some(s) => s.into(),
        None => return usage(),
    };
    let kind = match args.next().as_deref() {
        Some("bgm") => AudioKind::Bgm,
        Some("sfx") => AudioKind::Sfx,
        _ => return usage(),
    };
    let id: u32 = match args.next().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => return usage(),
    };
    let out: PathBuf = match args.next() {
        Some(s) => s.into(),
        None => return usage(),
    };

    let src = match find_audio(&install, kind, id) {
        Some(p) => p,
        None => {
            eprintln!(
                "not found: kind={:?} id={} under {}",
                kind,
                id,
                install.display()
            );
            return ExitCode::from(2);
        }
    };
    eprintln!("decoding {}", src.display());

    let decoded = match decode_file(&src) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("decode failed: {e}");
            return ExitCode::from(3);
        }
    };
    eprintln!(
        "{} frames @ {:.0}Hz × {}ch, loop_start={:?}",
        decoded.frames(),
        decoded.sample_rate,
        decoded.channels,
        decoded.loop_start_sample
    );

    let spec = hound::WavSpec {
        channels: decoded.channels as u16,
        sample_rate: decoded.sample_rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = match hound::WavWriter::create(&out, spec) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("create {}: {e}", out.display());
            return ExitCode::from(4);
        }
    };
    for s in &decoded.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        if writer.write_sample(v).is_err() {
            eprintln!("write failed");
            return ExitCode::from(5);
        }
    }
    if writer.finalize().is_err() {
        eprintln!("finalize failed");
        return ExitCode::from(6);
    }
    eprintln!("wrote {}", out.display());
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("usage: dump_wav <ffxi-install-root> <bgm|sfx> <id> <out.wav>");
    ExitCode::from(1)
}
