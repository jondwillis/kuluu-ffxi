//! Shared item-grid cell widget: the bordered square with a centered icon and
//! a small fallback label, used by the equipment screen's 4x4 slot grid and
//! the delivery-box 2x4 grid in the dialog panel.

use bevy::prelude::*;

use crate::hud::style::text_font;
use crate::hud::style::theme;

pub(crate) const CELL_PX: f32 = 36.0;
pub(crate) const ICON_PX: f32 = 30.0;
pub(crate) const CELL_GAP_PX: f32 = 4.0;

/// Type size for a cell's slot name.
const NAME_FONT_PX: f32 = 11.0;

const COUNT_FONT_PX: f32 = 11.0;
// Edging every side keeps pale digits readable over bright item art.
const COUNT_OUTLINE_PX: f32 = 1.0;
const COUNT_FOREGROUND_Z: i32 = 1;
const COUNT_OUTLINE_DIRECTIONS: [Vec2; 8] = [
    Vec2::new(-1.0, -1.0),
    Vec2::new(0.0, -1.0),
    Vec2::new(1.0, -1.0),
    Vec2::new(-1.0, 0.0),
    Vec2::new(1.0, 0.0),
    Vec2::new(-1.0, 1.0),
    Vec2::new(0.0, 1.0),
    Vec2::new(1.0, 1.0),
];

#[derive(Component)]
pub(crate) struct StackCount;

#[derive(Component)]
pub(crate) struct CountOutline(Entity);

pub(crate) fn spawn_stack_count(
    parent: &mut ChildSpawnerCommands,
    marker: impl Bundle,
    placement: Node,
) {
    let foreground = parent
        .spawn((
            marker,
            StackCount,
            Text::new(""),
            placement.clone(),
            text_font(COUNT_FONT_PX),
            TextColor(theme::TEXT),
            ZIndex(COUNT_FOREGROUND_Z),
        ))
        .id();
    for direction in COUNT_OUTLINE_DIRECTIONS {
        let offset = direction * COUNT_OUTLINE_PX;
        parent.spawn((
            CountOutline(foreground),
            Text::new(""),
            placement.clone(),
            text_font(COUNT_FONT_PX),
            TextColor(Color::BLACK),
            UiTransform::from_translation(Val2::px(offset.x, offset.y)),
        ));
    }
}

