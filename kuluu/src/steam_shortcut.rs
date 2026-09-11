//! Registers this binary as a non-Steam shortcut so Steam Deck Game Mode can
//! launch it under a per-app controller layout (kuluu-0uqd).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

pub const SHORTCUT_NAME: &str = "Kuluu";
pub const LAUNCH_OPTIONS: &str = "play";
pub const SHORTCUTS_FILE: &str = "shortcuts.vdf";
pub const BACKUP_SUFFIX: &str = ".kuluu-backup";
pub const CONTROLLER_CONFIGS_DIR: &str = "Steam Controller Configs";
pub const DECK_LAYOUT_FILE: &str = "controller_neptune.vdf";

// Steam marks shortcut app ids with the high bit; the low 31 bits are the
// CRC32 of "\"<exe>\"<AppName>". Documented at
// github.com/CorporalQuesadilla/Steam-Shortcut-Manager/wiki/Steam-Shortcuts-Documentation
const NON_STEAM_APPID_FLAG: u32 = 0x8000_0000;

// developer.valvesoftware.com/wiki/SteamID: individual-account SteamID64s
// are this constant plus the 32-bit account id that names userdata/<id>.
const STEAMID64_ACCOUNT_BASE: u64 = 76_561_197_960_265_728;

pub mod vdf {
    //! Steam's binary KeyValues, the encoding of userdata/<id>/config/shortcuts.vdf.

    use anyhow::{bail, Result};

    const TYPE_MAP: u8 = 0x00;
    const TYPE_STRING: u8 = 0x01;
    const TYPE_INT32: u8 = 0x02;
    const TYPE_FLOAT: u8 = 0x03;
    const TYPE_UINT64: u8 = 0x07;
    const TYPE_END: u8 = 0x08;
    const TYPE_INT64: u8 = 0x0A;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Map(Vec<(String, Value)>),
        Str(String),
        Int(u32),
        Float(f32),
        U64(u64),
        I64(i64),
    }

    impl Value {
        pub fn map() -> Self {
            Value::Map(Vec::new())
        }

        pub fn get(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Map(entries) => entries
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v),
                _ => None,
            }
        }

        pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
            match self {
                Value::Map(entries) => entries
                    .iter_mut()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v),
                _ => None,
            }
        }

        pub fn set(&mut self, key: &str, value: Value) {
            let Value::Map(entries) = self else {
                *self = Value::map();
                return self.set(key, value);
            };
            match entries
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
            {
                Some((_, slot)) => *slot = value,
                None => entries.push((key.to_string(), value)),
            }
        }

        pub fn as_str(&self) -> Option<&str> {
            match self {
                Value::Str(s) => Some(s),
                _ => None,
            }
        }

        pub fn as_u32(&self) -> Option<u32> {
            match self {
                Value::Int(i) => Some(*i),
                _ => None,
            }
        }

        pub fn entries(&self) -> &[(String, Value)] {
            match self {
                Value::Map(entries) => entries,
                _ => &[],
            }
        }

        pub fn entries_mut(&mut self) -> Option<&mut Vec<(String, Value)>> {
            match self {
                Value::Map(entries) => Some(entries),
                _ => None,
            }
        }
    }

    pub fn encode(root: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        for (key, value) in root.entries() {
            encode_entry(&mut out, key, value);
        }
        out.push(TYPE_END);
        out
    }

    fn encode_entry(out: &mut Vec<u8>, key: &str, value: &Value) {
        let tag = match value {
            Value::Map(_) => TYPE_MAP,
            Value::Str(_) => TYPE_STRING,
            Value::Int(_) => TYPE_INT32,
            Value::Float(_) => TYPE_FLOAT,
            Value::U64(_) => TYPE_UINT64,
            Value::I64(_) => TYPE_INT64,
        };
        out.push(tag);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        match value {
            Value::Map(entries) => {
                for (k, v) in entries {
                    encode_entry(out, k, v);
                }
                out.push(TYPE_END);
            }
            Value::Str(s) => {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Value::Int(i) => out.extend_from_slice(&i.to_le_bytes()),
            Value::Float(f) => out.extend_from_slice(&f.to_le_bytes()),
            Value::U64(u) => out.extend_from_slice(&u.to_le_bytes()),
            Value::I64(i) => out.extend_from_slice(&i.to_le_bytes()),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Value> {
        let mut cursor = Cursor { bytes, pos: 0 };
        let entries = cursor.map_body()?;
        Ok(Value::Map(entries))
    }

    struct Cursor<'a> {
        bytes: &'a [u8],
        pos: usize,
    }

    impl Cursor<'_> {
        fn byte(&mut self) -> Result<u8> {
            let b = *self
                .bytes
                .get(self.pos)
                .ok_or_else(|| anyhow::anyhow!("truncated binary vdf at byte {}", self.pos))?;
            self.pos += 1;
            Ok(b)
        }

        fn cstring(&mut self) -> Result<String> {
            let start = self.pos;
            let rel = self.bytes[start..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| anyhow::anyhow!("unterminated string at byte {start}"))?;
            self.pos = start + rel + 1;
            Ok(String::from_utf8_lossy(&self.bytes[start..start + rel]).into_owned())
        }

        fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
            let end = self.pos + N;
            let slice = self
                .bytes
                .get(self.pos..end)
                .ok_or_else(|| anyhow::anyhow!("truncated scalar at byte {}", self.pos))?;
            self.pos = end;
            Ok(slice.try_into().expect("slice length is N"))
        }

        fn map_body(&mut self) -> Result<Vec<(String, Value)>> {
            let mut entries = Vec::new();
            loop {
                if self.pos >= self.bytes.len() {
                    bail!("binary vdf ended without a closing map marker");
                }
                let tag = self.byte()?;
                if tag == TYPE_END {
                    return Ok(entries);
                }
                let key = self.cstring()?;
                let value = match tag {
                    TYPE_MAP => Value::Map(self.map_body()?),
                    TYPE_STRING => Value::Str(self.cstring()?),
                    TYPE_INT32 => Value::Int(u32::from_le_bytes(self.fixed()?)),
                    TYPE_FLOAT => Value::Float(f32::from_le_bytes(self.fixed()?)),
                    TYPE_UINT64 => Value::U64(u64::from_le_bytes(self.fixed()?)),
                    TYPE_INT64 => Value::I64(i64::from_le_bytes(self.fixed()?)),
                    other => bail!("unsupported binary vdf type 0x{other:02x} for key {key:?}"),
                };
                entries.push((key, value));
            }
        }
    }
}

