use bevy::diagnostic::{
    Diagnostic, DiagnosticPath, Diagnostics, DiagnosticsStore, FrameTimeDiagnosticsPlugin,
    RegisterDiagnostic,
};
use bevy::prelude::*;
use ffxi_viewer_wire::BlowfishStatus;

use crate::hud::palette;
use crate::snapshot::SceneState;

const STALE_THRESHOLD_MS: u64 = 5_000;

pub const VISIBLE_MESHES: DiagnosticPath = DiagnosticPath::const_new("ffxi/visible_meshes");

#[derive(Component)]
pub struct DiagnosticsBar;

#[derive(Component)]
pub struct DiagBfValue;

#[derive(Component)]
pub struct DiagSyncValue;

#[derive(Component)]
pub struct DiagLastValue;

#[derive(Component)]
pub struct DiagMapValue;

#[derive(Component)]
pub struct DiagFpsValue;

#[derive(Component)]
pub struct DiagDrawValue;

pub fn spawn_diagnostics(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            crate::hud::DevHud,
            DiagnosticsBar,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                height: Val::Px(28.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            crate::hud::stage_bar::spawn_stage_cluster_as_child(p);
            spawn_separator(p);
            spawn_label_value(p, "bf=", DiagBfValue, "—");
            spawn_separator(p);
            spawn_label_value(p, "sync=", DiagSyncValue, "—");
            spawn_separator(p);
            spawn_label_value(p, "last=", DiagLastValue, "—");
            spawn_separator(p);
            spawn_label_value(p, "map=", DiagMapValue, "—");
            spawn_separator(p);
            spawn_label_value(p, "fps=", DiagFpsValue, "—");
            spawn_separator(p);
            spawn_label_value(p, "draws=", DiagDrawValue, "—");

            p.spawn(Node {
                flex_grow: 1.0,
                ..default()
            });
        });
}

fn spawn_label_value<M: Component>(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    marker: M,
    initial: &str,
) {
    parent.spawn((
        Text::new(label.to_string()),
        TextFont {
            font_size: 13.0,
            ..default()
        },
        TextColor(palette::MUTED),
    ));
    parent.spawn((
        marker,
        Text::new(initial.to_string()),
        TextFont {
            font_size: 13.0,
            ..default()
        },
        TextColor(palette::TEXT),
    ));
}

fn spawn_separator(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        Text::new("   "),
        TextFont {
            font_size: 13.0,
            ..default()
        },
        TextColor(palette::MUTED),
    ));
}

pub fn update_diagnostics(
    state: Res<SceneState>,
    mut bf_q: Query<
        (&mut Text, &mut TextColor),
        (
            With<DiagBfValue>,
            Without<DiagSyncValue>,
            Without<DiagLastValue>,
            Without<DiagMapValue>,
        ),
    >,
    mut sync_q: Query<
        &mut Text,
        (
            With<DiagSyncValue>,
            Without<DiagBfValue>,
            Without<DiagLastValue>,
            Without<DiagMapValue>,
        ),
    >,
    mut last_q: Query<
        (&mut Text, &mut TextColor),
        (
            With<DiagLastValue>,
            Without<DiagBfValue>,
            Without<DiagSyncValue>,
            Without<DiagMapValue>,
        ),
    >,
    mut map_q: Query<
        &mut Text,
        (
            With<DiagMapValue>,
            Without<DiagBfValue>,
            Without<DiagSyncValue>,
            Without<DiagLastValue>,
        ),
    >,
) {
    if !state.dirty {
        return;
    }
    let d = &state.snapshot.diagnostics;

    if let Ok((mut text, mut tc)) = bf_q.single_mut() {
        let (s, color) = match d.blowfish_status {
            Some(BlowfishStatus::Accepted) => ("ok".into(), palette::STAGE_GOOD),
            Some(BlowfishStatus::Sent) => ("sent".into(), palette::STAGE_TRANSITIONING),
            Some(BlowfishStatus::Waiting) => ("waiting".into(), palette::STAGE_TRANSITIONING),
            Some(BlowfishStatus::PendingZone) => ("pending".into(), palette::STAGE_TRANSITIONING),
            None => ("—".into(), palette::MUTED),
        };
        **text = s;
        tc.0 = color;
    }

    if let Ok(mut text) = sync_q.single_mut() {
        **text = match (d.sync_in, d.sync_out) {
            (Some(i), Some(o)) => format!("{i}/{o}"),
            _ => "—".into(),
        };
    }

    if let Ok((mut text, mut tc)) = last_q.single_mut() {
        match d.last_server_packet_age_ms {
            Some(ms) if ms < STALE_THRESHOLD_MS => {
                **text = format!("{ms}ms");
                tc.0 = palette::TEXT;
            }
            Some(ms) => {
                **text = format!("{ms}ms");
                tc.0 = palette::STAGE_BAD;
            }
            None => {
                **text = "—".into();
                tc.0 = palette::MUTED;
            }
        }
    }

    if let Ok(mut text) = map_q.single_mut() {
        **text = d.map_server_addr.clone().unwrap_or_else(|| "—".into());
    }
}

pub fn update_fps_system(
    diagnostics: Res<DiagnosticsStore>,
    mut fps_q: Query<&mut Text, With<DiagFpsValue>>,
) {
    let Ok(mut text) = fps_q.single_mut() else {
        return;
    };
    let want = match diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
    {
        Some(fps) => format!("{:.0}", fps),
        None => "—".into(),
    };
    if **text != want {
        **text = want;
    }
}

pub fn count_visible_meshes_system(
    q: Query<&ViewVisibility, With<Mesh3d>>,
    mut diagnostics: Diagnostics,
) {
    let n = q.iter().filter(|v| v.get()).count();
    diagnostics.add_measurement(&VISIBLE_MESHES, || n as f64);
}

pub fn update_draws_system(
    diagnostics: Res<DiagnosticsStore>,
    mut draw_q: Query<&mut Text, With<DiagDrawValue>>,
) {
    let Ok(mut text) = draw_q.single_mut() else {
        return;
    };
    let want = match diagnostics.get(&VISIBLE_MESHES).and_then(|d| d.smoothed()) {
        Some(n) => format!("{:.0}", n),
        None => "—".into(),
    };
    if **text != want {
        **text = want;
    }
}

pub fn register_visible_meshes_diagnostic(app: &mut App) {
    app.register_diagnostic(Diagnostic::new(VISIBLE_MESHES).with_max_history_length(60));
}
