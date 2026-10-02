use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::input::ButtonState;
use bevy::input_focus::tab_navigation::{TabGroup, TabIndex};
use bevy::input_focus::{FocusCause, InputFocus, InputFocusVisible};
use bevy::prelude::*;
use bevy::ui::{Checked, ComputedNode, InteractionDisabled, ScrollPosition, UiGlobalTransform};
use bevy::ui_widgets::{Activate, Button, Checkbox, ValueChange};
use bevy::window::PrimaryWindow;

use crate::view_native::gamepad_input::{LauncherNav, NavDir, PageDir};
use crate::view_native::widgets::text_field::{TextEntryActive, TextField, TextFieldSubmitted};

use super::{LauncherState, ServerInfo};

#[derive(Clone)]
#[allow(dead_code)]
pub(super) enum Crumb {
    Server,

    Sign(Option<String>),

    Characters,

    Other(String),
}

impl Crumb {
    fn label(&self) -> String {
        match self {
            Crumb::Server => "Servers".to_string(),
            Crumb::Sign(Some(u)) => format!("Sign in: {u}"),
            Crumb::Sign(None) => "Sign in".to_string(),
            Crumb::Characters => "Characters".to_string(),
            Crumb::Other(s) => s.clone(),
        }
    }

    fn target(&self) -> Option<LauncherState> {
        match self {
            Crumb::Server => Some(LauncherState::ServerSelect),
            Crumb::Sign(_) => Some(LauncherState::Login),
            Crumb::Characters => Some(LauncherState::CharList),
            Crumb::Other(_) => None,
        }
    }
}

/// Marker for the widget that should receive initial keyboard focus when its
/// screen is shown (see [`focus_default_target_system`]). The blue focus
/// outline starts here instead of nowhere, so Enter activates it immediately.
#[derive(Component)]
pub(super) struct DefaultFocusTarget;

pub(super) const PANEL_BG: Color = Color::srgba(0.04, 0.04, 0.05, 0.85);
pub(super) const PANEL_BORDER_COLOR: Color = Color::srgb(0.20, 0.20, 0.24);

const PANEL_ROW_GAP: f32 = 12.0;
const PANEL_PADDING: f32 = 24.0;
const PANEL_BORDER: f32 = 1.0;
const PANEL_RADIUS: f32 = 6.0;
const SCREEN_EDGE_PADDING: f32 = 16.0;

fn panel_layout(width_px: f32) -> Node {
    Node {
        width: Val::Px(width_px),
        max_width: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Stretch,
        justify_content: JustifyContent::FlexStart,
        row_gap: Val::Px(PANEL_ROW_GAP),
        padding: UiRect::all(Val::Px(PANEL_PADDING)),
        border: UiRect::all(Val::Px(PANEL_BORDER)),
        border_radius: BorderRadius::all(Val::Px(PANEL_RADIUS)),
        ..default()
    }
}

fn panel_bundle(node: Node) -> impl Bundle {
    (
        node,
        BackgroundColor(PANEL_BG),
        BorderColor::all(PANEL_BORDER_COLOR),
        TabGroup::default(),
    )
}

pub(super) fn panel_node(width_px: f32) -> impl Bundle {
    panel_bundle(panel_layout(width_px))
}

/// Like [`panel_node`] but capped to `max_height` so a body taller than the
/// window scrolls inside the panel instead of spilling off screen. The caller
/// gives one child `flex_grow: 1` + `min_height: 0` + `Overflow::scroll_y()` to
/// be the scroll region; the panel's other children stay pinned.
pub(super) fn panel_node_capped(width_px: f32, max_height: Val) -> impl Bundle {
    panel_bundle(Node {
        max_height,
        ..panel_layout(width_px)
    })
}

pub(super) fn screen_root() -> impl Bundle {
    Node {
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        padding: UiRect::new(
            Val::Px(SCREEN_EDGE_PADDING),
            Val::Px(SCREEN_EDGE_PADDING),
            Val::Px(SCREEN_EDGE_PADDING),
            Val::Px(SCREEN_EDGE_PADDING + super::footer::FOOTER_RESERVED_PX),
        ),
        ..default()
    }
}

pub(super) fn title(text: impl Into<String>) -> impl Bundle {
    (
        Text::new(text.into()),
        TextFont {
            font_size: 22.0.into(),
            ..default()
        },
        TextColor(Color::srgb(0.0, 1.0, 1.0)),
        ThemedText,
    )
}

pub(super) fn hint(text: impl Into<String>) -> impl Bundle {
    (
        Text::new(text.into()),
        TextFont {
            font_size: 12.0.into(),
            ..default()
        },
        TextColor(Color::srgb(0.6, 0.6, 0.65)),
        ThemedText,
    )
}

#[derive(Component)]
pub(super) struct ServerChipLabel;

pub(super) fn spawn_breadcrumb(
    parent: &mut ChildSpawnerCommands,
    server: &ServerInfo,
    crumbs: &[Crumb],
) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            max_width: Val::Percent(100.0),
            column_gap: Val::Px(6.0),
            row_gap: Val::Px(4.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            margin: UiRect::bottom(Val::Px(12.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        })
        .insert((
            BackgroundColor(PANEL_BG),
            BorderColor::all(PANEL_BORDER_COLOR),
        ))
        .with_children(|chip| {
            chip.spawn(button_bundle(
                ButtonBundleProps::default(),
                (),
                Spawn((
                    Text::new(format!("Server: {}", server.display_label())),
                    ThemedText,
                    ServerChipLabel,
                )),
            ))
            .observe(
                |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                    next.set(LauncherState::ServerSelect);
                },
            );

            let Some((_current, trail)) = crumbs.split_last() else {
                return;
            };
            for crumb in trail {
                chip.spawn((
                    Text::new(">"),
                    TextFont {
                        font_size: 14.0.into(),
                        ..default()
                    },
                    TextColor(Color::srgb(0.55, 0.55, 0.60)),
                    ThemedText,
                ));
                let label = crumb.label();
                if let Some(target) = crumb.target() {
                    chip.spawn(button_bundle(
                        ButtonBundleProps {
                            variant: ButtonVariant::Normal,
                            ..default()
                        },
                        (),
                        Spawn((Text::new(label), ThemedText)),
                    ))
                    .observe(
                        move |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(target.clone());
                        },
                    );
                } else {
                    chip.spawn((
                        Text::new(label),
                        TextFont {
                            font_size: 14.0.into(),
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.85, 0.90)),
                        ThemedText,
                    ));
                }
            }
        });
}