pub fn crc32(bytes: &[u8]) -> u32 {
    const POLY: u32 = 0xEDB8_8320;
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (POLY & mask);
        }
    }
    !crc
}

pub fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

pub fn shortcut_app_id(quoted_exe: &str, app_name: &str) -> u32 {
    crc32(format!("{quoted_exe}{app_name}").as_bytes()) | NON_STEAM_APPID_FLAG
}

pub fn steam_root_candidates(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(home.join("Library/Application Support/Steam"));
    } else if cfg!(windows) {
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push(PathBuf::from(dir).join("Steam"));
            }
        }
        out.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    } else {
        out.push(home.join(".local/share/Steam"));
        out.push(home.join(".steam/steam"));
        out.push(home.join(".steam/root"));
        out.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
        out.push(home.join("snap/steam/common/.local/share/Steam"));
    }
    out
}

pub fn find_steam_root(home: &Path) -> Option<PathBuf> {
    steam_root_candidates(home)
        .into_iter()
        .find(|root| root.join("userdata").is_dir())
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("could not resolve the home directory"))
}

pub fn quoted_strings(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '/' && line.trim_start().starts_with("//") {
            break;
        }
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        for n in chars.by_ref() {
            match n {
                '"' => break,
                other => s.push(other),
            }
        }
        out.push(s);
    }
    out
}

pub fn most_recent_account(loginusers: &str) -> Option<u32> {
    let mut current_id: Option<u64> = None;
    let mut first_id: Option<u64> = None;
    for line in loginusers.lines() {
        match quoted_strings(line).as_slice() {
            [key] => {
                if let Ok(id) = key.parse::<u64>() {
                    current_id = Some(id);
                    first_id.get_or_insert(id);
                }
            }
            [key, value] if key.eq_ignore_ascii_case("MostRecent") && value == "1" => {
                return current_id.and_then(steamid64_to_account);
            }
            _ => {}
        }
    }
    first_id.and_then(steamid64_to_account)
}

fn steamid64_to_account(steamid64: u64) -> Option<u32> {
    let account = steamid64.checked_sub(STEAMID64_ACCOUNT_BASE)?;
    u32::try_from(account).ok()
}

