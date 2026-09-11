//! Project automation, invoked via the `cargo xtask` alias (.cargo/config.toml).
//!
//! ## `cargo xtask ffxi-client <verb>`
//!
//! Wire, list, download or patch the FFXI client installs under
//! `vendor/game-files/`, the workspace-side install roots (the default
//! `vendor/game-files/SquareEnix/FINAL FANTASY XI` plus named targets under
//! `vendor/game-files/targets/NAME/`). What the client actually loads is
//! decided by `kuluu ffxi-client which`, which also knows the launcher's
//! saved choice and the per-user client directory; this tool only manages
//! the checkout.
//!
//! - `link [PATH] [--target NAME] [--copy] [--force]`: detect an existing
//!   install (HorizonXI / Lutris / Wine / CrossOver / PlayOnline / a Parallels
//!   shared drive) or take PATH, validate it, and symlink it into place;
//!   `--copy` copies instead, `--force` replaces an existing link.
//! - `list`: the default install and every named target.
//! - `download [--region us|eu] [--target NAME] [--yes] [--no-update]`:
//!   opt-in, confirmation-gated download of Square Enix's official installer
//!   from the public PlayOnline CDN, unpacked natively by `ffxi-install`
//!   (RAR volumes, MSIs and cabinets; no Wine) into `targets/NAME` (default
//!   `retail`), then patched to the current version unless `--no-update`.
//! - `update (PATH | --target NAME) [--verify] [--yes]`: patch an install to
//!   the current retail version over the PlayOnline patch protocol (no
//!   viewer, no Wine). The unnamed default is never patched implicitly: it is
//!   usually a private server's pinned client, which retail patches would
//!   break. `--verify` re-checks every file even when the manifest stamp is
//!   already current.
//!
//! Downloading the client is free; a registration code / subscription is
//! needed to *play* on the official service.
//!
//! ## `cargo xtask install-hooks [--check]`
//!
//! Activate the versioned git hooks in `.githooks/` for this clone by pointing
//! `core.hooksPath` at it (the fmt+clippy pre-push gate plus Beads lifecycle
//! hooks). Mirrors
//! `scripts/install-hooks.sh`, kept as a compile-free fast path. It's per-clone
//! because `git config` writes the uncommitted `.git/config` — git won't let a
//! repo auto-enable its own hooks. `--check` only verifies (non-zero exit when
//! inactive) so the README / CI / a setup doctor can assert the gate is live.
//!
//! HTTP goes through `curl`.

mod dlss;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The install layout the client expects under `vendor/game-files/`.
const SQUARE_ENIX: &str = "SquareEnix";
const FFXI: &str = "FINAL FANTASY XI";
const GAME_FILES: &str = "vendor/game-files";
/// Named installs, mirrored by `ffxi_dat::archive::TARGETS_DIR`.
const TARGETS: &str = "targets";
const CLIENT_TARGET_ENV: &str = "FFXI_CLIENT_TARGET";
/// Where `--download` lands unless `--target` says otherwise.
const DEFAULT_DOWNLOAD_TARGET: &str = "retail";
/// File that proves a directory is the FFXI client DAT root.
const MARKER: &str = "VTABLE.DAT";
/// How deep to descend under each detection root looking for the marker.
const SEARCH_DEPTH: usize = 6;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dlss") => match dlss::run(&args[1..], &workspace_root()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some("ffxi-client") => match cmd_ffxi_client(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some("game") => {
            eprintln!("`cargo xtask game` is now `cargo xtask ffxi-client <verb>`\n");
            usage();
            ExitCode::FAILURE
        }
        Some("install-hooks") => match cmd_install_hooks(&args[1..]) {
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
        "usage: cargo xtask ffxi-client link [PATH] [--target NAME] [--copy] [--force]\n\
         \x20      cargo xtask ffxi-client list\n\
         \x20      cargo xtask ffxi-client download [--region us|eu] [--target NAME] [--yes] [--no-update]\n\
         \x20      cargo xtask ffxi-client update (PATH | --target NAME) [--verify] [--yes]\n\
         \x20      cargo xtask install-hooks [--check]\n\
         \n\
         Manage the FFXI client installs under vendor/game-files/ (the checkout side;\n\
         `kuluu ffxi-client which` tells you what the client will actually load).\n\
         link        wire an install: PATH, or auto-detect one\n\
         list        show the default install and every named target\n\
         download    fetch SE's official installer, unpack it natively into\n\
         \x20           targets/NAME (default: retail), then update it\n\
         update      patch an install to the current retail version over the\n\
         \x20           PlayOnline patch protocol; never the unnamed default\n\
         --target    a named install under vendor/game-files/targets/NAME/;\n\
         \x20           select it at runtime with FFXI_CLIENT_TARGET=NAME\n\
         --copy      copy instead of symlinking (link)\n\
         --force     replace an existing link (link)\n\
         --verify    re-check every file, not just the manifest stamp (update)\n\
         --no-update leave the 2019 base image unpatched (download)\n\
         --region    us (default) or eu (download)\n\
         --yes       skip confirmation prompts\n\
         \n\
         DLSS: cargo xtask dlss <check|build>\n\
         \n\
         Activate the versioned git hooks (.githooks/) for this clone.\n\
         --check     verify the versioned hooks are active; non-zero exit if not"
    );
}

