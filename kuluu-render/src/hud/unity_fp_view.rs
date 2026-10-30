//! Draws the windows that draw themselves (Unity, alter ego point, merit and job point windows)
//! at their own DAT record frames, every string at its window-relative position
//! (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md,
//! .agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md);
//! the ones whose registry entries carry a dock slot take their y from the dock law
//! (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).

use bevy::prelude::*;
use bevy::text::LineHeight;

use crate::hud::dock::DockBoard;
use crate::hud::menu::{place_at_record_frame, ConfigDatRows, DynamicMenu, DynamicMenuRow};
use crate::hud::merit_windows::{MeritContext, MeritDat};
use crate::hud::style;
use crate::hud::unity_fp::{
    panel_windows, retail_text_color, PanelContext, PanelWindow, TextAlign, UnityFpDat,
};
use crate::input_mode::InputMode;

/// More windows than one of these chains keeps open at once (the merit chain draws eight).
pub const MAX_UNITY_FP_WINDOWS: usize = 10;
/// More strings than the busiest of these windows draws (a full `jbpcat` page).
pub const MAX_UNITY_FP_TEXTS: usize = 64;
const UNITY_FP_FONT_PX: f32 = 13.0;

#[derive(Component)]
pub struct UnityFpRoot;

#[derive(Component)]
pub struct UnityFpWindowNode {
    pub slot: usize,
}

#[derive(Component)]
pub struct UnityFpTextNode {
    pub window: usize,
    pub slot: usize,
}

/// The second run of a text node, drawn in its own color.
#[derive(Component)]
pub struct UnityFpTextSuffix {
    pub window: usize,
    pub slot: usize,
}

pub fn spawn_unity_fp_windows(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            UnityFpRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::ZERO,
                top: Val::ZERO,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                ..default()
            },
            ZIndex(style::WINDOW_Z),
            bevy::picking::Pickable::IGNORE,
        ))
        .with_children(|root| {
            for slot in 0..MAX_UNITY_FP_WINDOWS {
                let (mut node, bg, border) = style::window_frame();
                node.position_type = PositionType::Absolute;
                node.padding = UiRect::ZERO;
                node.display = Display::None;
                root.spawn((UnityFpWindowNode { slot }, node, bg, border, ZIndex(0)))
                    .with_children(|window| {
                        for text_slot in 0..MAX_UNITY_FP_TEXTS {
                            window
                                .spawn((
                                    UnityFpTextNode {
                                        window: slot,
                                        slot: text_slot,
                                    },
                                    Node {
                                        position_type: PositionType::Absolute,
                                        display: Display::None,
                                        ..default()
                                    },
                                    Text::new(""),
                                    TextLayout {
                                        linebreak: LineBreak::NoWrap,
                                        ..default()
                                    },
                                    style::text_font(UNITY_FP_FONT_PX),
                                    TextColor(style::theme::TEXT),
                                ))
                                .with_child((
                                    UnityFpTextSuffix {
                                        window: slot,
                                        slot: text_slot,
                                    },
                                    TextSpan::new(""),
                                    style::text_font(UNITY_FP_FONT_PX),
                                    TextColor(style::theme::TEXT),
                                ));
                        }
                    });
            }
        });
}

