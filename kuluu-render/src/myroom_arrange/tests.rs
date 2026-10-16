use super::*;
use crate::input_mode::CONFIRM_NO_ROW;
use ffxi_dat::furniture::{FLAG_STACKABLE, FLAG_SURFACE, FLOOR_1F, FLOOR_2F, KIND_FURNISHING};
use kuluu_snapshot::MYROOM_MODE_NORMAL;

const STARTED: &str = "started";
const POT_LIMIT: &str = "pots";
const MANNEQUIN_LIMIT: &str = "mannequins";
const HELP: &str = "help";
const PLANTED_PROMPT: &str = "planted?";
const ALL_PROMPT: &str = "all?";
const ITEM_BASE: u16 = 100;
const ZONE: u64 = 7;

fn property(flags: u8, width: u8, depth: u8, height: u16) -> FurnitureProperty {
    FurnitureProperty {
        flags,
        model: 0,
        width,
        depth,
        height,
    }
}

fn chair() -> FurnitureProperty {
    property(0, 1, 1, 0)
}

fn table() -> FurnitureProperty {
    property(FLAG_SURFACE, 2, 1, 105)
}

fn vase() -> FurnitureProperty {
    property(FLAG_STACKABLE, 1, 1, 0)
}

fn pose(col: i8, row: i8, level: i8) -> FurniturePose {
    FurniturePose {
        col,
        level,
        row,
        rot: 0,
    }
}

fn piece(container: u8, slot: u8, property: FurnitureProperty, pose: FurniturePose) -> DrawnPiece {
    DrawnPiece {
        seat: law::seat_index(container, slot),
        info: PieceInfo {
            container,
            slot,
            item_no: ITEM_BASE + u16::from(slot),
            kind: KIND_FURNISHING,
            planted: false,
            storage: 1,
            recorded: Some(pose),
        },
        property,
        pose,
    }
}

fn of_kind(mut drawn: DrawnPiece, kind: u8) -> DrawnPiece {
    drawn.info.kind = kind;
    drawn
}

fn planted(mut drawn: DrawnPiece) -> DrawnPiece {
    drawn.info.planted = true;
    drawn
}

fn house(floor: u8, zone_generation: u64) -> HouseKey {
    HouseKey {
        zone_generation,
        floor,
        map_number: 0,
    }
}

fn frame(mode: u8, drawn: Vec<DrawnPiece>) -> ArrangeFrame {
    ArrangeFrame {
        house: Some(house(FLOOR_2F, ZONE)),
        arrangeable: true,
        myroom_mode: mode,
        busy: false,
        drawn,
        shelf: Vec::new(),
        storage: StorageGauge {
            free: 10,
            entries: 0,
        },
        second_shelf_open: true,
        frame_ticks: 0.0,
    }
}

fn texts() -> ArrangeTexts {
    let mut texts = ArrangeTexts {
        help: Some(HELP.into()),
        planted_prompt: Some(PLANTED_PROMPT.into()),
        put_away_all_prompt: Some(ALL_PROMPT.into()),
        ..ArrangeTexts::default()
    };
    for (id, text) in [
        (SYSMES_LAYOUT_STARTED, STARTED),
        (SYSMES_FLOWERPOT_LIMIT, POT_LIMIT),
        (SYSMES_MANNEQUIN_LIMIT, MANNEQUIN_LIMIT),
    ] {
        texts.system_messages.insert(id, text.into());
    }
    texts
}

fn editor() -> ArrangeEditor {
    ArrangeEditor {
        texts: texts(),
        ..ArrangeEditor::default()
    }
}

fn entered_with(frame: ArrangeFrame) -> (ArrangeEditor, Option<InputMode>) {
    let mut editor = editor();
    let mode = editor.update(&frame);
    (editor, mode)
}

fn entered(drawn: Vec<DrawnPiece>) -> (ArrangeEditor, Option<InputMode>) {
    entered_with(frame(MYROOM_MODE_LAYOUT, drawn))
}

fn menu(mode: Option<InputMode>) -> MenuStack {
    match mode {
        Some(InputMode::Menu(stack)) => stack,
        other => panic!("expected a window, got {other:?}"),
    }
}

fn top(stack: &MenuStack) -> Option<ArrangeWindow> {
    stack.current().and_then(arrange_window)
}

fn is_arrange(mode: &Option<InputMode>) -> bool {
    matches!(mode, Some(InputMode::Arrange))
}

/// A press and its release, as one tap of the key.
fn tap(editor: &mut ArrangeEditor, key: ArrangeKey) -> (Option<InputMode>, Vec<UiSound>) {
    let mut sounds = Vec::new();
    let mode = editor.key(key, KeyPhase::Press, false, &mut sounds);
    editor.key(key, KeyPhase::Release, false, &mut sounds);
    (mode, sounds)
}

fn choose(
    editor: &mut ArrangeEditor,
    stack: &mut MenuStack,
    item: u8,
) -> (Option<InputMode>, Vec<UiSound>) {
    let window = top(stack).expect("an arrange window");
    let mut sounds = Vec::new();
    let action = Some(DynamicMenuAction::Arrange(ArrangeAction::Item { item }));
    let mode = editor.select_row(window, action, stack, &mut sounds);
    (mode, sounds)
}

