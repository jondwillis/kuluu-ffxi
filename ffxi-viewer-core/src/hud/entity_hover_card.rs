use bevy::prelude::*;
use ffxi_viewer_wire::EntityKind;

use crate::hud::palette;
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
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
            ZIndex(i32::MAX - 1),
        ))
        .with_children(|p| {
            p.spawn((
                EntityHoverCardName,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
            p.spawn((
                EntityHoverCardHp,
                Text::new(""),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(palette::MUTED),
            ));
        });
}

pub fn update_entity_hover_card_system(
    hovered: Res<HoveredEntity>,
    target: Res<Target>,
    state: Res<SceneState>,
    pointer: Res<MousePointer>,
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

    let id = match hovered.id {
        Some(id) if target.id != Some(id) => id,
        _ => {
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
        let want_left = Val::Px(pos.x + CARD_OFFSET_PX.x);
        let want_top = Val::Px(pos.y + CARD_OFFSET_PX.y);
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
    fn name_format_uses_kind_tag() {
        assert_eq!(format_name(Some("Mandy"), EntityKind::Mob), "Mandy  [mob]");
        assert_eq!(format_name(None, EntityKind::Pc), "?  [pc]");
    }
}
