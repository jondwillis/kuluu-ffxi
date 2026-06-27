use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::*;

use ffxi_client::launcher_store;

use super::{LauncherState, ServerInfo, ServerSelectForm};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum VersionViolation {
    #[default]
    Ok,
    BelowRecommended,
    BelowMinimum,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct ServerVersionStatus {
    pub current: String,
    pub recommended: Option<String>,
    pub minimum: Option<String>,
    pub violation: VersionViolation,
    pub checked_url: Option<String>,
    pub checked: bool,
}

#[derive(Clone, Debug)]
struct ServerVersionAdvert {
    recommended: Option<String>,
    minimum: Option<String>,
}

#[derive(Resource, Clone)]
struct VersionCheckSlot {
    url: String,
    result: Arc<Mutex<Option<Option<ServerVersionAdvert>>>>,
}

fn active_version_check_url(form: &ServerSelectForm, info: &ServerInfo) -> Option<String> {
    let store = launcher_store::load();
    let by_name = form
        .selected
        .as_deref()
        .or(info.profile_name.as_deref())
        .and_then(|name| store.servers.iter().find(|p| p.name == name));
    let profile = by_name.or_else(|| store.servers.iter().find(|p| p.host == info.server))?;
    profile
        .version_check_url
        .as_ref()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
}

fn fetch_advert(url: &str) -> Option<ServerVersionAdvert> {
    // ureq bundles rustls + webpki-roots (CA-validated), matching updater.rs; the
    // client's own tls.rs is a TOFU verifier unsuitable for public CA checks.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(8))
        .build();
    let body = agent
        .get(url)
        .set(
            "User-Agent",
            concat!("kuluu-ffxi/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/json")
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let pick = |key: &str| {
        json.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
    };
    Some(ServerVersionAdvert {
        recommended: pick("recommended"),
        minimum: pick("minimum"),
    })
}

fn parse_ver(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn is_below(current: &str, bound: &str) -> bool {
    match (parse_ver(current), parse_ver(bound)) {
        (Some(c), Some(b)) => c < b,
        _ => false,
    }
}

fn classify(current: &str, advert: &ServerVersionAdvert) -> VersionViolation {
    if let Some(min) = advert.minimum.as_deref() {
        if is_below(current, min) {
            return VersionViolation::BelowMinimum;
        }
    }
    if let Some(rec) = advert.recommended.as_deref() {
        if is_below(current, rec) {
            return VersionViolation::BelowRecommended;
        }
    }
    VersionViolation::Ok
}

fn reset_on_enter(
    mut commands: Commands,
    form: Res<ServerSelectForm>,
    info: Res<ServerInfo>,
    rt: Res<super::RuntimeHandle>,
    mut status: ResMut<ServerVersionStatus>,
) {
    status.current = env!("CARGO_PKG_VERSION").to_string();
    let url = active_version_check_url(&form, &info);
    if status.checked && status.checked_url == url {
        return;
    }
    status.recommended = None;
    status.minimum = None;
    status.violation = VersionViolation::Ok;
    status.checked = false;
    status.checked_url = url.clone();
    commands.remove_resource::<VersionCheckSlot>();

    let Some(url) = url else {
        status.checked = true;
        return;
    };
    let result: Arc<Mutex<Option<Option<ServerVersionAdvert>>>> = Arc::new(Mutex::new(None));
    commands.insert_resource(VersionCheckSlot {
        url: url.clone(),
        result: result.clone(),
    });
    rt.0.spawn_blocking(move || {
        let advert = fetch_advert(&url);
        if let Ok(mut g) = result.lock() {
            *g = Some(advert);
        }
    });
}

fn poll_check(
    mut commands: Commands,
    slot: Option<Res<VersionCheckSlot>>,
    mut status: ResMut<ServerVersionStatus>,
) {
    let Some(slot) = slot else {
        return;
    };
    if status.checked && status.checked_url.as_deref() == Some(slot.url.as_str()) {
        return;
    }
    let Ok(mut g) = slot.result.lock() else {
        return;
    };
    let Some(result) = g.take() else {
        return;
    };
    status.checked = true;
    status.checked_url = Some(slot.url.clone());
    if let Some(advert) = result {
        status.recommended = advert.recommended.clone();
        status.minimum = advert.minimum.clone();
        status.violation = classify(&status.current, &advert);
    }
    commands.remove_resource::<VersionCheckSlot>();
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<ServerVersionStatus>()
        .add_systems(OnEnter(LauncherState::Login), reset_on_enter)
        .add_systems(Update, poll_check.run_if(in_state(LauncherState::Login)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(recommended: Option<&str>, minimum: Option<&str>) -> ServerVersionAdvert {
        ServerVersionAdvert {
            recommended: recommended.map(str::to_string),
            minimum: minimum.map(str::to_string),
        }
    }

    #[test]
    fn below_minimum_takes_precedence() {
        let a = advert(Some("0.3.0"), Some("0.2.0"));
        assert_eq!(classify("0.1.5", &a), VersionViolation::BelowMinimum);
    }

    #[test]
    fn between_minimum_and_recommended_warns() {
        let a = advert(Some("0.3.0"), Some("0.2.0"));
        assert_eq!(classify("0.2.4", &a), VersionViolation::BelowRecommended);
    }

    #[test]
    fn at_or_above_recommended_is_ok() {
        let a = advert(Some("0.3.0"), Some("0.2.0"));
        assert_eq!(classify("0.3.0", &a), VersionViolation::Ok);
        assert_eq!(classify("1.0.0", &a), VersionViolation::Ok);
    }

    #[test]
    fn missing_bounds_never_block() {
        let a = advert(None, None);
        assert_eq!(classify("0.0.1", &a), VersionViolation::Ok);
    }

    #[test]
    fn only_minimum_present() {
        let a = advert(None, Some("0.2.0"));
        assert_eq!(classify("0.1.0", &a), VersionViolation::BelowMinimum);
        assert_eq!(classify("0.2.0", &a), VersionViolation::Ok);
    }

    #[test]
    fn unparseable_bound_is_ignored() {
        let a = advert(Some("not-a-version"), None);
        assert_eq!(classify("0.1.0", &a), VersionViolation::Ok);
    }
}
