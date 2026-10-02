use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::map_image::{status_icon_at, STATUS_ICON_FILE_ID};
use ffxi_dat::DatRoot;

use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Resource, Default, Clone)]
pub struct StatusIconDatRoot(pub Option<Arc<DatRoot>>);

#[derive(Resource, Default)]
pub struct StatusIconCache {
    dat: Option<Arc<Vec<u8>>>,

    dat_unavailable: bool,

    icons: HashMap<u16, Option<Handle<Image>>>,
}

impl StatusIconCache {
    fn ensure(
        &mut self,
        status_id: u16,
        dat_root: &StatusIconDatRoot,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(slot) = self.icons.get(&status_id) {
            return slot.clone();
        }
        let handle = self
            .dat_bytes(dat_root)
            .and_then(|bytes| status_icon_at(&bytes, status_id))
            .map(|img| upload_icon(img, images));
        self.icons.insert(status_id, handle.clone());
        handle
    }

    fn dat_bytes(&mut self, dat_root: &StatusIconDatRoot) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = &self.dat {
            return Some(bytes.clone());
        }
        if self.dat_unavailable {
            return None;
        }
        let root = match &dat_root.0 {
            Some(r) => r,
            None => {
                return None;
            }
        };
        let loaded = root
            .resolve(STATUS_ICON_FILE_ID)
            .ok()
            .map(|loc| loc.path_under(root))
            .and_then(|path| std::fs::read(path).ok());
        match loaded {
            Some(bytes) => {
                let arc = Arc::new(bytes);
                self.dat = Some(arc.clone());
                Some(arc)
            }
            None => {
                warn!(
                    "status icons: DAT file_id {STATUS_ICON_FILE_ID} unreadable; numeric fallback"
                );
                self.dat_unavailable = true;
                None
            }
        }
    }
}

fn upload_icon(
    img: ffxi_dat::map_image::GraphicImage,
    images: &mut Assets<Image>,
) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    images.add(image)
}