fn layout_of(editor: &ArrangeEditor, drawn: &DrawnPiece) -> ArrangeRequest {
    let pose = editor.pose_of(drawn.seat).expect("a drawn piece");
    ArrangeRequest::Layout {
        item_no: drawn.info.item_no,
        slot: drawn.info.slot,
        container: drawn.info.container,
        floor: FLOOR_2F,
        col: pose.col,
        level: pose.level,
        row: pose.row,
        rot: pose.rot,
    }
}

fn bankin_of(drawn: &DrawnPiece) -> ArrangeRequest {
    ArrangeRequest::Bankin {
        item_no: drawn.info.item_no,
        slot: drawn.info.slot,
        container: drawn.info.container,
    }
}

fn first_pulse() -> ColorBytes {
    Pulse::default().tint(PULSE_ALPHA)
}

#[test]
fn entering_tints_every_piece_normal_selects_the_lowest_seat_and_says_so() {
    let upper_shelf = piece(LOC_MOGSAFE2, 1, chair(), pose(2, 2, 0));
    let lower_shelf = piece(LOC_MOGSAFE, 5, chair(), pose(6, 6, 0));
    let (editor, mode) = entered(vec![upper_shelf, lower_shelf]);
    assert!(is_arrange(&mode), "{mode:?}");
    assert!(editor.active());
    assert_eq!(
        editor.selection(),
        Some(lower_shelf.seat),
        "container 1 slots first"
    );
    assert_eq!(editor.chat_lines, vec![STARTED.to_string()]);
    assert_eq!(editor.tint_of(upper_shelf.seat), TINT_NORMAL);
    assert_eq!(editor.tint_of(lower_shelf.seat), first_pulse());
    assert!(arranging(&InputMode::Arrange));
}

#[test]
fn entering_with_nothing_placed_opens_rmlo1_and_an_unarrangeable_house_does_nothing() {
    let (editor, mode) = entered(Vec::new());
    let stack = menu(mode);
    assert_eq!(stack.levels.len(), 1);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Empty { put_away_all: true })
    );
    assert!(is_arrange_stack(&stack) && editor.active());

    let mut crowded = frame(MYROOM_MODE_LAYOUT, Vec::new());
    crowded.storage.entries = 2;
    let (_, mode) = entered_with(crowded);
    let window = top(&menu(mode)).unwrap();
    assert!(window.item_disabled(RMLO1_PUT_AWAY_ALL));
    assert!(window.row_disabled(usize::from(RMLO1_PUT_AWAY_ALL - 1)));
    assert!(!window.item_disabled(RMLO1_PLACE) && !window.item_disabled(RMLO1_FINISH));

    let mut downstairs = frame(MYROOM_MODE_LAYOUT, Vec::new());
    downstairs.arrangeable = false;
    let (editor, mode) = entered_with(downstairs);
    assert!(mode.is_none() && !editor.active() && editor.chat_lines.is_empty());
}

#[test]
fn esc_in_browse_and_rmlo1_finish_leave_with_the_closing_request() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair]);
    let (mode, sounds) = tap(&mut editor, ArrangeKey::Cancel);
    let stack = menu(mode);
    let kinds: Vec<MenuKind> = stack.levels.iter().map(|level| level.kind).collect();
    assert_eq!(kinds, vec![MenuKind::HomeView], "myroom alone reopens");
    assert_eq!(sounds, vec![UiSound::Cancel]);
    assert_eq!(editor.requests, vec![ArrangeRequest::Leave]);
    assert!(!editor.active() && editor.tints_reset);
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.tint_of(chair.seat), TINT_NORMAL);

    let (mut editor, mode) = entered(Vec::new());
    let mut stack = menu(mode);
    let (mode, _) = choose(&mut editor, &mut stack, RMLO1_FINISH);
    assert!(stack.levels.is_empty());
    assert!(matches!(mode, Some(InputMode::Menu(_))));
    assert_eq!(editor.requests, vec![ArrangeRequest::Leave]);
}

#[test]
fn the_session_leaving_layout_an_event_and_a_new_house_each_end_the_editor() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair]);
    let mode = editor.update(&frame(MYROOM_MODE_NORMAL, vec![chair]));
    assert!(matches!(mode, Some(InputMode::Menu(_))), "{mode:?}");
    assert!(!editor.active() && editor.requests.is_empty());
    assert!(
        editor
            .update(&frame(MYROOM_MODE_NORMAL, vec![chair]))
            .is_none(),
        "no re-entry while the session stays in normal play"
    );

    let (mut editor, _) = entered(vec![chair]);
    let mut event = frame(MYROOM_MODE_LAYOUT, vec![chair]);
    event.busy = true;
    assert!(matches!(editor.update(&event), Some(InputMode::World)));
    assert_eq!(editor.requests, vec![ArrangeRequest::Leave]);

    let (mut editor, _) = entered(vec![chair]);
    let mut zoned = frame(MYROOM_MODE_LAYOUT, vec![chair]);
    zoned.house = Some(house(FLOOR_2F, ZONE + 1));
    assert!(matches!(editor.update(&zoned), Some(InputMode::World)));
    assert!(
        editor.requests.is_empty(),
        "the zone change already ended it"
    );
}

