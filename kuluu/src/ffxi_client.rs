//! Which FFXI client install kuluu loads, and the verbs that manage the
//! per-user client directory. The checkout-side installs under
//! `vendor/game-files` belong to `cargo xtask ffxi-client`; this module reads
//! those, the launcher's saved choice, and the clients it downloaded itself,
//! and settles them into one `FFXI_DAT_PATH` for the rest of the process.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ffxi_dat::archive::{self, CLIENT_TARGET_ENV, DAT_PATH_ENV};
use ffxi_dat::client_profile::ClientProfile;
use ffxi_dat::install_detect;

use crate::launcher_store::{self, EnvOverride, Settings};

pub const DEFAULT_DOWNLOAD_NAME: &str = "retail";
pub const DEFAULT_REGION: &str = "us";
const INSTALLER_CACHE_DIR: &str = "ffxi-installer";
/// Display name of the unnamed checkout install.
pub const WORKSPACE_DEFAULT_NAME: &str = "default";
pub const INSTALLER_SIZE_NOTE: &str = "5 volumes, ~7.2 GB";
pub const PATCH_SIZE_NOTE: &str = "~0.5 GB";

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn user_clients_dir() -> Option<PathBuf> {
    kuluu_session::config_dir::clients_dir().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    UserClient,
    WorkspaceTarget,
    WorkspaceDefault,
    Detected,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::UserClient => "user",
            Origin::WorkspaceTarget => "workspace",
            Origin::WorkspaceDefault => "default",
            Origin::Detected => "detected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub name: String,
    pub origin: Origin,
    pub path: PathBuf,
}

pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn child_dirs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// The directory above `SquareEnix/`, which is how a detected install is
/// usually recognised (`HorizonXI`, `PlayOnline`, a bottle name).
fn detected_name(root: &Path) -> String {
    root.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// Every install kuluu can see: user clients, then workspace targets, the
/// workspace default, then auto-detected third-party installs. A directory
/// reachable under several names is listed once, under the first.
pub fn installs() -> Vec<Install> {
    let mut out: Vec<Install> = Vec::new();
    let mut push = |name: String, origin: Origin, path: PathBuf| {
        if install_detect::is_ffxi_root(&path) && !out.iter().any(|i| same_dir(&i.path, &path)) {
            out.push(Install { name, origin, path });
        }
    };
    if let Some(dir) = user_clients_dir() {
        for name in child_dirs(&dir) {
            let path = archive::target_install_dir(&dir, &name);
            push(name, Origin::UserClient, path);
        }
    }
    if let Some(targets) = archive::workspace_targets_dir() {
        for name in child_dirs(&targets) {
            let path = archive::target_install_dir(&targets, &name);
            push(name, Origin::WorkspaceTarget, path);
        }
    }
    if let Some(path) = archive::workspace_default() {
        push(
            WORKSPACE_DEFAULT_NAME.to_string(),
            Origin::WorkspaceDefault,
            path,
        );
    }
    for path in install_detect::detect() {
        push(detected_name(&path), Origin::Detected, path);
    }
    out
}

/// A named client: the user directory first, then the checkout's targets.
pub fn named(name: &str) -> Option<PathBuf> {
    user_clients_dir()
        .map(|dir| archive::target_install_dir(&dir, name))
        .filter(|p| install_detect::is_ffxi_root(p))
        .or_else(|| archive::workspace_target(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// launcher.json, with its override tick beating a set `FFXI_DAT_PATH`.
    ConfigOverride,
    EnvPath,
    Config,
    EnvTarget(String),
    WorkspaceDefault,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::ConfigOverride => {
                write!(f, "launcher.json (its Override tick beats the environment)")
            }
            Source::EnvPath => write!(f, "{DAT_PATH_ENV} from the environment"),
            Source::Config => write!(f, "launcher.json"),
            Source::EnvTarget(name) => {
                write!(f, "{CLIENT_TARGET_ENV}={name} from the environment")
            }
            Source::WorkspaceDefault => {
                write!(f, "the checkout default, {}", archive::DEFAULT_INSTALL_DIR)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub source: Source,
    pub reason: String,
}

impl fmt::Display for Unresolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.reason, self.source)
    }
}

fn env_string(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

/// `FFXI_DAT_PATH` as the shell handed it to us, captured before [`export`]
/// writes the settled choice back into the environment, so a later resolve
/// (settings saved mid-session) still sees the user's value, not our own.
pub fn shell_dat_path() -> Option<&'static str> {
    static SHELL: OnceLock<Option<String>> = OnceLock::new();
    SHELL.get_or_init(|| env_string(DAT_PATH_ENV)).as_deref()
}

/// The install the process will load, and why: the launcher's saved path when
/// its override tick is set, else `FFXI_DAT_PATH`, else `FFXI_CLIENT_TARGET`
/// by name, else the launcher's saved path, else the checkout default.
pub fn resolve(settings: &Settings) -> Result<Located, Unresolved> {
    let env_path = shell_dat_path().map(PathBuf::from);
    let config = settings.dat_path.value.trim();
    let from_config = |source: Source| Located {
        path: PathBuf::from(config),
        source,
    };
    if !config.is_empty() && settings.dat_path.override_env {
        let source = if env_path.is_some() || env_string(CLIENT_TARGET_ENV).is_some() {
            Source::ConfigOverride
        } else {
            Source::Config
        };
        return Ok(from_config(source));
    }
    if let Some(path) = env_path {
        return Ok(Located {
            path,
            source: Source::EnvPath,
        });
    }
    if let Some(name) = env_string(CLIENT_TARGET_ENV) {
        let source = Source::EnvTarget(name.clone());
        return match named(&name) {
            Some(path) => Ok(Located { path, source }),
            None => Err(Unresolved {
                source,
                reason: format!(
                    "no client named `{name}` under {} or {}",
                    user_clients_dir()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "the user client directory".into()),
                    archive::TARGETS_DIR
                ),
            }),
        };
    }
    if !config.is_empty() {
        return Ok(from_config(Source::Config));
    }
    archive::workspace_default()
        .map(|path| Located {
            path,
            source: Source::WorkspaceDefault,
        })
        .ok_or_else(|| Unresolved {
            source: Source::WorkspaceDefault,
            reason: "no install wired and nothing configured".to_string(),
        })
}