#[derive(Default)]
struct ClientArgs {
    path: Option<PathBuf>,
    target: Option<String>,
    copy: bool,
    force: bool,
    verify: bool,
    yes: bool,
    no_update: bool,
    region: Option<String>,
    given: Vec<&'static str>,
}

const FLAGS_LINK: &[&str] = &["--target", "--copy", "--force"];
const FLAGS_LIST: &[&str] = &[];
const FLAGS_DOWNLOAD: &[&str] = &["--target", "--region", "--yes", "--no-update"];
const FLAGS_UPDATE: &[&str] = &["--target", "--verify", "--yes"];

impl ClientArgs {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = Self::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--copy" => {
                    out.copy = true;
                    out.given.push("--copy");
                }
                "--force" => {
                    out.force = true;
                    out.given.push("--force");
                }
                "--verify" => {
                    out.verify = true;
                    out.given.push("--verify");
                }
                "--no-update" => {
                    out.no_update = true;
                    out.given.push("--no-update");
                }
                "--yes" | "-y" => {
                    out.yes = true;
                    out.given.push("--yes");
                }
                "--target" => {
                    let name = it.next().ok_or("--target needs a NAME")?;
                    if name.is_empty()
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    {
                        return Err(format!(
                            "--target `{name}` must be [A-Za-z0-9_-]+ (it becomes a directory name)"
                        ));
                    }
                    out.target = Some(name.clone());
                    out.given.push("--target");
                }
                "--region" => {
                    out.region = Some(
                        it.next()
                            .ok_or("--region needs a value (us|eu)")?
                            .to_lowercase(),
                    );
                    out.given.push("--region");
                }
                s if s.starts_with('-') => return Err(format!("unknown flag `{s}`")),
                s => out.path = Some(PathBuf::from(s)),
            }
        }
        Ok(out)
    }

    fn only(&self, verb: &str, allowed: &[&str], takes_path: bool) -> Result<(), String> {
        if let Some(flag) = self.given.iter().find(|f| !allowed.contains(f)) {
            return Err(format!("`{verb}` does not take {flag}"));
        }
        if !takes_path {
            if let Some(p) = &self.path {
                return Err(format!("`{verb}` does not take a path ({})", p.display()));
            }
        }
        Ok(())
    }
}

fn cmd_ffxi_client(args: &[String]) -> Result<(), String> {
    let Some((verb, rest)) = args.split_first() else {
        usage();
        return Err("ffxi-client needs a verb (link|list|download|update)".into());
    };
    let opts = ClientArgs::parse(rest)?;
    let workspace = workspace_root();
    match verb.as_str() {
        "link" => {
            opts.only("link", FLAGS_LINK, true)?;
            link_install(&workspace, &opts)
        }
        "list" => {
            opts.only("list", FLAGS_LIST, false)?;
            list_installs(&workspace)
        }
        "download" => {
            opts.only("download", FLAGS_DOWNLOAD, false)?;
            download_official(&workspace, &opts)
        }
        "update" => {
            opts.only("update", FLAGS_UPDATE, true)?;
            update_verb(&workspace, &opts)
        }
        other => Err(format!(
            "unknown verb `{other}` (link|list|download|update)"
        )),
    }
}

/// `SquareEnix/` parent and the DAT root for the default install or a named target.
fn install_dirs(workspace: &Path, target: Option<&str>) -> (PathBuf, PathBuf) {
    let game_files = workspace.join(GAME_FILES);
    let dest_se = match target {
        Some(name) => game_files.join(TARGETS).join(name).join(SQUARE_ENIX),
        None => game_files.join(SQUARE_ENIX),
    };
    let dest = dest_se.join(FFXI);
    (dest_se, dest)
}