pub(super) fn update_server_chips(
    server: Res<ServerInfo>,
    mut q: Query<&mut Text, With<ServerChipLabel>>,
) {
    if !server.is_changed() {
        return;
    }
    let want = format!("Server: {}", server.display_label());
    for mut t in q.iter_mut() {
        if t.0 != want {
            t.0 = want.clone();
        }
    }
}

pub(super) fn row() -> impl Bundle {
    Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::Wrap,
        column_gap: Val::Px(8.0),
        row_gap: Val::Px(8.0),
        align_items: AlignItems::Center,
        ..default()
    }
}

/// Visually attaches a value button and its companion action (e.g. an `x`
/// remove button) into one bordered unit, and refuses to shrink so a wrapping
/// parent moves the whole chip to the next line instead of squishing it.
pub(super) fn chip_group() -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(2.0),
            padding: UiRect::all(Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(PANEL_RADIUS)),
            flex_shrink: 0.0,
            ..default()
        },
        BorderColor::all(Color::srgb(0.28, 0.28, 0.33)),
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.04)),
    )
}

/// Marks a `ScrollPosition` node whose wheel scrolling is handled by
/// [`scroll_region_wheel_system`].
#[derive(Component)]
pub(super) struct ScrollRegion;

const SCROLL_LINE_PX: f32 = 28.0;

/// Scroll any visible [`ScrollRegion`] with the mouse wheel, clamped to its
/// content so it never over- or under-scrolls. Bevy clamps the *rendered*
/// offset but not the [`ScrollPosition`] component, so we clamp it here
/// against [`ComputedNode`].
pub(super) fn scroll_region_wheel_system(
    mut wheel: MessageReader<MouseWheel>,
    mut regions: Query<(&mut ScrollPosition, &ComputedNode), With<ScrollRegion>>,
) {
    let mut delta = 0.0;
    for ev in wheel.read() {
        delta += match ev.unit {
            MouseScrollUnit::Line => ev.y * SCROLL_LINE_PX,
            MouseScrollUnit::Pixel => ev.y,
        };
    }
    if delta == 0.0 {
        return;
    }
    for (mut scroll, node) in regions.iter_mut() {
        let max = node_max_scroll_y(node);
        scroll.0.y = (scroll.0.y - delta).clamp(0.0, max);
    }
}

fn max_scroll_y(content_y: f32, size_y: f32, scrollbar_y: f32, inverse_scale: f32) -> f32 {
    (content_y - size_y + scrollbar_y).max(0.0) * inverse_scale
}

fn node_max_scroll_y(node: &ComputedNode) -> f32 {
    max_scroll_y(
        node.content_size.y,
        node.size.y,
        node.scrollbar_size.y,
        node.inverse_scale_factor,
    )
}

/// The fraction of the incoming page that repeats the outgoing view, so the
/// reader keeps an anchor row.
const PAGE_SCROLL_OVERLAP_FRACTION: f32 = 0.15;

fn paged_scroll_y(current: f32, viewport_y: f32, page: PageDir, max: f32) -> f32 {
    let step = viewport_y * (1.0 - PAGE_SCROLL_OVERLAP_FRACTION);
    let next = match page {
        PageDir::Prev => current - step,
        PageDir::Next => current + step,
    };
    next.clamp(0.0, max)
}

/// Perpendicular misalignment is heavily penalized, so a directional move
/// prefers the widget actually in line with the current one.
pub(super) const SPATIAL_CROSS_PENALTY: f32 = 2.5;
/// Slack so a candidate sharing a row (within sub-pixel rounding) is not
/// counted as being ahead along the movement axis.
const SPATIAL_FORWARD_EPSILON: f32 = 0.25;

/// A node this small is either laid out to nothing or not laid out yet.
/// `graphics` collapses whole sections with `Display::None` while their
/// `button_bundle` children keep their `TabIndex`, so size is what separates a
/// reachable widget from a hidden one.
const MIN_FOCUSABLE_PX: f32 = 1.0;

pub(super) fn is_pad_focusable(size: Vec2, inherited_visible: bool) -> bool {
    inherited_visible && size.x >= MIN_FOCUSABLE_PX && size.y >= MIN_FOCUSABLE_PX
}

/// The first pad nudge on a screen whose ring is still hidden should show
/// where focus already is rather than move it out from under the player.
pub(super) fn move_reveals_only(ring_visible: bool, has_focus: bool) -> bool {
    has_focus && !ring_visible
}