#[test]
fn the_view_cycles_with_a_selection_and_survives_leaving_within_the_zone() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair]);
    assert_eq!(editor.view(), CameraView::TopDown);
    tap(&mut editor, ArrangeKey::CycleView);
    assert_eq!(editor.view(), CameraView::FromSouth);

    editor.update(&frame(MYROOM_MODE_NORMAL, vec![chair]));
    editor.update(&frame(MYROOM_MODE_LAYOUT, vec![chair]));
    assert!(editor.active());
    assert_eq!(editor.view(), CameraView::FromSouth, "same zone");

    editor.update(&frame(MYROOM_MODE_NORMAL, vec![chair]));
    let mut next_zone = frame(MYROOM_MODE_LAYOUT, vec![chair]);
    next_zone.house = Some(house(FLOOR_2F, ZONE + 1));
    editor.update(&next_zone);
    assert_eq!(editor.view(), CameraView::TopDown, "another zone");

    let (mut empty, _) = entered(Vec::new());
    tap(&mut empty, ArrangeKey::CycleView);
    assert_eq!(empty.view(), CameraView::TopDown, "no selection");
}

#[test]
fn arrows_follow_the_selection_rule_with_the_cursor_sound_and_flip_from_the_north() {
    let middle = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let east = piece(LOC_MOGSAFE, 2, chair(), pose(8, 4, 0));
    let west = piece(LOC_MOGSAFE, 3, chair(), pose(0, 4, 0));
    let (mut editor, _) = entered(vec![middle, east, west]);
    let x = |editor: &ArrangeEditor, drawn: &DrawnPiece| {
        editor.room().unwrap().point(drawn.seat).unwrap()[0]
    };
    let (right, left) = if x(&editor, &east) > x(&editor, &middle) {
        (east.seat, west.seat)
    } else {
        (west.seat, east.seat)
    };

    let (_, sounds) = tap(&mut editor, ArrangeKey::Arrow(Press::Right));
    assert_eq!(sounds, vec![UiSound::Cursor]);
    assert_eq!(editor.selection(), Some(right));
    assert_eq!(editor.tint_of(right), first_pulse(), "the pulse restarts");
    assert_eq!(editor.tint_of(middle.seat), TINT_NORMAL);
    tap(&mut editor, ArrangeKey::Arrow(Press::Left));
    assert_eq!(editor.selection(), Some(middle.seat));

    let (_, sounds) = tap(&mut editor, ArrangeKey::Arrow(Press::Up));
    assert_eq!(
        sounds,
        vec![UiSound::Cursor],
        "a press that finds nothing still sounds"
    );
    assert_eq!(editor.selection(), Some(middle.seat));

    tap(&mut editor, ArrangeKey::CycleView);
    tap(&mut editor, ArrangeKey::CycleView);
    assert_eq!(editor.view(), CameraView::FromNorth);
    tap(&mut editor, ArrangeKey::Arrow(Press::Right));
    assert_eq!(editor.selection(), Some(left), "both signs flip in view 2");
}

#[test]
fn held_arrows_combine_releases_anywhere_clear_them_and_modifiers_mute_every_action() {
    let mut editor = editor();
    editor.track(ArrangeKey::Arrow(Press::Right), KeyPhase::Press);
    editor.track(ArrangeKey::Arrow(Press::Up), KeyPhase::Repeat);
    let (rx, rz) = Press::Right.delta();
    let (ux, uz) = Press::Up.delta();
    assert_eq!(editor.held_delta(), (rx + ux, rz + uz));
    editor.track(ArrangeKey::Arrow(Press::Right), KeyPhase::Release);
    assert_eq!(editor.held_delta(), (ux, uz));
    editor.release_keys();
    assert_eq!(editor.held_delta(), (0, 0));

    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair]);
    let mut sounds = Vec::new();
    editor.key(ArrangeKey::Ctrl, KeyPhase::Press, false, &mut sounds);
    assert!(editor
        .key(ArrangeKey::Confirm, KeyPhase::Press, false, &mut sounds)
        .is_none());
    assert!(sounds.is_empty(), "nothing answers with Ctrl held");
    editor.track(ArrangeKey::Ctrl, KeyPhase::Release);
    let (mode, sounds) = tap(&mut editor, ArrangeKey::Confirm);
    assert_eq!(sounds, vec![UiSound::Confirm]);
    assert!(matches!(
        top(&menu(mode)),
        Some(ArrangeWindow::Piece {
            mannequin: false,
            ..
        })
    ));

    let mut sounds = Vec::new();
    assert!(
        editor
            .key(ArrangeKey::Confirm, KeyPhase::Repeat, false, &mut sounds)
            .is_none()
            && sounds.is_empty(),
        "Enter is single press"
    );
}

#[test]
fn keys_and_phases_map_from_the_default_keyboard() {
    assert_eq!(
        arrange_key_of(KeyCode::ArrowLeft),
        Some(ArrangeKey::Arrow(Press::Left))
    );
    assert_eq!(
        arrange_key_of(KeyCode::NumpadEnter),
        Some(ArrangeKey::Confirm)
    );
    assert_eq!(arrange_key_of(KeyCode::Escape), Some(ArrangeKey::Cancel));
    assert_eq!(
        arrange_key_of(KeyCode::Numpad5),
        Some(ArrangeKey::CycleView)
    );
    assert_eq!(
        arrange_key_of(KeyCode::NumpadSubtract),
        Some(ArrangeKey::Rotate)
    );
    assert_eq!(arrange_key_of(KeyCode::AltRight), Some(ArrangeKey::Alt));
    assert_eq!(arrange_key_of(KeyCode::Minus), None);
    assert_eq!(KeyPhase::of(true, false), KeyPhase::Press);
    assert_eq!(KeyPhase::of(true, true), KeyPhase::Repeat);
    assert_eq!(KeyPhase::of(false, true), KeyPhase::Release);
}