fn link_install(workspace: &Path, opts: &ClientArgs) -> Result<(), String> {
    let (dest_se, dest) = install_dirs(workspace, opts.target.as_deref());

    if is_ffxi_root(&dest) {
        println!(
            "{} already has a valid install:\n  {}",
            show(&dest_se),
            show(&dest)
        );
        print_env_hint(&dest, opts.target.as_deref());
        return Ok(());
    }

    let source = match &opts.path {
        Some(p) => find_ffxi_root(p, SEARCH_DEPTH).ok_or_else(|| {
            format!(
                "no FFXI install (a dir containing {MARKER}) found at or under {}",
                p.display()
            )
        })?,
        None => {
            let mut hits = detect();
            hits.dedup();
            match hits.len() {
                0 => return Err(no_install_help()),
                1 => hits.into_iter().next().unwrap(),
                _ => {
                    let mut msg = String::from("multiple installs detected; re-run with one:\n");
                    for h in &hits {
                        msg.push_str(&format!(
                            "  cargo xtask ffxi-client link \"{}\"\n",
                            h.display()
                        ));
                    }
                    return Err(msg);
                }
            }
        }
    };

    println!("Using install: {}", source.display());

    if dest.exists() || is_symlink(&dest) {
        if is_symlink(&dest) && opts.force {
            std::fs::remove_file(&dest)
                .map_err(|e| format!("removing old link {}: {e}", dest.display()))?;
        } else if is_symlink(&dest) {
            return Err(format!(
                "{} is already a link (to {}). Re-run with --force to replace it.",
                dest.display(),
                std::fs::read_link(&dest)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ));
        } else {
            return Err(format!(
                "{} already exists and is not a symlink; move it aside first.",
                dest.display()
            ));
        }
    }

    std::fs::create_dir_all(&dest_se)
        .map_err(|e| format!("creating {}: {e}", dest_se.display()))?;

    if opts.copy {
        println!("Copying (this can be ~19 GB) ...");
        copy_dir(&source, &dest)?;
    } else {
        symlink_dir(&source, &dest)?;
        println!("Linked {} -> {}", show(&dest), source.display());
    }

    if !is_ffxi_root(&dest) {
        return Err(format!(
            "wired {} but it does not validate ({MARKER} missing); install may be incomplete",
            dest.display()
        ));
    }
    println!("OK: {} is ready.", show(&dest_se));
    print_env_hint(&dest, opts.target.as_deref());
    Ok(())
}

fn update_verb(workspace: &Path, opts: &ClientArgs) -> Result<(), String> {
    let root = match (&opts.path, &opts.target) {
        (Some(p), _) => find_ffxi_root(p, SEARCH_DEPTH)
            .ok_or_else(|| format!("no FFXI install found at or under {}", p.display()))?,
        (None, Some(name)) => {
            let (_, dest) = install_dirs(workspace, Some(name));
            if !is_ffxi_root(&dest) {
                return Err(format!("{} holds no install to update", show(&dest)));
            }
            dest
        }
        (None, None) => {
            return Err(
                "update needs a PATH or --target NAME. The unnamed default install is \
                 never patched implicitly: it is usually a private server's pinned client, \
                 and retail patches would break it."
                    .into(),
            )
        }
    };
    println!(
        "Install: {}\nLocal patch version: {}",
        root.display(),
        ffxi_install::update::local_version(&root).unwrap_or_else(|| "none".into())
    );
    if !opts.yes && !confirm("Patch this install toward the current retail version?")? {
        return Err("aborted".into());
    }
    update_install(&root, opts.verify)
}

/// Patch an install to the current retail version by speaking the
/// PlayOnline patch protocol directly (`ffxi_install::update`); nothing of
/// SE's viewer runs.
fn update_install(root: &Path, verify: bool) -> Result<(), String> {
    let options = ffxi_install::update::Options { force: verify };
    match ffxi_install::update::run(root, options, &ffxi_install::report::print_progress)? {
        None => println!("already at the server's version; pass --verify to re-check every file"),
        Some(outcome) => println!(
            "now at {} ({} file(s) fetched, {} MB)",
            outcome.version,
            outcome.fetched,
            outcome.bytes / 1_000_000
        ),
    }
    println!(
        "Identify the patched build with:\n  \
         cargo run -p ffxi-dat --example dat-client-profile -- \"{}\"",
        root.display()
    );
    Ok(())
}