/// Nearest tabbable widget in `dir` from `current`, wrapping to the farthest
/// one behind at an edge. `cands` are widget centers in screen space; the
/// index is into that slice. With no current widget the group centroid is the
/// anchor, so the first move lands somewhere sensible.
pub(super) fn pick_directional(cands: &[Vec2], current: Option<usize>, dir: Vec2) -> Option<usize> {
    if cands.is_empty() {
        return None;
    }
    let cur_pos = match current.and_then(|i| cands.get(i)) {
        Some(p) => *p,
        None => cands.iter().copied().sum::<Vec2>() / cands.len() as f32,
    };

    let mut best_forward: Option<(f32, usize)> = None;
    let mut best_wrap: Option<(f32, f32, usize)> = None;
    for (i, p) in cands.iter().enumerate() {
        if current == Some(i) {
            continue;
        }
        let d = *p - cur_pos;
        let along = d.dot(dir);
        let cross = (d.x * dir.y - d.y * dir.x).abs();
        if along > SPATIAL_FORWARD_EPSILON {
            let score = along + cross * SPATIAL_CROSS_PENALTY;
            if best_forward.is_none_or(|(s, _)| score < s) {
                best_forward = Some((score, i));
            }
        } else if along < 0.0 && best_wrap.is_none_or(|(a, c, _)| (along, cross) < (a, c)) {
            best_wrap = Some((along, cross, i));
        }
    }

    best_forward
        .map(|(_, i)| i)
        .or(best_wrap.map(|(_, _, i)| i))
}

/// Whether pad input moves the focus ring or types into the focused field.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LauncherFocusMode {
    #[default]
    Navigate,
    TextEntry(Entity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavOutcome {
    Nothing,
    MoveFocus(NavDir),
    Activate,
    EnterTextEntry,
    CommitTextEntry,
    ExitTextEntry,
    SendEscape,
    Page(PageDir),
}

fn resolve_intent(
    mode: &LauncherFocusMode,
    focused_is_text_field: bool,
    nav: &LauncherNav,
) -> NavOutcome {
    match mode {
        LauncherFocusMode::Navigate => match *nav {
            LauncherNav::Move(dir) => NavOutcome::MoveFocus(dir),
            LauncherNav::Confirm if focused_is_text_field => NavOutcome::EnterTextEntry,
            LauncherNav::Confirm => NavOutcome::Activate,
            LauncherNav::Cancel => NavOutcome::SendEscape,
            LauncherNav::Page(dir) => NavOutcome::Page(dir),
        },
        LauncherFocusMode::TextEntry(_) => match *nav {
            LauncherNav::Confirm => NavOutcome::CommitTextEntry,
            LauncherNav::Cancel => NavOutcome::ExitTextEntry,
            LauncherNav::Move(_) | LauncherNav::Page(_) => NavOutcome::Nothing,
        },
    }
}

/// A press with no matching release would leave `ButtonInput<KeyCode>` stuck
/// on that key for the rest of the session - the hazard `PadKeyEvent` exists
/// to dodge in-game.
fn synth_key(window: Entity, key_code: KeyCode) -> Option<[KeyboardInput; 2]> {
    let logical_key = kuluu_render::keybinds::logical_key_for(key_code)?;
    let make = |state| KeyboardInput {
        key_code,
        logical_key: logical_key.clone(),
        state,
        text: None,
        repeat: false,
        window,
    };
    Some([make(ButtonState::Pressed), make(ButtonState::Released)])
}

/// How Confirm reaches the focused widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activation {
    Button,
    Toggle,
    /// Nothing focusable under the ring: fall back to the global Enter every
    /// screen's own `MessageReader<KeyboardInput>` already handles.
    ScreenEnter,
    None,
}

/// Buttons and checkboxes are driven through the very triggers their pointer
/// paths use, not a synthesized Enter: a screen-wide Enter handler
/// (`dat_setup::keyboard_input_system`) would otherwise fire its own default
/// action from the same press that activated the focused widget.
fn activation_for(is_button: bool, is_toggle: bool, disabled: bool) -> Activation {
    match (is_button, is_toggle, disabled) {
        (_, _, true) => Activation::None,
        (true, _, false) => Activation::Button,
        (false, true, false) => Activation::Toggle,
        (false, false, false) => Activation::ScreenEnter,
    }
}