#[test]
fn enter_opens_rmlo2_or_rmlo4_with_put_away_greyed_by_storage() {
    let dummy = of_kind(
        piece(LOC_MOGSAFE, 2, chair(), pose(8, 4, 0)),
        KIND_MANNEQUIN,
    );
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair, dummy]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    assert_eq!(
        top(&menu(mode)),
        Some(ArrangeWindow::Piece {
            mannequin: false,
            put_away: true,
            put_away_all: true,
        })
    );

    editor.select(Some(dummy.seat));
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    assert!(matches!(
        top(&menu(mode)),
        Some(ArrangeWindow::Piece {
            mannequin: true,
            ..
        })
    ));

    let mut full = frame(MYROOM_MODE_LAYOUT, vec![chair]);
    full.storage = StorageGauge {
        free: 0,
        entries: 2,
    };
    let (mut editor, _) = entered_with(full);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let window = top(&stack).unwrap();
    assert!(window.item_disabled(RMLO2_PUT_AWAY) && window.item_disabled(RMLO2_PUT_AWAY_ALL));
    assert!(!window.item_disabled(RMLO2_MOVE) && !window.item_disabled(RMLO2_PLACE));
    let (mode, sounds) = choose(&mut editor, &mut stack, RMLO2_PUT_AWAY);
    assert!(mode.is_none() && sounds.is_empty() && editor.requests.is_empty());
    assert_eq!(top(&stack), Some(window), "a grey item does nothing");
}

/// Enter with nothing selected selects the first piece silently; with nothing placed it opens
/// `rmlo1` with the confirm sound
/// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B4).
#[test]
fn enter_without_a_selection_selects_the_first_piece_silently() {
    let first = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let second = piece(LOC_MOGSAFE, 2, chair(), pose(8, 4, 0));
    let (mut editor, _) = entered(vec![first, second]);
    editor.select(None);
    let (mode, sounds) = tap(&mut editor, ArrangeKey::Confirm);
    assert!(mode.is_none(), "{mode:?}");
    assert_eq!(sounds, Vec::<UiSound>::new());
    assert_eq!(editor.selection(), Some(first.seat));

    let (mut empty, mode) = entered(Vec::new());
    let mut stack = menu(mode);
    empty.cancel_window(&mut stack, &mut Vec::new());
    let (mode, sounds) = tap(&mut empty, ArrangeKey::Confirm);
    assert_eq!(sounds, vec![UiSound::Confirm]);
    assert!(matches!(
        top(&menu(mode)),
        Some(ArrangeWindow::Empty { .. })
    ));
}

fn table_room() -> (DrawnPiece, DrawnPiece, DrawnPiece) {
    (
        piece(LOC_MOGSAFE, 1, table(), pose(4, 4, 0)),
        piece(LOC_MOGSAFE, 2, vase(), pose(5, 4, 10)),
        piece(LOC_MOGSAFE, 3, chair(), pose(12, 12, 0)),
    )
}

