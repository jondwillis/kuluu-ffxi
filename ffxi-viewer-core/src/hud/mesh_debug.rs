//! Hover-to-inspect debug HUD for MMB submeshes.
//!
//! Hovering the cursor over any zone-spawned MMB sub-mesh writes its
//! `MmbDebugInfo` (file_id, chunk_idx, sub_index, variant_name, asset)
//! into [`MeshHoverDebug`]. A small top-center panel renders that info
//! whenever an MMB mesh is under the pointer; otherwise the panel hides
//! itself. Useful for identifying misplaced wall slabs and asset-vs-
//! variant name mappings when running down placement bugs.

use bevy::picking::events::{Move, Out, Over, Pointer};
use bevy::picking::pointer::PointerId;
use bevy::picking::Pickable;
use bevy::prelude::*;

use crate::hud::palette;

/// One-time metadata attached to every MMB sub-mesh entity at spawn so
/// the hover system can identify the asset on a cursor hit.
///
/// Cheap to clone (just a few `u32`s plus two short strings) — copying
/// into [`MeshHoverDebug`] avoids cross-frame query lifetimes.
#[derive(Component, Debug, Clone)]
pub struct MmbDebugInfo {
    /// DAT file id the MMB came from.
    pub file_id: u32,
    /// MMB chunk index within that DAT file.
    pub chunk_idx: usize,
    /// Sub-record index inside the MMB (0..n).
    pub sub_index: usize,
    /// MMB asset name (e.g. `tshimonohiku_inb`).
    pub asset_name: String,
    /// Sub-record variant name (e.g. `san_kab0`).
    pub variant_name: String,
}

/// Most recently picked MMB debug info. `None` means the cursor is not
/// over an MMB submesh this frame. Updated by [`update_hover_state`];
/// read by [`update_mesh_debug_hud`].
#[derive(Resource, Default, Debug, Clone)]
pub struct MeshHoverDebug {
    pub current: Option<MmbDebugInfo>,
    /// World-space point where the picking ray hit the mesh, if the
    /// picking backend reported one. `None` for hits at backends that
    /// don't supply a position (rare). Useful for diagnosing
    /// placement-table coverage gaps: hover near the gap, read the
    /// world XYZ, look up which placement *should* be there.
    pub hit_position: Option<Vec3>,
}

/// Marker on the HUD root so we can find it for visibility toggles.
#[derive(Component)]
pub struct MeshDebugHud;

/// Marker on the body Text node so we can rewrite it each frame.
#[derive(Component)]
pub struct MeshDebugHudText;

pub fn spawn_mesh_debug_hud(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            crate::hud::DevHud,
            MeshDebugHud,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(60.0),
                // Centered horizontally via left+right both auto won't
                // work in Bevy UI; use a wide fixed-width strip and let
                // Text inside align itself.
                left: Val::Percent(30.0),
                right: Val::Percent(30.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
            // Hidden by default; hover system toggles to Inherited when
            // an MMB sub-mesh is under the pointer.
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

/// Read pointer hover events and update [`MeshHoverDebug`]. Uses the
/// `Pointer<Over>` / `Pointer<Out>` event stream — these fire exactly
/// once per enter/leave (not per-frame), so the resource only changes
/// when the hovered entity actually changes.
pub fn update_hover_state(
    mut over_events: MessageReader<Pointer<Over>>,
    mut out_events: MessageReader<Pointer<Out>>,
    mut move_events: MessageReader<Pointer<Move>>,
    debug_info_q: Query<&MmbDebugInfo>,
    mut hover: ResMut<MeshHoverDebug>,
) {
    // Process `Out` first so a same-frame `Out → Over` ends up showing
    // the new entity's info, not nothing.
    for ev in out_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }
        // Only clear if the leaving entity matches what we currently
        // have — otherwise we'd zero the panel when leaving an entity
        // we never recorded (e.g. an HP-bar capsule).
        if let Ok(info) = debug_info_q.get(ev.entity) {
            if hover
                .current
                .as_ref()
                .is_some_and(|cur| cur.file_id == info.file_id && cur.chunk_idx == info.chunk_idx && cur.sub_index == info.sub_index)
            {
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
    // Continuous position updates while staying over one mesh. `Over`
    // only fires on enter; without `Move` the HUD's XYZ would freeze
    // at the entry point.
    for ev in move_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }
        if debug_info_q.contains(ev.entity) {
            hover.hit_position = ev.hit.position;
        }
    }
}

/// Render the HUD text from [`MeshHoverDebug`]. Hides the panel when
/// nothing is hovered.
pub fn update_mesh_debug_hud(
    hover: Res<MeshHoverDebug>,
    verbosity: Res<crate::hud::HudVerbosity>,
    mut hud_q: Query<&mut Visibility, With<MeshDebugHud>>,
    mut text_q: Query<&mut Text, With<MeshDebugHudText>>,
) {
    let Ok(mut vis) = hud_q.single_mut() else {
        return;
    };
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };
    // Dev-HUD gate. When the operator has /devhud off, never reveal
    // the hover panel — the DevHud visibility system parks it Hidden,
    // and this short-circuit keeps us from racing to set Inherited.
    if !verbosity.dev_hud {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    }
    match &hover.current {
        Some(info) => {
            let pos = match hover.hit_position {
                Some(p) => format!("  pos=({:.1}, {:.1}, {:.1})", p.x, p.y, p.z),
                None => String::new(),
            };
            let want = format!(
                "MMB  file={}  chunk={}  sub={}  asset={}  variant={}{pos}",
                info.file_id, info.chunk_idx, info.sub_index, info.asset_name, info.variant_name,
            );
            if **text != want {
                **text = want;
            }
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
        }
        None => {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
    }
}

/// Required at spawn-time on every MMB sub-mesh so Bevy's mesh picking
/// backend will raycast against it. Bundled with `Pickable::default()`
/// at the call site rather than auto-inserted here so it remains
/// explicit which meshes participate in picking.
pub fn mesh_debug_bundle(
    info: MmbDebugInfo,
) -> (Pickable, MmbDebugInfo) {
    (Pickable::default(), info)
}