fn list_installs(workspace: &Path) -> Result<(), String> {
    let game_files = workspace.join(GAME_FILES);
    let describe = |dir: &Path| -> String {
        if is_ffxi_root(dir) {
            match std::fs::read_link(dir) {
                Ok(link) => format!("-> {}", link.display()),
                Err(_) => "(directory)".to_string(),
            }
        } else {
            "(missing)".to_string()
        }
    };
    let default = game_files.join(SQUARE_ENIX).join(FFXI);
    println!("default   {}  {}", show(&default), describe(&default));
    let targets = game_files.join(TARGETS);
    let mut names: Vec<String> = match std::fs::read_dir(&targets) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    for name in names {
        let dir = targets.join(&name).join(SQUARE_ENIX).join(FFXI);
        println!("{name:<9} {}  {}", show(&dir), describe(&dir));
    }
    println!(
        "\nSelect a named target with {CLIENT_TARGET_ENV}=NAME, or persist a choice with\n  \
         cargo run -p kuluu -- ffxi-client use NAME\n\
         `cargo run -p kuluu -- ffxi-client which` shows what the client will load."
    );
    Ok(())
}

// --- git hook activation ---

/// The hooks dir we point `core.hooksPath` at, relative to the workspace root.
const HOOKS_DIR: &str = ".githooks";

/// Activate (or, with `--check`, verify) the versioned git hooks for this clone.
/// Sets `core.hooksPath=.githooks` so the pre-push gate and Beads lifecycle hooks
/// run. The same effect as `scripts/install-hooks.sh`; both write byte-identical
/// config so they can't drift.
fn cmd_install_hooks(args: &[String]) -> Result<(), String> {
    let mut check = false;
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            s => {
                return Err(format!(
                    "unknown flag `{s}` (install-hooks takes only --check)"
                ))
            }
        }
    }

    require_tool("git")?;
    let workspace = workspace_root();
    // Guard against running outside the repo: the hook we're enabling must exist.
    if !workspace.join(HOOKS_DIR).join("pre-push").is_file() {
        return Err(format!(
            "{HOOKS_DIR}/pre-push not found under {} — run this from the ffxi repo",
            show(&workspace)
        ));
    }

    if check {
        return match git_config_get(&workspace, "core.hooksPath")?.as_deref() {
            Some(HOOKS_DIR) => {
                println!("ok: git hooks active (core.hooksPath={HOOKS_DIR})");
                Ok(())
            }
            Some(other) => Err(format!(
                "git hooks NOT active: core.hooksPath={other} (expected {HOOKS_DIR})\n\
                 run: cargo xtask install-hooks"
            )),
            None => Err("git hooks NOT active: core.hooksPath is unset\n\
                 run: cargo xtask install-hooks"
                .to_string()),
        };
    }

    git_config_set(&workspace, "core.hooksPath", HOOKS_DIR)?;
    make_executable(&workspace.join(HOOKS_DIR));
    println!("installed: core.hooksPath={HOOKS_DIR} (checks and Beads integration active)");
    println!("bypass a push with: git push --no-verify");
    Ok(())
}

/// Set a git config key in the workspace repo (writes `.git/config`).
fn git_config_set(workspace: &Path, key: &str, value: &str) -> Result<(), String> {
    let status = Command::new("git")
        .current_dir(workspace)
        .args(["config", key, value])
        .status()
        .map_err(|e| format!("running git config: {e}"))?;
    if !status.success() {
        return Err(format!("`git config {key} {value}` failed ({status})"));
    }
    Ok(())
}

/// Read a git config key; `None` if unset (`git config --get` exits 1 for that).
fn git_config_get(workspace: &Path, key: &str) -> Result<Option<String>, String> {
    let out = Command::new("git")
        .current_dir(workspace)
        .args(["config", "--get", key])
        .output()
        .map_err(|e| format!("running git config: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!val.is_empty()).then_some(val))
}

/// `chmod +x` every file in the hooks dir so git can run them. No-op on Windows,
/// where git ignores the unix exec bit.
#[cfg(unix)]
fn make_executable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&p) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o111);
            let _ = std::fs::set_permissions(&p, perms);
        }
    }
}

#[cfg(not(unix))]
fn make_executable(_dir: &Path) {}

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
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
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

