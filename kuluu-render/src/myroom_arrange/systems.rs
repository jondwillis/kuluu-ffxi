//! The arrange editor's scene side: it feeds the editor the drawn pieces and the shelves, writes the
//! poses and tints the editor decides, makes and discards new pieces, drives the camera and its focal
//! length, draws the moving overlay and the cursor, and plays the furniture sounds.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use ffxi_dat::furniture::{self as law, FurnitureAttr};
use ffxi_dat::furniture_arrange::{CameraRig, OverlayQuad, LAYOUT_FOCAL_LENGTH};
use ffxi_proto::map::container::LOC_STORAGE;

use super::{
    arranging, with_session_memory, ArrangeCameraHold, ArrangeFrame, ArrangeTexts, DrawnPiece,
    FurnitureSound, HouseKey, PieceInfo, ShelfEntry, StorageGauge, ARRANGE_GETTEXT,
    ARRANGE_WINDOWS, SYSMES_FLOWERPOT_LIMIT, SYSMES_LAYOUT_STARTED, SYSMES_MANNEQUIN_LIMIT,
};
use crate::audio::SfxEvent;
use crate::camera::{OperatorCamera, ViewFov};
use crate::components::{InGameEntity, IsSelf};
use crate::cutscene::CutsceneMode;
use crate::dat_root::SharedDatRoot;
use crate::hud::gettext::{GetText, GetTextTables, GETTEXT_TABLE_CLIENT_MESSAGES};
use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::menu::DynamicMenu;
use crate::hud::unity_fp::{item_label, load_record_windows, RetailFrameCarry};
use crate::input_mode::InputMode;
use crate::myroom_furniture::{
    bevy_to_ffxi_world, destroy_piece, ffxi_world_to_bevy, shelf_records, spawn_piece,
    MyroomFurnitureTables, MyroomItemKinds, MyroomPiece, MyroomPieceTint, MyroomPieces, MyroomRoom,
};
use crate::scene::TrackedEntities;
use crate::snapshot::{system_chat_line, SceneState};

const ARRANGE_SYSTEM_MESSAGES: [usize; 3] = [
    SYSMES_LAYOUT_STARTED,
    SYSMES_FLOWERPOT_LIMIT,
    SYSMES_MANNEQUIN_LIMIT,
];
const ARRANGE_CAPTION_TABLES: [u8; 3] = [
    crate::hud::gettext::GETTEXT_TABLE_12,
    crate::hud::gettext::GETTEXT_TABLE_SHORT_CAPTIONS,
    crate::hud::gettext::GETTEXT_TABLE_CAPTIONS,
];

/// Reads the editor's DAT text once in game with an install: window item captions, the help bar, the
/// put-away questions and the client and system messages it prints (`/layout`'s refusals print
/// outside a Mog House too).
pub fn load_arrange_texts(
    state: Res<SceneState>,
    dat_root: Res<SharedDatRoot>,
    mut dynamic: ResMut<DynamicMenu>,
    mut loaded: Local<bool>,
) {
    if *loaded || state.snapshot.zone_id.is_none() {
        return;
    }
    let Some(root) = dat_root.get().cloned() else {
        return;
    };
    *loaded = true;
    let captions = GetTextTables::load(&root, &ARRANGE_CAPTION_TABLES);
    let windows = load_record_windows(&root, &ARRANGE_WINDOWS);
    let item_captions = ARRANGE_WINDOWS.iter().flat_map(|&name| {
        let captions = &captions;
        windows
            .get(name)
            .into_iter()
            .flat_map(|window| window.items.iter().flatten())
            .filter_map(move |item| {
                let id = u8::try_from(item.id).ok()?;
                Some(((name, id), item_label(captions, item)?.to_string()))
            })
    });
    let sysmes = ffxi_dat::sysmes::SysMesDat::open(&root);
    let mut texts = arrange_texts(&captions, item_captions, |id| {
        sysmes.as_ref().and_then(|table| {
            table
                .message(id, &ffxi_dat::sysmes::SysMesParams::default())
                .map(|line| line.to_plain())
        })
    });
    texts.client_messages = GetTextTables::load(&root, &[GETTEXT_TABLE_CLIENT_MESSAGES]);
    dynamic.arrange.texts = texts;
}

