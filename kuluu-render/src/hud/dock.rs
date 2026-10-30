//! Docked menu windows: a window whose registry flags carry a dock slot keeps its x and takes its y
//! from the slot's anchor window
//! (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};

use crate::hud::unity_fp::UT_POINT_WINDOW;

/// The four dock slots and the registry flag bit that selects each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockSlot {
    /// Chat log window `logwindo` (`logwin2` while it is open).
    Log,
    /// Party window `partywin`.
    Party,
    /// Target window `targetwi`, falling back to the party window.
    Target,
    /// Event question window `query`.
    Question,
}

/// Registry flag 0x1: the window is display-only and never takes the keys.
pub const REGISTRY_FLAG_DISPLAY_ONLY: u32 = 0x1;

pub const DOCK_FLAG_LOG: u32 = 0x1000;
pub const DOCK_FLAG_PARTY: u32 = 0x2000;
pub const DOCK_FLAG_TARGET: u32 = 0x4000;
pub const DOCK_FLAG_QUESTION: u32 = 1 << 28;

/// The docked window's bottom edge sits this far above a shown anchor's top edge.
pub const DOCK_GAP: f32 = 2.0;
/// A docked window never sits above this y.
pub const DOCK_MIN_TOP: f32 = 16.0;

/// The target window in the party slot.
pub const TARGET_WINDOW: &str = "targetwi";
/// The map list in the log slot.
pub const MAP_LIST_WINDOW: &str = "maplist";

impl DockSlot {
    /// The order the per-frame pass visits the slots, so a chain of anchors settles in one frame.
    pub const PASS_ORDER: [DockSlot; 4] = [Self::Log, Self::Party, Self::Target, Self::Question];

    pub const fn flag(self) -> u32 {
        match self {
            Self::Log => DOCK_FLAG_LOG,
            Self::Party => DOCK_FLAG_PARTY,
            Self::Target => DOCK_FLAG_TARGET,
            Self::Question => DOCK_FLAG_QUESTION,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Log => 0,
            Self::Party => 1,
            Self::Target => 2,
            Self::Question => 3,
        }
    }

    /// The slot a registry flags word selects; no registry entry carries more than one.
    pub fn from_registry_flags(flags: u32) -> Option<Self> {
        Self::PASS_ORDER
            .into_iter()
            .find(|slot| flags & slot.flag() != 0)
    }

    /// The slot whose anchor stands in when this slot's own anchor did not place the window.
    pub fn fallback(self) -> Option<Self> {
        match self {
            Self::Target => Some(Self::Party),
            _ => None,
        }
    }
}

/// The dock slot of every registry window that carries one.
pub fn registry_dock_slot(window: &str) -> Option<DockSlot> {
    use crate::hud::menu::{
        ABILITY_LIST_WINDOW, EMOTE_LIST_WINDOW, JOBCHANG_WINDOW, MAGIC_LIST_WINDOW,
        MOUNT_LIST_WINDOW, MYROOM_WINDOW, ROOMLIST_DAT_WINDOW,
    };
    use crate::hud::unity_fp::{AREALIST_WINDOW, MERITYN_WINDOW};
    use crate::input_mode::{ABISELEC_WINDOW, CHATCTRL_WINDOW};
    use crate::myroom_arrange::{COMYN_WINDOW, RMLO1_WINDOW, RMLO2_WINDOW, RMLO4_WINDOW};
    const LOG: &[&str] = &[
        "query",
        "gmtell",
        "playermo",
        "mp_pmode",
        CHATCTRL_WINDOW,
        "magselec",
        ABISELEC_WINDOW,
        "mp_abisc",
        MYROOM_WINDOW,
        "rmpost",
        "mogdoor",
        "mogext",
        "mogpost",
        ROOMLIST_DAT_WINDOW,
        "storage",
        "storage2",
        JOBCHANG_WINDOW,
        "jobcsel",
        "jobcselu",
        AREALIST_WINDOW,
        "shopmain",
        "guildsho",
        MERITYN_WINDOW,
        MAP_LIST_WINDOW,
        COMYN_WINDOW,
        "resyn",
        "resynus",
        "resynde",
        "resynfr",
        MAGIC_LIST_WINDOW,
        ABILITY_LIST_WINDOW,
        EMOTE_LIST_WINDOW,
        MOUNT_LIST_WINDOW,
        RMLO1_WINDOW,
        RMLO2_WINDOW,
        RMLO4_WINDOW,
        "plnt1",
        "plnt2",
        "plnt3",
        "plnt4",
        "plnt5",
        "plnt6",
    ];
    const PARTY: &[&str] = &[
        TARGET_WINDOW,
        UT_POINT_WINDOW,
        "conq3pts",
        "imperial",
        "allied",
        "colopoin",
    ];
    const TARGET: &[&str] = &["subwindo"];
    if LOG.contains(&window) {
        Some(DockSlot::Log)
    } else if PARTY.contains(&window) {
        Some(DockSlot::Party)
    } else if TARGET.contains(&window) {
        Some(DockSlot::Target)
    } else {
        None
    }
}

