//! Which FFXI install kuluu loads, and the verbs that manage the registry
//! (`ffxi_dat::install`). The launcher's saved `dat_path` is a legacy tier
//! that a `use` clears; the `default` pointer is the saved choice.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ffxi_dat::archive::DAT_PATH_ENV;
use ffxi_dat::client_profile::ClientProfile;
use ffxi_dat::install;
use ffxi_dat::install_detect;

use crate::launcher_store::{self, EnvOverride, Settings};

pub const DEFAULT_DOWNLOAD_NAME: &str = "retail";
pub const DEFAULT_REGION: &str = "us";
const INSTALLER_CACHE_DIR: &str = "ffxi-installer";
pub use ffxi_dat::install::{same_dir, valid_name};
pub use ffxi_install::{INSTALLER_SIZE_NOTE, PATCH_SIZE_NOTE};

/// The registry directory, where downloads land.
pub fn user_clients_dir() -> Option<PathBuf> {
    install::installs_dir()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Registered,
    Detected,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Registered => "installed",
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

/// The directory above `SquareEnix/`, which is how a detected install is
/// usually recognised (`HorizonXI`, `PlayOnline`, a bottle name). A root
/// shallower than that (a bare folder, a bottle mount) takes its own name;
/// a full path carries separators, which `valid_name` rejects.
fn detected_name(root: &Path) -> String {
    root.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .or_else(|| root.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// Every install kuluu can see: the registry, then auto-detected third-party
/// installs. A directory reachable under several names is listed once, under
/// the first.
pub fn installs() -> Vec<Install> {
    let mut out: Vec<Install> = Vec::new();
    let mut push = |name: String, origin: Origin, path: PathBuf| {
        if install_detect::is_ffxi_root(&path) && !out.iter().any(|i| same_dir(&i.path, &path)) {
            out.push(Install { name, origin, path });
        }
    };
    for i in install::list() {
        push(i.name, Origin::Registered, i.path);
    }
    for path in install_detect::detect() {
        push(detected_name(&path), Origin::Detected, path);
    }
    out
}

pub fn named(name: &str) -> Option<PathBuf> {
    install::named(name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// launcher.json, with its override tick beating a set `FFXI_DAT_PATH`.
    ConfigOverride,
    EnvPath,
    Config,
    Default(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::ConfigOverride => {
                write!(f, "launcher.json (its Override tick beats the environment)")
            }
            Source::EnvPath => install::Source::EnvPath.fmt(f),
            Source::Config => write!(f, "launcher.json"),
            Source::Default(name) => install::Source::Default(name.clone()).fmt(f),
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
    pub source: Option<Source>,
    pub reason: String,
}

impl fmt::Display for Unresolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(s) => write!(f, "{} ({s})", self.reason),
            None => f.write_str(&self.reason),
        }
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
/// its override tick is set, else `FFXI_DAT_PATH`, else the registry's
/// `default` install, else the launcher's saved path. A saved path that no
/// longer holds an install is skipped.
pub fn resolve(settings: &Settings) -> Result<Located, Unresolved> {
    let env_path = shell_dat_path().map(PathBuf::from);
    let config = Some(settings.dat_path.value.trim())
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .filter(|p| install_detect::is_ffxi_root(p));
    if let (Some(path), true) = (&config, settings.dat_path.override_env) {
        let source = if env_path.is_some() {
            Source::ConfigOverride
        } else {
            Source::Config
        };
        return Ok(Located {
            path: path.clone(),
            source,
        });
    }
    match install::resolve_in(install::installs_dir().as_deref(), env_path) {
        Ok(r) => Ok(Located {
            path: r.path,
            source: match r.source {
                install::Source::EnvPath => Source::EnvPath,
                install::Source::Default(name) => Source::Default(name),
            },
        }),
        Err(u) if u.source == Some(install::Source::EnvPath) => Err(Unresolved {
            source: Some(Source::EnvPath),
            reason: u.reason,
        }),
        Err(u) => match config {
            Some(path) => Ok(Located {
                path,
                source: Source::Config,
            }),
            None => Err(Unresolved {
                source: u.source.map(|s| match s {
                    install::Source::EnvPath => Source::EnvPath,
                    install::Source::Default(name) => Source::Default(name),
                }),
                reason: u.reason,
            }),
        },
    }
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

/// Make `root` the `default` install, registering it by link first when it
/// lies outside the registry, and clear the launcher's legacy saved path so
/// the pointer is the one saved choice.
pub fn persist(root: &Path) -> Result<String, String> {
    let name = persist_registry(root)?;
    let mut store = launcher_store::load();
    if !store.settings.dat_path.value.trim().is_empty() {
        store.settings.dat_path = EnvOverride::default();
        launcher_store::save(&store).map_err(|e| format!("writing launcher.json: {e}"))?;
    }
    Ok(name)
}

/// The registry half of [`persist`], with the directory as a parameter so
/// tests run against a fixture instead of the real registry.
fn persist_registry(root: &Path) -> Result<String, String> {
    persist_registry_in(
        &install::installs_dir().ok_or("no installs registry directory")?,
        root,
    )
}

fn persist_registry_in(dir: &Path, root: &Path) -> Result<String, String> {
    let name = match install::name_of_in(dir, root) {
        Some(name) => name,
        None => {
            let name = detected_name(root);
            if !valid_name(&name) {
                return Err(format!(
                    "`{name}` is not usable as an install name; link it with `kuluu install link NAME {}`",
                    root.display()
                ));
            }
            install::link_in(dir, &name, root, false).map_err(|e| e.to_string())?;
            name
        }
    };
    install::set_default_in(dir, &name).map_err(|e| e.to_string())?;
    Ok(name)
}

/// `spec` is an install name or a path (an install root or a directory above one).
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
        .ok_or_else(|| format!("`{spec}` is neither a directory nor a known install name"))
}

pub fn use_install(spec: &str) -> Result<PathBuf, String> {
    let path = locate_spec(spec)?;
    persist(&path)?;
    Ok(path)
}

/// Fetch Square Enix's installer into the registry as `name`. Returns the
/// DAT root; the 2019 base image still needs [`update`].
pub fn download(
    name: &str,
    region: &str,
    cancel: &ffxi_install::Cancel,
    report: &ffxi_install::Reporter,
) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "install name `{name}` must be [A-Za-z0-9_-]+ (it becomes a directory name)"
        ));
    }
    let installs = user_clients_dir().ok_or("no user data directory on this system")?;
    let target_root = installs.join(name);
    let installer_dir =
        kuluu_session::config_dir::cache_dir(INSTALLER_CACHE_DIR).map_err(|e| e.to_string())?;
    let plan = ffxi_install::Plan {
        region: ffxi_install::region(region)?,
        installer_dir: &installer_dir,
        target_root: &target_root,
    };
    ffxi_install::download_and_unpack(&plan, cancel, report)?;
    let root = install::install_root_in(&installs, name);
    if !install_detect::is_ffxi_root(&root) {
        return Err(format!(
            "unpack finished but {} does not validate ({} missing)",
            root.display(),
            install_detect::VTABLE_MARKER
        ));
    }
    Ok(root)
}