/// The editor's text from the caption tables, the windows' item captions and a system message
/// source.
pub fn arrange_texts(
    tables: &impl GetText,
    item_captions: impl IntoIterator<Item = ((&'static str, u8), String)>,
    system_message: impl Fn(usize) -> Option<String>,
) -> ArrangeTexts {
    let text = |(table, id): (u8, u16)| tables.get_text(table, id).map(str::to_string);
    let [title, help, planted_prompt, put_away_all_prompt] = ARRANGE_GETTEXT.map(text);
    ArrangeTexts {
        title,
        help,
        planted_prompt,
        put_away_all_prompt,
        system_messages: ARRANGE_SYSTEM_MESSAGES
            .into_iter()
            .filter_map(|id| Some((id, system_message(id)?)))
            .collect(),
        item_labels: item_captions.into_iter().collect(),
        ..ArrangeTexts::default()
    }
}

/// One update's view of the scene for the editor.
pub fn arrange_frame(
    snap: &kuluu_snapshot::SceneSnapshot,
    busy: bool,
    pieces: &MyroomPieces,
    piece_of: impl Fn(Entity) -> Option<MyroomPiece>,
    tables: &MyroomFurnitureTables,
    furnishing: &mut dyn FnMut(u16) -> Option<(u8, u32)>,
    frame_ticks: f32,
) -> ArrangeFrame {
    let room = MyroomRoom::of(snap);
    let house = room.map(|room| HouseKey {
        zone_generation: room.zone_generation,
        floor: room.floor,
        map_number: room.map_number,
    });
    let records = shelf_records(snap);
    let dat = tables.get();
    let shelf: Vec<ShelfEntry> = records
        .iter()
        .filter_map(|record| {
            let attr = FurnitureAttr::decode(record.attr.as_ref()?);
            let (kind, storage) = furnishing(record.item_no)?;
            law::is_furniture_kind(kind).then(|| ShelfEntry {
                container: record.container,
                slot: record.slot,
                item_no: record.item_no,
                kind,
                storage,
                attr,
                property: dat.and_then(|dat| dat.properties.get(record.item_no)),
            })
        })
        .collect();
    let recorded = |container: u8, slot: u8| {
        shelf
            .iter()
            .find(|entry| entry.container == container && entry.slot == slot)
            .map(|entry| entry.attr.pose)
    };
    let drawn = if pieces.room() == room {
        pieces
            .seats()
            .filter_map(|(seat, entity)| {
                let piece = piece_of(entity)?;
                let property = dat?.properties.get(piece.item_no)?;
                let storage = furnishing(piece.item_no).map_or(0, |(_, storage)| storage);
                Some(DrawnPiece {
                    seat,
                    info: PieceInfo {
                        container: piece.container,
                        slot: piece.slot,
                        item_no: piece.item_no,
                        kind: piece.kind,
                        planted: piece.flowerpot_step > 0,
                        storage,
                        recorded: recorded(piece.container, piece.slot),
                    },
                    property,
                    pose: piece.pose,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let storage = snap
        .container(LOC_STORAGE)
        .map(|view| StorageGauge {
            free: i64::from(view.capacity) - view.items.len() as i64,
            entries: view.items.len(),
        })
        .unwrap_or_default();
    ArrangeFrame {
        house,
        arrangeable: room.is_some_and(|room| room.arrangeable(snap)),
        myroom_mode: snap.myroom_mode,
        busy,
        drawn,
        shelf,
        storage,
        second_shelf_open: snap.mh_2f_unlocked.unwrap_or(false),
        frame_ticks,
    }
}

/// Follows the session's mode and the drawn pieces, switches the input mode on entering and leaving,
/// and prints what the editor said.
pub fn sync_arrange_editor(
    time: Res<Time>,
    mut state: ResMut<SceneState>,
    cutscene: Res<CutsceneMode>,
    mut mode: ResMut<InputMode>,
    mut dynamic: ResMut<DynamicMenu>,
    pieces: Res<MyroomPieces>,
    q_pieces: Query<&MyroomPiece>,
    tables: Res<MyroomFurnitureTables>,
    mut kinds: ResMut<MyroomItemKinds>,
    item_root: Option<Res<ItemDatRoot>>,
    icon_cache: Option<ResMut<ItemIconCache>>,
) {
    let snap = &state.snapshot;
    if !dynamic.arrange.active() && snap.myroom.is_none() {
        dynamic.arrange.observe_mode(snap.myroom_mode);
        return;
    }
    let table = match (item_root, icon_cache) {
        (Some(root), Some(mut cache)) => cache.table(&root),
        _ => None,
    };
    let busy = snap.dialog.is_some() || cutscene.active;
    let frame = arrange_frame(
        snap,
        busy,
        &pieces,
        |entity| q_pieces.get(entity).ok().cloned(),
        &tables,
        &mut |item| kinds.furnishing(item, table.as_deref()),
        time.delta_secs() * crate::scheduler_runtime::ROUTINE_FPS,
    );
    let next = dynamic.arrange.update(&frame);
    if let Some(next) = with_session_memory(next, &dynamic.window_cursors) {
        if dynamic.arrange.active() || arranging(&mode) {
            *mode = next;
        }
    }
    for line in std::mem::take(&mut dynamic.arrange.chat_lines) {
        state.push_local_toast(system_chat_line(line));
    }
}

/// Makes and discards the editor's new pieces and writes the poses and tints it holds.
pub fn apply_arrange_pieces(
    mut commands: Commands,
    mut dynamic: ResMut<DynamicMenu>,
    mut pieces: ResMut<MyroomPieces>,
    mut tracked: ResMut<TrackedEntities>,
    mut q_pieces: Query<(&mut MyroomPiece, &mut MyroomPieceTint)>,
) {
    let editor = &mut dynamic.arrange;
    for new in std::mem::take(&mut editor.spawns) {
        let mut piece = MyroomPiece::new(new.container, new.slot, new.item_no, new.kind, &new.attr);
        piece.pose = new.pose;
        spawn_piece(&mut commands, &mut pieces, piece);
    }
    for seat in std::mem::take(&mut editor.despawns) {
        destroy_piece(&mut commands, &mut pieces, &mut tracked, seat);
    }
    let reset = std::mem::take(&mut editor.tints_reset);
    if !editor.active() && !reset {
        return;
    }
    for (seat, entity) in pieces.seats() {
        let Ok((mut piece, mut tint)) = q_pieces.get_mut(entity) else {
            continue;
        };
        if let Some(pose) = editor.pose_of(seat).filter(|pose| *pose != piece.pose) {
            piece.pose = pose;
        }
        let want = MyroomPieceTint::from_bytes(editor.tint_of(seat));
        if *tint != want {
            *tint = want;
        }
    }
}

/// The distance the chase camera's look point sits from its eye when layout mode begins, for an eye
/// with no player in front of it.
const START_LOOK_DISTANCE: f32 = 1.0;

/// While arranging the editor holds the camera: the focal length goes to the layout value (the old
/// one is restored on leaving) and the eye and focus follow the selection each client update.
pub fn arrange_camera(
    time: Res<Time>,
    mut dynamic: ResMut<DynamicMenu>,
    mut view_fov: ResMut<ViewFov>,
    mut hold: ResMut<ArrangeCameraHold>,
    mut q_camera: Query<&mut Transform, With<OperatorCamera>>,
    q_self: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut carry: Local<RetailFrameCarry>,
) {
    let editor = &mut dynamic.arrange;
    if !editor.active() {
        if let Some(focal) = editor.stored_focal.take() {
            view_fov.focal_length = focal;
        }
        hold.set_if_neq(ArrangeCameraHold(false));
        return;
    }
    if editor.stored_focal.is_none() {
        editor.stored_focal = Some(view_fov.focal_length);
        view_fov.focal_length = LAYOUT_FOCAL_LENGTH;
    }
    hold.set_if_neq(ArrangeCameraHold(true));
    let Ok(mut camera) = q_camera.single_mut() else {
        return;
    };
    let player = q_self.single().ok().map(|player| player.translation);
    let start = *camera;
    let updates = carry.frames(time.delta_secs());
    let Some(rig) = editor.camera_update(updates, || {
        let look = player.map_or(START_LOOK_DISTANCE, |player| {
            start.translation.distance(player)
        });
        CameraRig {
            eye: bevy_to_ffxi_world(start.translation),
            focus: bevy_to_ffxi_world(start.translation + start.forward() * look),
        }
    }) else {
        return;
    };
    let eye = ffxi_world_to_bevy(rig.eye);
    let focus = ffxi_world_to_bevy(rig.focus);
    *camera = Transform::from_translation(eye).looking_at(focus, Vec3::Y);
}

/// The overlay mesh drawn while a piece moves.
#[derive(Component)]
pub struct ArrangeOverlay;

const COLOR_BYTE_ONE: f32 = 128.0;

fn overlay_color(bytes: [u8; 4]) -> [f32; 4] {
    let [r, g, b, a] = bytes.map(|byte| (f32::from(byte) / COLOR_BYTE_ONE).min(1.0));
    Color::srgba(r, g, b, a).to_linear().to_f32_array()
}

/// One triangle-list mesh of the overlay quads, both faces drawn.
pub fn overlay_mesh(quads: &[OverlayQuad]) -> Mesh {
    let mut positions = Vec::with_capacity(quads.len() * 4);
    let mut colors = Vec::with_capacity(quads.len() * 4);
    let mut indices = Vec::with_capacity(quads.len() * 6);
    for quad in quads {
        let base = positions.len() as u32;
        for corner in quad.corners {
            positions.push(ffxi_world_to_bevy(corner).to_array());
            colors.push(overlay_color(quad.color));
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

/// Rebuilds the overlay when what it shows changes, and removes it when nothing moves.
pub fn draw_arrange_overlay(
    mut commands: Commands,
    dynamic: Res<DynamicMenu>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    q_overlay: Query<(Entity, &Mesh3d), With<ArrangeOverlay>>,
    mut shown: Local<Vec<OverlayQuad>>,
) {
    let quads = dynamic.arrange.overlay();
    if quads == *shown {
        return;
    }
    if quads.is_empty() {
        for (entity, _) in &q_overlay {
            commands.entity(entity).try_despawn();
        }
        shown.clear();
        return;
    }
    let mesh = meshes.add(overlay_mesh(&quads));
    match q_overlay.iter().next() {
        Some((entity, _)) => {
            commands.entity(entity).insert(Mesh3d(mesh));
        }
        None => {
            let material = materials.add(StandardMaterial {
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                double_sided: true,
                ..default()
            });
            commands.spawn((
                ArrangeOverlay,
                InGameEntity,
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                Visibility::Inherited,
            ));
        }
    }
    *shown = quads;
}

/// The selection cursor drawn at the selected piece's screen point.
#[derive(Component)]
pub struct ArrangeCursor;

/// The screen coordinate a projection that lands negative is drawn at.
const OFF_SCREEN_COORDINATE: f32 = 100.0;
const CURSOR_TEXT_SIZE: f32 = 16.0;

/// The cursor's screen point: a negative coordinate is drawn at [`OFF_SCREEN_COORDINATE`].
pub fn cursor_point(projected: Vec2) -> Vec2 {
    let fix = |v: f32| {
        if v < 0.0 {
            OFF_SCREEN_COORDINATE
        } else {
            v
        }
    };
    Vec2::new(fix(projected.x), fix(projected.y))
}

/// The selection cursor. Retail draws an animated sprite from its UI sprite group 6, which kuluu's
/// atlas cannot name by number, so the pointer stand-in marks the spot.
pub fn place_arrange_cursor(
    mut commands: Commands,
    dynamic: Res<DynamicMenu>,
    q_camera: Query<(&Camera, &GlobalTransform), With<OperatorCamera>>,
    mut q_cursor: Query<(&mut Node, &mut Visibility), With<ArrangeCursor>>,
) {
    let editor = &dynamic.arrange;
    let point = editor
        .selection()
        .and_then(|seat| editor.room()?.point(seat))
        .map(ffxi_world_to_bevy);
    let screen = point.and_then(|point| {
        let (camera, transform) = q_camera.single().ok()?;
        camera.world_to_viewport(transform, point).ok()
    });
    let Ok((mut node, mut visibility)) = q_cursor.single_mut() else {
        if screen.is_some() {
            commands.spawn((
                ArrangeCursor,
                InGameEntity,
                Text::new(crate::hud::unity_fp::POINTER_STAND_IN),
                crate::hud::style::text_font(CURSOR_TEXT_SIZE),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                Visibility::Hidden,
            ));
        }
        return;
    };
    match screen.filter(|_| editor.active()) {
        Some(screen) => {
            let at = cursor_point(screen);
            node.left = Val::Px(at.x);
            node.top = Val::Px(at.y);
            visibility.set_if_neq(Visibility::Inherited);
        }
        None => {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
}

/// A window that loses the keyboard never sees the releases of the keys held at that moment.
pub fn release_arrange_keys_on_focus_loss(
    mut lost: MessageReader<bevy::input::keyboard::KeyboardFocusLost>,
    mut dynamic: ResMut<DynamicMenu>,
) {
    if lost.read().count() > 0 {
        dynamic.arrange.release_keys();
    }
}

/// Plays the furniture DAT's own sound chunks the editor asked for.
pub fn play_arrange_sounds(
    mut dynamic: ResMut<DynamicMenu>,
    tables: Res<MyroomFurnitureTables>,
    mut writer: MessageWriter<SfxEvent>,
) {
    if dynamic.arrange.furniture_sounds.is_empty() {
        return;
    }
    let sounds = tables.get().map(|dat| dat.sounds).unwrap_or_default();
    for sound in std::mem::take(&mut dynamic.arrange.furniture_sounds) {
        let se_id = match sound {
            FurnitureSound::Step => sounds.step,
            FurnitureSound::Rotate => sounds.rotate,
            FurnitureSound::Placed => sounds.placed,
        };
        if let Some(se_id) = se_id {
            writer.write(SfxEvent::new(se_id));
        }
    }
}

pub struct MyroomArrangePlugin;

impl Plugin for MyroomArrangePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArrangeCameraHold>().add_systems(
            Update,
            (
                load_arrange_texts,
                sync_arrange_editor,
                apply_arrange_pieces,
            )
                .chain()
                .after(crate::myroom_furniture::scan_myroom_shelves)
                .before(crate::myroom_furniture::pose_myroom_pieces),
        );
        app.add_systems(
            Update,
            (
                arrange_camera,
                draw_arrange_overlay,
                place_arrange_cursor,
                play_arrange_sounds,
                release_arrange_keys_on_focus_loss,
            )
                .after(apply_arrange_pieces),
        );
    }
}