pub(crate) fn sync_count_outlines(
    mut commands: Commands,
    counts: Query<(&Text, &Node, &TextFont), (With<StackCount>, Without<CountOutline>)>,
    mut outlines: Query<(Entity, &CountOutline, &mut Text, &mut Node, &mut TextFont)>,
) {
    for (entity, outline, mut text, mut node, mut font) in &mut outlines {
        let Ok((source_text, source_node, source_font)) = counts.get(outline.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        if **text != **source_text {
            **text = source_text.0.clone();
        }
        if *node != *source_node {
            *node = source_node.clone();
        }
        if *font != *source_font {
            *font = source_font.clone();
        }
    }
}

/// What a cell draws over its art.
pub(crate) enum CellOverlay<'a> {
    /// The slot's name, which the art covers once something is in the slot.
    Name(&'a str),
    /// The stack count, which reads on top of the art.
    StackCount,
}

/// Spawn one grid cell: a `CELL_PX` framed square containing a (hidden by
/// default) `ICON_PX` icon and its overlay. Marker components for the
/// frame / icon / overlay are supplied by the caller so each screen can drive
/// its own update systems over the shared structure.
///
/// The overlay is absolutely positioned rather than laid out beside the icon,
/// and child order decides which of the two is on top. A slot name goes
/// *under* the art, so an equipped slot reads as its gear and an empty one as
/// its name; a stack count goes over it, because a count the art hides counts
/// nothing. Observed on the horizonxi-2023 client
/// (.agents/skills/retail-observe/references/vanilla-menu-spec.md,
/// "`/check` on a player → wares + gear").
pub(crate) fn spawn_item_cell(
    p: &mut ChildSpawnerCommands,
    frame_marker: impl Bundle,
    icon_marker: impl Bundle,
    label_marker: impl Bundle,
    overlay: CellOverlay<'_>,
    placeholder: Handle<Image>,
) {
    p.spawn((
        frame_marker,
        Node {
            width: Val::Px(CELL_PX),
            height: Val::Px(CELL_PX),
            border: UiRect::all(Val::Px(1.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme::CELL_BG),
        BorderColor::all(theme::CELL_EDGE),
    ))
    .with_children(|c| {
        let placement = Node {
            position_type: PositionType::Absolute,
            ..default()
        };
        match overlay {
            CellOverlay::Name(text) => {
                c.spawn((
                    label_marker,
                    Text::new(text),
                    text_font(NAME_FONT_PX),
                    TextColor(theme::MUTED),
                    placement,
                ));
                spawn_cell_icon(c, icon_marker, placeholder);
            }
            CellOverlay::StackCount => {
                spawn_cell_icon(c, icon_marker, placeholder);
                spawn_stack_count(c, label_marker, placement);
            }
        }
    });
}

fn spawn_cell_icon(
    c: &mut ChildSpawnerCommands,
    icon_marker: impl Bundle,
    placeholder: Handle<Image>,
) {
    c.spawn((
        icon_marker,
        Node {
            width: Val::Px(ICON_PX),
            height: Val::Px(ICON_PX),
            display: Display::None,
            ..default()
        },
        ImageNode::new(placeholder),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::item_ui::transparent_placeholder;

    #[derive(Component)]
    struct Frame;

    #[derive(Component)]
    struct Icon;

    #[derive(Component)]
    struct Overlay;

    #[derive(Resource)]
    struct WantName(bool);

    fn spawn_one(mut commands: Commands, mut images: ResMut<Assets<Image>>, want: Res<WantName>) {
        let placeholder = transparent_placeholder(&mut images);
        commands.spawn(Node::default()).with_children(|p| {
            let overlay = if want.0 {
                CellOverlay::Name("Main")
            } else {
                CellOverlay::StackCount
            };
            spawn_item_cell(p, Frame, Icon, Overlay, overlay, placeholder);
        });
    }

    fn cell(want_name: bool) -> (Vec<Entity>, App) {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .insert_resource(WantName(want_name))
            .add_systems(Startup, spawn_one)
            .add_systems(PostUpdate, sync_count_outlines);
        app.update();
        let mut q = app.world_mut().query_filtered::<&Children, With<Frame>>();
        let kids = q.single(app.world()).expect("one cell").to_vec();
        (kids, app)
    }

    /// Child order is draw order, and the two overlays want opposite answers:
    /// an equipped slot must read as its gear, and a stack count must read over
    /// whatever art it is counting.
    #[test]
    fn a_slot_name_sits_under_the_art_and_a_stack_count_over_it() {
        let (kids, app) = cell(true);
        assert_eq!(kids.len(), 2);
        assert!(
            app.world().get::<Overlay>(kids[0]).is_some(),
            "the slot name is spawned first, so the icon covers it"
        );
        assert!(app.world().get::<Icon>(kids[1]).is_some());

        let (kids, app) = cell(false);
        assert_eq!(kids.len(), 2 + COUNT_OUTLINE_DIRECTIONS.len());
        assert!(
            app.world().get::<Icon>(kids[0]).is_some(),
            "the count is spawned last, so it draws over the icon"
        );
        assert!(app.world().get::<Overlay>(kids[1]).is_some());
    }

    #[test]
    fn a_stack_count_has_edging_on_all_sides_without_a_box() {
        let (kids, app) = cell(false);
        let count = kids[1];
        let node = app.world().get::<Node>(count).expect("count node");
        assert_eq!(node.border, UiRect::ZERO);
        assert_eq!(node.padding, UiRect::ZERO);
        let bg = app
            .world()
            .get::<BackgroundColor>(count)
            .expect("default background");
        assert_eq!(bg.0, Color::NONE);
        let border = app
            .world()
            .get::<BorderColor>(count)
            .expect("default border");
        assert_eq!(*border, BorderColor::DEFAULT);
        for (&entity, direction) in kids[2..].iter().zip(COUNT_OUTLINE_DIRECTIONS) {
            let outline = app.world().get::<CountOutline>(entity).unwrap();
            assert_eq!(outline.0, count);
            assert_eq!(
                app.world().get::<TextColor>(entity).unwrap().0,
                Color::BLACK
            );
            let offset = direction * COUNT_OUTLINE_PX;
            assert_eq!(
                *app.world().get::<UiTransform>(entity).unwrap(),
                UiTransform::from_translation(Val2::px(offset.x, offset.y))
            );
        }
    }

    #[test]
    fn outline_tracks_count_changes_visibility_and_despawn() {
        let (kids, mut app) = cell(false);
        let count = kids[1];
        app.world_mut().get_mut::<Text>(count).unwrap().0 = "12".into();
        app.update();
        for &entity in &kids[2..] {
            assert_eq!(app.world().get::<Text>(entity).unwrap().0, "12");
        }
        app.world_mut().get_mut::<Text>(count).unwrap().0.clear();
        app.world_mut().get_mut::<Node>(count).unwrap().display = Display::None;
        app.update();
        for &entity in &kids[2..] {
            assert!(app.world().get::<Text>(entity).unwrap().0.is_empty());
            assert_eq!(
                app.world().get::<Node>(entity).unwrap().display,
                Display::None
            );
        }
        app.world_mut().despawn(count);
        app.update();
        for &entity in &kids[2..] {
            assert!(app.world().get_entity(entity).is_err());
        }
    }
}