/// The one pad-driven focus model for every launcher screen: it moves
/// `InputFocus` spatially over the tabbable widgets (so feathers' focus ring
/// and the widgets' own hover styling stay the single visual) and confirms
/// through each widget's own trigger, falling back to the Enter/Escape every
/// screen already handles. Reads only pad messages, so mouse and keyboard are
/// untouched.
pub(super) fn launcher_focus_nav_system(
    mut nav: MessageReader<LauncherNav>,
    mut mode: ResMut<LauncherFocusMode>,
    mut focus: ResMut<InputFocus>,
    mut visible: ResMut<InputFocusVisible>,
    q_tabs: Query<
        (
            Entity,
            &ComputedNode,
            &UiGlobalTransform,
            &InheritedVisibility,
        ),
        With<TabIndex>,
    >,
    q_fields: Query<&TextField>,
    q_widgets: Query<(
        Has<Button>,
        Has<Checkbox>,
        Has<InteractionDisabled>,
        Has<Checked>,
    )>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut keys: MessageWriter<KeyboardInput>,
    mut commands: Commands,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let mut moved = false;
    for msg in nav.read() {
        let focused = focus.get();
        let is_field = focused.is_some_and(|e| q_fields.contains(e));
        match resolve_intent(&mode, is_field, msg) {
            NavOutcome::Nothing | NavOutcome::Page(_) => {}
            NavOutcome::MoveFocus(dir) => {
                if moved {
                    continue;
                }
                moved = true;
                if move_reveals_only(visible.0, focused.is_some()) {
                    visible.0 = true;
                    continue;
                }
                let cands: Vec<(Vec2, Entity)> = q_tabs
                    .iter()
                    .filter(|(_, cn, _, vis)| is_pad_focusable(cn.size, vis.get()))
                    .map(|(e, cn, gt, _)| (gt.affine().translation + cn.size * 0.5, e))
                    .collect();
                let centers: Vec<Vec2> = cands.iter().map(|(p, _)| *p).collect();
                let current = focused.and_then(|f| cands.iter().position(|(_, e)| *e == f));
                if let Some(i) = pick_directional(&centers, current, dir.as_vec2()) {
                    focus.set(cands[i].1, FocusCause::Navigated);
                    visible.0 = true;
                }
            }
            NavOutcome::Activate => {
                let widget = focused.and_then(|e| Some((e, q_widgets.get(e).ok()?)));
                let (is_button, is_toggle, disabled, checked) =
                    widget.map_or((false, false, false, false), |(_, w)| w);
                match activation_for(is_button, is_toggle, disabled) {
                    Activation::Button => {
                        if let Some((entity, _)) = widget {
                            commands.trigger(Activate { entity });
                        }
                    }
                    Activation::Toggle => {
                        if let Some((source, _)) = widget {
                            commands.trigger(ValueChange {
                                source,
                                value: !checked,
                                is_final: true,
                            });
                        }
                    }
                    Activation::ScreenEnter => {
                        if let Some(events) = synth_key(window, KeyCode::Enter) {
                            keys.write_batch(events);
                        }
                    }
                    Activation::None => {}
                }
            }
            NavOutcome::EnterTextEntry => {
                if let Some(e) = focused {
                    *mode = LauncherFocusMode::TextEntry(e);
                    commands.entity(e).try_insert(TextEntryActive);
                    visible.0 = true;
                }
            }
            NavOutcome::CommitTextEntry => {
                if let LauncherFocusMode::TextEntry(e) = *mode {
                    commands.entity(e).try_remove::<TextEntryActive>();
                    if q_fields.get(e).is_ok_and(|f| f.submit_on_enter) {
                        commands.trigger(TextFieldSubmitted { entity: e });
                    }
                }
                *mode = LauncherFocusMode::Navigate;
            }
            NavOutcome::ExitTextEntry => {
                if let LauncherFocusMode::TextEntry(e) = *mode {
                    commands.entity(e).try_remove::<TextEntryActive>();
                }
                *mode = LauncherFocusMode::Navigate;
            }
            NavOutcome::SendEscape => {
                if let Some(events) = synth_key(window, KeyCode::Escape) {
                    keys.write_batch(events);
                }
            }
        }
    }
}

/// A screen rebuild despawns the focused widget; without this the ring points
/// at a dead entity and no directional move can anchor. Focus is only cleared
/// here - re-landing is [`focus_default_target_system`]'s job, so screens with
/// a global Enter handler do not gain an auto-focused widget.
pub(super) fn reconcile_focus_system(
    mut focus: ResMut<InputFocus>,
    mut mode: ResMut<LauncherFocusMode>,
    q_alive: Query<(), With<TabIndex>>,
    q_fields: Query<(), With<TextField>>,
    mut commands: Commands,
) {
    if focus.get().is_some_and(|e| !q_alive.contains(e)) {
        focus.clear();
    }
    if let LauncherFocusMode::TextEntry(e) = *mode {
        let still_editing = focus.get() == Some(e) && q_fields.contains(e);
        if !still_editing {
            commands.entity(e).try_remove::<TextEntryActive>();
            *mode = LauncherFocusMode::Navigate;
        }
    }
}

/// Initial focus for any screen carrying a [`DefaultFocusTarget`]: land on it
/// once per spawned instance, then leave it alone so Tab and the pad can move
/// away freely until a rebuild or re-entry produces a new instance.
pub(super) fn focus_default_target_system(
    mut input_focus: ResMut<InputFocus>,
    mut last: Local<Option<Entity>>,
    q: Query<Entity, With<DefaultFocusTarget>>,
) {
    let Some(target) = q.iter().next() else {
        *last = None;
        return;
    };
    if *last == Some(target) {
        return;
    }
    input_focus.set(target, FocusCause::Navigated);
    *last = Some(target);
}

/// Shoulder-button paging for the focused widget's [`ScrollRegion`], falling
/// back to every visible region the way the wheel does.
pub(super) fn launcher_page_scroll_system(
    mut nav: MessageReader<LauncherNav>,
    mode: Res<LauncherFocusMode>,
    focus: Res<InputFocus>,
    parents: Query<&ChildOf>,
    mut regions: Query<(Entity, &mut ScrollPosition, &ComputedNode), With<ScrollRegion>>,
) {
    for msg in nav.read() {
        let NavOutcome::Page(dir) = resolve_intent(&mode, false, msg) else {
            continue;
        };
        let owner = focus
            .get()
            .and_then(|f| parents.iter_ancestors(f).find(|a| regions.contains(*a)));
        for (e, mut scroll, node) in regions.iter_mut() {
            if owner.is_some_and(|o| o != e) {
                continue;
            }
            let viewport = node.size.y * node.inverse_scale_factor;
            scroll.0.y = paged_scroll_y(scroll.0.y, viewport, dir, node_max_scroll_y(node));
        }
    }
}

/// Keeps a pad-moved focus ring on screen when it lands inside a scrolled
/// region.
pub(super) fn scroll_focus_into_view_system(
    focus: Res<InputFocus>,
    parents: Query<&ChildOf>,
    q_nodes: Query<(&ComputedNode, &UiGlobalTransform)>,
    mut regions: Query<
        (
            Entity,
            &mut ScrollPosition,
            &ComputedNode,
            &UiGlobalTransform,
        ),
        With<ScrollRegion>,
    >,
) {
    if !focus.is_changed() {
        return;
    }
    let Some(focused) = focus.get() else {
        return;
    };
    let Ok((node, transform)) = q_nodes.get(focused) else {
        return;
    };
    let Some(owner) = parents
        .iter_ancestors(focused)
        .find(|a| regions.contains(*a))
    else {
        return;
    };
    let Ok((_, mut scroll, region_node, region_transform)) = regions.get_mut(owner) else {
        return;
    };

    let half = node.size.y * 0.5;
    let region_half = region_node.size.y * 0.5;
    let top = transform.affine().translation.y - half;
    let bottom = transform.affine().translation.y + half;
    let region_top = region_transform.affine().translation.y - region_half;
    let region_bottom = region_transform.affine().translation.y + region_half;

    let shift = if top < region_top {
        top - region_top
    } else if bottom > region_bottom {
        bottom - region_bottom
    } else {
        return;
    };
    let max = node_max_scroll_y(region_node);
    scroll.0.y = (scroll.0.y + shift * region_node.inverse_scale_factor).clamp(0.0, max);
}