fn start_moving(editor: &mut ArrangeEditor) {
    let (mode, _) = tap(editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let (mode, _) = choose(editor, &mut stack, RMLO2_MOVE);
    assert!(is_arrange(&mode) && stack.levels.is_empty());
}

#[test]
fn moving_dims_the_room_steps_turns_and_places_one_packet_per_piece_in_seat_order() {
    let (table, vase, chair) = table_room();
    let (mut editor, _) = entered(vec![chair, vase, table]);
    assert_eq!(editor.selection(), Some(table.seat));
    start_moving(&mut editor);
    assert_eq!(editor.moving_seat(), Some(table.seat));
    assert_eq!(editor.tint_of(chair.seat), TINT_DIMMED);
    assert_eq!(editor.tint_of(table.seat), first_pulse());
    assert_eq!(
        editor.tint_of(vase.seat),
        first_pulse(),
        "what it carries pulses"
    );
    assert!(!editor.overlay().is_empty());

    tap(&mut editor, ArrangeKey::Arrow(Press::Right));
    assert_eq!(editor.furniture_sounds, vec![FurnitureSound::Step]);
    assert_eq!(editor.pose_of(table.seat), Some(pose(5, 4, 0)));
    assert_eq!(
        editor.pose_of(vase.seat),
        Some(pose(6, 4, 10)),
        "carried along"
    );
    tap(&mut editor, ArrangeKey::Rotate);
    assert_eq!(
        editor.furniture_sounds,
        vec![FurnitureSound::Step, FurnitureSound::Rotate]
    );
    assert_eq!(editor.pose_of(table.seat).unwrap().rot, 3, "rot - 1");
    assert!(editor.requests.is_empty(), "no packet per step or turn");

    let (mode, sounds) = tap(&mut editor, ArrangeKey::Confirm);
    assert!(mode.is_none() && sounds.is_empty());
    assert_eq!(
        editor.requests,
        vec![layout_of(&editor, &table), layout_of(&editor, &vase)]
    );
    assert_eq!(
        editor.furniture_sounds.last(),
        Some(&FurnitureSound::Placed)
    );
    assert_eq!(editor.moving_seat(), None);
    assert_eq!(editor.tint_of(chair.seat), TINT_NORMAL);
    assert!(editor.overlay().is_empty());
}

#[test]
fn a_refused_place_buzzes_and_esc_puts_the_piece_back() {
    let left = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let right = piece(LOC_MOGSAFE, 2, chair(), pose(5, 4, 0));
    let (mut editor, _) = entered(vec![left, right]);
    start_moving(&mut editor);
    tap(&mut editor, ArrangeKey::Arrow(Press::Right));
    assert_eq!(
        editor.pose_of(left.seat),
        Some(pose(5, 4, 0)),
        "moving ignores overlaps"
    );
    let (_, sounds) = tap(&mut editor, ArrangeKey::Confirm);
    assert_eq!(sounds, vec![UiSound::Buzzer]);
    assert_eq!(editor.moving_seat(), Some(left.seat), "still moving");
    assert!(editor.requests.is_empty());

    let (_, sounds) = tap(&mut editor, ArrangeKey::Cancel);
    assert_eq!(sounds, vec![UiSound::Cancel]);
    assert_eq!(editor.moving_seat(), None);
    assert_eq!(
        editor.pose_of(left.seat),
        Some(left.pose),
        "its record's pose"
    );
    assert!(editor.requests.is_empty() && editor.active());
}

#[test]
fn layoutctrl_type1_reverses_steps_while_the_eye_looks_from_the_north() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair]);
    tap(&mut editor, ArrangeKey::CycleView);
    tap(&mut editor, ArrangeKey::CycleView);
    let start = || CameraRig {
        eye: [0.0; 3],
        focus: [0.0; 3],
    };
    let rig = editor.camera_update(1, start).unwrap();
    assert!(rig.reverses_type1_presses());
    start_moving(&mut editor);
    let mut sounds = Vec::new();
    editor.key(
        ArrangeKey::Arrow(Press::Up),
        KeyPhase::Press,
        true,
        &mut sounds,
    );
    assert_eq!(
        editor.pose_of(chair.seat),
        Some(pose(4, 3, 0)),
        "type1: Up is row - 1"
    );
    editor.key(
        ArrangeKey::Arrow(Press::Up),
        KeyPhase::Repeat,
        false,
        &mut sounds,
    );
    assert_eq!(
        editor.pose_of(chair.seat),
        Some(pose(4, 4, 0)),
        "type2 never reverses"
    );

    let mut dynamic = DynamicMenu::default();
    assert!(layout_type1(&dynamic), "unset reads as type1");
    dynamic.config_option_values.insert(
        CONFIG_OPTION_LAYOUT_CONTROL_TYPE,
        crate::hud::menu::LAYOUT_CONTROL_TYPE2,
    );
    assert!(!layout_type1(&dynamic));
}

fn shelf_entry(container: u8, slot: u8, kind: u8, installed: bool, layer: u8) -> ShelfEntry {
    ShelfEntry {
        container,
        slot,
        item_no: ITEM_BASE + u16::from(slot),
        kind,
        storage: 1,
        attr: FurnitureAttr {
            installed,
            layer,
            pose: FurniturePose::default(),
            flowerpot_step: 0,
            flowerpot_kind: 0,
            mannequin_race: 0,
            mannequin_pose: 0,
        },
        property: Some(chair()),
    }
}

fn open_shelf(editor: &mut ArrangeEditor) -> MenuStack {
    let (mode, _) = tap(editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let (mode, sounds) = choose(editor, &mut stack, RMLO2_PLACE);
    assert!(mode.is_none() && sounds == vec![UiSound::Confirm]);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Shelf { second_shelf: true })
    );
    let (mode, _) = choose(editor, &mut stack, BANKMENU_SHELF);
    assert!(mode.is_none());
    assert_eq!(stack.levels.len(), 1, "the rmlo window closes");
    stack
}

