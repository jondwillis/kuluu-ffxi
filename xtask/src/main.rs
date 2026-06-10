//! Project automation, invoked via the `cargo xtask` alias (.cargo/config.toml).
//!
//! ## `cargo xtask game [PATH] [--copy] [--force]`
//!
//! Wire a retail FFXI install into `vendor/game-files/` so the client finds it
//! by default (it reads `vendor/game-files/SquareEnix/FINAL FANTASY XI`, or
//! wherever `FFXI_DAT_PATH` points). Detects an existing install (HorizonXI /
//! Lutris / Wine / CrossOver / PlayOnline), validates it, and symlinks it into
//! place. Pass an explicit PATH to skip detection; `--copy` to copy instead of
//! symlink; `--force` to replace an existing link.
//!
//! ## `cargo xtask game --download [--region us|eu] [--yes]`
//!
//! Opt-in (and confirmation-gated): download Square Enix's official FFXI client
//! installer from the public PlayOnline CDN and launch it. The installer is an
//! interactive GUI (run via Wine on macOS/Linux); once it finishes, re-run
//! `cargo xtask game` to wire the result into `vendor/game-files/`. Downloading
//! the client is free; a registration code / subscription is needed to *play*.
//!
//! Std-only by design — see Cargo.toml; HTTP and the installer run by shelling
//! out to `curl` and `wine`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The install layout the client expects under `vendor/game-files/`.
const SQUARE_ENIX: &str = "SquareEnix";
const FFXI: &str = "FINAL FANTASY XI";
/// File that proves a directory is the FFXI client DAT root.
const MARKER: &str = "VTABLE.DAT";
/// How deep to descend under each detection root looking for the marker.
const SEARCH_DEPTH: usize = 6;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("game") => match cmd_game(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("unknown xtask `{other}`\n");
            usage();
            ExitCode::FAILURE
        }
        None => {
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "usage: cargo xtask game [PATH] [--copy] [--force]\n\
         \x20      cargo xtask game --download [--region us|eu] [--yes]\n\
         \n\
         Wire a retail FFXI install into vendor/game-files/.\n\
         PATH        an install dir to use (skips auto-detection)\n\
         --copy      copy the install instead of symlinking it\n\
         --force     replace an existing vendor/game-files link\n\
         --download  download SE's official client installer and launch it\n\
         --region    us (default) or eu, for --download\n\
         --yes       skip the --download confirmation prompt"
    );
}

fn cmd_game(args: &[String]) -> Result<(), String> {
    let mut explicit: Option<PathBuf> = None;
    let mut copy = false;
    let mut force = false;
    let mut download = false;
    let mut yes = false;
    let mut region = String::from("us");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--copy" => copy = true,
            "--force" => force = true,
            "--download" => download = true,
            "--yes" | "-y" => yes = true,
            "--region" => {
                region = it.next().ok_or("--region needs a value (us|eu)")?.to_lowercase();
            }
            s if s.starts_with("--") => return Err(format!("unknown flag `{s}`")),
            s => explicit = Some(PathBuf::from(s)),
        }
    }

    let workspace = workspace_root();

    if download {
        return download_official(&region, yes, &workspace);
    }
    let dest_se = workspace.join("vendor/game-files").join(SQUARE_ENIX);
    let dest = dest_se.join(FFXI);

    // Already wired up and valid? Nothing to do.
    if is_ffxi_root(&dest) {
        println!("vendor/game-files already has a valid install:\n  {}", show(&dest));
        print_env_hint(&dest);
        return Ok(());
    }

    // Resolve the source install: explicit path, else auto-detect.
    let source = match explicit {
        Some(p) => {
            let found = find_ffxi_root(&p, SEARCH_DEPTH);
            found.ok_or_else(|| {
                format!(
                    "no FFXI install (a dir containing {MARKER}) found at or under {}",
                    p.display()
                )
            })?
        }
        None => {
            let mut hits = detect();
            hits.dedup();
            match hits.len() {
                0 => {
                    return Err(no_install_help());
                }
                1 => hits.into_iter().next().unwrap(),
                _ => {
                    let mut msg = String::from("multiple installs detected — re-run with one:\n");
                    for h in &hits {
                        msg.push_str(&format!("  cargo xtask game \"{}\"\n", h.display()));
                    }
                    return Err(msg);
                }
            }
        }
    };

    println!("Using install: {}", source.display());

    // Refuse to clobber a real directory (only ever replace our own symlink).
    if dest.exists() || is_symlink(&dest) {
        if is_symlink(&dest) && force {
            std::fs::remove_file(&dest).map_err(|e| format!("removing old link {}: {e}", dest.display()))?;
        } else if is_symlink(&dest) {
            return Err(format!(
                "{} is already a link (to {}). Re-run with --force to replace it.",
                dest.display(),
                std::fs::read_link(&dest).map(|p| p.display().to_string()).unwrap_or_default()
            ));
        } else {
            return Err(format!(
                "{} already exists and is not a symlink — move it aside first.",
                dest.display()
            ));
        }
    }

    std::fs::create_dir_all(&dest_se).map_err(|e| format!("creating {}: {e}", dest_se.display()))?;

    if copy {
        println!("Copying (this can be ~19 GB) ...");
        copy_dir(&source, &dest)?;
    } else {
        symlink_dir(&source, &dest)?;
        println!("Linked {} -> {}", show(&dest), source.display());
    }

    if !is_ffxi_root(&dest) {
        return Err(format!(
            "wired {} but it does not validate ({MARKER} missing) — install may be incomplete",
            dest.display()
        ));
    }
    println!("OK: vendor/game-files is ready.");
    print_env_hint(&dest);
    Ok(())
}