/// Settle the choice into the environment so every `DatRoot::from_env_or_default`
/// caller in the process agrees with the launcher and the CLI; the other
/// saved overrides (navmesh dir, MAC) are applied alongside.
pub fn export(settings: &Settings) -> Result<Located, Unresolved> {
    shell_dat_path();
    let located = resolve(settings);
    for (var, ov) in settings.entries() {
        if var == DAT_PATH_ENV {
            continue;
        }
        if let Some(v) = ov.resolved(var) {
            std::env::set_var(var, v);
        }
    }
    match &located {
        Ok(l) => std::env::set_var(DAT_PATH_ENV, &l.path),
        Err(_) => std::env::remove_var(DAT_PATH_ENV),
    }
    located
}

/// Persist `path` as the launcher's install. The override tick is left off so
/// a shell env var still wins for one-off runs; the settings screen can set it.
pub fn persist(path: &Path) -> Result<(), String> {
    let mut store = launcher_store::load();
    store.settings.dat_path = EnvOverride {
        value: path.display().to_string(),
        override_env: false,
    };
    launcher_store::save(&store).map_err(|e| format!("writing launcher.json: {e}"))
}

/// `spec` is a client name or a path (an install root or a directory above one).
pub fn locate_spec(spec: &str) -> Result<PathBuf, String> {
    let as_path = Path::new(spec);
    if as_path.is_dir() {
        return install_detect::find_ffxi_root(as_path, install_detect::DEFAULT_SEARCH_DEPTH)
            .ok_or_else(|| format!("no FFXI install at or under {spec}"));
    }
    if let Some(path) = named(spec) {
        return Ok(path);
    }
    installs()
        .into_iter()
        .find(|i| i.name == spec)
        .map(|i| i.path)
        .ok_or_else(|| format!("`{spec}` is neither a directory nor a known client name"))
}

pub fn use_install(spec: &str) -> Result<PathBuf, String> {
    let path = locate_spec(spec)?;
    persist(&path)?;
    Ok(path)
}