#[test]
fn the_furniture_list_tints_this_floors_pieces_and_a_pick_starts_a_new_piece_moving() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let stool = piece(LOC_MOGSAFE, 2, self::chair(), pose(12, 12, 0));
    let mut room = frame(MYROOM_MODE_LAYOUT, vec![chair, stool]);
    let new = shelf_entry(LOC_MOGSAFE, 9, KIND_FURNISHING, false, FLOOR_1F);
    room.shelf = vec![
        new,
        shelf_entry(
            LOC_MOGSAFE,
            chair.info.slot,
            KIND_FURNISHING,
            true,
            FLOOR_2F,
        ),
        shelf_entry(LOC_MOGSAFE, 4, KIND_FURNISHING, true, FLOOR_1F),
        shelf_entry(LOC_MOGSAFE, 6, KIND_FLOWERPOT, false, FLOOR_1F),
        shelf_entry(LOC_MOGSAFE2, 7, KIND_FURNISHING, false, FLOOR_1F),
    ];
    let (mut editor, _) = entered_with(room);
    let mut stack = open_shelf(&mut editor);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Furniture {
            container: LOC_MOGSAFE
        })
    );
    let rows = editor.rows(ArrangeWindow::Furniture {
        container: LOC_MOGSAFE,
    });
    let listed: Vec<(ArrangeAction, Option<MenuRowTint>)> = rows
        .iter()
        .map(|row| match row.action {
            DynamicMenuAction::Arrange(action) => (action, row.tint),
            other => panic!("{other:?}"),
        })
        .collect();
    let furniture = |entry: &ShelfEntry| ArrangeAction::Furniture {
        container: entry.container,
        slot: entry.slot,
        item_no: entry.item_no,
    };
    assert_eq!(
        listed,
        vec![
            (
                furniture(&shelf_entry(
                    LOC_MOGSAFE,
                    chair.info.slot,
                    KIND_FURNISHING,
                    true,
                    FLOOR_2F
                )),
                Some(MenuRowTint::FurniturePlaced)
            ),
            (furniture(&new), None),
        ],
        "slot order; other floor, upstairs pots and the other shelf left out"
    );

    let mut sounds = Vec::new();
    let mode = editor.select_row(
        top(&stack).unwrap(),
        Some(DynamicMenuAction::Arrange(furniture(&new))),
        &mut stack,
        &mut sounds,
    );
    assert!(is_arrange(&mode) && stack.levels.is_empty());
    assert_eq!(sounds, vec![UiSound::Confirm]);
    let seat = law::seat_index(LOC_MOGSAFE, new.slot);
    assert_eq!(editor.moving_seat(), Some(seat));
    assert_eq!(editor.spawns.len(), 1);
    assert_eq!(editor.spawns[0].pose, editor.pose_of(seat).unwrap());
    assert_eq!(
        editor.pose_of(seat),
        Some(pose(4, 4, 0)),
        "from the selected piece"
    );

    tap(&mut editor, ArrangeKey::Cancel);
    assert_eq!(editor.despawns, vec![seat], "discarded");
    assert_eq!(editor.selection(), Some(chair.seat));
    assert_eq!(editor.tint_of(chair.seat), first_pulse());
    assert_eq!(
        editor.tint_of(stool.seat),
        TINT_DIMMED,
        "dimmed until the selection changes"
    );
    assert!(editor.pose_of(seat).is_none());
    editor.select(Some(stool.seat));
    assert_eq!(editor.tint_of(chair.seat), TINT_NORMAL);
}

#[test]
fn picks_over_the_pot_and_mannequin_limits_print_their_system_messages() {
    let pots: Vec<DrawnPiece> = (1..=10)
        .map(|slot| {
            of_kind(
                piece(LOC_MOGSAFE, slot, chair(), pose(slot as i8, 0, 0)),
                KIND_FLOWERPOT,
            )
        })
        .collect();
    let mut room = frame(MYROOM_MODE_LAYOUT, pots);
    room.house = Some(house(FLOOR_1F, ZONE));
    let pot = shelf_entry(LOC_MOGSAFE, 20, KIND_FLOWERPOT, false, FLOOR_1F);
    room.shelf = vec![pot];
    let (mut editor, _) = entered_with(room);
    let mut stack = open_shelf(&mut editor);
    let mut sounds = Vec::new();
    let action = ArrangeAction::Furniture {
        container: pot.container,
        slot: pot.slot,
        item_no: pot.item_no,
    };
    let mode = editor.select_row(
        top(&stack).unwrap(),
        Some(DynamicMenuAction::Arrange(action)),
        &mut stack,
        &mut sounds,
    );
    assert!(mode.is_none() && sounds.is_empty() && editor.moving_seat().is_none());
    assert_eq!(
        editor.chat_lines.last().map(String::as_str),
        Some(POT_LIMIT)
    );

    let dummies: Vec<DrawnPiece> = (1..=8)
        .map(|slot| {
            of_kind(
                piece(LOC_MOGSAFE, slot, chair(), pose(slot as i8, 0, 0)),
                KIND_MANNEQUIN,
            )
        })
        .collect();
    let mut room = frame(MYROOM_MODE_LAYOUT, dummies);
    let dummy = shelf_entry(LOC_MOGSAFE, 20, KIND_MANNEQUIN, false, FLOOR_1F);
    room.shelf = vec![dummy];
    let (mut editor, _) = entered_with(room);
    editor.select(Some(law::seat_index(LOC_MOGSAFE, 1)));
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    choose(&mut editor, &mut stack, RMLO4_PLACE);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Shelf { second_shelf: true })
    );
    assert_eq!(stack.levels.len(), 1, "rmlo4 closes at once");
    choose(&mut editor, &mut stack, BANKMENU_SHELF);
    let action = ArrangeAction::Furniture {
        container: dummy.container,
        slot: dummy.slot,
        item_no: dummy.item_no,
    };
    let mut sounds = Vec::new();
    editor.select_row(
        top(&stack).unwrap(),
        Some(DynamicMenuAction::Arrange(action)),
        &mut stack,
        &mut sounds,
    );
    assert_eq!(
        editor.chat_lines.last().map(String::as_str),
        Some(MANNEQUIN_LIMIT)
    );
}