/// A directory is the FFXI DAT root if it holds VTABLE.DAT and a ROM/ tree.
fn is_ffxi_root(dir: &Path) -> bool {
    dir.join(MARKER).is_file() && dir.join("ROM").is_dir()
}

/// Search `start` (and descendants up to `depth`) for an FFXI DAT root.
/// Returns the first match, preferring a dir literally named "FINAL FANTASY XI".
fn find_ffxi_root(start: &Path, depth: usize) -> Option<PathBuf> {
    if is_ffxi_root(start) {
        return Some(start.to_path_buf());
    }
    // BFS so shallow matches win; cap visited dirs to stay snappy on big trees.
    let mut queue: Vec<(PathBuf, usize)> = vec![(start.to_path_buf(), 0)];
    let mut visited = 0usize;
    while let Some((dir, d)) = queue.pop() {
        if d > depth || visited > 20_000 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() || is_symlink(&p) {
                continue;
            }
            visited += 1;
            if is_ffxi_root(&p) {
                return Some(p);
            }
            queue.push((p, d + 1));
        }
    }
    None
}

/// Platform-specific likely install locations that actually exist on disk.
fn detect() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    if cfg!(target_os = "windows") {
        for drive in ["C:\\", "D:\\"] {
            roots.push(PathBuf::from(format!("{drive}Program Files (x86)\\PlayOnline")));
            roots.push(PathBuf::from(format!("{drive}Program Files (x86)\\HorizonXI")));
        }
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(p).join("HorizonXI"));
        }
        if let Some(p) = std::env::var_os("USERPROFILE") {
            roots.push(PathBuf::from(p).join("Games"));
        }
    } else if let Some(home) = home {
        // macOS CrossOver, Linux Lutris/Wine prefixes.
        roots.push(home.join("Library/Application Support/CrossOver/Bottles"));
        roots.push(home.join("Games"));
        roots.push(home.join(".wine"));
        roots.push(home.join(".local/share/lutris"));
        roots.push(home.join("Library/Application Support/HorizonXI"));
    }

    let mut hits = Vec::new();
    for r in roots {
        if r.is_dir() {
            if let Some(found) = find_ffxi_root(&r, SEARCH_DEPTH) {
                hits.push(found);
            }
        }
    }
    hits
}

fn print_env_hint(dest: &Path) {
    println!(
        "\nThe client uses vendor/game-files by default. To point elsewhere, set:\n  \
         export FFXI_DAT_PATH=\"{}\"",
        dest.display()
    );
}

fn no_install_help() -> String {
    format!(
        "no FFXI install detected.\n\
         Get one (see README \"Getting the game files\"), then re-run:\n\
         \x20 - HorizonXI launcher (Windows): https://horizonxi.com\n\
         \x20 - Lutris (Linux):               https://lutris.net/games/horizonxi/\n\
         \x20 - or pass a path: cargo xtask game \"/path/to/.../{SQUARE_ENIX}/{FFXI}\""
    )
}

