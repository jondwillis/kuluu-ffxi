use bevy::prelude::*;
use kuluu_snapshot::EntityKind;

use crate::hud::style::{self, theme};
use crate::mouse::MousePointer;
use crate::picking::HoveredEntity;
use crate::scene::Target;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct EntityHoverCard;

#[derive(Component)]
pub struct EntityHoverCardName;

#[derive(Component)]
pub struct EntityHoverCardHp;

const CARD_OFFSET_PX: Vec2 = Vec2::new(18.0, 18.0);
const CARD_MIN_WIDTH_PX: f32 = 140.0;

pub fn spawn_entity_hover_card(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            EntityHoverCard,
            bevy::picking::Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                min_width: Val::Px(CARD_MIN_WIDTH_PX),
                padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
            ZIndex(i32::MAX - 1),
        ))
        .with_children(|p| {
            p.spawn((
                EntityHoverCardName,
                bevy::picking::Pickable::IGNORE,
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TEXT),
            ));
            p.spawn((
                EntityHoverCardHp,
                bevy::picking::Pickable::IGNORE,
                Text::new(""),
                style::text_font(12.0),
                TextColor(theme::MUTED),
            ));
        });
}

pub fn update_entity_hover_card_system(
    hovered: Res<HoveredEntity>,
    target: Res<Target>,
    state: Res<SceneState>,
    pointer: Res<MousePointer>,
    ui_scale: Res<UiScale>,
    mut card_q: Query<&mut Node, With<EntityHoverCard>>,
    mut name_q: Query<
        &mut Text,
        (
            With<EntityHoverCardName>,
            Without<EntityHoverCardHp>,
            Without<EntityHoverCard>,
        ),
    >,
    mut hp_q: Query<
        &mut Text,
        (
            With<EntityHoverCardHp>,
            Without<EntityHoverCardName>,
            Without<EntityHoverCard>,
        ),
    >,
) {
    let Ok(mut card) = card_q.single_mut() else {
        return;
    };

    let id = match hover_card_id(hovered.id, target.id, state.snapshot.self_char_id) {
        Some(id) => id,
        None => {
            if card.display != Display::None {
                card.display = Display::None;
            }
            return;
        }
    };

    let Some(ent) = state.snapshot.entities.iter().find(|e| e.id == id) else {
        if card.display != Display::None {
            card.display = Display::None;
        }
        return;
    };

    if card.display == Display::None {
        card.display = Display::Flex;
    }
    if let Some(pos) = pointer.cursor_pos {
        let ui_pos = (pos + CARD_OFFSET_PX) / ui_scale.0;
        let want_left = Val::Px(ui_pos.x);
        let want_top = Val::Px(ui_pos.y);
        if card.left != want_left {
            card.left = want_left;
        }
        if card.top != want_top {
            card.top = want_top;
        }
    }

    if let Ok(mut text) = name_q.single_mut() {
        let want = format_name(ent.name.as_deref(), ent.kind);
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = hp_q.single_mut() {
        let want = match ent.hp_pct {
            Some(p) => format!("HP {p}%"),
            None => String::new(),
        };
        if **text != want {
            **text = want;
        }
    }
}

/// Hovering your own body still click-targets it, but retail draws no
/// mouse-over card for yourself; the current target's card is likewise
/// suppressed in favor of the target frame.
fn hover_card_id(hovered: Option<u32>, target: Option<u32>, self_id: Option<u32>) -> Option<u32> {
    let id = hovered?;
    if target == Some(id) || self_id == Some(id) {
        return None;
    }
    Some(id)
}

fn format_name(name: Option<&str>, kind: EntityKind) -> String {
    let n = name.unwrap_or("?");
    format!("{n}  [{}]", kind_tag(kind))
}

fn kind_tag(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "pc",
        EntityKind::Npc => "npc",
        EntityKind::Mob => "mob",
        EntityKind::Pet => "pet",
        EntityKind::Other => "obj",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_card_cannot_intercept_its_own_world_hover() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::picking::{
            backend::{HitData, PointerHits},
            hover::{generate_hovermap, HoverMap, PreviousHoverMap},
            pointer::{PointerId, PointerInput},
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<PointerHits>()
            .add_message::<PointerInput>()
            .init_resource::<HoverMap>()
            .init_resource::<PreviousHoverMap>()
            .init_resource::<HoveredEntity>()
            .init_resource::<crate::picking::PickBridgePointer>()
            .init_resource::<Target>()
            .init_resource::<UiScale>()
            .insert_resource(MousePointer {
                cursor_pos: Some(Vec2::new(600.0, 300.0)),
                ..default()
            })
            .init_resource::<SceneState>()
            .add_systems(
                Update,
                (
                    generate_hovermap,
                    crate::picking::update_hovered_entity_system,
                    update_entity_hover_card_system,
                )
                    .chain(),
            );
        app.world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .entities
            .push(
                serde_json::from_value(serde_json::json!({
                    "id":42, "act_index":1, "kind":"npc", "name":"Guide",
                    "pos":{"x":0.0,"y":0.0,"z":0.0}, "heading":0, "hp_pct":100, "bt_target_id":0
                }))
                .unwrap(),
            );
        app.world_mut().spawn(PointerId::Mouse);
        let plate = app
            .world_mut()
            .spawn(crate::components::Nameplate {
                entity_id: 42,
                kind: EntityKind::Npc,
            })
            .id();
        app.world_mut()
            .run_system_once(spawn_entity_hover_card)
            .unwrap();
        let card = app
            .world_mut()
            .query_filtered::<Entity, With<EntityHoverCard>>()
            .single(app.world())
            .unwrap();
        let mut surfaces = vec![card];
        surfaces.extend(app.world().get::<Children>(card).unwrap().iter());
        for _ in 0..6 {
            if app.world().get::<Node>(card).unwrap().display != Display::None {
                for surface in &surfaces {
                    app.world_mut().write_message(PointerHits::new(
                        PointerId::Mouse,
                        vec![(*surface, HitData::new(Entity::PLACEHOLDER, 0.0, None, None))],
                        0.5,
                    ));
                }
            }
            app.world_mut().write_message(PointerHits::new(
                PointerId::Mouse,
                vec![(plate, HitData::new(Entity::PLACEHOLDER, 5.0, None, None))],
                0.0,
            ));
            app.update();
            assert_eq!(app.world().resource::<HoveredEntity>().id, Some(42));
            assert_eq!(
                app.world().get::<Node>(card).unwrap().display,
                Display::Flex
            );
        }
        for scale in [0.5, 1.0, 1.5] {
            app.world_mut().resource_mut::<UiScale>().0 = scale;
            app.world_mut()
                .run_system_once(update_entity_hover_card_system)
                .unwrap();
            let card = app.world().get::<Node>(card).unwrap();
            assert_eq!(card.left, Val::Px(618.0 / scale));
            assert_eq!(card.top, Val::Px(318.0 / scale));
        }
    }

    #[test]
    fn self_hover_shows_no_card() {
        assert_eq!(hover_card_id(Some(7), None, Some(7)), None);
        assert_eq!(hover_card_id(Some(42), None, Some(7)), Some(42));
        assert_eq!(hover_card_id(Some(42), Some(42), Some(7)), None);
        assert_eq!(hover_card_id(None, None, Some(7)), None);
    }

    #[test]
    fn name_format_uses_kind_tag() {
        assert_eq!(format_name(Some("Mandy"), EntityKind::Mob), "Mandy  [mob]");
        assert_eq!(format_name(None, EntityKind::Pc), "?  [pc]");
    }
}