/// An anchor window's edges in menu coordinates (y grows downward).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockAnchor {
    pub top: f32,
    pub bottom: f32,
    /// Its frame shows or is fading in; one fading out counts as hidden.
    pub frame_shown: bool,
    /// It draws without its frame, so its bottom edge is the one the per-frame pass watches.
    pub frameless: bool,
}

impl DockAnchor {
    pub fn watched_edge(self) -> f32 {
        if self.frameless {
            self.bottom
        } else {
            self.top
        }
    }

    /// The line a window docked against this anchor puts its bottom edge on: 2 above a shown
    /// frame's top, level with a hidden frame's bottom.
    pub fn docked_bottom(self) -> f32 {
        if self.frame_shown {
            self.top - DOCK_GAP
        } else {
            self.bottom
        }
    }
}

/// A kuluu window drawn as a dock anchor: its laid-out edges, while it has a size.
pub fn node_anchor(computed: &ComputedNode, transform: &UiGlobalTransform) -> Option<DockAnchor> {
    let scale = computed.inverse_scale_factor();
    let height = computed.size().y * scale;
    if height <= 0.0 {
        return None;
    }
    let center = transform.translation.y * scale;
    Some(DockAnchor {
        top: center - height / 2.0,
        bottom: center + height / 2.0,
        frame_shown: true,
        frameless: false,
    })
}

/// Where the law put a docked window: the line its bottom edge sits on, and the window's own floor
/// when it sets one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockPlacement {
    pub bottom: f32,
    pub lower_bound: Option<f32>,
}

impl DockPlacement {
    /// The window's top for its height, after its own floor and the dock floor.
    pub fn top(self, height: f32) -> f32 {
        let top = self.bottom - height;
        let top = self.lower_bound.map_or(top, |bound| top.max(bound));
        top.max(DOCK_MIN_TOP)
    }
}

/// The docked window's new top for a window `height` tall, or `None` when there is no anchor to
/// dock against and the window keeps whatever position it has. `lower_bound` is the window's own
/// floor, when it sets one.
pub fn docked_top(
    anchor: Option<DockAnchor>,
    height: f32,
    lower_bound: Option<f32>,
) -> Option<f32> {
    let anchor = anchor?;
    Some(
        DockPlacement {
            bottom: anchor.docked_bottom(),
            lower_bound,
        }
        .top(height),
    )
}

/// Puts a docked window's node on its placement. With the surface height known the node hangs from
/// its bottom edge, so a window whose drawn height changes stays on the docked line; when the
/// floors push the top down, or the surface is unknown, the top is set from `height`.
pub fn place_docked(
    node: &mut Node,
    placement: DockPlacement,
    height: f32,
    surface_height: Option<f32>,
) {
    let top = placement.top(height);
    node.margin.top = Val::ZERO;
    node.margin.bottom = Val::ZERO;
    match surface_height {
        Some(surface) if top <= placement.bottom - height => {
            node.top = Val::Auto;
            node.bottom = Val::Px(surface - placement.bottom);
        }
        _ => {
            node.top = Val::Px(top);
            node.bottom = Val::Auto;
        }
    }
}

/// An open window that carries a dock slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockedWindow {
    pub window: &'static str,
    pub slot: DockSlot,
    /// A closing window is not placed.
    pub closing: bool,
    pub lower_bound: Option<f32>,
}

impl DockedWindow {
    /// An open registry window, when it carries a dock slot.
    pub fn open(window: &'static str) -> Option<Self> {
        Some(Self {
            window,
            slot: registry_dock_slot(window)?,
            closing: false,
            lower_bound: None,
        })
    }
}

/// The anchor windows open this frame, by slot; `None` is an anchor that is not open.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DockAnchors {
    pub log: Option<DockAnchor>,
    pub party: Option<DockAnchor>,
    pub target: Option<DockAnchor>,
    pub question: Option<DockAnchor>,
}

