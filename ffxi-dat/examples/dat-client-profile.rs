use std::path::PathBuf;

use ffxi_dat::client_profile::KNOWN_CLIENTS;
use ffxi_dat::{ClientProfile, DatRoot};

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let root = match arg {
        Some(p) => p,
        None => match DatRoot::from_env_or_default() {
            Ok(root) => root.root().to_path_buf(),
            Err(e) => {
                eprintln!("usage: dat-client-profile [install dir]  ({e})");
                std::process::exit(2);
            }
        },
    };
    let profile = ClientProfile::probe(&root);
    println!("root:            {}", root.display());
    println!("profile:         {}", profile.name());
    println!(
        "ffximain sha256: {}",
        profile.ffximain_sha256.as_deref().unwrap_or("missing")
    );
    println!(
        "ffximain bytes:  {}",
        profile
            .ffximain_len
            .map(|n| n.to_string())
            .unwrap_or_else(|| "missing".into())
    );
    println!(
        "patch version:   {}",
        profile.patch_version.as_deref().unwrap_or("unknown")
    );
    println!(
        "item layout:     {}",
        profile.item_layout.map(|l| l.name()).unwrap_or("unprobed")
    );
    if !profile.is_known() {
        println!(
            "\nnot a known build; measure it and add a row to KNOWN_CLIENTS ({} known)",
            KNOWN_CLIENTS.len()
        );
    }
}