pub fn userdata_accounts(steam_root: &Path) -> Vec<u32> {
    let Ok(entries) = fs::read_dir(steam_root.join("userdata")) else {
        return Vec::new();
    };
    let mut ids: Vec<u32> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .filter(|id| *id != 0)
        .collect();
    ids.sort_unstable();
    ids
}

pub fn resolve_account(steam_root: &Path, requested: Option<u32>) -> Result<u32> {
    if let Some(id) = requested {
        return Ok(id);
    }
    let accounts = userdata_accounts(steam_root);
    if let Ok(text) = fs::read_to_string(steam_root.join("config/loginusers.vdf")) {
        if let Some(id) = most_recent_account(&text) {
            if accounts.contains(&id) {
                return Ok(id);
            }
        }
    }
    match accounts.as_slice() {
        [only] => Ok(*only),
        [] => bail!(
            "no Steam accounts under {}; log into Steam once first",
            steam_root.join("userdata").display()
        ),
        many => bail!(
            "several Steam accounts ({}); pass --account <id>",
            many.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[derive(Debug, Clone)]
pub struct SteamPaths {
    pub root: PathBuf,
    pub account: u32,
}

impl SteamPaths {
    pub fn resolve(steam_dir: Option<&Path>, account: Option<u32>) -> Result<Self> {
        let root = match steam_dir {
            Some(dir) => dir.to_path_buf(),
            None => {
                let home = home_dir()?;
                find_steam_root(&home).ok_or_else(|| {
                    anyhow!(
                        "no Steam install found (looked in {}); pass --steam-dir",
                        steam_root_candidates(&home)
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?
            }
        };
        let account = resolve_account(&root, account)?;
        Ok(Self { root, account })
    }

    pub fn shortcuts_file(&self) -> PathBuf {
        self.root
            .join("userdata")
            .join(self.account.to_string())
            .join("config")
            .join(SHORTCUTS_FILE)
    }

    pub fn layout_file(&self) -> PathBuf {
        self.root
            .join("steamapps/common")
            .join(CONTROLLER_CONFIGS_DIR)
            .join(self.account.to_string())
            .join("config")
            .join(SHORTCUT_NAME.to_ascii_lowercase())
            .join(DECK_LAYOUT_FILE)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShortcutSpec {
    pub exe: PathBuf,
    pub start_dir: PathBuf,
}

impl ShortcutSpec {
    pub fn for_current_exe() -> Result<Self> {
        let exe = std::env::current_exe().context("locating this binary")?;
        let exe = exe.canonicalize().unwrap_or(exe);
        let start_dir = exe
            .parent()
            .ok_or_else(|| anyhow!("binary {} has no parent directory", exe.display()))?
            .to_path_buf();
        Ok(Self { exe, start_dir })
    }

    pub fn app_id(&self) -> u32 {
        shortcut_app_id(&quoted(&self.exe), SHORTCUT_NAME)
    }
}

fn is_kuluu_entry(entry: &vdf::Value) -> bool {
    entry
        .get("AppName")
        .and_then(vdf::Value::as_str)
        .is_some_and(|n| n.eq_ignore_ascii_case(SHORTCUT_NAME))
}

fn shortcuts_list(root: &mut vdf::Value) -> &mut Vec<(String, vdf::Value)> {
    if root.get("shortcuts").is_none() {
        root.set("shortcuts", vdf::Value::map());
    }
    let list = root.get_mut("shortcuts").expect("just inserted");
    if list.entries_mut().is_none() {
        *list = vdf::Value::map();
    }
    list.entries_mut().expect("map")
}

fn fresh_entry(spec: &ShortcutSpec) -> vdf::Value {
    use vdf::Value::{Int, Str};
    let mut e = vdf::Value::map();
    e.set("appid", Int(spec.app_id()));
    e.set("AppName", Str(SHORTCUT_NAME.into()));
    e.set("Exe", Str(quoted(&spec.exe)));
    e.set("StartDir", Str(quoted(&spec.start_dir)));
    e.set("icon", Str(String::new()));
    e.set("ShortcutPath", Str(String::new()));
    e.set("LaunchOptions", Str(LAUNCH_OPTIONS.into()));
    e.set("IsHidden", Int(0));
    e.set("AllowDesktopConfig", Int(1));
    e.set("AllowOverlay", Int(1));
    e.set("OpenVR", Int(0));
    e.set("Devkit", Int(0));
    e.set("DevkitGameID", Str(String::new()));
    e.set("DevkitOverrideAppID", Int(0));
    e.set("LastPlayTime", Int(0));
    e.set("FlatpakAppID", Str(String::new()));
    e.set("tags", vdf::Value::map());
    e
}

/// Returns true when an existing entry was updated rather than appended.
pub fn upsert_shortcut(root: &mut vdf::Value, spec: &ShortcutSpec) -> bool {
    let list = shortcuts_list(root);
    if let Some((_, entry)) = list.iter_mut().find(|(_, e)| is_kuluu_entry(e)) {
        use vdf::Value::{Int, Str};
        entry.set("appid", Int(spec.app_id()));
        entry.set("Exe", Str(quoted(&spec.exe)));
        entry.set("StartDir", Str(quoted(&spec.start_dir)));
        entry.set("LaunchOptions", Str(LAUNCH_OPTIONS.into()));
        return true;
    }
    let next_index = list.len();
    list.push((next_index.to_string(), fresh_entry(spec)));
    false
}

/// Returns how many entries were removed; surviving entries are re-indexed.
pub fn remove_shortcut(root: &mut vdf::Value) -> usize {
    let list = shortcuts_list(root);
    let before = list.len();
    list.retain(|(_, e)| !is_kuluu_entry(e));
    for (i, (key, _)) in list.iter_mut().enumerate() {
        *key = i.to_string();
    }
    before - list.len()
}

pub fn find_shortcut(root: &vdf::Value) -> Option<&vdf::Value> {
    root.get("shortcuts")?
        .entries()
        .iter()
        .map(|(_, e)| e)
        .find(|e| is_kuluu_entry(e))
}

pub fn load_shortcuts(path: &Path) -> Result<vdf::Value> {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(vdf::Value::map()),
        Ok(bytes) => vdf::decode(&bytes).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vdf::Value::map()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn save_shortcuts(path: &Path, root: &vdf::Value) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    fs::create_dir_all(dir)?;
    if path.exists() {
        let backup = path.with_file_name(format!("{SHORTCUTS_FILE}{BACKUP_SUFFIX}"));
        fs::copy(path, &backup).with_context(|| format!("backing up to {}", backup.display()))?;
    }
    let tmp = path.with_file_name(format!("{SHORTCUTS_FILE}.tmp"));
    fs::write(&tmp, vdf::encode(root))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn steam_is_running() -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq steam.exe", "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("steam.exe"))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        let name = if cfg!(target_os = "macos") {
            "steam_osx"
        } else {
            "steam"
        };
        std::process::Command::new("pgrep")
            .args(["-x", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

pub fn add_non_steam_game_url(exe: &Path) -> String {
    let encoded: String = exe
        .display()
        .to_string()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect();
    format!("steam://addnonsteamgame/{encoded}")
}

fn open_url(url: &str) -> Result<()> {
    let status = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    }
    .with_context(|| format!("opening {url}"))?;
    if !status.success() {
        bail!("URL handler exited with {status}");
    }
    Ok(())
}

pub mod cli {
    use std::path::PathBuf;

    use clap::Subcommand;

    use super::*;

    #[derive(Debug, Subcommand)]
    pub enum Action {
        /// Add or update the Kuluu entry in Steam's non-Steam shortcuts, so
        /// Game Mode can launch this binary. Run with Steam closed.
        Install {
            /// Hand the path to the running Steam client instead of editing
            /// its files; no name or layout control, no restart needed.
            #[arg(long)]
            live: bool,
            /// A Steam Input layout file to install for the shortcut.
            #[arg(long)]
            layout: Option<PathBuf>,
            /// Write even if Steam looks like it is running.
            #[arg(long)]
            force: bool,
            #[arg(long)]
            steam_dir: Option<PathBuf>,
            #[arg(long)]
            account: Option<u32>,
        },
        /// Show which Steam install and account would be edited and whether
        /// the shortcut already points at this binary.
        Status {
            #[arg(long)]
            steam_dir: Option<PathBuf>,
            #[arg(long)]
            account: Option<u32>,
        },
        /// Remove the Kuluu shortcut. Run with Steam closed.
        Remove {
            #[arg(long)]
            force: bool,
            #[arg(long)]
            steam_dir: Option<PathBuf>,
            #[arg(long)]
            account: Option<u32>,
        },
    }

    pub fn run(action: &Action) -> Result<()> {
        match action {
            Action::Install {
                live,
                layout,
                force,
                steam_dir,
                account,
            } => install(
                *live,
                layout.as_deref(),
                *force,
                steam_dir.as_deref(),
                *account,
            ),
            Action::Status { steam_dir, account } => status(steam_dir.as_deref(), *account),
            Action::Remove {
                force,
                steam_dir,
                account,
            } => remove(*force, steam_dir.as_deref(), *account),
        }
    }

    fn guard_running(force: bool) -> Result<()> {
        if steam_is_running() && !force {
            bail!(
                "Steam is running; it rewrites {SHORTCUTS_FILE} on exit and would discard this change. \
                 Quit Steam fully (Steam > Exit) and rerun, or pass --force."
            );
        }
        Ok(())
    }

    fn install(
        live: bool,
        layout: Option<&Path>,
        force: bool,
        steam_dir: Option<&Path>,
        account: Option<u32>,
    ) -> Result<()> {
        let spec = ShortcutSpec::for_current_exe()?;
        if live {
            let url = add_non_steam_game_url(&spec.exe);
            open_url(&url)?;
            println!(
                "handed {} to the running Steam client.\n\
                 Steam names the entry after the file and picks its own controller \
                 template; rename it to {SHORTCUT_NAME} and set Launch Options to \
                 \"{LAUNCH_OPTIONS}\" in its Properties.",
                spec.exe.display()
            );
            return Ok(());
        }
        let paths = SteamPaths::resolve(steam_dir, account)?;
        guard_running(force)?;
        let file = paths.shortcuts_file();
        let mut root = load_shortcuts(&file)?;
        let updated = upsert_shortcut(&mut root, &spec);
        save_shortcuts(&file, &root)?;
        println!(
            "{} {SHORTCUT_NAME} in {}\n  exe     {}\n  options {LAUNCH_OPTIONS}\n  appid   {}",
            if updated { "updated" } else { "added" },
            file.display(),
            spec.exe.display(),
            spec.app_id()
        );
        if let Some(layout) = layout {
            let dest = paths.layout_file();
            fs::create_dir_all(dest.parent().expect("layout path has a parent"))?;
            fs::copy(layout, &dest)
                .with_context(|| format!("copying {} to {}", layout.display(), dest.display()))?;
            println!("  layout  {}", dest.display());
        } else {
            println!(
                "  layout  none installed; pick the Gamepad template under the shortcut's \
                 controller settings the first time you launch"
            );
        }
        println!(
            "Next: start Steam (or switch to Game Mode) and launch {SHORTCUT_NAME} from the \
             Non-Steam library section."
        );
        Ok(())
    }

    fn status(steam_dir: Option<&Path>, account: Option<u32>) -> Result<()> {
        let paths = SteamPaths::resolve(steam_dir, account)?;
        let spec = ShortcutSpec::for_current_exe()?;
        let file = paths.shortcuts_file();
        println!("steam     {}", paths.root.display());
        println!("account   {}", paths.account);
        println!(
            "running   {}",
            if steam_is_running() { "yes" } else { "no" }
        );
        println!("shortcuts {}", file.display());
        let root = load_shortcuts(&file)?;
        match find_shortcut(&root) {
            Some(entry) => {
                let exe = entry
                    .get("Exe")
                    .and_then(vdf::Value::as_str)
                    .unwrap_or_default();
                let matches = exe == quoted(&spec.exe);
                println!(
                    "entry     {SHORTCUT_NAME} -> {exe} ({})",
                    if matches {
                        "this binary"
                    } else {
                        "a different path; rerun install"
                    }
                );
                println!(
                    "appid     {}",
                    entry.get("appid").and_then(vdf::Value::as_u32).unwrap_or(0)
                );
            }
            None => println!("entry     none; run steam-shortcut install"),
        }
        let layout = paths.layout_file();
        println!(
            "layout    {} ({})",
            layout.display(),
            if layout.is_file() {
                "present"
            } else {
                "absent"
            }
        );
        Ok(())
    }

    fn remove(force: bool, steam_dir: Option<&Path>, account: Option<u32>) -> Result<()> {
        let paths = SteamPaths::resolve(steam_dir, account)?;
        guard_running(force)?;
        let file = paths.shortcuts_file();
        let mut root = load_shortcuts(&file)?;
        let removed = remove_shortcut(&mut root);
        if removed == 0 {
            println!("no {SHORTCUT_NAME} entry in {}", file.display());
            return Ok(());
        }
        save_shortcuts(&file, &root)?;
        println!(
            "removed {removed} {SHORTCUT_NAME} entr{} from {}",
            if removed == 1 { "y" } else { "ies" },
            file.display()
        );
        let layout = paths.layout_file();
        if layout.is_file() {
            println!("left the controller layout at {}", layout.display());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vdf::Value;

    fn spec(exe: &str) -> ShortcutSpec {
        let exe = PathBuf::from(exe);
        let start_dir = exe.parent().unwrap().to_path_buf();
        ShortcutSpec { exe, start_dir }
    }

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn app_id_sets_the_high_bit_over_the_crc_of_quoted_exe_and_name() {
        assert_eq!(shortcut_app_id("\"/opt/k/kuluu\"", "Kuluu"), 3_558_984_290);
        assert_eq!(
            shortcut_app_id("\"/home/deck/Sync-ffxi-deck/kuluu\"", "Kuluu"),
            2_324_683_605
        );
        assert_eq!(spec("/opt/k/kuluu").app_id(), 3_558_984_290);
    }

    #[test]
    fn binary_vdf_encodes_the_documented_layout() {
        let mut root = Value::map();
        let mut list = Value::map();
        let mut entry = Value::map();
        entry.set("appid", Value::Int(0x8000_0001));
        entry.set("AppName", Value::Str("G".into()));
        entry.set("tags", Value::map());
        list.set("0", entry);
        root.set("shortcuts", list);
        let bytes = vdf::encode(&root);
        let expected: Vec<u8> = [
            &[0x00][..],
            b"shortcuts\0",
            &[0x00],
            b"0\0",
            &[0x02],
            b"appid\0",
            &0x8000_0001u32.to_le_bytes(),
            &[0x01],
            b"AppName\0G\0",
            &[0x00],
            b"tags\0",
            &[0x08, 0x08, 0x08, 0x08],
        ]
        .concat();
        assert_eq!(bytes, expected);
        assert_eq!(vdf::decode(&bytes).unwrap(), root);
    }

    #[test]
    fn binary_vdf_roundtrips_every_scalar_type() {
        let mut root = Value::map();
        root.set("s", Value::Str("x y".into()));
        root.set("i", Value::Int(7));
        root.set("f", Value::Float(1.5));
        root.set("u", Value::U64(u64::MAX));
        root.set("l", Value::I64(-3));
        let bytes = vdf::encode(&root);
        assert_eq!(vdf::decode(&bytes).unwrap(), root);
    }

    #[test]
    fn decode_rejects_truncated_input() {
        let mut bytes = vdf::encode(&Value::map());
        bytes.pop();
        assert!(vdf::decode(&bytes).is_err());
        assert!(vdf::decode(&[0x01, b'k', 0]).is_err());
    }

    #[test]
    fn upsert_appends_once_then_updates_in_place() {
        let mut root = Value::map();
        let mut other = Value::map();
        other.set("appid", Value::Int(1));
        other.set("AppName", Value::Str("Other".into()));
        shortcuts_list(&mut root).push(("0".into(), other));

        assert!(!upsert_shortcut(&mut root, &spec("/a/kuluu")));
        assert!(upsert_shortcut(&mut root, &spec("/b/kuluu")));
        let list = root.get("shortcuts").unwrap().entries();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].0, "1");
        let entry = find_shortcut(&root).unwrap();
        assert_eq!(entry.get("Exe").unwrap().as_str(), Some("\"/b/kuluu\""));
        assert_eq!(entry.get("StartDir").unwrap().as_str(), Some("\"/b\""));
        assert_eq!(
            entry.get("LaunchOptions").unwrap().as_str(),
            Some(LAUNCH_OPTIONS)
        );
        assert_eq!(
            entry.get("appid").unwrap().as_u32(),
            Some(spec("/b/kuluu").app_id())
        );
        assert_eq!(entry.get("AllowDesktopConfig").unwrap().as_u32(), Some(1));
        assert!(matches!(entry.get("tags"), Some(Value::Map(m)) if m.is_empty()));
    }

    #[test]
    fn remove_drops_every_kuluu_entry_and_reindexes() {
        let mut root = Value::map();
        upsert_shortcut(&mut root, &spec("/a/kuluu"));
        let mut dup = Value::map();
        dup.set("AppName", Value::Str("kuluu".into()));
        shortcuts_list(&mut root).push(("1".into(), dup));
        let mut other = Value::map();
        other.set("AppName", Value::Str("Other".into()));
        shortcuts_list(&mut root).push(("2".into(), other));

        assert_eq!(remove_shortcut(&mut root), 2);
        let list = root.get("shortcuts").unwrap().entries();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "0");
        assert!(find_shortcut(&root).is_none());
        assert_eq!(remove_shortcut(&mut root), 0);
    }

    #[test]
    fn save_backs_up_and_load_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config").join(SHORTCUTS_FILE);
        assert_eq!(load_shortcuts(&file).unwrap(), Value::map());

        let mut root = Value::map();
        upsert_shortcut(&mut root, &spec("/a/kuluu"));
        save_shortcuts(&file, &root).unwrap();
        assert!(!file
            .with_file_name(format!("{SHORTCUTS_FILE}{BACKUP_SUFFIX}"))
            .exists());

        let mut second = load_shortcuts(&file).unwrap();
        assert_eq!(second, root);
        upsert_shortcut(&mut second, &spec("/b/kuluu"));
        save_shortcuts(&file, &second).unwrap();
        let backup = file.with_file_name(format!("{SHORTCUTS_FILE}{BACKUP_SUFFIX}"));
        assert_eq!(vdf::decode(&fs::read(backup).unwrap()).unwrap(), root);
        assert_eq!(load_shortcuts(&file).unwrap(), second);
    }

    #[test]
    fn most_recent_account_comes_from_loginusers() {
        let text = r#"
"users"
{
	"76561197960287930"
	{
		"AccountName"		"old"
		"MostRecent"		"0"
	}
	"76561198012345678"
	{
		"AccountName"		"deck"
		"MostRecent"		"1"
	}
}
"#;
        assert_eq!(most_recent_account(text), Some(52_079_950));
        let none_marked = text.replace("\"MostRecent\"\t\t\"1\"", "\"MostRecent\"\t\t\"0\"");
        assert_eq!(most_recent_account(&none_marked), Some(22_202));
        assert_eq!(most_recent_account(""), None);
    }

    #[test]
    fn resolve_account_prefers_loginusers_then_single_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("userdata/22202/config")).unwrap();
        fs::create_dir_all(root.join("userdata/52079950/config")).unwrap();
        fs::create_dir_all(root.join("userdata/0")).unwrap();
        assert!(resolve_account(root, None).is_err());
        assert_eq!(resolve_account(root, Some(5)).unwrap(), 5);

        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/loginusers.vdf"),
            "\"users\"\n{\n\t\"76561198012345678\"\n\t{\n\t\t\"MostRecent\"\t\t\"1\"\n\t}\n}\n",
        )
        .unwrap();
        assert_eq!(resolve_account(root, None).unwrap(), 52_079_950);

        fs::remove_dir_all(root.join("userdata/52079950")).unwrap();
        assert_eq!(resolve_account(root, None).unwrap(), 22_202);
    }

    #[test]
    fn steam_paths_place_layout_under_lowercased_shortcut_name() {
        let paths = SteamPaths {
            root: PathBuf::from("/s"),
            account: 7,
        };
        assert_eq!(
            paths.shortcuts_file(),
            PathBuf::from("/s/userdata/7/config/shortcuts.vdf")
        );
        assert_eq!(
            paths.layout_file(),
            PathBuf::from("/s/steamapps/common/Steam Controller Configs/7/config/kuluu/controller_neptune.vdf")
        );
    }

    #[test]
    fn find_steam_root_requires_a_userdata_dir() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_eq!(find_steam_root(home), None);
        let candidate = steam_root_candidates(home)
            .into_iter()
            .find(|c| c.starts_with(home))
            .unwrap();
        fs::create_dir_all(candidate.join("userdata")).unwrap();
        assert_eq!(find_steam_root(home), Some(candidate));
    }

    #[test]
    fn add_non_steam_game_url_percent_encodes_spaces_and_quotes() {
        assert_eq!(
            add_non_steam_game_url(Path::new("/home/deck/My Games/kuluu")),
            "steam://addnonsteamgame//home/deck/My%20Games/kuluu"
        );
    }
}
