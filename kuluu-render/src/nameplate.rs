use std::collections::HashMap;

use bevy::prelude::*;
use kuluu_snapshot::EntityKind;

use crate::camera::{nameplate_anchor_y, OperatorCamera};
use crate::components::{Nameplate, WorldEntity};
use crate::scene::BakedActor;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct NameplateLabel {
    pub entity_id: u32,

    pub base_name: String,
}

#[derive(Component)]
pub struct NameplateCoord;

pub fn spawn_nameplate(
    commands: &mut Commands,
    entity_id: u32,
    kind: EntityKind,
    name: &str,
    color: Color,
) -> Entity {
    let owned = name.to_string();
    commands
        .spawn((
            crate::components::InGameEntity,
            crate::hud_hide::HudHideExempt,
            Nameplate { entity_id, kind },
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(-1000.0),
                left: Val::Px(-1000.0),

                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                NameplateLabel {
                    entity_id,
                    base_name: owned.clone(),
                },
                Text::new(owned),
                TextFont {
                    font_size: 12.0.into(),
                    ..default()
                },
                TextColor(color),
            ));

            p.spawn((
                NameplateCoord,
                Text::new(""),
                TextFont {
                    font_size: 10.0.into(),
                    ..default()
                },
                TextColor(Color::srgba(0.78, 0.78, 0.82, 0.85)),
            ));
        })
        .id()
}

pub fn format_label(base_name: &str, hp_pct: Option<u8>, kind: EntityKind) -> String {
    let show_hp = matches!(kind, EntityKind::Mob | EntityKind::Pet);
    match (show_hp, hp_pct) {
        (true, Some(pct)) => format!("{base_name} {pct}%"),
        _ => base_name.to_string(),
    }
}

pub fn format_coord(pos: Vec3) -> String {
    format!("{:.1} / {:.1} / {:.1}", pos.x, pos.y, pos.z)
}

pub fn update_nameplates_system(
    state: Res<SceneState>,
    settings: Res<crate::graphics::settings::GraphicsSettings>,
    cam_q: Query<(&Camera, &Transform), (With<OperatorCamera>, Without<WorldEntity>)>,
    world_q: Query<
        (
            &Transform,
            &WorldEntity,
            Option<&BakedActor>,
            Has<crate::components::MountedRider>,
        ),
        Without<Nameplate>,
    >,
    mut nameplate_q: Query<(Entity, &Nameplate, &mut Node, &Children)>,
    mut label_q: Query<(&NameplateLabel, &mut Text), Without<NameplateCoord>>,
    mut coord_q: Query<&mut Text, (With<NameplateCoord>, Without<NameplateLabel>)>,
    mut commands: Commands,
    mut hp_by_id: Local<HashMap<u32, Option<u8>>>,
) {
    let Ok((camera, cam_t)) = cam_q.single() else {
        return;
    };
    let cam_global = GlobalTransform::from(*cam_t);

    // world_to_viewport is in the camera's target space = the off-screen image at
    // render scale; rescale to the native-res HUD (1.0 → no-op).
    let viewport_to_window = 1.0 / settings.render_scale();

    let mut pos_by_id: HashMap<u32, (Vec3, f32)> = HashMap::new();
    for (t, w, baked, mounted) in &world_q {
        pos_by_id.insert(w.id, (t.translation, nameplate_anchor_y(baked, mounted)));
    }

    // Doors and namevis-hidden helpers never keep a plate: retail shows no
    // floating name over a door in any state. Cull EXISTING plates here (the
    // earlier gate only filtered the HP map, which never controlled plate
    // existence -- that's why door names survived it).
    let suppressed: std::collections::HashSet<u32> = state
        .snapshot
        .entities
        .iter()
        .filter(|e| e.is_door() || e.name_hidden())
        .map(|e| e.id)
        .collect();

    // HP only changes with a snapshot; screen-space repositioning below still
    // runs every frame.
    let dirty = state.dirty;
    if dirty {
        hp_by_id.clear();
        for ent in &state.snapshot.entities {
            // Retail-hidden helpers (namevis hide bits) never get a plate,
            // and neither do DOORS: retail draws no floating name over a door
            // in any state (verified against retail 2026-08-26). The door's
            // name still shows in the target panel when targeted.
            if ent.name_hidden() || ent.is_door() {
                continue;
            }
            hp_by_id.insert(ent.id, ent.hp_pct);
        }
    }

    for (ui_entity, np, mut node, children) in &mut nameplate_q {
        if suppressed.contains(&np.entity_id) {
            commands.entity(ui_entity).try_despawn();
            continue;
        }
        match pos_by_id.get(&np.entity_id) {
            Some(&(world_pos, label_y)) => {
                let head = world_pos + Vec3::Y * label_y;
                let (want_left, want_top) = match camera.world_to_viewport(&cam_global, head) {
                    Ok(screen) => (
                        Val::Px(screen.x * viewport_to_window - 40.0),
                        Val::Px(screen.y * viewport_to_window - 16.0),
                    ),
                    Err(_) => (Val::Px(-9999.0), Val::Px(-9999.0)),
                };
                if node.left != want_left || node.top != want_top {
                    node.left = want_left;
                    node.top = want_top;
                }

                // Retail+ gate: the "{name} {pct}%" suffix shares the billboard
                // bar's toggle (off by default); enhanced-mob-hp-under is its
                // compile-time half, so a persisted on from an enhanced build
                // can't light it in a plain one.
                #[cfg(feature = "enhanced-mob-hp-under")]
                let hp_pct = if settings.mob_hp_under {
                    hp_by_id.get(&np.entity_id).copied().flatten()
                } else {
                    None
                };
                #[cfg(not(feature = "enhanced-mob-hp-under"))]
                let hp_pct: Option<u8> = None;
                let coord_str = format_coord(world_pos);
                for child in children.iter() {
                    if let Ok((label, mut text)) = label_q.get_mut(child) {
                        if dirty {
                            let want = format_label(&label.base_name, hp_pct, np.kind);
                            if **text != want {
                                **text = want;
                            }
                        }
                    } else if let Ok(mut text) = coord_q.get_mut(child) {
                        if **text != coord_str {
                            **text = coord_str.clone();
                        }
                    }
                }
            }
            None => {
                commands.entity(ui_entity).try_despawn();
            }
        }
    }
}