#[test]
fn put_away_sends_one_bankin_per_piece_after_asking_about_a_planted_pot() {
    let (table, vase, chair) = table_room();
    let vase = planted(vase);
    let (mut editor, _) = entered(vec![table, vase, chair]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let (mode, sounds) = choose(&mut editor, &mut stack, RMLO2_PUT_AWAY);
    assert!(mode.is_none() && sounds == vec![UiSound::Confirm]);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Confirm(ArrangeConfirm::Planted {
            keep_window: false
        }))
    );
    assert_eq!(
        stack.current().unwrap().cursor,
        CONFIRM_NO_ROW,
        "opens on No"
    );
    assert_eq!(editor.window_help(top(&stack).unwrap()), PLANTED_PROMPT);
    assert!(editor.requests.is_empty(), "nothing until Yes");
    let (mode, _) = choose(&mut editor, &mut stack, CONFIRM_ITEM_YES);
    assert!(is_arrange(&mode) && stack.levels.is_empty());
    assert_eq!(editor.requests, vec![bankin_of(&table), bankin_of(&vase)]);

    let (mut editor, _) = entered(vec![chair]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let (mode, _) = choose(&mut editor, &mut stack, RMLO2_PUT_AWAY);
    assert!(is_arrange(&mode), "rmlo2 closes");
    assert_eq!(editor.requests, vec![bankin_of(&chair)]);

    let dummy = of_kind(chair, KIND_MANNEQUIN);
    let (mut editor, _) = entered(vec![dummy]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let (mode, _) = choose(&mut editor, &mut stack, RMLO4_PUT_AWAY);
    assert!(mode.is_none(), "rmlo4 stays open");
    assert!(matches!(
        top(&stack),
        Some(ArrangeWindow::Piece {
            mannequin: true,
            ..
        })
    ));
    assert_eq!(editor.requests, vec![bankin_of(&dummy)]);
}

#[test]
fn put_away_all_asks_twice_with_a_planted_pot_and_flags_every_piece() {
    let (table, vase, chair) = table_room();
    let vase = planted(vase);
    let (mut editor, _) = entered(vec![table, vase, chair]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    choose(&mut editor, &mut stack, RMLO2_PUT_AWAY_ALL);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Confirm(ArrangeConfirm::All))
    );
    assert_eq!(editor.window_help(top(&stack).unwrap()), ALL_PROMPT);
    let (mode, _) = choose(
        &mut editor,
        &mut stack,
        crate::hud::unity_fp::CONFIRM_ITEM_NO,
    );
    assert!(mode.is_none());
    assert!(
        matches!(top(&stack), Some(ArrangeWindow::Piece { .. })),
        "No goes back"
    );

    choose(&mut editor, &mut stack, RMLO2_PUT_AWAY_ALL);
    choose(&mut editor, &mut stack, CONFIRM_ITEM_YES);
    assert_eq!(
        top(&stack),
        Some(ArrangeWindow::Confirm(ArrangeConfirm::PlantedAll))
    );
    assert!(editor.requests.is_empty());
    let (mode, _) = choose(&mut editor, &mut stack, CONFIRM_ITEM_YES);
    assert!(is_arrange(&mode) && stack.levels.is_empty());
    assert_eq!(
        editor.requests,
        vec![bankin_of(&table), bankin_of(&vase), bankin_of(&chair)]
    );
    let occupancy = editor.room().unwrap().occupancy();
    assert_eq!(
        occupancy.cell(i16::from(table.pose.col), i16::from(table.pose.row)),
        Some(arrange::Cell::Free),
        "the grid is cleared"
    );
}

#[test]
fn a_removed_piece_hands_the_selection_to_the_nearest_or_clears_it() {
    let (table, vase, chair) = table_room();
    let (mut editor, _) = entered(vec![table, vase, chair]);
    assert_eq!(editor.selection(), Some(table.seat));
    editor.update(&frame(MYROOM_MODE_LAYOUT, vec![vase, chair]));
    assert_eq!(
        editor.selection(),
        Some(vase.seat),
        "nearest the removed selection"
    );
    editor.update(&frame(MYROOM_MODE_LAYOUT, vec![vase]));
    assert_eq!(
        editor.selection(),
        None,
        "nearest the selection is the selection itself"
    );
}

#[test]
fn windows_remember_their_cursor_and_esc_closes_one_window_at_a_time() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let (mut editor, _) = entered(vec![chair]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    let remembered = usize::from(RMLO2_PLACE - 1);
    stack.current_mut().unwrap().cursor = remembered;
    let mut sounds = Vec::new();
    let mode = editor.cancel_window(&mut stack, &mut sounds);
    assert!(is_arrange(&mode) && sounds == vec![UiSound::Cancel]);
    let (mode, _) = tap(&mut editor, ArrangeKey::Confirm);
    let mut stack = menu(mode);
    assert_eq!(stack.current().unwrap().cursor, remembered);

    choose(&mut editor, &mut stack, RMLO2_PLACE);
    assert_eq!(
        stack.current().unwrap().cursor,
        0,
        "bankmenu opens on item 1"
    );
    let mut sounds = Vec::new();
    assert!(editor.cancel_window(&mut stack, &mut sounds).is_none());
    assert!(
        matches!(top(&stack), Some(ArrangeWindow::Piece { .. })),
        "back to rmlo2"
    );
    choose(&mut editor, &mut stack, RMLO2_PLACE);
    choose(&mut editor, &mut stack, BANKMENU_SHELF);
    let mode = editor.cancel_window(&mut stack, &mut sounds);
    assert!(
        is_arrange(&mode) && stack.levels.is_empty(),
        "the list closes to browse"
    );
}