impl DockAnchors {
    pub fn of(&self, slot: DockSlot) -> Option<DockAnchor> {
        match slot {
            DockSlot::Log => self.log,
            DockSlot::Party => self.party,
            DockSlot::Target => self.target,
            DockSlot::Question => self.question,
        }
    }
}

/// The dock law's state: each slot's remembered anchor edge, the windows seen open, and where each
/// docked window was put.
#[derive(Resource, Debug, Default)]
pub struct DockBoard {
    watched: [Option<f32>; DockSlot::PASS_ORDER.len()],
    open: BTreeSet<&'static str>,
    placed: BTreeMap<&'static str, DockPlacement>,
    /// The height of the surface the menu windows are laid out on.
    pub surface_height: Option<f32>,
}

impl DockBoard {
    pub fn placement(&self, window: &str) -> Option<DockPlacement> {
        self.placed.get(window).copied()
    }

    /// One frame of the law. A window that became active is placed once against its slot's
    /// anchor; then, in the in-zone play state only, every slot whose anchor's watched edge moved
    /// or whose anchor is not open places its windows again, slot by slot in pass order. While
    /// docking is suspended nothing is placed. A window placed against nothing keeps its position,
    /// and so does one whose anchor closes.
    pub fn step(
        &mut self,
        windows: &[DockedWindow],
        anchors: &DockAnchors,
        in_zone: bool,
        suspended: bool,
    ) {
        self.placed
            .retain(|name, _| windows.iter().any(|window| window.window == *name));
        let activated: Vec<DockedWindow> = windows
            .iter()
            .filter(|window| !self.open.contains(window.window) && !window.closing)
            .copied()
            .collect();
        self.open = windows.iter().map(|window| window.window).collect();
        if suspended {
            return;
        }
        for window in &activated {
            self.place(window, anchors);
        }
        if !in_zone {
            return;
        }
        for slot in DockSlot::PASS_ORDER {
            let anchor = anchors.of(slot);
            let edge = anchor.map(DockAnchor::watched_edge);
            if anchor.is_none() || edge != self.watched[slot.index()] {
                for window in windows
                    .iter()
                    .filter(|window| window.slot == slot && !window.closing)
                {
                    self.place(window, anchors);
                }
            }
            self.watched[slot.index()] = edge;
        }
    }

    /// A request from a window's own logic: the slot's remembered edge is forgotten, so the next
    /// pass places its windows again, and `window`, when given, is placed now.
    pub fn request(
        &mut self,
        slot: DockSlot,
        window: Option<&DockedWindow>,
        anchors: &DockAnchors,
        suspended: bool,
    ) {
        if suspended {
            return;
        }
        self.watched[slot.index()] = None;
        if let Some(window) = window {
            self.place(window, anchors);
        }
    }

    fn place(&mut self, window: &DockedWindow, anchors: &DockAnchors) {
        let anchor = anchors.of(window.slot).or_else(|| {
            window
                .slot
                .fallback()
                .and_then(|fallback| anchors.of(fallback))
        });
        if let Some(anchor) = anchor {
            self.placed.insert(
                window.window,
                DockPlacement {
                    bottom: anchor.docked_bottom(),
                    lower_bound: window.lower_bound,
                },
            );
        }
    }
}

/// The windows open this frame that carry a dock slot: the menu's own windows, `ut_point` (which
/// opens and closes with `ut_menu`), the map list and the target window.
pub fn open_docked_windows(
    mode: &crate::input_mode::InputMode,
    map_list_open: bool,
    target_open: bool,
) -> Vec<DockedWindow> {
    let mut names: Vec<&'static str> = Vec::new();
    if let Some(stack) = crate::hud::menu::live_menu(mode) {
        for level in &stack.levels {
            names.extend(crate::hud::menu::registry_window(level.kind));
            if level.kind == crate::input_mode::MenuKind::UnityMenu {
                names.push(UT_POINT_WINDOW);
            }
        }
    }
    if map_list_open {
        names.push(MAP_LIST_WINDOW);
    }
    if target_open {
        names.push(TARGET_WINDOW);
    }
    let mut windows: Vec<DockedWindow> = Vec::new();
    for window in names.into_iter().filter_map(DockedWindow::open) {
        if !windows.iter().any(|open| open.window == window.window) {
            windows.push(window);
        }
    }
    windows
}