/// A known non-retail build (a private server's pinned client) is refused the
/// patch toward retail; an unknown build is allowed through, since a freshly
/// patched retail install is unknown until its row is measured.
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
    cancel: &ffxi_install::Cancel,
    report: &ffxi_install::Reporter,
) -> Result<Option<ffxi_install::update::Outcome>, String> {
    refuse_non_retail(root)?;
    let _claim = install::lock::exclusive(root).map_err(|e| {
        if e.is_held() {
            format!("{e}; switch installs and relaunch, then update")
        } else {
            e.to_string()
        }
    })?;
    ffxi_install::update::run(
        root,
        ffxi_install::update::Options { force: verify },
        cancel,
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

/// One shot from nothing to a current retail client: reuse an install already
/// carrying `name` (not re-downloaded over), else download it, then patch it.
/// Selecting it is the caller's decision.
pub fn setup(
    opts: &SetupOptions,
    cancel: &ffxi_install::Cancel,
    report: &ffxi_install::Reporter,
) -> Result<SetupOutcome, String> {
    let (root, reused) = match named(&opts.name) {
        Some(root) => (root, true),
        None => (download(&opts.name, &opts.region, cancel, report)?, false),
    };
    let update = if opts.update {
        Some(update(&root, false, cancel, report)?)
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
    let era = era_summary(profile.patch_version.as_deref());
    match &profile.patch_version {
        Some(v) => format!("{} patch={v}; {era}", profile.name()),
        None => format!("{}; {era}", profile.name()),
    }
}

/// How the install's patch era sits against the vendored LSB pin; a saved
/// server entry's own expectation is what the launcher actually gates on.
pub fn era_summary(patch_version: Option<&str>) -> String {
    use ffxi_proto::login::{
        compare_client_ver_era, lobby_accepts_client_ver, VerLock, LSB_CLIENT_VER,
        LSB_DEFAULT_VER_LOCK,
    };
    let Some(stamp) = patch_version else {
        return "patch era unknown (no patch.cfg stamp)".to_string();
    };
    let relation = match compare_client_ver_era(stamp, LSB_CLIENT_VER) {
        std::cmp::Ordering::Equal => return format!("era matches LSB {LSB_CLIENT_VER}"),
        std::cmp::Ordering::Less => "older than",
        std::cmp::Ordering::Greater => "newer than",
    };
    let lock = VerLock::from_setting(LSB_DEFAULT_VER_LOCK);
    let verdict = if lobby_accepts_client_ver(stamp, LSB_CLIENT_VER, lock) {
        "admitted"
    } else {
        "refused"
    };
    format!("era {relation} LSB {LSB_CLIENT_VER} ({verdict} at VER_LOCK {LSB_DEFAULT_VER_LOCK})")
}

/// One line per saved server that records the era it admits, with the
/// lobby's verdict on `patch_version`.
pub fn server_era_lines(
    servers: &[launcher_store::ServerProfile],
    patch_version: Option<&str>,
) -> Vec<String> {
    use ffxi_proto::login::lobby_accepts_client_ver;
    servers
        .iter()
        .filter(|p| {
            p.client_ver
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty())
        })
        .map(|p| {
            let expected = p.expected_client_ver();
            let verdict = match patch_version {
                Some(stamp) if lobby_accepts_client_ver(stamp, expected, p.ver_lock()) => {
                    "admits this install"
                }
                Some(_) => "would refuse this install",
                None => "cannot judge an install without a patch stamp",
            };
            format!(
                "{}: expects era {expected} ({:?}); {verdict}",
                p.name,
                p.ver_lock()
            )
        })
        .collect()
}

pub mod cli {
    use std::io::Write;
    use std::path::PathBuf;

    use clap::Subcommand;

    use super::*;

    #[derive(Debug, Subcommand)]
    pub enum Action {
        /// Every install kuluu can see, with the default marked.
        List {
            /// Print only the DAT roots, one per line.
            #[arg(long)]
            roots: bool,
        },
        /// The install kuluu will load, and why.
        Which,
        /// Make an install (by name or path) the default. A path outside the
        /// registry is linked under its folder name first.
        Use { install: String },
        /// Print the DAT root of a named install.
        Path { name: String },
        /// Register an existing install under NAME by symlink (or --copy).
        Link {
            name: String,
            /// An install root or a directory above one; auto-detected when omitted.
            path: Option<PathBuf>,
            #[arg(long)]
            copy: bool,
        },
        /// Get a current retail client in one shot: download (or reuse) a
        /// named install, patch it, and make it the default. Prompts for
        /// anything not given unless --yes.
        #[command(alias = "setup")]
        Get {
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
            /// Do not make it the default.
            #[arg(long)]
            no_use: bool,
        },
        /// Patch an install (by name or path) to the current retail version.
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
            Action::List { roots } => list(*roots),
            Action::Which => which(),
            Action::Use { install } => {
                let path = locate_spec(install)?;
                let name = persist(&path)?;
                println!(
                    "`{name}` is now the default install\n  {}\n  {}",
                    path.display(),
                    describe(&path)
                );
                Ok(())
            }
            Action::Path { name } => {
                let path = named(name)
                    .ok_or_else(|| format!("no install named `{name}` (kuluu install list)"))?;
                println!("{}", path.display());
                Ok(())
            }
            Action::Link { name, path, copy } => link_cli(name, path.as_deref(), *copy),
            Action::Get {
                name,
                region,
                yes,
                no_update,
                no_use,
            } => get_cli(
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

    fn list(roots: bool) -> Result<(), String> {
        let installs = installs();
        if roots {
            for i in installs.iter().filter(|i| i.origin == Origin::Registered) {
                println!("{}", i.path.display());
            }
            return Ok(());
        }
        let active = active_path();
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
            println!("\nregistry: {}", dir.display());
        }
        println!("* = what `kuluu play` will load; change it with `kuluu install use NAME`");
        Ok(())
    }

    fn which() -> Result<(), String> {
        let store = launcher_store::load();
        match resolve(&store.settings) {
            Ok(l) => {
                println!(
                    "{}\n  source: {}\n  status: {}",
                    l.path.display(),
                    l.source,
                    describe(&l.path)
                );
                let stamp = ClientProfile::probe(&l.path).patch_version;
                for line in server_era_lines(&store.servers, stamp.as_deref()) {
                    println!("  server {line}");
                }
                Ok(())
            }
            Err(u) => Err(format!(
                "no install will load: {u}\n  \
                 pick one with `kuluu install use NAME|PATH`, or run `kuluu install get`"
            )),
        }
    }

    fn link_cli(name: &str, path: Option<&Path>, copy: bool) -> Result<(), String> {
        let source = match path {
            Some(p) => install_detect::find_ffxi_root(p, install_detect::DEFAULT_SEARCH_DEPTH)
                .ok_or_else(|| format!("no FFXI install at or under {}", p.display()))?,
            None => {
                let mut hits = install_detect::detect();
                hits.dedup();
                match hits.len() {
                    0 => return Err("no FFXI install detected; pass a path".to_string()),
                    1 => hits.remove(0),
                    _ => {
                        let mut msg = String::from("several installs detected; pass one:\n");
                        for h in &hits {
                            msg.push_str(&format!(
                                "  kuluu install link {name} \"{}\"\n",
                                h.display()
                            ));
                        }
                        return Err(msg);
                    }
                }
            }
        };
        if copy {
            println!("Copying {} (this can be ~19 GB) ...", source.display());
        }
        let root = install::link(name, &source, copy).map_err(|e| e.to_string())?;
        println!(
            "`{name}` registered\n  {}\n  {}\nMake it the default with `kuluu install use {name}`",
            root.display(),
            describe(&root)
        );
        Ok(())
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

    fn get_cli(
        name: Option<&str>,
        region: Option<&str>,
        yes: bool,
        no_update: bool,
        no_use: bool,
    ) -> Result<(), String> {
        let name = match name {
            Some(n) => n.to_string(),
            None if yes => DEFAULT_DOWNLOAD_NAME.to_string(),
            None => ask("Install name", DEFAULT_DOWNLOAD_NAME)?,
        };
        if !valid_name(&name) {
            return Err(format!(
                "install name `{name}` must be [A-Za-z0-9_-]+ (it becomes a directory name)"
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
        let installs = user_clients_dir().ok_or("no user data directory on this system")?;
        match &existing {
            Some(root) => println!(
                "Reusing the install already named `{name}`:\n  {}\n  {}",
                root.display(),
                describe(root)
            ),
            None => println!(
                "This downloads Square Enix's official FINAL FANTASY XI client installer\n\
                 ({INSTALLER_SIZE_NOTE}) from {}/{region}/ and unpacks it into\n  {}",
                ffxi_install::CDN_BASE,
                installs.join(&name).display(),
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
        let cancel = ffxi_install::Cancel::default();
        let outcome = setup(&opts, &cancel, &ffxi_install::report::print_progress)?;
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
            println!("Make it the default later with:\n  kuluu install use {name}");
            return Ok(());
        }
        let current = active_path();
        if current
            .as_deref()
            .is_some_and(|c| same_dir(c, &outcome.root))
        {
            println!("It is already the default install.");
            return Ok(());
        }
        if let Some(c) = &current {
            println!("kuluu currently loads {}", c.display());
        }
        if yes || confirm("Make it the default install?", true)? {
            persist(&outcome.root)?;
            println!("`{name}` is now the default install");
        } else {
            println!("Left as is. Make it the default later with:\n  kuluu install use {name}");
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
        let cancel = ffxi_install::Cancel::default();
        match update(
            &root,
            verify,
            &cancel,
            &ffxi_install::report::print_progress,
        )? {
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

    fn fake_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("kuluu-ffxi-client-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("ROM")).unwrap();
        std::fs::write(root.join(install_detect::VTABLE_MARKER), b"").unwrap();
        root
    }

    #[test]
    fn detected_name_is_the_dir_above_square_enix() {
        assert_eq!(
            detected_name(Path::new("/x/HorizonXI/SquareEnix/FINAL FANTASY XI")),
            "HorizonXI"
        );
    }

    fn registry_install(dir: &Path, name: &str) -> PathBuf {
        let root = install::install_root_in(dir, name);
        std::fs::create_dir_all(root.join("ROM")).unwrap();
        std::fs::write(root.join(install_detect::VTABLE_MARKER), b"").unwrap();
        root
    }

    /// The DAT-gate picker commits through this: the selection has to land in
    /// the registry default, which is what cold starts and the lobby version
    /// stamp resolve.
    #[test]
    fn persist_registry_makes_the_choice_the_default_install() {
        let dir = std::env::temp_dir().join(format!("kuluu-persist-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let old = registry_install(&dir, "old");
        let new = registry_install(&dir, "new");
        install::set_default_in(&dir, "old").unwrap();

        assert_eq!(persist_registry_in(&dir, &new).as_deref(), Ok("new"));
        assert_eq!(install::read_default_in(&dir).as_deref(), Some("new"));
        let r = install::resolve_in(Some(&dir), None).unwrap();
        assert_eq!(r.path, new);

        let foreign = fake_root("foreign");
        let name = persist_registry_in(&dir, &foreign).unwrap();
        assert_eq!(
            install::read_default_in(&dir).as_deref(),
            Some(name.as_str())
        );
        let r = install::resolve_in(Some(&dir), None).unwrap();
        assert!(install::same_dir(&r.path, &foreign));

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&foreign).ok();
        drop(old);
    }

    /// The shell value is captured once per process, so this test pins both
    /// orderings against whatever the test runner's shell had.
    #[test]
    fn config_beats_the_shell_only_with_its_override_tick() {
        let cfg = fake_root("cfg");
        let shell = shell_dat_path().map(PathBuf::from);
        let l = resolve(&settings(&cfg.display().to_string(), true)).unwrap();
        assert_eq!(l.path, cfg);
        let l = resolve(&settings(&cfg.display().to_string(), false));
        match shell {
            Some(p) if install_detect::is_ffxi_root(&p) => {
                let l = l.unwrap();
                assert_eq!(l.path, p);
                assert_eq!(l.source, Source::EnvPath);
            }
            Some(_) => assert_eq!(l.unwrap_err().source, Some(Source::EnvPath)),
            None => match l {
                Ok(l) => assert!(
                    matches!(l.source, Source::Default(_) | Source::Config),
                    "{:?}",
                    l.source
                ),
                Err(u) => panic!("nothing resolved: {u}"),
            },
        }
    }

    #[test]
    fn a_stale_saved_path_is_skipped_even_with_its_tick() {
        let l = resolve(&settings("/nowhere/at/all", true));
        if let Ok(l) = l {
            assert_ne!(l.path, PathBuf::from("/nowhere/at/all"));
            assert!(!matches!(l.source, Source::Config | Source::ConfigOverride));
        }
    }
}

#[cfg(test)]
mod era_tests {
    use super::*;
    use crate::launcher_store::{AuthFlavorKind, ServerProfile};
    use ffxi_proto::login::LSB_CLIENT_VER;

    fn server(name: &str, client_ver: Option<&str>, ver_lock: Option<u8>) -> ServerProfile {
        ServerProfile {
            name: name.into(),
            host: "127.0.0.1".into(),
            auth_port: ffxi_proto::login::LOGIN_AUTH_PORT,
            data_port: ffxi_proto::login::LOGIN_DATA_PORT,
            view_port: ffxi_proto::login::LOGIN_VIEW_PORT,
            flavor: AuthFlavorKind::Json,
            xiloader_version: None,
            version_check_url: None,
            client_ver: client_ver.map(str::to_string),
            ver_lock,
            preferred_client: None,
            terms_acknowledged: false,
        }
    }

    #[test]
    fn era_summary_names_the_relation_and_the_default_lock_verdict() {
        assert_eq!(
            era_summary(Some(LSB_CLIENT_VER)),
            format!("era matches LSB {LSB_CLIENT_VER}")
        );
        let older = era_summary(Some("30230905_0"));
        assert!(older.starts_with("era older than LSB"), "{older}");
        assert!(older.contains("refused"), "{older}");
        let newer = era_summary(Some("39990101_0"));
        assert!(newer.starts_with("era newer than LSB"), "{newer}");
        assert!(newer.contains("admitted"), "{newer}");
        assert!(era_summary(None).contains("unknown"));
    }

    #[test]
    fn server_era_lines_skip_entries_without_a_recorded_era() {
        let servers = vec![
            server("hxi", None, None),
            server("lsb", Some("30260904_1"), Some(2)),
            server("old", Some("30230905_0"), Some(1)),
        ];
        let lines = server_era_lines(&servers, Some("30230905_0"));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("lsb:") && lines[0].ends_with("would refuse this install"));
        assert!(lines[1].starts_with("old:") && lines[1].ends_with("admits this install"));
        assert!(server_era_lines(&servers, None)[0].contains("cannot judge"));
    }
}