/// Parallels Desktop mounts a guest's drives as `/Volumes/[C] <VM name>`; the
/// retail PlayOnline tree inside one is the usual way a macOS host reaches a
/// current retail client.
fn parallels_shared_drives() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('['))
        })
        .flat_map(|p| {
            [
                p.join("Program Files (x86)/PlayOnline"),
                p.join("Program Files (x86)/HorizonXI"),
                p.join("Program Files (x86)/SquareEnix"),
            ]
        })
        .collect()
}

/// Platform-specific likely install locations that actually exist on disk.
fn detect() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    if cfg!(target_os = "windows") {
        for drive in ["C:\\", "D:\\"] {
            roots.push(PathBuf::from(format!(
                "{drive}Program Files (x86)\\PlayOnline"
            )));
            roots.push(PathBuf::from(format!(
                "{drive}Program Files (x86)\\HorizonXI"
            )));
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
    roots.extend(parallels_shared_drives());

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

fn print_env_hint(dest: &Path, target: Option<&str>) {
    match target {
        Some(name) => println!(
            "\nThe client uses vendor/game-files by default. To use this target instead:\n  \
             export {CLIENT_TARGET_ENV}={name}        # this shell\n  \
             cargo run -p kuluu -- ffxi-client use {name}   # persisted for the launcher"
        ),
        None => println!(
            "\nThe client uses vendor/game-files by default. To point elsewhere:\n  \
             export FFXI_DAT_PATH=\"{}\"",
            dest.display()
        ),
    }
}

fn no_install_help() -> String {
    format!(
        "no FFXI install detected.\n\
         Get one (see README \"Getting the game files\"), then re-run:\n\
         \x20 - cargo xtask ffxi-client download   (Square Enix's official client)\n\
         \x20 - HorizonXI launcher (Windows): https://horizonxi.com\n\
         \x20 - Lutris (Linux):               https://lutris.net/games/horizonxi/\n\
         \x20 - or pass a path: cargo xtask ffxi-client link \"/path/to/.../{SQUARE_ENIX}/{FFXI}\""
    )
}

// --- official-client download (opt-in, confirmation-gated) ---

fn download_official(workspace: &Path, opts: &ClientArgs) -> Result<(), String> {
    let region = ffxi_install::region(opts.region.as_deref().unwrap_or("us"))?;
    let target = opts.target.as_deref().unwrap_or(DEFAULT_DOWNLOAD_TARGET);
    let installer_dir = workspace.join("target/ffxi-installer");
    let target_root = workspace.join(GAME_FILES).join(TARGETS).join(target);
    println!(
        "This downloads Square Enix's official FINAL FANTASY XI client installer\n\
         (5 volumes, ~7.2 GB) from {}/{}/ and unpacks it into\n  {}{}",
        ffxi_install::CDN_BASE,
        region.sub,
        show(&target_root),
        if opts.no_update {
            ""
        } else {
            "\nthen patches it to the current retail version (~0.5 GB more)."
        }
    );
    if !opts.yes && !confirm("Proceed?")? {
        return Err("aborted".into());
    }
    require_tool("curl")?;
    let plan = ffxi_install::Plan {
        region,
        installer_dir: &installer_dir,
        target_root: &target_root,
    };
    ffxi_install::download_and_unpack(&plan, &ffxi_install::report::print_progress)?;
    let ffxi = target_root.join(SQUARE_ENIX).join(FFXI);
    if !is_ffxi_root(&ffxi) {
        return Err(format!(
            "unpack finished but {} does not validate ({MARKER} missing)",
            ffxi.display()
        ));
    }
    if opts.no_update {
        println!(
            "\nThis is SE's 2019 base image; patch it later with\n  \
             cargo xtask ffxi-client update --target {target}"
        );
    } else {
        update_install(&ffxi, false)?;
    }
    print_env_hint(&ffxi, Some(target));
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool, String> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("reading input: {e}"))?;
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

// --- small fs helpers (std-only) ---

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn show(p: &Path) -> String {
    p.strip_prefix(workspace_root())
        .unwrap_or(p)
        .display()
        .to_string()
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(src, dst)
        .map_err(|e| format!("symlink {} -> {}: {e}", dst.display(), src.display()))
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
        let ft = e
            .file_type()
            .map_err(|err| format!("stat {}: {err}", from.display()))?;
        if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|err| format!("copy {} -> {}: {err}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// Workspace root = the dir holding this xtask crate's parent. `CARGO_MANIFEST_DIR`
/// is `<workspace>/xtask`, so its parent is the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