#[test]
fn rows_carry_the_window_records_item_captions_in_item_order() {
    let mut editor = editor();
    for item in 1..=RMLO2_PUT_AWAY_ALL {
        editor
            .texts
            .item_labels
            .insert((RMLO2_WINDOW, item), format!("item {item}"));
    }
    let window = ArrangeWindow::Piece {
        mannequin: false,
        put_away: true,
        put_away_all: true,
    };
    let rows = editor.rows(window);
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, vec!["item 1", "item 2", "item 3", "item 4"]);
    assert_eq!(
        rows[usize::from(RMLO2_PLACE - 1)].action,
        DynamicMenuAction::Arrange(ArrangeAction::Item { item: RMLO2_PLACE })
    );
    assert_eq!(window.record_name(), RMLO2_WINDOW);
    assert_eq!(editor.window_help(window), HELP);
    assert_eq!(
        editor
            .rows(ArrangeWindow::Confirm(ArrangeConfirm::All))
            .len(),
        usize::from(crate::hud::unity_fp::CONFIRM_ITEM_NO)
    );
}

#[test]
fn the_arrange_modes_are_layout_mode_and_its_window_stacks() {
    let windows = ArrangeEditor::default().open(ArrangeWindow::Shelf {
        second_shelf: false,
    });
    assert!(arranging(&windows) && arranging(&InputMode::Arrange));
    assert!(!arranging(&InputMode::World));
    assert!(!arranging(&InputMode::Menu(MenuStack::root())));

    let InputMode::Menu(stack) = myroom_menu() else {
        panic!("myroom reopens as a menu");
    };
    let kinds: Vec<MenuKind> = stack.levels.iter().map(|level| level.kind).collect();
    assert_eq!(kinds, vec![MenuKind::HomeView]);
}

/// Every way out of layout mode reopens the Mog House menu alone, on the row the session remembers
/// for it, and leaves the main menu's remembered row as it was
/// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B1).
#[test]
fn every_exit_reopens_only_the_mog_house_menu_on_its_remembered_row() {
    const MYROOM_ROW: usize = 2;
    const MAIN_MENU_ROW: usize = 3;
    let myroom = crate::hud::menu::registry_window(MenuKind::HomeView).unwrap();
    let main_menu = crate::hud::menu::registry_window(MenuKind::Root).unwrap();
    let mut memory = CursorMemory::default();
    memory.record(myroom, Some(MYROOM_ROW));
    memory.record(main_menu, Some(MAIN_MENU_ROW));
    let check = |mode: Option<InputMode>, exit: &str| {
        let mut stack = menu(mode);
        let kinds: Vec<MenuKind> = stack.levels.iter().map(|level| level.kind).collect();
        assert_eq!(kinds, vec![MenuKind::HomeView], "{exit}");
        assert_eq!(stack.current().unwrap().cursor, MYROOM_ROW, "{exit}");
        stack.close_all();
        let mut after = memory.clone();
        after.absorb(&stack.cursor_memory);
        assert_eq!(after.recall(main_menu), Some(MAIN_MENU_ROW), "{exit}");
    };
    let with_memory = |arrange: ArrangeEditor| DynamicMenu {
        arrange,
        window_cursors: memory.clone(),
        ..DynamicMenu::default()
    };
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));

    let mut dynamic = with_memory(entered(vec![chair]).0);
    check(
        arrange_key(&mut dynamic, ArrangeKey::Cancel, KeyPhase::Press),
        "Esc",
    );

    let (editor, mode) = entered(Vec::new());
    let mut dynamic = with_memory(editor);
    let mut stack = menu(mode);
    dynamic.rows = dynamic.arrange.rows(top(&stack).unwrap());
    stack.current_mut().unwrap().cursor = usize::from(RMLO1_FINISH - 1);
    check(select_arrange_row(&mut stack, &mut dynamic), "rmlo1 Finish");

    let (mut editor, _) = entered(vec![chair]);
    let next = editor.update(&frame(MYROOM_MODE_NORMAL, vec![chair]));
    check(
        with_session_memory(next, &memory),
        "the session left layout",
    );
}

#[test]
fn the_dynamic_menu_seams_route_to_the_editor() {
    let chair = piece(LOC_MOGSAFE, 1, chair(), pose(4, 4, 0));
    let mut dynamic = DynamicMenu {
        arrange: entered(vec![chair]).0,
        ..DynamicMenu::default()
    };
    let mode = arrange_key(&mut dynamic, ArrangeKey::Confirm, KeyPhase::Press);
    assert_eq!(dynamic.ui_sounds, vec![UiSound::Confirm]);
    let mut stack = menu(mode);
    let window = top(&stack).unwrap();
    dynamic.rows = dynamic.arrange.rows(window);
    stack.current_mut().unwrap().cursor = usize::from(RMLO2_MOVE - 1);
    let mode = select_arrange_row(&mut stack, &mut dynamic);
    assert!(is_arrange(&mode));
    assert_eq!(dynamic.arrange.moving_seat(), Some(chair.seat));

    let mut other = MenuStack::root();
    assert!(cancel_arrange_window(&mut other, &mut dynamic).is_none());
    assert_eq!(other.levels.len(), 1, "not an arrange window: untouched");
}