fn open_anchor(
    node: &Node,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
) -> Option<DockAnchor> {
    (node.display != Display::None)
        .then(|| node_anchor(computed, transform))
        .flatten()
}

/// Runs the dock law each frame with kuluu's own windows as the anchors: the chat log panel for
/// the log slot, party window A for the party slot and the target panel for the target slot.
#[allow(clippy::type_complexity)]
pub fn dock_windows_system(
    mode: Res<crate::input_mode::InputMode>,
    scene: Res<crate::snapshot::SceneState>,
    #[cfg(not(target_arch = "wasm32"))] map: Option<Res<crate::hud::map_screen::MapScreenState>>,
    primary: Query<&Window, With<bevy::window::PrimaryWindow>>,
    ui_scale: Option<Res<UiScale>>,
    chat: Query<
        (&Node, &ComputedNode, &UiGlobalTransform),
        With<crate::hud::chat_panel::ChatPanelGroup>,
    >,
    party: Query<(
        &crate::hud::party_frame::PartyFrameRoot,
        &Node,
        &ComputedNode,
        &UiGlobalTransform,
    )>,
    target: Query<
        (&Node, &ComputedNode, &UiGlobalTransform),
        With<crate::hud::target_panel::TargetPanel>,
    >,
    mut board: ResMut<DockBoard>,
    mut target_was_open: Local<bool>,
) {
    #[cfg(target_arch = "wasm32")]
    let map_list_open = false;
    #[cfg(not(target_arch = "wasm32"))]
    let map_list_open = crate::hud::map_screen::map_open(&mode)
        && map
            .as_ref()
            .is_some_and(|map| map.mode == crate::hud::map_screen::MapSubMode::ChangeMap);
    let anchors = DockAnchors {
        log: chat.iter().find_map(|(n, c, t)| open_anchor(n, c, t)),
        party: party
            .iter()
            .filter(|(root, ..)| root.party_no == 0)
            .find_map(|(_, n, c, t)| open_anchor(n, c, t)),
        target: target.iter().find_map(|(n, c, t)| open_anchor(n, c, t)),
        question: None,
    };
    let target_open = target
        .iter()
        .any(|(node, ..)| node.display != Display::None);
    board.surface_height = primary
        .single()
        .ok()
        .map(|window| window.height() / ui_scale.as_ref().map_or(1.0, |scale| scale.0));
    // The record does not tie the suspension to a user-visible condition, so kuluu does not
    // suspend.
    let suspended = false;
    if target_open && !*target_was_open {
        board.request(DockSlot::Target, None, &anchors, suspended);
    }
    *target_was_open = target_open;
    let windows = open_docked_windows(&mode, map_list_open, target_open);
    board.step(
        &windows,
        &anchors,
        scene.snapshot.zone_id.is_some(),
        suspended,
    );
}

/// The logical height kuluu drew a window node at last frame.
pub fn drawn_height(computed: &ComputedNode) -> f32 {
    computed.size().y * computed.inverse_scale_factor()
}

