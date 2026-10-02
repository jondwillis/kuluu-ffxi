//! The FFXI install registry: every install has a name under the user data
//! directory, a one-line `default` file names the one that loads, and
//! `FFXI_DAT_PATH` overrides it for one run. [`lock`] keeps an open install
//! and its updater out of each other's way.

pub mod lock;

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::archive::{DAT_PATH_ENV, INSTALL_SUBDIR};
use crate::install_detect;

pub const APP_DIR: &str = "kuluu";
pub const INSTALLS_DIR: &str = "installs";
/// The registry's directory name before it became the only one; renamed to
/// [`INSTALLS_DIR`] on first use.
const LEGACY_CLIENTS_DIR: &str = "clients";
/// One line holding the name of the install that loads.
pub const DEFAULT_POINTER: &str = "default";

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != DEFAULT_POINTER
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `<data dir>/kuluu/installs`, created on demand; `None` when the platform
/// has no user data directory.
pub fn installs_dir() -> Option<PathBuf> {
    let app = dirs::data_dir()?.join(APP_DIR);
    let dir = app.join(INSTALLS_DIR);
    let legacy = app.join(LEGACY_CLIENTS_DIR);
    if !dir.exists() && legacy.is_dir() {
        if let Err(e) = fs::rename(&legacy, &dir) {
            eprintln!(
                "could not rename {} to {}: {e}",
                legacy.display(),
                dir.display()
            );
        }
    }
    Some(dir)
}

/// The DAT root a named install would have, validated or not.
pub fn install_root_in(dir: &Path, name: &str) -> PathBuf {
    dir.join(name).join(INSTALL_SUBDIR)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub name: String,
    pub path: PathBuf,
}

pub fn list_in(dir: &Path) -> Vec<Install> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let path = install_root_in(dir, &name);
            install_detect::is_ffxi_root(&path).then_some(Install { name, path })
        })
        .collect()
}

pub fn list() -> Vec<Install> {
    installs_dir().map(|d| list_in(&d)).unwrap_or_default()
}

pub fn named_in(dir: &Path, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    let path = install_root_in(dir, name);
    install_detect::is_ffxi_root(&path).then_some(path)
}

pub fn named(name: &str) -> Option<PathBuf> {
    named_in(&installs_dir()?, name)
}

/// The name a DAT root is registered under, when it lies in `dir`.
pub fn name_of_in(dir: &Path, root: &Path) -> Option<String> {
    list_in(dir)
        .into_iter()
        .find(|i| same_dir(&i.path, root))
        .map(|i| i.name)
}

pub fn name_of(root: &Path) -> Option<String> {
    name_of_in(&installs_dir()?, root)
}

pub fn read_default_in(dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(dir.join(DEFAULT_POINTER)).ok()?;
    let name = raw.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub fn read_default() -> Option<String> {
    read_default_in(&installs_dir()?)
}

/// Point `default` at a registered install; refuses a name nothing holds.
pub fn set_default_in(dir: &Path, name: &str) -> io::Result<()> {
    if named_in(dir, name).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no install named `{name}` under {} (kuluu install list)",
                dir.display()
            ),
        ));
    }
    fs::create_dir_all(dir)?;
    fs::write(dir.join(DEFAULT_POINTER), format!("{name}\n"))
}

