use std::cmp::Ordering;

use bevy::prelude::*;
use ffxi_dat::client_profile::ClientProfile;
use ffxi_proto::login::{compare_client_ver_era, lobby_accepts_client_ver, VerLock};

use crate::ffxi_client;
use crate::launcher_store::{self, ServerProfile};

use super::server_version_check::active_server_profile;
use super::{LauncherState, ServerInfo, ServerSelectForm};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum EraVerdict {
    /// No install resolved, or no server entry to compare against.
    #[default]
    Unchecked,
    Ok,
    /// The lobby would admit the install but it is not the era the server
    /// was built against, the entry prefers another install, or the entry
    /// records no era and the vendored pin alone would refuse it.
    Warn,
    /// The entry records the era its lobby admits and
    /// vendor/server/src/login/view_session.cpp view_session::read_func
    /// case 0x26 would reject this patch stamp.
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveInstall {
    pub name: String,
    /// ffxi_dat::client_profile::KNOWN_CLIENTS row name, or "unknown".
    pub client: String,
    pub patch_version: Option<String>,
    /// A hash-matched KNOWN_CLIENTS row with retail: true.
    pub known_retail: bool,
}

impl ActiveInstall {
    pub fn label(&self) -> String {
        format!(
            "Install: {} ({}, patch {})",
            self.name,
            self.client,
            self.patch_version.as_deref().unwrap_or("unknown")
        )
    }
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientEraStatus {
    pub server_name: Option<String>,
    pub expected: String,
    /// Whether `expected` came from the entry rather than the vendored pin.
    pub configured: bool,
    pub lock: Option<VerLock>,
    pub install: Option<ActiveInstall>,
    pub preferred_client: Option<String>,
    pub verdict: EraVerdict,
}

impl ClientEraStatus {
    pub fn blocks_login(&self) -> bool {
        self.verdict == EraVerdict::Refused
    }

    pub fn install_stamp(&self) -> &str {
        self.install
            .as_ref()
            .and_then(|i| i.patch_version.as_deref())
            .unwrap_or("unknown")
    }

    pub fn install_name(&self) -> &str {
        self.install
            .as_ref()
            .map(|i| i.name.as_str())
            .unwrap_or("none")
    }

    pub fn preferred_mismatch(&self) -> Option<&str> {
        let preferred = self.preferred_client.as_deref()?;
        let active = self.install.as_ref()?;
        (active.name != preferred).then_some(preferred)
    }
}

pub(crate) fn classify(profile: &ServerProfile, install: Option<ActiveInstall>) -> ClientEraStatus {
    let expected = profile.expected_client_ver().to_string();
    let lock = profile.ver_lock();
    let preferred_client = profile.preferred_client.clone();
    let configured = profile
        .client_ver
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty());
    let mut status = ClientEraStatus {
        server_name: Some(profile.name.clone()),
        expected,
        configured,
        lock: Some(lock),
        install,
        preferred_client,
        verdict: EraVerdict::Unchecked,
    };
    let Some(install) = status.install.as_ref() else {
        return status;
    };
    status.verdict = match install.patch_version.as_deref() {
        // A hash-recognized retail client with no readable patch.cfg (SE base
        // image / a POL install without the stamp, e.g. the phoenix-bundle row):
        // the DLL is a measured retail build, and the lobby's own 0x26 check is
        // the authority. Do not pre-warn on a stamp the launcher cannot read.
        None if install.known_retail => EraVerdict::Ok,
        None => EraVerdict::Warn,
        Some(stamp) if !lobby_accepts_client_ver(stamp, &status.expected, lock) => {
            if configured {
                EraVerdict::Refused
            } else {
                EraVerdict::Warn
            }
        }
        Some(stamp) if compare_client_ver_era(stamp, &status.expected) != Ordering::Equal => {
            EraVerdict::Warn
        }
        Some(_) => EraVerdict::Ok,
    };
    if status.verdict == EraVerdict::Ok && status.preferred_mismatch().is_some() {
        status.verdict = EraVerdict::Warn;
    }
    status
}

/// The install every `DatRoot::from_env_or_default` in this process will
/// load, named the way the install screen lists it; the loaded root's probe
/// is reused when it is the same directory, else the tree is probed here.
pub(super) fn active_install(
    settings: &launcher_store::Settings,
    loaded: Option<&ffxi_dat::DatRoot>,
) -> Option<ActiveInstall> {
    let located = ffxi_client::resolve(settings).ok()?;
    let name = ffxi_client::installs()
        .into_iter()
        .find(|i| ffxi_client::same_dir(&i.path, &located.path))
        .map(|i| i.name)
        .unwrap_or_else(|| located.path.display().to_string());
    let probed;
    let profile = match loaded {
        Some(root) if ffxi_client::same_dir(root.root(), &located.path) => root.profile(),
        _ => {
            probed = ClientProfile::probe(&located.path);
            &probed
        }
    };
    Some(ActiveInstall {
        name,
        client: profile.name().to_string(),
        patch_version: profile.patch_version.clone(),
        known_retail: profile.known.is_some_and(|k| k.retail),
    })
}

fn evaluate_on_enter(
    form: Res<ServerSelectForm>,
    info: Res<ServerInfo>,
    loaded: Option<Res<crate::view_native::DatRootRes>>,
    mut status: ResMut<ClientEraStatus>,
) {
    let next = match active_server_profile(&form, &info) {
        Some(profile) => {
            let settings = launcher_store::load().settings;
            let loaded = loaded.as_ref().and_then(|r| r.0.as_deref());
            classify(&profile, active_install(&settings, loaded))
        }
        None => ClientEraStatus::default(),
    };
    if *status != next {
        if next.verdict != EraVerdict::Ok {
            tracing::warn!(
                verdict = ?next.verdict,
                expected = %next.expected,
                lock = ?next.lock,
                install = next.install_name(),
                install_patch = next.install_stamp(),
                "client era check against the selected server entry"
            );
        }
        *status = next;
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<ClientEraStatus>().add_systems(
        OnEnter(LauncherState::Login),
        evaluate_on_enter
            .after(super::decide_initial_screen)
            .before(super::login::spawn_login_ui),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_proto::login::{LSB_CLIENT_VER, LSB_DEFAULT_VER_LOCK};

    fn profile(client_ver: Option<&str>, ver_lock: Option<u8>) -> ServerProfile {
        ServerProfile {
            client_ver: client_ver.map(str::to_string),
            ver_lock,
            ..ServerProfile::lsb_defaults("local", launcher_store::LOCALHOST)
        }
    }

    fn install(name: &str, stamp: Option<&str>) -> Option<ActiveInstall> {
        Some(ActiveInstall {
            name: name.into(),
            client: "horizonxi-2023".into(),
            patch_version: stamp.map(str::to_string),
            known_retail: false,
        })
    }

    #[test]
    fn footer_label_carries_name_client_and_stamp() {
        let i = install("hxi", Some("30230905_0")).unwrap();
        assert_eq!(i.label(), "Install: hxi (horizonxi-2023, patch 30230905_0)");
        let i = install("odd", None).unwrap();
        assert_eq!(i.label(), "Install: odd (horizonxi-2023, patch unknown)");
    }

    #[test]
    fn no_install_is_unchecked_but_still_reports_the_expectation() {
        let s = classify(&profile(None, None), None);
        assert_eq!(s.verdict, EraVerdict::Unchecked);
        assert_eq!(s.expected, LSB_CLIENT_VER);
        assert_eq!(s.lock, Some(VerLock::from_setting(LSB_DEFAULT_VER_LOCK)));
    }

    #[test]
    fn same_era_is_ok() {
        let s = classify(
            &profile(Some("30260901_0"), Some(2)),
            install("retail", Some("30260904_1")),
        );
        assert_eq!(s.verdict, EraVerdict::Ok);
    }

    #[test]
    fn older_era_under_at_least_is_refused() {
        let s = classify(
            &profile(Some("30260203_0"), Some(2)),
            install("hxi", Some("30230905_0")),
        );
        assert_eq!(s.verdict, EraVerdict::Refused);
        assert!(s.blocks_login());
    }

    #[test]
    fn newer_era_under_at_least_warns() {
        let s = classify(
            &profile(Some("30230518_0"), Some(2)),
            install("retail", Some("30260904_1")),
        );
        assert_eq!(s.verdict, EraVerdict::Warn);
        assert!(!s.blocks_login());
    }

    #[test]
    fn newer_era_under_exact_is_refused() {
        let s = classify(
            &profile(Some("30230518_0"), Some(1)),
            install("retail", Some("30260904_1")),
        );
        assert_eq!(s.verdict, EraVerdict::Refused);
    }

    #[test]
    fn an_entry_without_a_recorded_era_warns_instead_of_blocking() {
        let s = classify(&profile(None, None), install("hxi", Some("30230905_0")));
        assert_eq!(s.verdict, EraVerdict::Warn);
        assert!(!s.configured);
        assert!(!s.blocks_login());
        let s = classify(
            &profile(Some("  "), None),
            install("hxi", Some("30230905_0")),
        );
        assert_eq!(s.verdict, EraVerdict::Warn);
        assert!(!s.configured);
    }

    #[test]
    fn lock_off_never_blocks_but_flags_the_drift() {
        let s = classify(
            &profile(Some("30260203_0"), Some(0)),
            install("hxi", Some("30230905_0")),
        );
        assert_eq!(s.verdict, EraVerdict::Warn);
    }

    #[test]
    fn unreadable_patch_stamp_warns_rather_than_blocks() {
        let s = classify(&profile(None, None), install("odd", None));
        assert_eq!(s.verdict, EraVerdict::Warn);
        assert_eq!(s.install_stamp(), "unknown");
    }

    #[test]
    fn known_retail_without_a_stamp_is_ok() {
        let install = ActiveInstall {
            name: "phoenix".into(),
            client: "phoenix-bundle".into(),
            patch_version: None,
            known_retail: true,
        };
        let s = classify(&profile(Some("30260904_1"), Some(2)), Some(install));
        assert_eq!(s.verdict, EraVerdict::Ok);
        assert!(!s.blocks_login());
    }

    #[test]
    fn preferred_install_mismatch_warns_on_an_otherwise_ok_era() {
        let mut p = profile(Some("30260901_0"), Some(2));
        p.preferred_client = Some("retail".into());
        let s = classify(&p, install("retail-copy", Some("30260904_1")));
        assert_eq!(s.verdict, EraVerdict::Warn);
        assert_eq!(s.preferred_mismatch(), Some("retail"));
        let s = classify(&p, install("retail", Some("30260904_1")));
        assert_eq!(s.verdict, EraVerdict::Ok);
        assert_eq!(s.preferred_mismatch(), None);
    }
}