fn transparent_placeholder(images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

#[derive(Component)]
pub struct StatusRibbon;

#[derive(Component)]
pub struct StatusChip {
    pub slot: usize,
}

#[derive(Component)]
pub struct StatusChipFallback;

const MAX_VISIBLE: usize = 32;

const ICON_SIZE_PX: f32 = 20.0;

/// Retail packs the ribbon tight, so this is the whole gap between chips; only
/// the enhanced countdown (the `timers` module) widens the pitch past it.
const MIN_ICON_GAP_PX: f32 = 2.0;

/// Clearance between two wrapped rows of icons, on top of whatever line the
/// enhanced countdown adds.
const ROW_CLEARANCE_PX: f32 = 2.0;

pub const ICONS_PER_ROW: usize = 16;

#[cfg(feature = "enhanced-buff-timers")]
use timers::{cell_pitch_px, timer_line_px};

#[cfg(not(feature = "enhanced-buff-timers"))]
fn cell_pitch_px() -> f32 {
    ICON_SIZE_PX + MIN_ICON_GAP_PX
}

#[cfg(not(feature = "enhanced-buff-timers"))]
fn timer_line_px() -> f32 {
    0.0
}

pub fn spawn_status_ribbon(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);
    let pitch = cell_pitch_px();
    let icon_gap = pitch - ICON_SIZE_PX;
    let timer_line = timer_line_px();
    let row_width = ICONS_PER_ROW as f32 * pitch;

    commands
        .spawn((
            crate::components::InGameEntity,
            StatusRibbon,
            Node {
                position_type: PositionType::Absolute,

                // Below the menu help bar so chips never overlap it when open.
                top: Val::Px(crate::hud::menu_help_bar::BAR_HEIGHT + 6.0),
                left: Val::Px(8.0),
                width: Val::Px(row_width),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::FlexStart,
                align_content: AlignContent::FlexStart,
                column_gap: Val::Px(icon_gap),
                row_gap: Val::Px(timer_line + ROW_CLEARANCE_PX),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(Color::NONE),
        ))
        .with_children(|p| {
            for slot in 0..MAX_VISIBLE {
                p.spawn((
                    StatusChip { slot },
                    Node {
                        width: Val::Px(ICON_SIZE_PX),
                        height: Val::Px(ICON_SIZE_PX),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border: UiRect::all(Val::Px(1.0)),
                        display: Display::None,
                        ..default()
                    },
                    ImageNode::new(placeholder.clone()),
                    BackgroundColor(Color::NONE),
                    BorderColor::all(Color::NONE),
                    Interaction::default(),
                ))
                .with_children(|chip| {
                    chip.spawn((
                        StatusChipFallback,
                        Text::new(""),
                        style::text_font(10.0),
                        TextColor(theme::TEXT),
                    ));
                    #[cfg(feature = "enhanced-buff-timers")]
                    timers::spawn_chip_timer(chip, pitch);
                });
            }
        });
}

pub fn update_status_ribbon(
    state: Res<SceneState>,
    dat_root: Res<StatusIconDatRoot>,
    mut cache: ResMut<StatusIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut chips: Query<(
        &StatusChip,
        &Children,
        &mut Node,
        &mut ImageNode,
        &mut BackgroundColor,
    )>,
    mut text_q: Query<&mut Text, With<StatusChipFallback>>,
) {
    if !state.dirty {
        return;
    }
    let icons = &state.snapshot.status_icons;

    for (chip, children, mut node, mut image_node, mut bg) in chips.iter_mut() {
        let Some(&icon_id) = icons.get(chip.slot) else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        if node.display == Display::None {
            node.display = Display::Flex;
        }

        match cache.ensure(icon_id, &dat_root, &mut images) {
            Some(handle) => {
                if image_node.image != handle {
                    image_node.image = handle;
                }
                if image_node.color != Color::WHITE {
                    image_node.color = Color::WHITE;
                }
                if bg.0 != Color::NONE {
                    bg.0 = Color::NONE;
                }
                set_fallback_text(children, &mut text_q, "");
            }
            None => {
                if image_node.color.alpha() != 0.0 {
                    image_node.color = Color::NONE;
                }
                if bg.0 != theme::CELL_BG {
                    bg.0 = theme::CELL_BG;
                }
                set_fallback_text(children, &mut text_q, &format!("{icon_id}"));
            }
        }
    }
}

/// Highlights the status ribbon while it holds the active-window cursor: a
/// frame border on the ribbon and a cursor border on the selected chip — bright
/// (`CURSOR`) when that buff is player-cancelable, muted otherwise, so the
/// player can see which buffs Confirm will click off (retail's status window).
pub fn update_status_ribbon_selection(
    mode: Res<crate::input_mode::InputMode>,
    state: Res<SceneState>,
    mut ribbon_q: Query<&mut BorderColor, (With<StatusRibbon>, Without<StatusChip>)>,
    mut chips: Query<(&StatusChip, &mut BorderColor), Without<StatusRibbon>>,
) {
    use crate::input_mode::{InputMode, PassiveCursorFocus};

    if !mode.is_changed() && !state.is_changed() {
        return;
    }

    let (focused, cursor) = match &*mode {
        InputMode::PassiveCursor(s) if matches!(s.focus, PassiveCursorFocus::StatusIcons) => {
            (true, s.status_cursor)
        }
        _ => (false, usize::MAX),
    };

    let ribbon_border = if focused {
        theme::FRAME_EDGE
    } else {
        Color::NONE
    };
    for mut border in ribbon_q.iter_mut() {
        if border.left != ribbon_border {
            *border = BorderColor::all(ribbon_border);
        }
    }

    let icons = &state.snapshot.status_icons;
    for (chip, mut border) in chips.iter_mut() {
        let want = if focused && chip.slot == cursor {
            let icon = icons.get(chip.slot).copied().unwrap_or(0);
            if ffxi_vocab::status_effects::is_cancelable(icon) {
                theme::CURSOR
            } else {
                theme::MUTED
            }
        } else {
            Color::NONE
        };
        if border.left != want {
            *border = BorderColor::all(want);
        }
    }
}

fn set_fallback_text(
    children: &Children,
    text_q: &mut Query<&mut Text, With<StatusChipFallback>>,
    want: &str,
) {
    for child in children.iter() {
        if let Ok(mut text) = text_q.get_mut(child) {
            if **text != want {
                **text = want.to_string();
            }
        }
    }
}

/// Enhanced (non-retail) countdown under every status chip. Retail draws no
/// remaining-time text on the ribbon at all, so both the format and the extra
/// chip pitch the label needs are gated here.
#[cfg(feature = "enhanced-buff-timers")]
pub mod timers {
    use super::*;

    #[derive(Component)]
    pub struct StatusChipTimer;

    const TIMER_FONT_PX: f32 = 8.0;

    /// Blank space kept between two neighbouring countdowns at their widest, so a
    /// full row of long timers still reads as one number per icon.
    const TIMER_LABEL_GAP_PX: f32 = 4.0;

    /// The widest string [`ribbon_timer`] emits, which is what the chip pitch
    /// reserves room for; `ribbon_timer_never_exceeds_reserved_width` pins it.
    const WIDEST_RIBBON_TIMER: &str = "59:59";

    /// Past this many hours even the minutes are noise, and dropping them is what
    /// holds the label inside the reserved width for expiries out at
    /// [`kuluu_snapshot::MAX_STATUS_TIMER_SECS`].
    const COARSE_TIMER_HOURS: u32 = 10;

    /// Countdown text for a 20px chip: seconds are what matter on a 30-second
    /// debuff and noise on a 3-hour food buff, and shedding precision as the
    /// duration grows is what keeps every label inside one chip pitch instead of
    /// running into its neighbour.
    fn ribbon_timer(remaining_secs: u32) -> String {
        let (h, m, s) = (
            remaining_secs / 3600,
            (remaining_secs % 3600) / 60,
            remaining_secs % 60,
        );
        match h {
            0 => format!("{m}:{s:02}"),
            _ if h < COARSE_TIMER_HOURS => format!("{h}h{m:02}"),
            _ => format!("{h}h"),
        }
    }

    /// Horizontal pitch of one chip: wide enough for the icon and for a
    /// full-width countdown centred under it, so neither the art nor the label
    /// can collide with the neighbouring slot.
    pub(super) fn cell_pitch_px() -> f32 {
        let label = crate::ui_font::text_width_px(WIDEST_RIBBON_TIMER, TIMER_FONT_PX);
        (ICON_SIZE_PX + MIN_ICON_GAP_PX).max(label + TIMER_LABEL_GAP_PX)
    }

    pub(super) fn timer_line_px() -> f32 {
        crate::ui_font::line_height_px(TIMER_FONT_PX)
    }

    /// The label is centred on the chip pitch, not on the icon, so it
    /// overhangs evenly into the gap either side.
    pub(super) fn spawn_chip_timer(chip: &mut ChildSpawnerCommands, pitch: f32) {
        let overhang = (pitch - ICON_SIZE_PX) / 2.0;
        chip.spawn((
            StatusChipTimer,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(-timer_line_px()),
                left: Val::Px(-overhang),
                width: Val::Px(pitch),
                ..default()
            },
            Text::new(""),
            TextLayout {
                justify: Justify::Center,
                linebreak: LineBreak::NoWrap,
            },
            style::text_font(TIMER_FONT_PX),
            TextColor(theme::TITLE),
        ));
    }

    pub fn update_status_timers(
        state: Res<SceneState>,
        clock: Res<crate::vana_time::VanaClock>,
        chips: Query<(&StatusChip, &Children)>,
        mut timer_q: Query<&mut Text, With<StatusChipTimer>>,
    ) {
        let now = clock.earth_unix_secs_now() as u32;
        let expiries = &state.snapshot.status_icon_expiries;
        for (chip, children) in chips.iter() {
            let want = expiries
                .get(chip.slot)
                .copied()
                .filter(|&e| e != 0)
                .map(|e| e.saturating_sub(now))
                .filter(|&r| r > 0)
                .map(ribbon_timer)
                .unwrap_or_default();
            for child in children.iter() {
                if let Ok(mut text) = timer_q.get_mut(child) {
                    if **text != want {
                        **text = want.clone();
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use bevy::ecs::system::RunSystemOnce;

        /// Every countdown the pipeline can deliver has to fit the width the
        /// chip pitch reserves, or neighbouring labels run together.
        #[test]
        fn ribbon_timer_never_exceeds_reserved_width() {
            let reserved = WIDEST_RIBBON_TIMER.chars().count();
            for secs in 1..=kuluu_snapshot::MAX_STATUS_TIMER_SECS {
                let label = ribbon_timer(secs);
                assert!(
                    label.chars().count() <= reserved,
                    "{secs}s renders as {label:?}, wider than {WIDEST_RIBBON_TIMER:?}"
                );
            }
        }

        #[test]
        fn ribbon_timer_drops_seconds_past_the_hour() {
            assert_eq!(ribbon_timer(84), "1:24");
            assert_eq!(ribbon_timer(602), "10:02");
            assert_eq!(ribbon_timer(3599), "59:59");
            assert_eq!(ribbon_timer(3600), "1h00");
            assert_eq!(ribbon_timer(7_245), "2h00");
            assert_eq!(ribbon_timer(10_800), "3h00");
            assert_eq!(ribbon_timer(kuluu_snapshot::MAX_STATUS_TIMER_SECS), "100h");
        }

        #[test]
        fn cell_pitch_fits_the_widest_label_and_the_icon() {
            let pitch = cell_pitch_px();
            let label = crate::ui_font::text_width_px(WIDEST_RIBBON_TIMER, TIMER_FONT_PX);
            assert!(
                pitch >= label + TIMER_LABEL_GAP_PX,
                "pitch {pitch} leaves no gap between {label}px labels"
            );
            assert!(pitch >= ICON_SIZE_PX + MIN_ICON_GAP_PX, "pitch {pitch}");
            assert!(
                pitch < ICON_SIZE_PX * 2.0,
                "pitch {pitch} orphans the label (centring on the pitch must not push it off its own icon)"
            );
        }

        /// `update_status_timers` reads the clock again after this test does, so
        /// the expiry sits mid-bucket: every remaining time in 2h00..2h01 renders
        /// identically, leaving 30s of slack for wall clock passing in between.
        const MID_BUCKET_REMAINING_SECS: u32 = 2 * 3600 + 30;
        const MID_BUCKET_LABEL: &str = "2h00";

        #[test]
        fn enhanced_build_draws_a_countdown_under_each_chip() {
            let clock = crate::vana_time::VanaClock::anchored_at_hour(12.0);
            let now = clock.earth_unix_secs_now() as u32;
            let mut app = App::new();
            app.init_resource::<Assets<Image>>()
                .init_resource::<SceneState>()
                .insert_resource(clock);
            app.world_mut()
                .run_system_once(spawn_status_ribbon)
                .unwrap();

            let mut state = app.world_mut().resource_mut::<SceneState>();
            state.snapshot.status_icons = vec![10, 20];
            state.snapshot.status_icon_expiries = vec![now + MID_BUCKET_REMAINING_SECS, 0];
            app.world_mut()
                .run_system_once(update_status_timers)
                .unwrap();

            assert_eq!(
                chip_timer_texts(app.world_mut())[..2],
                [MID_BUCKET_LABEL, ""]
            );
        }

        fn chip_timer_texts(world: &mut World) -> Vec<String> {
            let labels: HashMap<Entity, String> = world
                .query_filtered::<(Entity, &Text), With<StatusChipTimer>>()
                .iter(world)
                .map(|(e, text)| (e, text.0.clone()))
                .collect();
            let mut by_slot: Vec<(usize, String)> = world
                .query::<(&StatusChip, &Children)>()
                .iter(world)
                .map(|(chip, children)| {
                    let label = children
                        .iter()
                        .find_map(|child| labels.get(&child).cloned())
                        .expect("every chip carries a countdown label");
                    (chip.slot, label)
                })
                .collect();
            by_slot.sort_by_key(|(slot, _)| *slot);
            by_slot.into_iter().map(|(_, label)| label).collect()
        }
    }
}

/// Enhanced (non-retail) hover tooltip for the status ribbon: shows the buff's
/// name from the scraped status-effect table when the pointer is over a chip.
#[cfg(feature = "enhanced-buff-tooltips")]
pub mod tooltip {
    use super::*;
    use crate::mouse::MousePointer;

    #[derive(Component)]
    pub struct BuffTooltip;

    #[derive(Component)]
    pub struct BuffTooltipText;

    const TOOLTIP_OFFSET_PX: Vec2 = Vec2::new(16.0, 16.0);

    pub fn spawn_buff_tooltip(mut commands: Commands) {
        commands
            .spawn((
                crate::components::InGameEntity,
                BuffTooltip,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(-1000.0),
                    top: Val::Px(-1000.0),
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(theme::FRAME_BG),
                BorderColor::all(theme::FRAME_EDGE),
                ZIndex(i32::MAX - 1),
            ))
            .with_children(|p| {
                p.spawn((
                    BuffTooltipText,
                    Text::new(""),
                    style::text_font(12.0),
                    TextColor(theme::TEXT),
                ));
            });
    }

    pub fn update_buff_tooltip(
        state: Res<SceneState>,
        pointer: Res<MousePointer>,
        chips: Query<(&StatusChip, &Interaction)>,
        mut card_q: Query<&mut Node, With<BuffTooltip>>,
        mut text_q: Query<&mut Text, With<BuffTooltipText>>,
    ) {
        let Ok(mut card) = card_q.single_mut() else {
            return;
        };

        let icons = &state.snapshot.status_icons;
        let hovered_icon = chips
            .iter()
            .find(|(_, i)| matches!(i, Interaction::Hovered | Interaction::Pressed))
            .and_then(|(chip, _)| icons.get(chip.slot).copied());

        let name = hovered_icon.and_then(ffxi_vocab::status_names::lookup);
        let Some(name) = name else {
            if card.display != Display::None {
                card.display = Display::None;
            }
            return;
        };

        if card.display == Display::None {
            card.display = Display::Flex;
        }
        if let Some(pos) = pointer.cursor_pos {
            let want_left = Val::Px(pos.x + TOOLTIP_OFFSET_PX.x);
            let want_top = Val::Px(pos.y + TOOLTIP_OFFSET_PX.y);
            if card.left != want_left {
                card.left = want_left;
            }
            if card.top != want_top {
                card.top = want_top;
            }
        }
        if let Ok(mut text) = text_q.single_mut() {
            if text.as_str() != name {
                **text = name.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_allocation_matches_icon_index() {
        let icons = [10u16, 20, 30];
        for slot in 0..MAX_VISIBLE {
            let got = icons.get(slot).copied();
            let want = match slot {
                0 => Some(10),
                1 => Some(20),
                2 => Some(30),
                _ => None,
            };
            assert_eq!(got, want, "slot {slot}");
        }
    }

    /// Retail draws no countdown on the ribbon, so a build without
    /// `enhanced-buff-timers` spawns no label under a chip and does not
    /// reserve the pitch/row space one would need.
    #[cfg(not(feature = "enhanced-buff-timers"))]
    #[test]
    fn default_build_packs_the_ribbon_tight_with_no_countdown() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        app.world_mut()
            .run_system_once(spawn_status_ribbon)
            .unwrap();

        let world = app.world_mut();
        let ribbon = world
            .query_filtered::<&Node, With<StatusRibbon>>()
            .iter(world)
            .next()
            .expect("ribbon spawned")
            .clone();
        assert_eq!(ribbon.column_gap, Val::Px(MIN_ICON_GAP_PX));
        assert_eq!(ribbon.row_gap, Val::Px(ROW_CLEARANCE_PX));
        assert_eq!(
            ribbon.width,
            Val::Px(ICONS_PER_ROW as f32 * (ICON_SIZE_PX + MIN_ICON_GAP_PX))
        );

        let texts: Vec<Entity> = world
            .query_filtered::<Entity, With<Text>>()
            .iter(world)
            .collect();
        assert_eq!(
            texts.len(),
            MAX_VISIBLE,
            "only the icon-id fallback text belongs under a chip"
        );
    }

    #[test]
    fn cache_without_root_does_not_latch() {
        let mut cache = StatusIconCache::default();
        let root = StatusIconDatRoot(None);
        assert!(cache.dat_bytes(&root).is_none());
        assert!(!cache.dat_unavailable, "must retry once root is provided");
    }
}