/// Fetch Square Enix's installer into the user client directory as `name`.
/// Returns the DAT root; the 2019 base image still needs [`update`].
pub fn download(
    name: &str,
    region: &str,
    report: &ffxi_install::Reporter,
) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "client name `{name}` must be [A-Za-z0-9_-]+ (it becomes a directory name)"
        ));
    }
    let clients = user_clients_dir().ok_or("no user data directory on this system")?;
    let target_root = clients.join(name);
    let installer_dir =
        kuluu_session::config_dir::cache_dir(INSTALLER_CACHE_DIR).map_err(|e| e.to_string())?;
    let plan = ffxi_install::Plan {
        region: ffxi_install::region(region)?,
        installer_dir: &installer_dir,
        target_root: &target_root,
    };
    ffxi_install::download_and_unpack(&plan, report)?;
    let root = target_root.join(archive::INSTALL_SUBDIR);
    if !install_detect::is_ffxi_root(&root) {
        return Err(format!(
            "unpack finished but {} does not validate ({} missing)",
            root.display(),
            install_detect::VTABLE_MARKER
        ));
    }
    Ok(root)
}

/// A known non-retail build (a private server's pinned client) must never be
/// patched toward retail; an unknown build is allowed through, since a
/// freshly patched retail install is unknown until its row is measured.
pub fn refuse_non_retail(root: &Path) -> Result<(), String> {
    match ClientProfile::probe(root).known {
        Some(k) if !k.retail => Err(format!(
            "{} is {}, not a retail lineage; the PlayOnline patch server would break it",
            root.display(),
            k.name
        )),
        _ => Ok(()),
    }
}