/// A `KeyboardInput` stays readable for a frame after the transition it
/// caused, so without this the Escape that backs out of one screen is read
/// again by the screen it lands on and falls through several screens at once.
/// Runs in `First`, where a [`NextState`] set last frame is still pending.
/// Releases survive: dropping one would strand `ButtonInput<KeyCode>` holding
/// that key, the hazard [`synth_key`] pairs its events to avoid.
pub(super) fn drop_keys_leaving_screen_system(
    next: Res<NextState<LauncherState>>,
    mut keys: ResMut<Messages<KeyboardInput>>,
) {
    if !matches!(*next, NextState::Pending(_)) {
        return;
    }
    let releases: Vec<KeyboardInput> = keys
        .drain()
        .filter(|k| k.state == ButtonState::Released)
        .collect();
    keys.write_batch(releases);
}

/// Launcher-scoped pad state must not survive into the game (the
/// bevy-lifecycle-symmetry rule).
pub(super) fn drain_focus_mode(mut mode: ResMut<LauncherFocusMode>, mut focus: ResMut<InputFocus>) {
    *mode = LauncherFocusMode::Navigate;
    focus.clear();
}

enum NavAction {
    Close,
    Back(LauncherState),
}

fn spawn_titlebar(
    parent: &mut ChildSpawnerCommands,
    title_text: impl Into<String>,
    action: NavAction,
    show_settings: bool,
) {
    let label = match &action {
        NavAction::Close => "X",
        NavAction::Back(_) => "Back to login",
    };
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .with_children(|bar| {
            bar.spawn((
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
                Text::new(title_text.into()),
                TextFont {
                    font_size: 22.0.into(),
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
                ThemedText,
            ));
            if show_settings {
                bar.spawn(button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("Settings"), ThemedText)),
                ))
                .observe(
                    |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                        next.set(LauncherState::Settings);
                    },
                );
            }
            bar.spawn(Node::default()).with_children(|slot| {
                let mut btn = slot.spawn(button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new(label), ThemedText)),
                ));
                match action {
                    NavAction::Close => {
                        btn.observe(|_ev: On<Activate>, mut exit: MessageWriter<AppExit>| {
                            exit.write_default();
                        });
                    }
                    NavAction::Back(target) => {
                        btn.observe(
                            move |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                                next.set(target.clone());
                            },
                        );
                    }
                }
            });
        });
}

/// Close titlebar with a "Settings" entry point next to the close button.
pub(super) fn spawn_settings_close_titlebar(
    parent: &mut ChildSpawnerCommands,
    title_text: impl Into<String>,
) {
    spawn_titlebar(parent, title_text, NavAction::Close, true);
}

pub(super) fn spawn_back_titlebar(
    parent: &mut ChildSpawnerCommands,
    title_text: impl Into<String>,
) {
    spawn_titlebar(
        parent,
        title_text,
        NavAction::Back(LauncherState::Login),
        false,
    );
}

// Hand-rolled per-platform shell-out rather than a crate, to keep the
// launcher's dependency surface at zero for this.
pub(super) fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = cmd {
        tracing::warn!(error = %e, url, "could not open external url");
    }
}

/// Native blocking folder picker shared by the DAT-setup and Settings screens.
/// On Windows the dialog runs on its own thread: winit owns the main thread's
/// COM apartment and message pump, and a blocking IFileDialog there took the
/// whole client down (kuluu-38bj); the join also turns a dialog panic into a
/// cancelled pick instead of a dead launcher. macOS must stay on the main
/// thread (NSOpenPanel requirement); Linux goes through the xdg portal.
pub(super) fn pick_folder_blocking(
    title: String,
    start_dir: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::thread::spawn(move || run_folder_dialog(title, start_dir))
            .join()
            .ok()
            .flatten()
    }
    #[cfg(not(target_os = "windows"))]
    run_folder_dialog(title, start_dir)
}