/// The target panel docks as `targetwi` in the party slot: its record frame gives x and width, the
/// law gives y; with neither it keeps its place in kuluu's panel column.
pub fn place_target_window(
    board: Res<DockBoard>,
    config_rows: Res<crate::hud::menu::ConfigDatRows>,
    mut panels: Query<
        (
            &mut Node,
            &ComputedNode,
            &mut crate::hud::panel_column::DockHeld,
        ),
        With<crate::hud::target_panel::TargetPanel>,
    >,
) {
    for (mut node, computed, mut held) in &mut panels {
        let frame = config_rows.record_frame(TARGET_WINDOW);
        let placement = board.placement(TARGET_WINDOW);
        let hold = node.display != Display::None && (frame.is_some() || placement.is_some());
        if !hold {
            if held.0 {
                held.0 = false;
                node.top = Val::Auto;
            }
            continue;
        }
        if !held.0 {
            held.0 = true;
        }
        let mut want = node.clone();
        if let Some(frame) = frame {
            crate::hud::menu::place_at_record_frame(&mut want, &frame);
        }
        if let Some(placement) = placement {
            place_docked(
                &mut want,
                placement,
                drawn_height(computed),
                board.surface_height,
            );
        }
        node.set_if_neq(want);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::menu::{
        EMOTE_LIST_WINDOW, JOBCHANG_WINDOW, MAGIC_LIST_WINDOW, MYROOM_WINDOW,
        RETAIL_CONFIG_HUB_WINDOW,
    };
    use crate::myroom_arrange::COMYN_WINDOW;

    const SHOWN: DockAnchor = DockAnchor {
        top: 300.0,
        bottom: 420.0,
        frame_shown: true,
        frameless: false,
    };

    #[test]
    fn shown_anchor_puts_the_bottom_edge_two_above_its_top() {
        assert_eq!(docked_top(Some(SHOWN), 30.0, None), Some(268.0));
    }

    #[test]
    fn hidden_anchor_levels_the_bottom_edges() {
        let hidden = DockAnchor {
            frame_shown: false,
            ..SHOWN
        };
        assert_eq!(docked_top(Some(hidden), 30.0, None), Some(390.0));
    }

    #[test]
    fn lower_bound_then_floor_clamp_the_top() {
        assert_eq!(docked_top(Some(SHOWN), 30.0, Some(280.0)), Some(280.0));
        let high = DockAnchor { top: 20.0, ..SHOWN };
        assert_eq!(docked_top(Some(high), 30.0, None), Some(DOCK_MIN_TOP));
    }

    #[test]
    fn no_anchor_keeps_the_window_where_it_is() {
        assert_eq!(docked_top(None, 30.0, None), None);
    }

    /// `ut_point`'s registry flags (display-only plus the party slot) dock it to the party window,
    /// and the target slot falls back to the party window.
    #[test]
    fn registry_flags_select_one_slot() {
        const UT_POINT_FLAGS: u32 = DOCK_FLAG_PARTY | REGISTRY_FLAG_DISPLAY_ONLY;
        assert_eq!(
            DockSlot::from_registry_flags(UT_POINT_FLAGS),
            Some(DockSlot::Party)
        );
        assert_eq!(
            DockSlot::from_registry_flags(DOCK_FLAG_QUESTION),
            Some(DockSlot::Question)
        );
        assert_eq!(DockSlot::from_registry_flags(0x2), None);
        assert_eq!(DockSlot::Target.fallback(), Some(DockSlot::Party));
        assert_eq!(DockSlot::Party.fallback(), None);
    }

    #[test]
    fn the_registry_table_names_each_windows_slot() {
        assert_eq!(registry_dock_slot(COMYN_WINDOW), Some(DockSlot::Log));
        assert_eq!(registry_dock_slot(TARGET_WINDOW), Some(DockSlot::Party));
        assert_eq!(registry_dock_slot("subwindo"), Some(DockSlot::Target));
        assert_eq!(registry_dock_slot(RETAIL_CONFIG_HUB_WINDOW), None);
    }

    fn docked(window: &'static str) -> DockedWindow {
        DockedWindow::open(window).expect("a docked window")
    }

    fn with_log(anchor: DockAnchor) -> DockAnchors {
        DockAnchors {
            log: Some(anchor),
            ..Default::default()
        }
    }

    const HEIGHT: f32 = 40.0;

    fn top_of(board: &DockBoard, window: &str) -> Option<f32> {
        board
            .placement(window)
            .map(|placement| placement.top(HEIGHT))
    }

    /// Opening places a window once; the anchor's top edge moving places it again; a fade alone
    /// does not.
    #[test]
    fn a_window_docks_when_it_opens_and_follows_its_anchors_top_edge() {
        let mut board = DockBoard::default();
        let comyn = [docked(COMYN_WINDOW)];
        board.step(&comyn, &with_log(SHOWN), true, false);
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(SHOWN), HEIGHT, None)
        );

        let faded = DockAnchor {
            frame_shown: false,
            ..SHOWN
        };
        board.step(&comyn, &with_log(faded), true, false);
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(SHOWN), HEIGHT, None),
            "a frame fade alone does not move it"
        );

        let moved = DockAnchor {
            top: SHOWN.top - HEIGHT,
            ..SHOWN
        };
        board.step(&comyn, &with_log(moved), true, false);
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(moved), HEIGHT, None)
        );
    }

    /// A window opened while the anchor's frame is faded docks level with its bottom and stays
    /// there after the frame fades back in.
    #[test]
    fn a_window_opened_over_a_faded_anchor_keeps_the_bottom_line() {
        let faded = DockAnchor {
            frame_shown: false,
            ..SHOWN
        };
        let mut board = DockBoard::default();
        board.step(&[], &with_log(faded), true, false);
        board.step(&[docked(MAGIC_LIST_WINDOW)], &with_log(faded), true, false);
        assert_eq!(
            top_of(&board, MAGIC_LIST_WINDOW),
            Some(SHOWN.bottom - HEIGHT)
        );
        board.step(&[docked(MAGIC_LIST_WINDOW)], &with_log(SHOWN), true, false);
        assert_eq!(
            top_of(&board, MAGIC_LIST_WINDOW),
            Some(SHOWN.bottom - HEIGHT)
        );
    }

    /// An anchor that closes leaves its windows where they are; a window that opens with no
    /// anchor is not placed and keeps its record frame.
    #[test]
    fn a_closed_or_missing_anchor_places_nothing() {
        let mut board = DockBoard::default();
        board.step(&[docked(EMOTE_LIST_WINDOW)], &with_log(SHOWN), true, false);
        let docked_at = top_of(&board, EMOTE_LIST_WINDOW);
        board.step(
            &[docked(EMOTE_LIST_WINDOW)],
            &DockAnchors::default(),
            true,
            false,
        );
        assert_eq!(top_of(&board, EMOTE_LIST_WINDOW), docked_at);

        board.step(
            &[docked(EMOTE_LIST_WINDOW), docked(COMYN_WINDOW)],
            &DockAnchors::default(),
            true,
            false,
        );
        assert_eq!(top_of(&board, COMYN_WINDOW), None);
        assert_eq!(top_of(&board, EMOTE_LIST_WINDOW), docked_at);
    }

    /// Windows in one slot share the anchor's line; a tall one stops at the floor.
    #[test]
    fn a_slot_has_no_stacking_and_the_floor_holds() {
        let mut board = DockBoard::default();
        board.step(
            &[docked(MYROOM_WINDOW), docked(JOBCHANG_WINDOW)],
            &with_log(SHOWN),
            true,
            false,
        );
        assert_eq!(
            board.placement(MYROOM_WINDOW),
            board.placement(JOBCHANG_WINDOW)
        );
        let placement = board.placement(MYROOM_WINDOW).unwrap();
        assert_eq!(placement.top(SHOWN.top), DOCK_MIN_TOP);
    }

    /// Party-slot windows follow the party window; target-slot windows fall back to it while no
    /// target window is open, every frame.
    #[test]
    fn the_target_slot_falls_back_to_the_party_window() {
        let party = DockAnchor {
            top: 250.0,
            ..SHOWN
        };
        let anchors = DockAnchors {
            party: Some(party),
            ..Default::default()
        };
        let mut board = DockBoard::default();
        board.step(
            &[docked(TARGET_WINDOW), docked("subwindo")],
            &anchors,
            true,
            false,
        );
        let at_party = docked_top(Some(party), HEIGHT, None);
        assert_eq!(top_of(&board, TARGET_WINDOW), at_party);
        assert_eq!(top_of(&board, "subwindo"), at_party);

        let target = DockAnchor {
            top: 180.0,
            ..SHOWN
        };
        let with_target = DockAnchors {
            target: Some(target),
            ..anchors
        };
        board.step(
            &[docked(TARGET_WINDOW), docked("subwindo")],
            &with_target,
            true,
            false,
        );
        assert_eq!(
            top_of(&board, "subwindo"),
            docked_top(Some(target), HEIGHT, None)
        );
    }

    /// The anchor's watched edge is its bottom when it draws without its frame.
    #[test]
    fn a_frameless_anchor_is_watched_by_its_bottom_edge() {
        let frameless = DockAnchor {
            frameless: true,
            ..SHOWN
        };
        let mut board = DockBoard::default();
        board.step(&[docked(COMYN_WINDOW)], &with_log(frameless), true, false);
        let first = top_of(&board, COMYN_WINDOW);
        let top_moved = DockAnchor {
            top: SHOWN.top - HEIGHT,
            ..frameless
        };
        board.step(&[docked(COMYN_WINDOW)], &with_log(top_moved), true, false);
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            first,
            "the top edge is not watched"
        );
        let bottom_moved = DockAnchor {
            bottom: SHOWN.bottom + HEIGHT,
            ..top_moved
        };
        board.step(
            &[docked(COMYN_WINDOW)],
            &with_log(bottom_moved),
            true,
            false,
        );
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(bottom_moved), HEIGHT, None)
        );
    }

    /// Outside the in-zone play state only activation and requests place; suspended, nothing
    /// does.
    #[test]
    fn the_frame_pass_needs_the_zone_and_suspension_stops_everything() {
        let moved = DockAnchor {
            top: SHOWN.top - HEIGHT,
            ..SHOWN
        };
        let mut board = DockBoard::default();
        board.step(&[docked(COMYN_WINDOW)], &with_log(SHOWN), false, false);
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(SHOWN), HEIGHT, None)
        );
        board.step(&[docked(COMYN_WINDOW)], &with_log(moved), false, false);
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(SHOWN), HEIGHT, None)
        );

        let mut suspended = DockBoard::default();
        suspended.step(&[docked(COMYN_WINDOW)], &with_log(SHOWN), true, true);
        assert_eq!(top_of(&suspended, COMYN_WINDOW), None);
        suspended.request(
            DockSlot::Log,
            Some(&docked(COMYN_WINDOW)),
            &with_log(SHOWN),
            true,
        );
        assert_eq!(top_of(&suspended, COMYN_WINDOW), None);
    }

    /// A request places its window at once and makes the next pass place the slot again.
    #[test]
    fn a_request_forgets_the_slot_edge_and_places_now() {
        let mut board = DockBoard::default();
        board.step(&[docked(COMYN_WINDOW)], &with_log(SHOWN), true, false);
        board.step(
            &[docked(COMYN_WINDOW)],
            &DockAnchors::default(),
            false,
            false,
        );
        let moved = DockAnchor {
            top: SHOWN.top - HEIGHT,
            ..SHOWN
        };
        board.request(
            DockSlot::Log,
            Some(&docked(COMYN_WINDOW)),
            &with_log(moved),
            false,
        );
        assert_eq!(
            top_of(&board, COMYN_WINDOW),
            docked_top(Some(moved), HEIGHT, None)
        );
    }

    /// A closing window is not placed, and a window that closes and opens again docks again.
    #[test]
    fn closing_windows_are_left_alone_and_reopening_docks_again() {
        let mut board = DockBoard::default();
        let closing = DockedWindow {
            closing: true,
            ..docked(COMYN_WINDOW)
        };
        board.step(&[closing], &with_log(SHOWN), true, false);
        assert_eq!(top_of(&board, COMYN_WINDOW), None);
        board.step(&[], &with_log(SHOWN), true, false);
        board.step(&[docked(COMYN_WINDOW)], &with_log(SHOWN), true, false);
        assert!(top_of(&board, COMYN_WINDOW).is_some());
    }

    /// With the surface height the node hangs from its docked bottom edge; a window too tall for
    /// the room above sits on the floor instead.
    #[test]
    fn placed_nodes_hang_from_the_docked_line_unless_the_floor_holds_them() {
        const SURFACE: f32 = 480.0;
        let placement = DockPlacement {
            bottom: SHOWN.top - DOCK_GAP,
            lower_bound: None,
        };
        let mut node = Node::default();
        place_docked(&mut node, placement, HEIGHT, Some(SURFACE));
        assert_eq!(
            (node.top, node.bottom),
            (Val::Auto, Val::Px(SURFACE - placement.bottom))
        );
        place_docked(&mut node, placement, SHOWN.top, Some(SURFACE));
        assert_eq!((node.top, node.bottom), (Val::Px(DOCK_MIN_TOP), Val::Auto));
        place_docked(&mut node, placement, HEIGHT, None);
        assert_eq!(
            (node.top, node.bottom),
            (Val::Px(placement.bottom - HEIGHT), Val::Auto)
        );
    }

    /// The windows the law sees open: the menu's docked windows, `ut_point` with the Unity menu,
    /// the map list and the target window.
    #[test]
    fn the_open_docked_windows_come_from_the_menu_the_map_list_and_the_target_window() {
        use crate::input_mode::{InputMode, MenuKind, MenuStack};
        let names = |mode: &InputMode, map_list: bool, target: bool| -> Vec<&'static str> {
            open_docked_windows(mode, map_list, target)
                .into_iter()
                .map(|window| window.window)
                .collect()
        };
        let mut emotes = MenuStack::root();
        emotes.push(MenuKind::EmoteList);
        assert_eq!(
            names(&InputMode::Menu(emotes), false, true),
            [EMOTE_LIST_WINDOW, TARGET_WINDOW]
        );
        let mut unity = MenuStack::root();
        unity.push(MenuKind::UnityMenu);
        assert_eq!(
            names(&InputMode::Menu(unity), false, false),
            [UT_POINT_WINDOW]
        );
        assert_eq!(names(&InputMode::default(), true, false), [MAP_LIST_WINDOW]);
    }

    /// The target panel docks as `targetwi`: record x and width, bottom edge 2 above party window
    /// A, out of kuluu's panel column while it holds. It follows the party window's top edge, stays
    /// put when the party window goes, and returns to the column once it hides.
    #[test]
    fn the_target_window_docks_above_the_party_window() {
        use crate::hud::panel_column::{layout_panel_column_system, ColumnPanel, DockHeld};
        use crate::hud::party_frame::PartyFrameRoot;
        use crate::hud::target_panel::TargetPanel;
        const SURFACE_WIDTH: u32 = 900;
        const SURFACE_HEIGHT: u32 = 700;
        const PARTY_TOP: f32 = 520.0;
        const PARTY_HEIGHT: f32 = 120.0;
        const PARTY_RAISE: f32 = 40.0;
        const TARGET_FRAME: (i16, i16, i16) = (300, 200, 180);
        const TARGET_HEIGHT: f32 = 50.0;
        const IN_ZONE: u16 = 1;

        let mut app = App::new();
        app.insert_resource(crate::input_mode::InputMode::default())
            .init_resource::<crate::snapshot::SceneState>()
            .init_resource::<crate::hud::menu::ConfigDatRows>()
            .init_resource::<DockBoard>()
            .add_systems(
                Update,
                (
                    dock_windows_system,
                    place_target_window,
                    layout_panel_column_system,
                )
                    .chain(),
            );
        app.world_mut()
            .resource_mut::<crate::snapshot::SceneState>()
            .snapshot
            .zone_id = Some(IN_ZONE);
        app.world_mut()
            .resource_mut::<crate::hud::menu::ConfigDatRows>()
            .frames
            .insert(TARGET_WINDOW.into(), TARGET_FRAME);
        app.world_mut().spawn((
            Window {
                resolution: bevy::window::WindowResolution::new(SURFACE_WIDTH, SURFACE_HEIGHT),
                ..default()
            },
            bevy::window::PrimaryWindow,
        ));
        let party = app
            .world_mut()
            .spawn((PartyFrameRoot { party_no: 0 }, Node::default()))
            .id();
        let place_party = |app: &mut App, top: f32| {
            app.world_mut().entity_mut(party).insert((
                ComputedNode {
                    size: Vec2::new(crate::hud::style::PANEL_WIDTH_PX, PARTY_HEIGHT),
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                UiGlobalTransform::from_xy(0.0, top + PARTY_HEIGHT / 2.0),
            ));
        };
        place_party(&mut app, PARTY_TOP);
        let target = app
            .world_mut()
            .spawn((
                TargetPanel,
                ColumnPanel::TARGET,
                DockHeld::default(),
                Node::default(),
                ComputedNode {
                    size: Vec2::new(f32::from(TARGET_FRAME.2), TARGET_HEIGHT),
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                UiGlobalTransform::default(),
            ))
            .id();
        let docked_bottom =
            |party_top: f32| Val::Px(SURFACE_HEIGHT as f32 - (party_top - DOCK_GAP));
        let drawn = |app: &App| {
            (
                app.world().get::<Node>(target).unwrap().clone(),
                *app.world().get::<DockHeld>(target).unwrap(),
            )
        };

        app.update();
        let (node, held) = drawn(&app);
        assert_eq!(held, DockHeld(true));
        assert_eq!(
            (node.left, node.width),
            (
                Val::Px(f32::from(TARGET_FRAME.0)),
                Val::Px(f32::from(TARGET_FRAME.2))
            )
        );
        assert_eq!(
            (node.top, node.bottom),
            (Val::Auto, docked_bottom(PARTY_TOP))
        );

        place_party(&mut app, PARTY_TOP - PARTY_RAISE);
        app.update();
        assert_eq!(drawn(&app).0.bottom, docked_bottom(PARTY_TOP - PARTY_RAISE));

        app.world_mut().entity_mut(party).despawn();
        app.update();
        assert_eq!(
            drawn(&app).0.bottom,
            docked_bottom(PARTY_TOP - PARTY_RAISE),
            "a party window that goes leaves the target window where it is"
        );

        app.world_mut().get_mut::<Node>(target).unwrap().display = Display::None;
        app.update();
        let (node, held) = drawn(&app);
        assert_eq!(held, DockHeld(false));
        assert_eq!(node.top, Val::Auto, "the column places it again");
    }
}