pub fn update(
    root: &Path,
    verify: bool,
    report: &ffxi_install::Reporter,
) -> Result<Option<ffxi_install::update::Outcome>, String> {
    refuse_non_retail(root)?;
    ffxi_install::update::run(
        root,
        ffxi_install::update::Options { force: verify },
        report,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOptions {
    pub name: String,
    pub region: String,
    pub update: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    pub root: PathBuf,
    /// A client of that name already existed, so nothing was downloaded.
    pub reused: bool,
    /// `None` when the update was skipped; `Some(None)` when already current.
    pub update: Option<Option<ffxi_install::update::Outcome>>,
}

/// One shot from nothing to a current retail client: reuse a client already
/// carrying `name` (never re-downloaded over), else download it, then patch
/// it. Selecting it is the caller's decision.
pub fn setup(opts: &SetupOptions, report: &ffxi_install::Reporter) -> Result<SetupOutcome, String> {
    let (root, reused) = match named(&opts.name) {
        Some(root) => (root, true),
        None => (download(&opts.name, &opts.region, report)?, false),
    };
    let update = if opts.update {
        Some(update(&root, false, report)?)
    } else {
        None
    };
    Ok(SetupOutcome {
        root,
        reused,
        update,
    })
}

pub fn describe(root: &Path) -> String {
    if !install_detect::is_ffxi_root(root) {
        return format!(
            "not an FFXI install ({} or ROM/ missing)",
            install_detect::VTABLE_MARKER
        );
    }
    let profile = ClientProfile::probe(root);
    match &profile.patch_version {
        Some(v) => format!("{} patch={v}", profile.name()),
        None => profile.name().to_string(),
    }
}

pub mod cli {
    use std::io::Write;
    use std::path::PathBuf;

    use clap::Subcommand;

    use super::*;

    #[derive(Debug, Subcommand)]
    pub enum Action {
        /// Every install kuluu can see, with the active one marked.
        List,
        /// The install the client will load, and why.
        Which,
        /// Make a client (by name or path) the launcher's install.
        Use { install: String },
        /// Get a current retail client in one shot: download (or reuse) a
        /// named client, patch it, and select it. Prompts for anything not
        /// given unless --yes.
        Setup {
            #[arg(long)]
            name: Option<String>,
            #[arg(long)]
            region: Option<String>,
            /// Take every default without asking.
            #[arg(long)]
            yes: bool,
            /// Leave the 2019 base image unpatched.
            #[arg(long)]
            no_update: bool,
            /// Do not make it the launcher's install.
            #[arg(long)]
            no_use: bool,
        },
        /// Patch a client (by name or path) to the current retail version.
        Update {
            install: String,
            /// Re-check every file, not just the manifest stamp.
            #[arg(long)]
            verify: bool,
            #[arg(long)]
            yes: bool,
        },
    }

    pub fn run(action: &Action) -> Result<(), String> {
        match action {
            Action::List => list(),
            Action::Which => which(),
            Action::Use { install } => {
                let path = use_install(install)?;
                println!(
                    "launcher.json now selects {}\n  {}",
                    path.display(),
                    describe(&path)
                );
                Ok(())
            }
            Action::Setup {
                name,
                region,
                yes,
                no_update,
                no_use,
            } => setup_cli(
                name.as_deref(),
                region.as_deref(),
                *yes,
                *no_update,
                *no_use,
            ),
            Action::Update {
                install,
                verify,
                yes,
            } => update_cli(install, *verify, *yes),
        }
    }

    fn active_path() -> Option<PathBuf> {
        resolve(&launcher_store::load().settings)
            .ok()
            .map(|l| l.path)
    }

    fn list() -> Result<(), String> {
        let active = active_path();
        let installs = installs();
        if installs.is_empty() {
            println!("no FFXI installs found");
        }
        let width = installs
            .iter()
            .map(|i| i.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        for i in &installs {
            let mark = if active.as_deref().is_some_and(|a| same_dir(a, &i.path)) {
                '*'
            } else {
                ' '
            };
            println!(
                "{mark} {:<width$}  {:<9}  {}\n  {:width$}  {}",
                i.name,
                i.origin.label(),
                describe(&i.path),
                "",
                i.path.display(),
            );
        }
        if let Some(dir) = user_clients_dir() {
            println!("\nuser clients: {}", dir.display());
        }
        println!("* = what `kuluu play` will load; change it with `kuluu ffxi-client use NAME`");
        Ok(())
    }

    fn which() -> Result<(), String> {
        match resolve(&launcher_store::load().settings) {
            Ok(l) => {
                println!(
                    "{}\n  source: {}\n  status: {}",
                    l.path.display(),
                    l.source,
                    describe(&l.path)
                );
                Ok(())
            }
            Err(u) => Err(format!(
                "no install will load: {u}\n  \
                 pick one with `kuluu ffxi-client use NAME|PATH`, or run `kuluu ffxi-client setup`"
            )),
        }
    }

    fn read_line(prompt: &str) -> Result<String, String> {
        print!("{prompt}");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("reading input: {e}"))?;
        Ok(line.trim().to_string())
    }

    fn confirm(prompt: &str, default_yes: bool) -> Result<bool, String> {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        let answer = read_line(&format!("{prompt} {hint} "))?.to_lowercase();
        Ok(match answer.as_str() {
            "" => default_yes,
            "y" | "yes" => true,
            _ => false,
        })
    }

    fn ask(label: &str, default: &str) -> Result<String, String> {
        let answer = read_line(&format!("{label} [{default}]: "))?;
        Ok(if answer.is_empty() {
            default.to_string()
        } else {
            answer
        })
    }

    fn setup_cli(
        name: Option<&str>,
        region: Option<&str>,
        yes: bool,
        no_update: bool,
        no_use: bool,
    ) -> Result<(), String> {
        let name = match name {
            Some(n) => n.to_string(),
            None if yes => DEFAULT_DOWNLOAD_NAME.to_string(),
            None => ask("Client name", DEFAULT_DOWNLOAD_NAME)?,
        };
        if !valid_name(&name) {
            return Err(format!(
                "client name `{name}` must be [A-Za-z0-9_-]+ (it becomes a directory name)"
            ));
        }
        let existing = named(&name);
        let region = match (region, &existing) {
            (Some(r), _) => r.to_string(),
            (None, Some(_)) => DEFAULT_REGION.to_string(),
            (None, None) if yes => DEFAULT_REGION.to_string(),
            (None, None) => ask("Region (us|eu)", DEFAULT_REGION)?,
        };
        ffxi_install::region(&region)?;
        let clients = user_clients_dir().ok_or("no user data directory on this system")?;
        match &existing {
            Some(root) => println!(
                "Reusing the client already named `{name}`:\n  {}\n  {}",
                root.display(),
                describe(root)
            ),
            None => println!(
                "This downloads Square Enix's official FINAL FANTASY XI client installer\n\
                 ({INSTALLER_SIZE_NOTE}) from {}/{region}/ and unpacks it into\n  {}",
                ffxi_install::CDN_BASE,
                clients.join(&name).display(),
            ),
        }
        if !no_update {
            println!("then patches it to the current retail version ({PATCH_SIZE_NOTE} at most).");
        }
        if !yes && !confirm("Proceed?", true)? {
            return Err("aborted".into());
        }
        let opts = SetupOptions {
            name: name.clone(),
            region,
            update: !no_update,
        };
        let outcome = setup(&opts, &ffxi_install::report::print_progress)?;
        println!(
            "\n{}\n  {}",
            outcome.root.display(),
            describe(&outcome.root)
        );
        match outcome.update {
            Some(None) => println!("  already at the server's version"),
            Some(Some(o)) => println!(
                "  now at {} ({} file(s) fetched, {} MB)",
                o.version,
                o.fetched,
                o.bytes / 1_000_000
            ),
            None => println!("  left unpatched (--no-update)"),
        }
        if no_use {
            println!("Select it later with:\n  kuluu ffxi-client use {name}");
            return Ok(());
        }
        let current = active_path();
        if current
            .as_deref()
            .is_some_and(|c| same_dir(c, &outcome.root))
        {
            println!("It is already the launcher's install.");
            return Ok(());
        }
        if let Some(c) = &current {
            println!("The launcher currently loads {}", c.display());
        }
        if yes || confirm("Make it the launcher's install?", true)? {
            persist(&outcome.root)?;
            println!("launcher.json now selects `{name}`");
        } else {
            println!("Left as is. Select it later with:\n  kuluu ffxi-client use {name}");
        }
        Ok(())
    }

    fn update_cli(install: &str, verify: bool, yes: bool) -> Result<(), String> {
        let root = locate_spec(install)?;
        refuse_non_retail(&root)?;
        println!(
            "Install: {}\n  {}\nLocal patch version: {}",
            root.display(),
            describe(&root),
            ffxi_install::update::local_version(&root).unwrap_or_else(|| "none".into())
        );
        if !yes
            && !confirm(
                "Patch this install toward the current retail version?",
                false,
            )?
        {
            return Err("aborted".into());
        }
        match update(&root, verify, &ffxi_install::report::print_progress)? {
            None => {
                println!("already at the server's version; pass --verify to re-check every file")
            }
            Some(outcome) => println!(
                "now at {} ({} file(s) fetched, {} MB)",
                outcome.version,
                outcome.fetched,
                outcome.bytes / 1_000_000
            ),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(value: &str, override_env: bool) -> Settings {
        Settings {
            dat_path: EnvOverride {
                value: value.to_string(),
                override_env,
            },
            ..Default::default()
        }
    }

    #[test]
    fn names_are_directory_safe() {
        assert!(valid_name("retail"));
        assert!(valid_name("retail-eu_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a b"));
    }

    #[test]
    fn detected_name_is_the_dir_above_square_enix() {
        assert_eq!(
            detected_name(Path::new("/x/HorizonXI/SquareEnix/FINAL FANTASY XI")),
            "HorizonXI"
        );
    }

    // The shell value is captured once per process, so this test pins both
    // orderings against whatever the test runner's shell had.
    #[test]
    fn config_beats_the_shell_only_with_its_override_tick() {
        let shell = shell_dat_path().map(PathBuf::from);
        let l = resolve(&settings("/cfg", true)).unwrap();
        assert_eq!(l.path, PathBuf::from("/cfg"));
        let l = resolve(&settings("/cfg", false)).unwrap();
        match shell {
            Some(p) => {
                assert_eq!(l.path, p);
                assert_eq!(l.source, Source::EnvPath);
            }
            None => {
                assert_eq!(l.path, PathBuf::from("/cfg"));
                assert_eq!(l.source, Source::Config);
            }
        }
    }
}