pub fn set_default(name: &str) -> io::Result<()> {
    let dir = installs_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no user data directory on this system",
        )
    })?;
    set_default_in(&dir, name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    EnvPath,
    Default(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::EnvPath => write!(f, "{DAT_PATH_ENV} from the environment"),
            Source::Default(name) => write!(f, "`{name}`, the {DEFAULT_POINTER} install"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
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

impl std::error::Error for Unresolved {}

fn env_path() -> Option<PathBuf> {
    env::var_os(DAT_PATH_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `env_path` (the shell's `FFXI_DAT_PATH`) else the `default` install in
/// `dir`; a set-but-unusable value errors instead of falling through.
pub fn resolve_in(dir: Option<&Path>, env_path: Option<PathBuf>) -> Result<Resolved, Unresolved> {
    if let Some(path) = env_path {
        if !install_detect::is_ffxi_root(&path) {
            return Err(Unresolved {
                source: Some(Source::EnvPath),
                reason: format!(
                    "{} is not a FINAL FANTASY XI install ({} or ROM/ missing)",
                    path.display(),
                    install_detect::VTABLE_MARKER
                ),
            });
        }
        return Ok(Resolved {
            path,
            source: Source::EnvPath,
        });
    }
    let Some(dir) = dir else {
        return Err(Unresolved {
            source: None,
            reason: "no user data directory on this system; set FFXI_DAT_PATH".to_string(),
        });
    };
    let Some(name) = read_default_in(dir) else {
        return Err(Unresolved {
            source: None,
            reason: format!(
                "no install selected: {} names nothing (kuluu install use NAME, or kuluu install get)",
                dir.join(DEFAULT_POINTER).display()
            ),
        });
    };
    match named_in(dir, &name) {
        Some(path) => Ok(Resolved {
            path,
            source: Source::Default(name),
        }),
        None => Err(Unresolved {
            reason: format!(
                "{} names `{name}` but {} holds no install",
                dir.join(DEFAULT_POINTER).display(),
                install_root_in(dir, &name).display()
            ),
            source: Some(Source::Default(name)),
        }),
    }
}

pub fn resolve() -> Result<Resolved, Unresolved> {
    resolve_in(installs_dir().as_deref(), env_path())
}

pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> io::Result<()> {
    match std::os::windows::fs::symlink_dir(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => junction_dir(src, dst),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!(
                "{e} (directory symlinks need Developer Mode or an elevated shell; link with copy instead)"
            ),
        )),
    }
}

/// CreateSymbolicLink's refusal when the shell lacks SeCreateSymbolicLinkPrivilege
/// and Developer Mode is off.
#[cfg(windows)]
const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

/// A directory symlink needs SeCreateSymbolicLinkPrivilege (Developer Mode or an elevated
/// shell). A junction (mount-point reparse point) does not: it binds two local roots, which
/// is all this tree's links need. std has no junction constructor, so the system's own
/// mount-point tool is the privilege-free way to make one.
#[cfg(windows)]
fn junction_dir(src: &Path, dst: &Path) -> io::Result<()> {
    let target = src.canonicalize().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("a junction needs an absolute target: {e}"),
        )
    })?;
    mklink_junction(&target, dst).and_then(|()| link_exists(dst))
}

/// A blocked machine can report success without creating anything, so the
/// reparse point has to exist before the link counts.
#[cfg(windows)]
fn link_exists(dst: &Path) -> io::Result<()> {
    is_symlink(dst)
        .then_some(())
        .ok_or_else(|| io::Error::other(format!("no link at {}", dst.display())))
}

/// The system's own mount-point tool: it creates the junction without the symlink
/// privilege, where the raw reparse APIs and std's symlink_dir both refuse an
/// unsigned, unelevated process.
#[cfg(windows)]
fn mklink_junction(target: &Path, dst: &Path) -> io::Result<()> {
    let out = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(dst)
        .arg(target)
        .output()
        .map_err(|e| io::Error::new(e.kind(), format!("launching cmd for mklink: {e}")))?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "mklink /J exited with {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn symlink_dir(_src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this target has no directory symlinks; link with copy instead",
    ))
}

fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)?.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Register `source_root` (a DAT root) under `name`, by symlink or by copy.
/// Returns the registered root.
pub fn link_in(dir: &Path, name: &str, source_root: &Path, copy: bool) -> io::Result<PathBuf> {
    if !valid_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("install name `{name}` must be [A-Za-z0-9_-]+ (it becomes a directory name)"),
        ));
    }
    if !install_detect::is_ffxi_root(source_root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is not a FINAL FANTASY XI install ({} or ROM/ missing)",
                source_root.display(),
                install_detect::VTABLE_MARKER
            ),
        ));
    }
    let dest = install_root_in(dir, name);
    if dest.exists() || is_symlink(&dest) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists; pick another name", dest.display()),
        ));
    }
    let parent = dest
        .parent()
        .expect("an install root always sits under SquareEnix/");
    fs::create_dir_all(parent)?;
    if copy {
        copy_dir(source_root, &dest)?;
    } else {
        symlink_dir(source_root, &dest)?;
    }
    if !install_detect::is_ffxi_root(&dest) {
        return Err(io::Error::other(format!(
            "{} was created but does not validate",
            dest.display()
        )));
    }
    Ok(dest)
}

