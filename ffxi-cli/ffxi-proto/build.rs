//! Extract the FFXI Blowfish subkey table from the LSB C++ source, and copy
//! the FFXI custom-zlib lookup tables (`compress.dat`/`decompress.dat`) from
//! the LSB resources, so the Rust port stays in sync with upstream
//! byte-for-byte.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};

const LSB_BLOWFISH_CPP: &str = "../../server/src/common/blowfish.cpp";
const LSB_COMPRESS_DAT: &str = "../../server/res/compress.dat";
const LSB_DECOMPRESS_DAT: &str = "../../server/res/decompress.dat";
const SUBKEY_LEN: usize = 4168;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={LSB_BLOWFISH_CPP}");
    println!("cargo:rerun-if-changed={LSB_COMPRESS_DAT}");
    println!("cargo:rerun-if-changed={LSB_DECOMPRESS_DAT}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("OUT_DIR not set")?);

    // Blowfish subkey table.
    let src = fs::read_to_string(LSB_BLOWFISH_CPP)
        .with_context(|| format!("reading {LSB_BLOWFISH_CPP}"))?;
    let start = src
        .find("uint8 subkey[4168]")
        .context("could not locate `uint8 subkey[4168]` in blowfish.cpp")?;
    let body_start = src[start..]
        .find('{')
        .context("could not locate opening `{` of subkey table")?
        + start
        + 1;
    let body_end = src[body_start..]
        .find('}')
        .context("could not locate closing `}` of subkey table")?
        + body_start;
    let body = &src[body_start..body_end];

    let mut bytes = Vec::with_capacity(SUBKEY_LEN);
    for tok in body.split([',', ' ', '\n', '\r', '\t']) {
        let t = tok.trim();
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            let b = u8::from_str_radix(hex, 16)
                .with_context(|| format!("parsing hex byte `{t}`"))?;
            bytes.push(b);
        } else if !t.is_empty() {
            bail!("unexpected token in subkey table: {t:?}");
        }
    }
    if bytes.len() != SUBKEY_LEN {
        bail!(
            "extracted {} subkey bytes, expected {SUBKEY_LEN}",
            bytes.len()
        );
    }
    fs::write(out_dir.join("blowfish_subkey.bin"), &bytes)?;

    // FFXI custom-zlib tables: copy verbatim.
    let compress = fs::read(LSB_COMPRESS_DAT)
        .with_context(|| format!("reading {LSB_COMPRESS_DAT}"))?;
    let decompress = fs::read(LSB_DECOMPRESS_DAT)
        .with_context(|| format!("reading {LSB_DECOMPRESS_DAT}"))?;
    if compress.len() % 4 != 0 || decompress.len() % 4 != 0 {
        bail!(
            "compress.dat ({}) / decompress.dat ({}) byte counts must be multiples of 4",
            compress.len(),
            decompress.len()
        );
    }
    fs::write(out_dir.join("compress.dat"), &compress)?;
    fs::write(out_dir.join("decompress.dat"), &decompress)?;

    Ok(())
}
