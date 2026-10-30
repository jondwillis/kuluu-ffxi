//! Temporary probe: print decoded context around byte offsets in a DAT, both as
//! stored and with the d_msg XOR applied, so a located string can be identified.

const TARGETS: &[(u32, &[&str])] = &[
    (
        0x27685,
        &["C:/PhoenixXI/SquareEnix/FINAL FANTASY XI/ROM/173/89.DAT"],
    ),
    (
        0x116f61,
        &[
            "C:/PhoenixXI/SquareEnix/FINAL FANTASY XI/ROM/175/35.DAT",
            "C:/PhoenixXI/polplugins/DATs/EraDATs/ROM/175/35.DAT",
        ],
    ),
];

fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

#[test]
fn dump_hit_context() {
    for (offset, paths) in TARGETS {
        let Some(path) = paths.iter().find(|p| std::path::Path::new(p).is_file()) else {
            continue;
        };
        let bytes = std::fs::read(path).unwrap();
        let start = (*offset as usize).saturating_sub(0x40);
        let end = (*offset as usize + 0xC0).min(bytes.len());
        println!("== {} {offset:#x}", path.rsplit('/').next().unwrap_or(path));
        for (i, chunk) in bytes[start..end].chunks(16).enumerate() {
            let xor: Vec<u8> = chunk.iter().map(|b| !b).collect();
            println!(
                "{:#08x}  {} | {}",
                start + i * 16,
                printable(chunk),
                printable(&xor)
            );
        }
    }
}