fn run_folder_dialog(
    title: String,
    start_dir: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = start_dir.filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// `HOME` is a Unix convention; a plain PowerShell launch has only
/// `USERPROFILE`, which left the picker with no start directory on Windows.
pub(super) fn home_dir_fallback() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::Key;

    const NEAR_PX: f32 = 20.0;
    const FAR_PX: f32 = 200.0;

    const IN_COLUMN_DROP_PX: f32 = 200.0;
    const OFF_AXIS_DROP_PX: f32 = 40.0;
    const OFF_AXIS_SIDESTEP_PX: f32 = 150.0;

    #[test]
    fn pick_directional_prefers_the_aligned_candidate_over_a_nearer_offset_one() {
        let origin = Vec2::ZERO;
        let in_column = Vec2::new(0.0, IN_COLUMN_DROP_PX);
        let off_axis = Vec2::new(OFF_AXIS_SIDESTEP_PX, OFF_AXIS_DROP_PX);
        assert!(
            off_axis.length() < in_column.length(),
            "the off-axis candidate must be the nearer one for this to bite"
        );
        let cands = [origin, in_column, off_axis];
        assert_eq!(
            pick_directional(&cands, Some(0), NavDir::Down.as_vec2()),
            Some(1)
        );
    }

    #[test]
    fn pick_directional_wraps_to_the_far_edge() {
        let cands = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, NEAR_PX),
            Vec2::new(0.0, FAR_PX),
        ];
        assert_eq!(
            pick_directional(&cands, Some(2), NavDir::Down.as_vec2()),
            Some(0)
        );
        assert_eq!(
            pick_directional(&cands, Some(0), NavDir::Up.as_vec2()),
            Some(2)
        );
    }

    #[test]
    fn pick_directional_anchors_on_the_centroid_when_nothing_is_focused() {
        let cands = [
            Vec2::new(-FAR_PX, -FAR_PX),
            Vec2::new(FAR_PX, -FAR_PX),
            Vec2::new(-FAR_PX, FAR_PX),
            Vec2::new(FAR_PX, FAR_PX),
        ];
        for dir in [NavDir::Up, NavDir::Down, NavDir::Left, NavDir::Right] {
            assert!(pick_directional(&cands, None, dir.as_vec2()).is_some());
        }
        assert_eq!(pick_directional(&[], None, NavDir::Down.as_vec2()), None);
        assert_eq!(
            pick_directional(&[Vec2::ZERO], Some(0), NavDir::Down.as_vec2()),
            None
        );
    }

    #[test]
    fn resolve_intent_in_navigate_mode_routes_each_pad_message() {
        let mode = LauncherFocusMode::Navigate;
        assert_eq!(
            resolve_intent(&mode, false, &LauncherNav::Move(NavDir::Left)),
            NavOutcome::MoveFocus(NavDir::Left)
        );
        assert_eq!(
            resolve_intent(&mode, false, &LauncherNav::Confirm),
            NavOutcome::Activate
        );
        assert_eq!(
            resolve_intent(&mode, true, &LauncherNav::Confirm),
            NavOutcome::EnterTextEntry
        );
        assert_eq!(
            resolve_intent(&mode, false, &LauncherNav::Cancel),
            NavOutcome::SendEscape
        );
        assert_eq!(
            resolve_intent(&mode, false, &LauncherNav::Page(PageDir::Next)),
            NavOutcome::Page(PageDir::Next)
        );
    }

    #[test]
    fn resolve_intent_in_text_entry_mode_swallows_movement_and_paging() {
        let mode = LauncherFocusMode::TextEntry(Entity::from_raw_u32(1).unwrap());
        assert_eq!(
            resolve_intent(&mode, true, &LauncherNav::Move(NavDir::Down)),
            NavOutcome::Nothing
        );
        assert_eq!(
            resolve_intent(&mode, true, &LauncherNav::Page(PageDir::Prev)),
            NavOutcome::Nothing
        );
        assert_eq!(
            resolve_intent(&mode, true, &LauncherNav::Confirm),
            NavOutcome::CommitTextEntry
        );
        assert_eq!(
            resolve_intent(&mode, true, &LauncherNav::Cancel),
            NavOutcome::ExitTextEntry
        );
    }

    #[test]
    fn paged_scroll_y_advances_one_viewport_less_the_overlap_and_clamps() {
        let viewport = FAR_PX;
        let step = viewport * (1.0 - PAGE_SCROLL_OVERLAP_FRACTION);
        let max = viewport * 3.0;
        assert_eq!(paged_scroll_y(0.0, viewport, PageDir::Next, max), step);
        assert_eq!(paged_scroll_y(max, viewport, PageDir::Next, max), max);
        assert_eq!(paged_scroll_y(0.0, viewport, PageDir::Prev, max), 0.0);
        assert_eq!(paged_scroll_y(step, viewport, PageDir::Prev, max), 0.0);
    }

    #[test]
    fn max_scroll_y_is_zero_when_the_content_fits() {
        assert_eq!(max_scroll_y(NEAR_PX, FAR_PX, 0.0, 1.0), 0.0);
        let overflow = max_scroll_y(FAR_PX * 2.0, FAR_PX, 0.0, 1.0);
        assert_eq!(overflow, FAR_PX);
        assert_eq!(max_scroll_y(FAR_PX * 2.0, FAR_PX, 0.0, 0.5), FAR_PX * 0.5);
    }

    #[test]
    fn hidden_and_unlaid_out_widgets_are_not_focus_candidates() {
        assert!(is_pad_focusable(Vec2::splat(FAR_PX), true));
        assert!(!is_pad_focusable(Vec2::ZERO, true));
        assert!(!is_pad_focusable(Vec2::new(FAR_PX, 0.0), true));
        assert!(!is_pad_focusable(Vec2::splat(FAR_PX), false));
    }

    #[test]
    fn first_move_only_reveals_an_invisible_ring() {
        assert!(move_reveals_only(false, true));
        assert!(!move_reveals_only(true, true));
        assert!(!move_reveals_only(false, false));
        assert!(!move_reveals_only(true, false));
    }

    #[derive(Resource, Default)]
    struct SeenKeys(Vec<KeyboardInput>);

    fn collect_keys(mut events: MessageReader<KeyboardInput>, mut seen: ResMut<SeenKeys>) {
        seen.0.extend(events.read().cloned());
    }

    fn nav_app() -> App {
        let mut app = App::new();
        app.add_message::<LauncherNav>()
            .add_message::<KeyboardInput>()
            .init_resource::<InputFocus>()
            .init_resource::<InputFocusVisible>()
            .init_resource::<LauncherFocusMode>()
            .init_resource::<SeenKeys>()
            .add_systems(Update, (launcher_focus_nav_system, collect_keys).chain());
        app.world_mut().spawn(PrimaryWindow);
        app
    }

    fn focus_on(app: &mut App, entity: Entity) {
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(entity, FocusCause::Navigated);
    }

    #[test]
    fn pad_confirm_synthesizes_a_paired_enter_press_and_release() {
        let mut app = nav_app();
        let widget = app.world_mut().spawn(TabIndex(0)).id();
        focus_on(&mut app, widget);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();

        let seen = &app.world().resource::<SeenKeys>().0;
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|k| k.logical_key == Key::Enter));
        assert_eq!(seen[0].state, ButtonState::Pressed);
        assert_eq!(seen[1].state, ButtonState::Released);
    }

    #[test]
    fn pad_confirm_on_a_text_field_arms_text_entry_then_cancel_disarms() {
        let mut app = nav_app();
        let field = app
            .world_mut()
            .spawn((TabIndex(0), TextField::default()))
            .id();
        focus_on(&mut app, field);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();

        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::TextEntry(field)
        );
        assert!(app.world().get::<TextEntryActive>(field).is_some());
        assert!(app.world().resource::<InputFocusVisible>().0);
        assert!(app.world().resource::<SeenKeys>().0.is_empty());

        app.world_mut().write_message(LauncherNav::Cancel);
        app.update();
        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::Navigate
        );
        assert!(app.world().get::<TextEntryActive>(field).is_none());
        assert!(app.world().resource::<SeenKeys>().0.is_empty());
    }

    #[test]
    fn reconcile_clears_focus_when_the_focused_widget_despawns_and_drops_text_entry() {
        let mut app = App::new();
        app.init_resource::<InputFocus>()
            .init_resource::<LauncherFocusMode>()
            .add_systems(Update, reconcile_focus_system);
        let widget = app.world_mut().spawn(TabIndex(0)).id();
        focus_on(&mut app, widget);
        app.world_mut().entity_mut(widget).despawn();
        app.update();
        assert_eq!(app.world().resource::<InputFocus>().get(), None);
        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::Navigate
        );

        let field = app
            .world_mut()
            .spawn((TabIndex(0), TextField::default(), TextEntryActive))
            .id();
        let other = app.world_mut().spawn(TabIndex(0)).id();
        *app.world_mut().resource_mut::<LauncherFocusMode>() = LauncherFocusMode::TextEntry(field);
        focus_on(&mut app, other);
        app.update();
        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::Navigate
        );
        assert!(app.world().get::<TextEntryActive>(field).is_none());
    }

    #[derive(Resource, Default)]
    struct SeenActivations(Vec<Entity>);

    #[derive(Resource, Default)]
    struct SeenToggles(Vec<(Entity, bool)>);

    fn activation_app() -> App {
        let mut app = nav_app();
        app.init_resource::<SeenActivations>()
            .init_resource::<SeenToggles>()
            .add_observer(|ev: On<Activate>, mut seen: ResMut<SeenActivations>| {
                seen.0.push(ev.entity);
            })
            .add_observer(|ev: On<ValueChange<bool>>, mut seen: ResMut<SeenToggles>| {
                seen.0.push((ev.source, ev.value));
            });
        app
    }

    #[test]
    fn activation_routes_widgets_to_their_own_trigger_and_bare_focus_to_enter() {
        assert_eq!(activation_for(true, false, false), Activation::Button);
        assert_eq!(activation_for(false, true, false), Activation::Toggle);
        assert_eq!(activation_for(false, false, false), Activation::ScreenEnter);
        assert_eq!(activation_for(true, false, true), Activation::None);
        assert_eq!(activation_for(false, true, true), Activation::None);
    }

    #[test]
    fn pad_confirm_on_a_button_fires_activate_without_a_screen_wide_enter() {
        let mut app = activation_app();
        let button = app.world_mut().spawn((TabIndex(0), Button)).id();
        focus_on(&mut app, button);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();

        assert_eq!(app.world().resource::<SeenActivations>().0, vec![button]);
        assert!(
            app.world().resource::<SeenKeys>().0.is_empty(),
            "a screen-wide Enter handler would fire its own default action too"
        );
    }

    #[test]
    fn pad_confirm_on_a_checkbox_toggles_it() {
        let mut app = activation_app();
        let unchecked = app.world_mut().spawn((TabIndex(0), Checkbox)).id();
        focus_on(&mut app, unchecked);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();
        assert_eq!(
            app.world().resource::<SeenToggles>().0,
            vec![(unchecked, true)]
        );

        let checked = app.world_mut().spawn((TabIndex(0), Checkbox, Checked)).id();
        focus_on(&mut app, checked);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();
        assert_eq!(
            app.world().resource::<SeenToggles>().0.last(),
            Some(&(checked, false))
        );
        assert!(app.world().resource::<SeenKeys>().0.is_empty());
    }

    #[test]
    fn pad_confirm_on_a_disabled_widget_does_nothing() {
        let mut app = activation_app();
        let button = app
            .world_mut()
            .spawn((TabIndex(0), Button, InteractionDisabled))
            .id();
        focus_on(&mut app, button);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();
        assert!(app.world().resource::<SeenActivations>().0.is_empty());
        assert!(app.world().resource::<SeenKeys>().0.is_empty());
    }

    #[test]
    fn pad_cancel_still_reaches_the_screen_as_escape() {
        let mut app = activation_app();
        let button = app.world_mut().spawn((TabIndex(0), Button)).id();
        focus_on(&mut app, button);
        app.world_mut().write_message(LauncherNav::Cancel);
        app.update();

        let seen = &app.world().resource::<SeenKeys>().0;
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|k| k.key_code == KeyCode::Escape));
        assert!(app.world().resource::<SeenActivations>().0.is_empty());
    }

    const VIEWPORT_PX: f32 = 300.0;
    const CONTENT_PAGES: f32 = 4.0;

    fn scroll_node(content_pages: f32) -> ComputedNode {
        ComputedNode {
            size: Vec2::new(FAR_PX, VIEWPORT_PX),
            content_size: Vec2::new(FAR_PX, VIEWPORT_PX * content_pages),
            inverse_scale_factor: 1.0,
            ..default()
        }
    }

    fn scroll_y(app: &App, region: Entity) -> f32 {
        app.world().get::<ScrollPosition>(region).unwrap().0.y
    }

    #[test]
    fn shoulder_paging_moves_the_focused_regions_scroll_position() {
        let mut app = App::new();
        app.add_message::<LauncherNav>()
            .init_resource::<InputFocus>()
            .init_resource::<LauncherFocusMode>()
            .add_systems(Update, launcher_page_scroll_system);
        let region = app
            .world_mut()
            .spawn((
                ScrollRegion,
                ScrollPosition::default(),
                scroll_node(CONTENT_PAGES),
            ))
            .id();
        let row = app.world_mut().spawn((TabIndex(0), ChildOf(region))).id();
        focus_on(&mut app, row);

        app.world_mut()
            .write_message(LauncherNav::Page(PageDir::Next));
        app.update();
        let paged = scroll_y(&app, region);
        assert_eq!(
            paged,
            paged_scroll_y(
                0.0,
                VIEWPORT_PX,
                PageDir::Next,
                max_scroll_y(VIEWPORT_PX * CONTENT_PAGES, VIEWPORT_PX, 0.0, 1.0),
            )
        );
        assert!(paged > 0.0);

        app.world_mut()
            .write_message(LauncherNav::Page(PageDir::Prev));
        app.update();
        assert_eq!(scroll_y(&app, region), 0.0);

        *app.world_mut().resource_mut::<LauncherFocusMode>() = LauncherFocusMode::TextEntry(row);
        app.world_mut()
            .write_message(LauncherNav::Page(PageDir::Next));
        app.update();
        assert_eq!(
            scroll_y(&app, region),
            0.0,
            "paging is inert while a field is armed for typing"
        );
    }

    #[test]
    fn default_focus_lands_once_per_spawned_screen_instance() {
        let mut app = App::new();
        app.init_resource::<InputFocus>()
            .add_systems(Update, focus_default_target_system);
        let target = app
            .world_mut()
            .spawn((TabIndex(0), DefaultFocusTarget))
            .id();
        app.update();
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(target));

        let elsewhere = app.world_mut().spawn(TabIndex(0)).id();
        focus_on(&mut app, elsewhere);
        app.update();
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(elsewhere),
            "the default target must not steal focus back mid-screen"
        );

        app.world_mut().entity_mut(target).despawn();
        let rebuilt = app
            .world_mut()
            .spawn((TabIndex(0), DefaultFocusTarget))
            .id();
        app.update();
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(rebuilt));
    }

    #[test]
    fn a_pending_screen_change_drops_the_key_that_caused_it() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .init_resource::<NextState<LauncherState>>()
            .init_resource::<SeenKeys>()
            .add_systems(
                Update,
                (drop_keys_leaving_screen_system, collect_keys).chain(),
            );
        let window = app.world_mut().spawn(PrimaryWindow).id();

        for event in synth_key(window, KeyCode::Escape).unwrap() {
            app.world_mut().write_message(event);
        }
        app.update();
        assert_eq!(
            app.world().resource::<SeenKeys>().0.len(),
            2,
            "with no screen change pending the key reaches the screen"
        );

        app.world_mut().resource_mut::<SeenKeys>().0.clear();
        for event in synth_key(window, KeyCode::Escape).unwrap() {
            app.world_mut().write_message(event);
        }
        app.world_mut()
            .resource_mut::<NextState<LauncherState>>()
            .set(LauncherState::ServerSelect);
        app.update();
        let seen = &app.world().resource::<SeenKeys>().0;
        assert!(
            seen.iter().all(|k| k.state == ButtonState::Released),
            "the screen being left must not hand its Escape press to the next one"
        );
        assert_eq!(
            seen.len(),
            1,
            "the release must survive or ButtonInput keeps reporting the key held"
        );
    }

    #[derive(Resource, Default)]
    struct SeenSubmits(Vec<Entity>);

    fn submit_app() -> App {
        let mut app = nav_app();
        app.init_resource::<SeenSubmits>().add_observer(
            |ev: On<TextFieldSubmitted>, mut seen: ResMut<SeenSubmits>| {
                seen.0.push(ev.entity);
            },
        );
        app
    }

    fn arm(app: &mut App, field: Entity) {
        focus_on(app, field);
        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();
        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::TextEntry(field)
        );
    }

    #[test]
    fn committing_a_field_submits_it_without_a_screen_wide_enter() {
        let mut app = submit_app();
        let field = app
            .world_mut()
            .spawn((
                TabIndex(0),
                TextField {
                    submit_on_enter: true,
                    ..default()
                },
            ))
            .id();
        arm(&mut app, field);

        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();
        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::Navigate
        );
        assert!(app.world().get::<TextEntryActive>(field).is_none());
        assert_eq!(app.world().resource::<SeenSubmits>().0, vec![field]);
        assert!(
            app.world().resource::<SeenKeys>().0.is_empty(),
            "a screen-wide Enter handler would fire its own default action too"
        );
    }

    #[test]
    fn committing_a_field_that_does_not_submit_on_enter_only_disarms_it() {
        let mut app = submit_app();
        let field = app
            .world_mut()
            .spawn((TabIndex(0), TextField::default()))
            .id();
        arm(&mut app, field);

        app.world_mut().write_message(LauncherNav::Confirm);
        app.update();
        assert_eq!(
            *app.world().resource::<LauncherFocusMode>(),
            LauncherFocusMode::Navigate
        );
        assert!(app.world().resource::<SeenSubmits>().0.is_empty());
        assert!(app.world().resource::<SeenKeys>().0.is_empty());
    }
}