// --- official-client download (opt-in, confirmation-gated) ---

/// SE's public PlayOnline CDN for the full client installer. part1 is a
/// self-extracting exe; part2..5 are its rar volumes (must sit beside it).
fn download_official(region: &str, yes: bool, workspace: &Path) -> Result<(), String> {
    let (tag, sub) = match region {
        "us" => ("FFXIFullSetup_US", "us"),
        "eu" => ("FFXIFullSetup_EU", "eu"),
        other => return Err(format!("unknown --region `{other}` (use us or eu)")),
    };
    let base = format!("https://gdl.square-enix.com/ffxi/download/{sub}");
    let parts = [
        format!("{tag}.part1.exe"),
        format!("{tag}.part2.rar"),
        format!("{tag}.part3.rar"),
        format!("{tag}.part4.rar"),
        format!("{tag}.part5.rar"),
    ];

    println!(
        "This downloads Square Enix's official FINAL FANTASY XI client installer\n\
         (~several GB, 5 files) from {base}/ and launches it.\n\
         The download is free; a registration code / subscription is required to\n\
         actually play on the official service. On macOS/Linux the installer runs\n\
         under Wine."
    );
    if !yes && !confirm("Proceed?")? {
        return Err("aborted".into());
    }

    require_tool("curl")?;
    let dir = workspace.join("target/ffxi-installer");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;

    for name in &parts {
        let url = format!("{base}/{name}");
        let dest = dir.join(name);
        println!("Downloading {name} ...");
        curl(&url, &dest)?;
    }

    let entry = dir.join(&parts[0]);
    println!("\nLaunching installer: {}", entry.display());
    launch_installer(&entry)?;

    println!(
        "\nComplete the installer's GUI (DirectX -> PlayOnline Viewer -> FINAL FANTASY XI),\n\
         then wire the result up with:\n  cargo xtask game"
    );
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool, String> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| format!("reading input: {e}"))?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn require_tool(name: &str) -> Result<(), String> {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|_| format!("`{name}` not found on PATH — install it and retry"))
}

fn curl(url: &str, dest: &Path) -> Result<(), String> {
    // -L follow redirects, --fail on HTTP errors, -C - resume partial downloads.
    let status = Command::new("curl")
        .args(["-L", "--fail", "--retry", "3", "-C", "-", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("running curl: {e}"))?;
    if !status.success() {
        return Err(format!("curl failed for {url} ({status})"));
    }
    Ok(())
}

/// Run the self-extracting installer entry point: directly on Windows, under
/// Wine elsewhere.
fn launch_installer(exe: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "windows") {
        Command::new(exe)
    } else {
        require_tool("wine").map_err(|_| {
            "wine not found — the FFXI installer is a Windows executable. Install Wine \
             (macOS: `brew install --cask wine-stable`; Linux: your distro's winehq pkg) \
             and re-run, or run the installer yourself from target/ffxi-installer/."
                .to_string()
        })?;
        let mut c = Command::new("wine");
        c.arg(exe);
        c
    };
    // Run from the installer dir so part1.exe finds its .rar volumes.
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    let status = cmd.status().map_err(|e| format!("launching installer: {e}"))?;
    if !status.success() {
        return Err(format!("installer exited with {status}"));
    }
    Ok(())
}

// --- small fs helpers (std-only) ---

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

fn show(p: &Path) -> String {
    p.strip_prefix(workspace_root()).unwrap_or(p).display().to_string()
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| format!("symlink {} -> {}: {e}", dst.display(), src.display()))
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::windows::fs::symlink_dir(src, dst).map_err(|e| {
        format!(
            "symlink {} -> {}: {e}\n(On Windows, directory symlinks need Developer Mode or an \
             elevated shell. Re-run with --copy to copy instead.)",
            dst.display(),
            src.display()
        )
    })
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    for e in entries.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        let ft = e.file_type().map_err(|err| format!("stat {}: {err}", from.display()))?;
        if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|err| format!("copy {} -> {}: {err}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// Workspace root = the dir holding this xtask crate's parent. `CARGO_MANIFEST_DIR`
/// is `<workspace>/xtask`, so its parent is the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}