#[allow(clippy::type_complexity)]
pub fn update_unity_fp_windows(
    mode: Res<InputMode>,
    scene: Res<crate::snapshot::SceneState>,
    dynamic: Res<DynamicMenu>,
    dat: Res<UnityFpDat>,
    merit_dat: Res<MeritDat>,
    config_rows: Res<ConfigDatRows>,
    dock: Option<Res<DockBoard>>,
    mut root_q: Query<
        &mut Node,
        (
            With<UnityFpRoot>,
            Without<UnityFpWindowNode>,
            Without<UnityFpTextNode>,
        ),
    >,
    mut window_q: Query<
        (&UnityFpWindowNode, &mut Node, &mut ZIndex),
        (Without<UnityFpTextNode>, Without<UnityFpRoot>),
    >,
    mut text_q: Query<
        (
            &UnityFpTextNode,
            &mut Node,
            &mut Text,
            &mut TextColor,
            &mut TextLayout,
            &mut LineHeight,
        ),
        (
            Without<UnityFpWindowNode>,
            Without<UnityFpRoot>,
            Without<UnityFpTextSuffix>,
        ),
    >,
    mut suffix_q: Query<
        (&UnityFpTextSuffix, &mut TextSpan, &mut TextColor),
        Without<UnityFpTextNode>,
    >,
) {
    let shown: Vec<PanelWindow> = match &*mode {
        InputMode::Menu(stack) => {
            let cx = PanelContext {
                snap: &scene.snapshot,
                dat: &dat,
                windows: &dynamic.unity_fp,
            };
            let merit_cx = MeritContext {
                snap: &scene.snapshot,
                dat: &merit_dat,
                windows: &dynamic.merit,
            };
            let top = stack.levels.len().saturating_sub(1);
            let rows_of = |index: usize| -> Vec<DynamicMenuRow> {
                if index == top {
                    return dynamic.rows.clone();
                }
                let kind = stack.levels[index].kind;
                dynamic
                    .stacked_rows
                    .iter()
                    .find(|(open, _)| *open == kind)
                    .map(|(_, rows)| rows.clone())
                    .unwrap_or_default()
            };
            let mut shown = panel_windows(stack, rows_of, &cx);
            shown.extend(crate::hud::merit_windows::panel_windows(
                stack, rows_of, &merit_cx,
            ));
            shown.sort_by_key(|window| window.stack_index);
            shown
        }
        _ => Vec::new(),
    };
    let placed: Vec<(PanelWindow, crate::hud::menu::RecordFrame)> = shown
        .into_iter()
        .filter_map(|window| {
            let frame = config_rows.record_frame(window.name)?;
            Some((window, frame))
        })
        .take(MAX_UNITY_FP_WINDOWS)
        .collect();

    if let Ok(mut root) = root_q.single_mut() {
        let want = if placed.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if root.display != want {
            root.display = want;
        }
    }

    for (window, mut node, mut z_index) in window_q.iter_mut() {
        let mut want = node.clone();
        match placed.get(window.slot) {
            Some((panel, frame)) => {
                place_at_record_frame(
                    &mut want,
                    &crate::hud::menu::RecordFrame {
                        y: frame.y + panel.shift_y,
                        ..*frame
                    },
                );
                want.height = frame.height.map_or(Val::Auto, Val::Px);
                if let Some(placement) = panel
                    .dock
                    .and(dock.as_deref())
                    .and_then(|board| board.placement(panel.name))
                {
                    want.top = Val::Px(placement.top(frame.height.unwrap_or_default()));
                    want.margin.top = Val::ZERO;
                    want.bottom = Val::Auto;
                }
                want.display = Display::Flex;
                z_index.set_if_neq(ZIndex(i32::try_from(window.slot).unwrap_or(i32::MAX)));
            }
            None => want.display = Display::None,
        }
        node.set_if_neq(want);
    }

    for (slot, mut node, mut text, mut color, mut layout, mut line_height) in text_q.iter_mut() {
        let Some((panel, frame)) = placed.get(slot.window) else {
            continue;
        };
        let Some(entry) = panel.texts.get(slot.slot) else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        let mut want = node.clone();
        want.display = Display::Flex;
        want.top = Val::Px(entry.y);
        (want.left, want.right) = match entry.align {
            TextAlign::Left => (Val::Px(entry.x), Val::Auto),
            TextAlign::Right => (Val::Auto, Val::Px(frame.width - entry.x)),
        };
        want.width = entry.wrap_width.map_or(Val::Auto, Val::Px);
        node.set_if_neq(want);
        if text.0 != entry.text {
            text.0.clone_from(&entry.text);
        }
        let want_color = retail_text_color(entry.color);
        if color.0 != want_color {
            color.0 = want_color;
        }
        let linebreak = if entry.wrap_width.is_some() {
            LineBreak::WordBoundary
        } else {
            LineBreak::NoWrap
        };
        if layout.linebreak != linebreak {
            layout.linebreak = linebreak;
        }
        let want_line_height = entry
            .line_pitch
            .map_or_else(LineHeight::default, LineHeight::Px);
        if *line_height != want_line_height {
            *line_height = want_line_height;
        }
    }

    for (slot, mut span, mut color) in suffix_q.iter_mut() {
        let suffix = placed
            .get(slot.window)
            .and_then(|(panel, _)| panel.texts.get(slot.slot))
            .and_then(|entry| entry.suffix.as_ref());
        let (want_text, want_color) = match suffix {
            Some((text, argb)) => (text.as_str(), retail_text_color(*argb)),
            None => ("", color.0),
        };
        if span.as_str() != want_text {
            **span = want_text.to_string();
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real schedule checks query conflicts when the system first runs; ticking it once on a
    /// bare app pins that every mutable query here is disjoint.
    #[test]
    fn update_runs_in_a_real_schedule_without_query_conflicts() {
        let mut app = App::new();
        app.insert_resource(InputMode::default())
            .init_resource::<crate::snapshot::SceneState>()
            .init_resource::<DynamicMenu>()
            .init_resource::<UnityFpDat>()
            .init_resource::<MeritDat>()
            .init_resource::<ConfigDatRows>()
            .add_systems(Startup, spawn_unity_fp_windows)
            .add_systems(Update, update_unity_fp_windows);
        app.update();
        let root = app
            .world_mut()
            .query_filtered::<&Node, With<UnityFpRoot>>()
            .single(app.world())
            .expect("one root")
            .clone();
        assert_eq!(root.display, Display::None, "nothing open, nothing drawn");
    }

    /// The `merityn` prompt's lines draw at its pitch, not the font's own line height.
    #[test]
    fn the_merityn_prompt_draws_its_lines_at_its_pitch() {
        use crate::hud::unity_fp::{
            client_message_fallback, layout, ALTER_EGO_PROMPT_ENTRY, MERITYN_WINDOW,
        };
        const MERITYN_HEIGHT: i16 = 60;
        let mut stack = crate::input_mode::MenuStack::root();
        stack.push(crate::input_mode::MenuKind::AlterEgoConfirm { upgrade: 1 });
        let mut app = App::new();
        app.insert_resource(InputMode::Menu(stack))
            .init_resource::<crate::snapshot::SceneState>()
            .init_resource::<DynamicMenu>()
            .init_resource::<UnityFpDat>()
            .init_resource::<MeritDat>()
            .init_resource::<ConfigDatRows>()
            .add_systems(Startup, spawn_unity_fp_windows)
            .add_systems(Update, update_unity_fp_windows);
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            rows.frames.insert(MERITYN_WINDOW.into(), (40, 60, 200));
            rows.frame_extents.insert(
                MERITYN_WINDOW.into(),
                (MERITYN_HEIGHT, crate::hud::menu::FRAME_ANCHOR_NONE),
            );
        }
        app.update();
        let prompt = client_message_fallback(
            crate::hud::gettext::GETTEXT_TABLE_10,
            ALTER_EGO_PROMPT_ENTRY,
        );
        let line_height = app
            .world_mut()
            .query::<(&Text, &LineHeight)>()
            .iter(app.world())
            .find(|(text, _)| text.0 == prompt)
            .map(|(_, line_height)| *line_height)
            .expect("the prompt is drawn");
        assert_eq!(line_height, LineHeight::Px(layout::MERITYN_LINE_PITCH));
    }

    /// `ut_point` keeps its record x and sits 2 above the party window's top edge; once the party
    /// window goes away it stays where it was docked.
    #[test]
    fn ut_point_docks_above_the_party_window_and_stays_when_it_closes() {
        const UT_POINT_HEIGHT: i16 = 30;
        const PARTY_TOP: f32 = 300.0;
        const PARTY_HEIGHT: f32 = 100.0;
        let mut stack = crate::input_mode::MenuStack::root();
        stack.push(crate::input_mode::MenuKind::UnityMenu);
        let mut app = App::new();
        app.insert_resource(InputMode::Menu(stack))
            .init_resource::<crate::snapshot::SceneState>()
            .init_resource::<DynamicMenu>()
            .init_resource::<UnityFpDat>()
            .init_resource::<MeritDat>()
            .init_resource::<ConfigDatRows>()
            .init_resource::<DockBoard>()
            .add_systems(Startup, spawn_unity_fp_windows)
            .add_systems(
                Update,
                (
                    crate::hud::dock::dock_windows_system,
                    update_unity_fp_windows,
                )
                    .chain(),
            );
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            for (name, frame, height) in [
                (crate::hud::unity_fp::UT_MENU_WINDOW, (40, 60, 120), 80),
                (
                    crate::hud::unity_fp::UT_POINT_WINDOW,
                    (24, 200, 110),
                    UT_POINT_HEIGHT,
                ),
            ] {
                rows.frames.insert(name.into(), frame);
                rows.frame_extents
                    .insert(name.into(), (height, crate::hud::menu::FRAME_ANCHOR_NONE));
            }
        }
        let party = app
            .world_mut()
            .spawn((
                crate::hud::party_frame::PartyFrameRoot { party_no: 0 },
                Node::default(),
                bevy::ui::ComputedNode {
                    size: Vec2::new(style::PANEL_WIDTH_PX, PARTY_HEIGHT),
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                bevy::ui::UiGlobalTransform::from_xy(0.0, PARTY_TOP + PARTY_HEIGHT / 2.0),
            ))
            .id();
        app.update();

        let ut_point = |app: &mut App| -> Node {
            app.world_mut()
                .query::<(&UnityFpWindowNode, &Node)>()
                .iter(app.world())
                .find(|(w, _)| w.slot == 1)
                .map(|(_, n)| n.clone())
                .expect("ut_point window")
        };
        let docked_at = PARTY_TOP - f32::from(UT_POINT_HEIGHT) - crate::hud::dock::DOCK_GAP;
        let node = ut_point(&mut app);
        assert_eq!(node.display, Display::Flex);
        assert_eq!((node.left, node.top), (Val::Px(24.0), Val::Px(docked_at)));

        app.world_mut().entity_mut(party).despawn();
        app.update();
        assert_eq!(
            ut_point(&mut app).top,
            Val::Px(docked_at),
            "an anchor that closes leaves its docked window in place"
        );
    }

    /// `merityn` docks in the log slot: record x, bottom edge 2 above the chat log's top edge.
    #[test]
    fn merityn_docks_above_the_chat_log() {
        use crate::hud::unity_fp::MERITYN_WINDOW;
        const MERITYN_X: i16 = 40;
        const MERITYN_HEIGHT: i16 = 60;
        const LOG_TOP: f32 = 400.0;
        const LOG_HEIGHT: f32 = 120.0;
        let mut stack = crate::input_mode::MenuStack::root();
        stack.push(crate::input_mode::MenuKind::AlterEgoConfirm { upgrade: 1 });
        let mut app = App::new();
        app.insert_resource(InputMode::Menu(stack))
            .init_resource::<crate::snapshot::SceneState>()
            .init_resource::<DynamicMenu>()
            .init_resource::<UnityFpDat>()
            .init_resource::<MeritDat>()
            .init_resource::<ConfigDatRows>()
            .init_resource::<DockBoard>()
            .add_systems(Startup, spawn_unity_fp_windows)
            .add_systems(
                Update,
                (
                    crate::hud::dock::dock_windows_system,
                    update_unity_fp_windows,
                )
                    .chain(),
            );
        {
            let mut rows = app.world_mut().resource_mut::<ConfigDatRows>();
            rows.frames
                .insert(MERITYN_WINDOW.into(), (MERITYN_X, 60, 200));
            rows.frame_extents.insert(
                MERITYN_WINDOW.into(),
                (MERITYN_HEIGHT, crate::hud::menu::FRAME_ANCHOR_NONE),
            );
        }
        app.world_mut().spawn((
            crate::hud::chat_panel::ChatPanelGroup,
            Node::default(),
            bevy::ui::ComputedNode {
                size: Vec2::new(style::PANEL_WIDTH_PX, LOG_HEIGHT),
                inverse_scale_factor: 1.0,
                ..default()
            },
            bevy::ui::UiGlobalTransform::from_xy(0.0, LOG_TOP + LOG_HEIGHT / 2.0),
        ));
        app.update();
        let node = app
            .world_mut()
            .query::<(&UnityFpWindowNode, &Node)>()
            .iter(app.world())
            .find(|(w, _)| w.slot == 0)
            .map(|(_, n)| n.clone())
            .expect("merityn window");
        assert_eq!(node.display, Display::Flex);
        assert_eq!(
            (node.left, node.top),
            (
                Val::Px(f32::from(MERITYN_X)),
                Val::Px(LOG_TOP - crate::hud::dock::DOCK_GAP - f32::from(MERITYN_HEIGHT))
            )
        );
    }
}