pub fn link(name: &str, source_root: &Path, copy: bool) -> io::Result<PathBuf> {
    let dir = installs_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no user data directory on this system",
        )
    })?;
    link_in(&dir, name, source_root, copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "kuluu-install-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_install(dir: &Path, name: &str) -> PathBuf {
        let root = install_root_in(dir, name);
        fs::create_dir_all(root.join("ROM")).unwrap();
        fs::write(root.join(install_detect::VTABLE_MARKER), b"").unwrap();
        root
    }

    #[test]
    fn names_are_directory_safe_and_never_the_pointer() {
        assert!(valid_name("retail"));
        assert!(valid_name("retail-eu_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a b"));
        assert!(!valid_name(DEFAULT_POINTER));
    }

    #[test]
    fn list_holds_only_validated_installs_sorted_by_name() {
        let dir = temp_dir("list");
        fake_install(&dir, "retail");
        fake_install(&dir, "hxi");
        fs::create_dir_all(dir.join("empty")).unwrap();
        let names: Vec<_> = list_in(&dir).into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["hxi", "retail"]);
        assert_eq!(
            name_of_in(&dir, &install_root_in(&dir, "hxi")).as_deref(),
            Some("hxi")
        );
        assert_eq!(name_of_in(&dir, Path::new("/nowhere")), None);
    }

    #[test]
    fn env_path_beats_the_pointer_and_an_unusable_one_is_an_error() {
        let dir = temp_dir("env");
        let retail = fake_install(&dir, "retail");
        let hxi = fake_install(&dir, "hxi");
        set_default_in(&dir, "retail").unwrap();
        let r = resolve_in(Some(&dir), Some(hxi.clone())).unwrap();
        assert_eq!((r.path, r.source), (hxi, Source::EnvPath));
        let r = resolve_in(Some(&dir), None).unwrap();
        assert_eq!(
            (r.path, r.source),
            (retail, Source::Default("retail".into()))
        );
        let u = resolve_in(Some(&dir), Some(dir.join("empty"))).unwrap_err();
        assert_eq!(u.source, Some(Source::EnvPath));
    }

    #[test]
    fn missing_and_stale_pointers_name_the_file_and_the_name() {
        let dir = temp_dir("pointer");
        let u = resolve_in(Some(&dir), None).unwrap_err();
        assert_eq!(u.source, None);
        assert!(u.reason.contains(DEFAULT_POINTER), "{u}");
        fake_install(&dir, "retail");
        set_default_in(&dir, "retail").unwrap();
        assert_eq!(read_default_in(&dir).as_deref(), Some("retail"));
        fs::remove_dir_all(dir.join("retail")).unwrap();
        let u = resolve_in(Some(&dir), None).unwrap_err();
        assert_eq!(u.source, Some(Source::Default("retail".into())));
        assert!(u.reason.contains("retail"), "{u}");
        assert!(set_default_in(&dir, "nothing").is_err());
        assert_eq!(resolve_in(None, None).unwrap_err().source, None);
    }

    #[test]
    fn link_registers_a_root_under_a_name_and_refuses_a_taken_one() {
        let dir = temp_dir("link");
        let elsewhere = temp_dir("source");
        fs::create_dir_all(elsewhere.join("ROM")).unwrap();
        fs::write(elsewhere.join(install_detect::VTABLE_MARKER), b"").unwrap();
        let linked = link_in(&dir, "ext", &elsewhere, false).unwrap();
        assert_eq!(named_in(&dir, "ext"), Some(linked.clone()));
        assert!(same_dir(&linked, &elsewhere));
        assert_eq!(
            link_in(&dir, "ext", &elsewhere, false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let copied = link_in(&dir, "ext-copy", &elsewhere, true).unwrap();
        assert!(!is_symlink(&copied) && copied.join("ROM").is_dir());
        assert!(link_in(&dir, "bad", &dir.join("nowhere"), false).is_err());
    }
}
