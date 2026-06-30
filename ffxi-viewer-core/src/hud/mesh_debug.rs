use bevy::picking::events::{Move, Out, Over, Pointer};
use bevy::picking::pointer::PointerId;
use bevy::picking::Pickable;
use bevy::prelude::*;

use crate::hud::palette;

#[derive(Component, Debug, Clone)]
pub struct MmbDebugInfo {
    pub file_id: u32,

    pub chunk_idx: usize,

    pub sub_index: usize,

    pub asset_name: String,

    pub variant_name: String,
}

#[derive(Resource, Default, Debug, Clone)]
pub struct MeshHoverDebug {
    pub current: Option<MmbDebugInfo>,

    pub hit_position: Option<Vec3>,
}

#[derive(Component)]
pub struct MeshDebugHud;

#[derive(Component)]
pub struct MeshDebugHudText;

pub fn spawn_mesh_debug_hud(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MeshDebugHud,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(60.0),

                left: Val::Percent(30.0),
                right: Val::Percent(30.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((
                MeshDebugHudText,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
}

pub fn update_hover_state(
    mut over_events: MessageReader<Pointer<Over>>,
    mut out_events: MessageReader<Pointer<Out>>,
    mut move_events: MessageReader<Pointer<Move>>,
    debug_info_q: Query<&MmbDebugInfo>,
    mut hover: ResMut<MeshHoverDebug>,
) {
    for ev in out_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }

        if let Ok(info) = debug_info_q.get(ev.entity) {
            if hover.current.as_ref().is_some_and(|cur| {
                cur.file_id == info.file_id
                    && cur.chunk_idx == info.chunk_idx
                    && cur.sub_index == info.sub_index
            }) {
                hover.current = None;
                hover.hit_position = None;
            }
        }
    }
    for ev in over_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }
        if let Ok(info) = debug_info_q.get(ev.entity) {
            hover.current = Some(info.clone());
            hover.hit_position = ev.hit.position;
        }
    }

    for ev in move_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }
        if debug_info_q.contains(ev.entity) {
            hover.hit_position = ev.hit.position;
        }
    }
}

pub fn update_mesh_debug_hud(
    hover: Res<MeshHoverDebug>,
    panels: Res<crate::hud::HudPanels>,
    mut hud_q: Query<&mut Visibility, With<MeshDebugHud>>,
    mut text_q: Query<&mut Text, With<MeshDebugHudText>>,
) {
    let Ok(mut vis) = hud_q.single_mut() else {
        return;
    };
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };

    if !panels.mesh_debug {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    }
    if *vis != Visibility::Inherited {
        *vis = Visibility::Inherited;
    }
    let want = match &hover.current {
        Some(info) => {
            let pos = match hover.hit_position {
                Some(p) => format!("  pos=({:.1}, {:.1}, {:.1})", p.x, p.y, p.z),
                None => String::new(),
            };
            format!(
                "MMB  file={}  chunk={}  sub={}  asset={}  variant={}{pos}",
                info.file_id, info.chunk_idx, info.sub_index, info.asset_name, info.variant_name,
            )
        }
        None => "MESH DEBUG — hover zone geometry for MMB details".to_string(),
    };
    if **text != want {
        **text = want;
    }
}

pub fn mesh_debug_bundle(info: MmbDebugInfo) -> (Pickable, MmbDebugInfo) {
    (Pickable::default(), info)
}
