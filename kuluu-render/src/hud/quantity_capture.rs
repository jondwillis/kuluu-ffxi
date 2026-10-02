use super::{delivery, dialog, item_dat_root, item_detail, item_screen};
use crate::input_mode::{DialogCursor, InputMode, MenuKind, MenuStack};
use crate::snapshot::SceneState;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};
use kuluu_snapshot::{
    ContainerView, DeliveryBoxNo, DeliveryBoxState, DeliverySlot, DialogGrid, DialogGridCell,
    DialogState, InventoryItem,
};
use std::sync::Arc;

const CAPTURE_WIDTH: u32 = 800;
const CAPTURE_HEIGHT: u32 = 600;
const SETTLE_FRAMES: usize = 50;
const MAX_CAPTURE_FRAMES: usize = 180;
const FRAME_WAIT: std::time::Duration = std::time::Duration::from_millis(16);
const MULTI_COUNT: u32 = 12;
const CAPTURE_BACKGROUND: Color = Color::srgb(0.28, 0.23, 0.18);
const RGBA_PIXEL_BYTES: usize = 4;

#[test]
#[ignore = "requires an installed client and GPU; writes production HUD captures"]
fn quantity_overlay_production_capture() {
    let root = ffxi_dat::install::named("retail").expect("registered retail install");
    let dat = ffxi_dat::DatRoot::open(root).expect("retail DAT root");
    println!("capture DAT profile: {:?}", dat.profile());
    let output = std::env::var("KULUU_CAPTURE_DIR").expect("KULUU_CAPTURE_DIR");
    std::fs::create_dir_all(&output).unwrap();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
    );
    app.insert_resource(ClearColor(CAPTURE_BACKGROUND))
        .insert_resource(item_dat_root::ItemDatRoot(Some(Arc::new(dat))))
        .init_resource::<item_dat_root::ItemIconCache>()
        .init_resource::<SceneState>()
        .init_resource::<InputMode>()
        .init_resource::<delivery::DeliveryScreenState>()
        .init_resource::<delivery::DeliveryInventory>()
        .init_resource::<item_detail::ItemMenuFocus>()
        .init_resource::<item_detail::SortOptions>()
        .init_resource::<item_screen::ItemScreenContainer>()
        .init_resource::<item_screen::ItemListViewport>()
        .init_resource::<crate::keybinds::Bindings>()
        .add_systems(
            Startup,
            (
                delivery::spawn_delivery_screen,
                dialog::spawn_dialog_panel,
                item_screen::spawn_item_screen,
            ),
        )
        .add_systems(
            Update,
            (
                delivery::update_delivery_screen,
                dialog::update_dialog_panel_system,
                dialog::update_dialog_grid_system,
                item_screen::update_item_screen_layout,
                item_screen::update_item_screen,
            )
                .chain(),
        );
    app.finish();
    app.cleanup();
    let mut image = Image::new_target_texture(
        CAPTURE_WIDTH,
        CAPTURE_HEIGHT,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera2d,
        IsDefaultUiCamera,
        bevy::camera::RenderTarget::Image(target.clone().into()),
    ));
    let lizard_egg = item_id("Lizard Egg");
    let bat_wing = item_id("Bat Wing");
    let cases = [
        (Some(lizard_egg), MULTI_COUNT),
        (Some(bat_wing), MULTI_COUNT),
        (Some(lizard_egg), 1),
        (None, 0),
    ];
    app.world_mut()
        .resource_mut::<delivery::DeliveryScreenState>()
        .open(DeliveryBoxNo::Incoming);
    app.world_mut()
        .resource_mut::<SceneState>()
        .snapshot
        .delivery_box = Some(DeliveryBoxState {
        box_no: DeliveryBoxNo::Incoming,
        slots: cases
            .iter()
            .map(|&(item, quantity)| {
                item.map(|item_no| DeliverySlot {
                    item_no,
                    quantity,
                    counterpart: Some("Fixture sender".into()),
                    ..default()
                })
            })
            .chain(std::iter::repeat_n(
                None,
                delivery::GRID_SLOTS - cases.len(),
            ))
            .collect(),
        ..default()
    });
    capture(&mut app, &target, &format!("{output}/delivery.png"));
    app.world_mut()
        .resource_mut::<SceneState>()
        .snapshot
        .delivery_box = None;
    app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(DialogState {
        npc_name: Some("Production dialog grid fixture".into()),
        prompt: Some("Bright / dark / single / empty".into()),
        grid: Some(DialogGrid {
            cols: delivery::GRID_COLS as u8,
            rows: 1,
            cells: cases
                .iter()
                .map(|&(item_no, quantity)| DialogGridCell {
                    item_no,
                    quantity,
                    ..default()
                })
                .collect(),
        }),
        ..default()
    });
    *app.world_mut().resource_mut::<InputMode>() = InputMode::Dialog(DialogCursor::default());
    capture(&mut app, &target, &format!("{output}/dialog.png"));
    app.world_mut().resource_mut::<SceneState>().snapshot.dialog = None;
    let inventory = ffxi_proto::map::container::LOC_INVENTORY;
    app.world_mut()
        .resource_mut::<SceneState>()
        .snapshot
        .containers = vec![ContainerView {
        id: inventory,
        capacity: cases.len() as u16,
        items: cases
            .iter()
            .enumerate()
            .filter_map(|(index, &(item, quantity))| {
                item.map(|item_no| InventoryItem {
                    container: inventory,
                    index: index as u8 + 1,
                    item_no,
                    quantity,
                    locked: false,
                    unselectable: false,
                    charges_remaining: None,
                    next_use_vana_ts: None,
                    use_delay_end_vana_ts: None,
                    ready: None,
                })
            })
            .collect(),
    }];
    let mut menu = MenuStack::root();
    menu.push(MenuKind::Items);
    *app.world_mut().resource_mut::<InputMode>() = InputMode::Menu(menu);
    capture(&mut app, &target, &format!("{output}/items.png"));
}

fn item_id(name: &str) -> u16 {
    ffxi_vocab::item_names::ITEM_NAMES
        .iter()
        .find(|(_, candidate)| candidate.eq_ignore_ascii_case(name))
        .map(|&(id, _)| id)
        .expect("fixture item in scraped vocabulary")
}

fn capture(app: &mut App, target: &Handle<Image>, path: &str) {
    if std::path::Path::new(path).exists() {
        std::fs::remove_file(path).unwrap();
    }
    for _ in 0..SETTLE_FRAMES {
        app.update();
        std::thread::sleep(FRAME_WAIT);
    }
    app.world_mut()
        .spawn(Screenshot::image(target.clone()))
        .observe(|capture: On<ScreenshotCaptured>| {
            let pixels = capture.image.data.as_ref().expect("captured pixel data");
            let background = &pixels[..RGBA_PIXEL_BYTES];
            assert!(
                pixels
                    .chunks_exact(RGBA_PIXEL_BYTES)
                    .any(|pixel| pixel != background),
                "production HUD capture contains only the background"
            );
        })
        .observe(save_to_disk(std::path::PathBuf::from(path)));
    for _ in 0..MAX_CAPTURE_FRAMES {
        app.update();
        std::thread::sleep(FRAME_WAIT);
        if std::path::Path::new(path).exists() {
            println!("production HUD capture: {path}");
            return;
        }
    }
    panic!("capture did not finish: {path}");
}
