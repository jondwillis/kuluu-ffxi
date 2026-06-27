use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::common::PANEL_BG;
use super::RuntimeHandle;
use crate::view_native::AppPhase;

const API_URL: &str = "https://api.github.com/repos/jondwillis/kuluu-ffxi/releases/latest";
const RELEASE_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/latest");

#[derive(Resource, Clone)]
pub(crate) struct UpdateStatus {
    pub current: String,
    pub latest: Option<String>,
    pub newer_available: bool,
    pub checked: bool,
    pub dismissed: bool,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: None,
            newer_available: false,
            checked: false,
            dismissed: false,
        }
    }
}

// Outer Option: None until the background fetch finishes. Inner Option: the
// latest tag, or None when the fetch failed (we fail silently).
#[derive(Resource, Clone)]
struct UpdateCheckSlot(Arc<Mutex<Option<Option<String>>>>);

#[derive(Component)]
struct UpdateBanner;

fn fetch_latest_tag() -> Option<String> {
    // ureq is already a runtime dep (via ffxi-nav-recast) and bundles rustls +
    // webpki-roots, so this is a CA-validated GET with no new dependency. The
    // client's own tls.rs is a TOFU verifier, unsuitable for public CA checks.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(8))
        .build();
    // GitHub rejects requests without a User-Agent.
    let body = agent
        .get(API_URL)
        .set(
            "User-Agent",
            concat!("kuluu-ffxi/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("tag_name")?.as_str().map(str::to_string)
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

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_ver(latest), parse_ver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = cmd {
        tracing::warn!(error = %e, url, "could not open release page");
    }
}

fn start_update_check(
    mut commands: Commands,
    rt: Res<RuntimeHandle>,
    mut status: ResMut<UpdateStatus>,
) {
    if status.checked {
        return;
    }
    status.current = env!("CARGO_PKG_VERSION").to_string();
    let slot: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
    commands.insert_resource(UpdateCheckSlot(slot.clone()));
    rt.0.spawn_blocking(move || {
        let tag = fetch_latest_tag();
        if let Ok(mut g) = slot.lock() {
            *g = Some(tag);
        }
    });
}

fn poll_update_check(slot: Option<Res<UpdateCheckSlot>>, mut status: ResMut<UpdateStatus>) {
    if status.checked {
        return;
    }
    let Some(slot) = slot else {
        return;
    };
    let Ok(mut g) = slot.0.lock() else {
        return;
    };
    let Some(result) = g.take() else {
        return;
    };
    status.checked = true;
    if let Some(tag) = result {
        status.newer_available = is_newer(&tag, &status.current);
        status.latest = Some(tag);
    }
}

fn sync_update_banner(
    mut commands: Commands,
    status: Res<UpdateStatus>,
    existing: Query<Entity, With<UpdateBanner>>,
) {
    let want = status.newer_available && !status.dismissed;
    let have = !existing.is_empty();
    if want && !have {
        let latest = status.latest.clone().unwrap_or_default();
        build_banner(&mut commands, &latest);
    } else if !want && have {
        for e in existing.iter() {
            commands.entity(e).despawn();
        }
    }
}

fn build_banner(commands: &mut Commands, latest: &str) {
    commands
        .spawn((
            UpdateBanner,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(10.0),
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    ..default()
                },
                BackgroundColor(PANEL_BG),
                BorderColor::all(Color::srgb(0.20, 0.45, 0.50)),
            ))
            .with_children(|bar| {
                bar.spawn((
                    Text::new(format!("Update available → {latest}")),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.0, 1.0, 1.0)),
                    ThemedText,
                ));
                bar.spawn(button(
                    ButtonProps {
                        variant: ButtonVariant::Primary,
                        ..default()
                    },
                    (),
                    Spawn((Text::new("Open release page"), ThemedText)),
                ))
                .observe(|_ev: On<Activate>| open_url(RELEASE_URL));
                bar.spawn(button(
                    ButtonProps::default(),
                    (),
                    Spawn((Text::new("Dismiss"), ThemedText)),
                ))
                .observe(|_ev: On<Activate>, mut status: ResMut<UpdateStatus>| {
                    status.dismissed = true;
                });
            });
        });
}

fn despawn_banner(mut commands: Commands, q: Query<Entity, With<UpdateBanner>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<UpdateStatus>()
        .add_systems(OnEnter(AppPhase::Launcher), start_update_check)
        .add_systems(OnExit(AppPhase::Launcher), despawn_banner)
        .add_systems(
            Update,
            (poll_update_check, sync_update_banner)
                .chain()
                .run_if(in_state(AppPhase::Launcher)),
        );
}
