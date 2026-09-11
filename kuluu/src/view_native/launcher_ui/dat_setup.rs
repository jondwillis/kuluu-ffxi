use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui_widgets::{Activate, ValueChange};

use std::path::{Path, PathBuf};

use crate::ffxi_client::{self, Install, SetupOptions};
use crate::launcher_store::{self, EnvOverride};
use ffxi_dat::install_detect;

use super::client_job::{self, ClientJob, JobBarFill, JobDetailText, JobKind, JobPhaseText};
use super::common::{hint, panel_node, screen_root, title, PANEL_BORDER_COLOR};
use super::{DatGateDone, DatSetupReturn, LauncherState};
use crate::view_native::widgets::text_field::text_field;
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

const OK_COLOR: Color = Color::srgb(0.35, 0.85, 0.40);
const ERR_COLOR: Color = Color::srgb(0.95, 0.35, 0.30);
const LABEL_COLOR: Color = Color::srgb(0.92, 0.92, 0.95);
const MUTED_COLOR: Color = Color::srgb(0.60, 0.60, 0.65);
const ACCENT_COLOR: Color = Color::srgb(0.55, 0.85, 0.90);
const SECTION_COLOR: Color = Color::srgb(0.72, 0.72, 0.78);
const CARD_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.035);
const CARD_SELECTED_BORDER: Color = Color::srgb(0.30, 0.55, 0.85);
const BAR_TRACK: Color = Color::srgba(1.0, 1.0, 1.0, 0.08);
const BAR_FILL: Color = Color::srgb(0.30, 0.55, 0.85);

const PANEL_WIDTH: f32 = 780.0;
const FIELD_LABEL_WIDTH: f32 = 120.0;
const BUTTON_WIDTH: f32 = 120.0;
const WIDE_BUTTON_WIDTH: f32 = 200.0;
const OFFICIAL_BUTTON_WIDTH: f32 = 260.0;
const SMALL_BUTTON_WIDTH: f32 = 56.0;
const NAME_SIZE: f32 = 14.0;
const BODY_SIZE: f32 = 13.0;
const FINE_SIZE: f32 = 11.5;
const REGIONS: [&str; 2] = ["us", "eu"];

#[derive(Component)]
pub(super) struct DatSetupRoot;

#[derive(Resource)]
pub(super) struct DatSetupForm {
    pub path: String,
    pub feedback: Option<Result<String, String>>,
    pub installs: Vec<Install>,
    pub download_name: String,
    pub download_region: String,
    pub download_open: bool,
    prefilled: bool,
}

impl Default for DatSetupForm {
    fn default() -> Self {
        Self {
            path: String::new(),
            feedback: None,
            installs: Vec::new(),
            download_name: ffxi_client::DEFAULT_DOWNLOAD_NAME.to_string(),
            download_region: ffxi_client::DEFAULT_REGION.to_string(),
            download_open: false,
            prefilled: false,
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct DatSetupUiDirty(pub bool);

fn is_valid(path: &str) -> bool {
    let p = path.trim();
    !p.is_empty() && install_detect::is_ffxi_root(Path::new(p))
}

pub(super) fn enter_prefill(mut form: ResMut<DatSetupForm>) {
    form.installs = ffxi_client::installs();
    if form.prefilled {
        return;
    }
    form.prefilled = true;

    let persisted = launcher_store::load().settings.dat_path.value;
    if !persisted.trim().is_empty() {
        form.path = persisted;
        return;
    }
    if let Some(found) = form.installs.first() {
        form.path = found.path.display().to_string();
        form.feedback = Some(Ok(
            "Found an install automatically - press Continue to use it.".into(),
        ));
    }
}

pub(super) fn spawn_ui(
    mut commands: Commands,
    form: Res<DatSetupForm>,
    job: Option<Res<ClientJob>>,
    ret: Res<DatSetupReturn>,
) {
    build_ui(&mut commands, &form, job.as_deref(), ret.0.is_some());
}

pub(super) fn rebuild_ui_system(
    mut dirty: ResMut<DatSetupUiDirty>,
    mut commands: Commands,
    existing: Query<Entity, With<DatSetupRoot>>,
    form: Res<DatSetupForm>,
    job: Option<Res<ClientJob>>,
    ret: Res<DatSetupReturn>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in existing.iter() {
        commands.entity(e).despawn();
    }
    build_ui(&mut commands, &form, job.as_deref(), ret.0.is_some());
}

fn text(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(text.into()),
        TextFont {
            font_size: size.into(),
            ..default()
        },
        TextColor(color),
        ThemedText,
    )
}

/// One line that clips instead of wrapping: paths and field contents. The
/// clip lives on a wrapper because a node only clips its children, never its
/// own glyphs.
fn clipped_text(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            min_width: Val::Px(0.0),
            overflow: Overflow::clip_x(),
            ..default()
        },
        children![(
            Text::new(text.into()),
            TextFont {
                font_size: size.into(),
                ..default()
            },
            TextColor(color),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            ThemedText,
        )],
    )
}

/// Lets a text field shrink below its content width so a long path clips
/// inside the row instead of pushing the row past the panel.
fn field_slot() -> Node {
    Node {
        flex_grow: 1.0,
        min_width: Val::Px(0.0),
        overflow: Overflow::clip_x(),
        ..default()
    }
}

fn section(label: &str) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            margin: UiRect::top(Val::Px(4.0)),
            ..default()
        },
        children![
            text(label.to_uppercase(), FINE_SIZE, SECTION_COLOR),
            (
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(1.0),
                    ..default()
                },
                BackgroundColor(PANEL_BORDER_COLOR),
            ),
        ],
    )
}

fn h_row(gap: f32) -> Node {
    Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(gap),
        ..default()
    }
}

fn actions_row() -> Node {
    Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        justify_content: JustifyContent::FlexEnd,
        align_items: AlignItems::Center,
        column_gap: Val::Px(8.0),
        ..default()
    }
}

/// Feathers buttons grow to fill their row; a fixed slot keeps them button-sized.
fn button_slot(width: f32) -> Node {
    Node {
        width: Val::Px(width),
        flex_shrink: 0.0,
        ..default()
    }
}

fn card(selected: bool) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(10.0),
            padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(CARD_BG),
        BorderColor::all(if selected {
            CARD_SELECTED_BORDER
        } else {
            PANEL_BORDER_COLOR
        }),
    )
}

fn field_display() -> impl Bundle {
    (
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            overflow: Overflow::clip_x(),
            ..default()
        },
        Text::new(String::new()),
        TextFont {
            font_size: BODY_SIZE.into(),
            ..default()
        },
        TextColor(LABEL_COLOR),
        TextLayout {
            linebreak: LineBreak::NoWrap,
            ..default()
        },
        TextFieldDisplay {
            owner: Entity::PLACEHOLDER,
        },
        ThemedText,
    )
}

fn build_ui(
    commands: &mut Commands,
    form: &DatSetupForm,
    job: Option<&ClientJob>,
    can_go_back: bool,
) {
    commands
        .spawn((DatSetupRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(PANEL_WIDTH))
                .with_children(|panel| match job {
                    Some(job) => build_job_panel(panel, job),
                    None => build_setup_panel(panel, form, can_go_back),
                });
        });
}

fn build_job_panel(panel: &mut ChildSpawnerCommands, job: &ClientJob) {
    panel.spawn(title(job.title.clone()));
    panel.spawn((text(job.phase.clone(), 15.0, LABEL_COLOR), JobPhaseText));
    panel
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(8.0),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(BAR_TRACK),
        ))
        .with_children(|track| {
            track.spawn((
                Node {
                    width: Val::Percent(job.fraction.unwrap_or(0.0) * 100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(BAR_FILL),
                JobBarFill,
            ));
        });
    panel.spawn((
        clipped_text(job.detail.clone(), BODY_SIZE, MUTED_COLOR),
        JobDetailText,
    ));
    panel.spawn(hint(
        "The installer is 5 volumes (~7.2 GB) and the patch up to ~0.5 GB more, so this takes \
         a while. Closing the launcher stops it; a later run resumes from the volumes already \
         downloaded.",
    ));
}

fn build_setup_panel(panel: &mut ChildSpawnerCommands, form: &DatSetupForm, can_go_back: bool) {
    let path = form.path.clone();
    let valid = is_valid(&path);

    let status = match &form.feedback {
        Some(Ok(msg)) => Some((msg.clone(), OK_COLOR)),
        Some(Err(msg)) => Some((msg.clone(), ERR_COLOR)),
        None if valid => Some((
            format!(
                "Looks good: {}.",
                ffxi_client::describe(Path::new(path.trim()))
            ),
            OK_COLOR,
        )),
        None if !path.trim().is_empty() => Some((
            "Not a FINAL FANTASY XI install (needs VTABLE.DAT and a ROM folder).".to_string(),
            ERR_COLOR,
        )),
        None => None,
    };

    panel.spawn(title("Choose your FINAL FANTASY XI install"));
    panel.spawn(hint(
        "Kuluu ships no game data. It reads geometry, textures, audio and names from a retail \
         FINAL FANTASY XI install you already own: pick one below, point at its folder, or get \
         Square Enix's official client.",
    ));

    if !form.installs.is_empty() {
        panel.spawn(section("Installs found"));
        for install in &form.installs {
            spawn_install_row(panel, install, &path);
        }
    }

    panel.spawn(section("Or point at a folder"));
    spawn_path_row(panel, &path);
    if let Some((msg, color)) = status {
        panel.spawn(text(msg, BODY_SIZE, color));
    }

    if form.download_open {
        spawn_download_block(panel, form);
    }

    panel.spawn(actions_row()).with_children(|r| {
        if !form.download_open {
            r.spawn(Node {
                flex_grow: 1.0,
                ..default()
            })
            .with_children(|left| {
                left.spawn(button_slot(OFFICIAL_BUTTON_WIDTH))
                    .with_children(|s| {
                        s.spawn(button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new("Get the official client..."), ThemedText)),
                        ))
                        .observe(
                            |_ev: On<Activate>,
                             mut form: ResMut<DatSetupForm>,
                             mut dirty: ResMut<DatSetupUiDirty>| {
                                form.download_open = true;
                                dirty.0 = true;
                            },
                        );
                    });
            });
        }
        if can_go_back {
            r.spawn(button_slot(BUTTON_WIDTH)).with_children(|s| {
                s.spawn(button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("Back"), ThemedText)),
                ))
                .observe(
                    |_ev: On<Activate>,
                     mut ret: ResMut<DatSetupReturn>,
                     mut next: ResMut<NextState<LauncherState>>| {
                        go_back(&mut ret, &mut next);
                    },
                );
            });
        }
        r.spawn(button_slot(BUTTON_WIDTH)).with_children(|s| {
            s.spawn(button_bundle(
                ButtonBundleProps {
                    variant: ButtonVariant::Primary,
                    ..default()
                },
                (),
                Spawn((Text::new("Continue"), ThemedText)),
            ))
            .observe(continue_observer);
        });
    });
}

fn spawn_install_row(panel: &mut ChildSpawnerCommands, install: &Install, current: &str) {
    let selected = ffxi_client::same_dir(Path::new(current.trim()), &install.path);
    let root = install.path.clone();
    let summary = ffxi_client::describe(&install.path);
    let updatable = ffxi_client::refuse_non_retail(&install.path).is_ok();
    panel.spawn(card(selected)).with_children(|r| {
        r.spawn(Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        })
        .with_children(|col| {
            col.spawn(h_row(10.0)).with_children(|line| {
                line.spawn(text(install.name.clone(), NAME_SIZE, LABEL_COLOR));
                line.spawn(text(install.origin.label(), FINE_SIZE, MUTED_COLOR));
                line.spawn(text(summary, BODY_SIZE, ACCENT_COLOR));
            });
            col.spawn(clipped_text(
                install.path.display().to_string(),
                FINE_SIZE,
                MUTED_COLOR,
            ));
        });
        let select_root = root.clone();
        r.spawn(button_slot(BUTTON_WIDTH)).with_children(|s| {
            s.spawn(button_bundle(
                ButtonBundleProps {
                    variant: if selected {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Normal
                    },
                    ..default()
                },
                (),
                Spawn((
                    Text::new(if selected { "Selected" } else { "Select" }),
                    ThemedText,
                )),
            ))
            .observe(
                move |_ev: On<Activate>,
                      mut form: ResMut<DatSetupForm>,
                      mut dirty: ResMut<DatSetupUiDirty>| {
                    form.path = select_root.display().to_string();
                    form.feedback = None;
                    dirty.0 = true;
                },
            );
        });
        r.spawn(button_slot(BUTTON_WIDTH)).with_children(|s| {
            if updatable {
                s.spawn(button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("Update"), ThemedText)),
                ))
                .observe(
                    move |_ev: On<Activate>,
                          mut commands: Commands,
                          mut dirty: ResMut<DatSetupUiDirty>| {
                        client_job::start(
                            &mut commands,
                            JobKind::Update {
                                root: root.clone(),
                                verify: false,
                            },
                        );
                        dirty.0 = true;
                    },
                );
            }
        });
    });
}

fn spawn_path_row(panel: &mut ChildSpawnerCommands, path: &str) {
    panel.spawn(h_row(8.0)).with_children(|rowc| {
        rowc.spawn((
            Node {
                width: Val::Px(FIELD_LABEL_WIDTH),
                flex_shrink: 0.0,
                ..default()
            },
            children![text("Install folder", BODY_SIZE, LABEL_COLOR)],
        ));
        rowc.spawn(field_slot()).with_children(|slot| {
            slot.spawn(text_field(TextFieldProps {
                initial: path.to_string(),
                placeholder: "/path/to/SquareEnix/FINAL FANTASY XI".to_string(),
                submit_on_enter: false,
                ..default()
            }))
            .with_children(|tf| {
                tf.spawn(field_display());
            })
            .observe(
                |ev: On<ValueChange<String>>, mut form: ResMut<DatSetupForm>| {
                    form.path = ev.value.clone();
                    form.feedback = None;
                },
            );
        });
        rowc.spawn(button_slot(BUTTON_WIDTH)).with_children(|s| {
            s.spawn(button_bundle(
                ButtonBundleProps::default(),
                (),
                Spawn((Text::new("Browse..."), ThemedText)),
            ))
            .observe(
                |_ev: On<Activate>,
                 mut form: ResMut<DatSetupForm>,
                 mut dirty: ResMut<DatSetupUiDirty>| {
                    pick_folder(&mut form);
                    dirty.0 = true;
                },
            );
        });
    });
}

fn spawn_download_block(panel: &mut ChildSpawnerCommands, form: &DatSetupForm) {
    panel.spawn(section("Get the official client"));
    panel.spawn(hint(format!(
        "Downloads Square Enix's installer ({}) from the PlayOnline CDN, unpacks it natively, \
         and patches it to the current version ({} more). Free to download; a registration \
         code or subscription is needed to play on the official service. Existing installs \
         are never touched.",
        ffxi_client::INSTALLER_SIZE_NOTE,
        ffxi_client::PATCH_SIZE_NOTE
    )));
    panel.spawn(h_row(8.0)).with_children(|r| {
        r.spawn((
            Node {
                width: Val::Px(FIELD_LABEL_WIDTH),
                flex_shrink: 0.0,
                ..default()
            },
            children![text("Name", BODY_SIZE, LABEL_COLOR)],
        ));
        r.spawn(field_slot()).with_children(|slot| {
            slot.spawn(text_field(TextFieldProps {
                initial: form.download_name.clone(),
                placeholder: ffxi_client::DEFAULT_DOWNLOAD_NAME.to_string(),
                submit_on_enter: false,
                ..default()
            }))
            .with_children(|tf| {
                tf.spawn(field_display());
            })
            .observe(
                |ev: On<ValueChange<String>>, mut form: ResMut<DatSetupForm>| {
                    form.download_name = ev.value.clone();
                },
            );
        });
        r.spawn(text("Region", BODY_SIZE, LABEL_COLOR));
        for region in REGIONS {
            let active = form.download_region == region;
            r.spawn(button_slot(SMALL_BUTTON_WIDTH)).with_children(|s| {
                s.spawn(button_bundle(
                    ButtonBundleProps {
                        variant: if active {
                            ButtonVariant::Primary
                        } else {
                            ButtonVariant::Normal
                        },
                        ..default()
                    },
                    (),
                    Spawn((Text::new(region.to_uppercase()), ThemedText)),
                ))
                .observe(
                    move |_ev: On<Activate>,
                          mut form: ResMut<DatSetupForm>,
                          mut dirty: ResMut<DatSetupUiDirty>| {
                        form.download_region = region.to_string();
                        dirty.0 = true;
                    },
                );
            });
        }
    });
    if let Some(dir) = ffxi_client::user_clients_dir() {
        panel.spawn(clipped_text(
            format!("Lands in {}", dir.join(form.download_name.trim()).display()),
            FINE_SIZE,
            MUTED_COLOR,
        ));
    }
    panel.spawn(actions_row()).with_children(|r| {
        r.spawn(button_slot(BUTTON_WIDTH)).with_children(|s| {
            s.spawn(button_bundle(
                ButtonBundleProps::default(),
                (),
                Spawn((Text::new("Cancel"), ThemedText)),
            ))
            .observe(
                |_ev: On<Activate>,
                 mut form: ResMut<DatSetupForm>,
                 mut dirty: ResMut<DatSetupUiDirty>| {
                    form.download_open = false;
                    dirty.0 = true;
                },
            );
        });
        r.spawn(button_slot(WIDE_BUTTON_WIDTH)).with_children(|s| {
            s.spawn(button_bundle(
                ButtonBundleProps {
                    variant: ButtonVariant::Primary,
                    ..default()
                },
                (),
                Spawn((Text::new("Download and patch"), ThemedText)),
            ))
            .observe(start_download_observer);
        });
    });
}

fn start_download_observer(
    _ev: On<Activate>,
    mut commands: Commands,
    mut form: ResMut<DatSetupForm>,
    mut dirty: ResMut<DatSetupUiDirty>,
) {
    let name = form.download_name.trim().to_string();
    if !ffxi_client::valid_name(&name) {
        form.feedback = Some(Err(
            "The client name may only use letters, digits, - and _ (it becomes a folder name)."
                .into(),
        ));
        dirty.0 = true;
        return;
    }
    if ffxi_client::named(&name).is_some() {
        form.feedback = Some(Ok(format!(
            "A client named `{name}` already exists; it is reused and patched, not downloaded \
             again."
        )));
    }
    client_job::start(
        &mut commands,
        JobKind::Setup(SetupOptions {
            name,
            region: form.download_region.clone(),
            update: true,
        }),
    );
    form.download_open = false;
    dirty.0 = true;
}

fn pick_folder(form: &mut DatSetupForm) {
    let start = form.path.trim().to_string();
    let start_dir = if !start.is_empty() {
        Some(PathBuf::from(&start))
    } else {
        super::common::home_dir_fallback()
    };
    let Some(picked) = super::common::pick_folder_blocking(
        "Select your FINAL FANTASY XI folder".into(),
        start_dir,
    ) else {
        return;
    };
    let mut chosen = picked.display().to_string();
    // If they picked a parent (e.g. the SquareEnix folder), descend to the
    // actual DAT root so a one-level-off pick still works.
    if !is_valid(&chosen) {
        if let Some(found) =
            install_detect::find_ffxi_root(Path::new(&chosen), install_detect::DEFAULT_SEARCH_DEPTH)
        {
            chosen = found.display().to_string();
        }
    }
    form.feedback = if is_valid(&chosen) {
        None
    } else {
        Some(Err(
            "That folder doesn't contain a FINAL FANTASY XI install.".into(),
        ))
    };
    form.path = chosen;
}

fn go_back(ret: &mut DatSetupReturn, next: &mut NextState<LauncherState>) {
    next.set(ret.0.take().unwrap_or(LauncherState::Login));
}

fn try_continue(
    form: &mut DatSetupForm,
    commands: &mut Commands,
    ret: &mut DatSetupReturn,
    next: &mut NextState<LauncherState>,
    dirty: &mut DatSetupUiDirty,
) {
    let path = form.path.trim().to_string();
    if !is_valid(&path) {
        form.feedback = Some(Err(
            "That folder isn't a FINAL FANTASY XI install (needs VTABLE.DAT and a ROM folder). \
             Use Browse to pick it."
                .into(),
        ));
        dirty.0 = true;
        return;
    }

    let mut store = launcher_store::load();
    // This screen is reached when the shell's FFXI_DAT_PATH is unusable, so the
    // saved choice has to beat it; with no shell value there is nothing to override.
    store.settings.dat_path = EnvOverride {
        value: path,
        override_env: ffxi_client::shell_dat_path().is_some(),
    };
    if let Err(e) = launcher_store::save(&store) {
        tracing::warn!(error = %e, "launcher_store: dat_path save failed");
    }
    crate::ffxi_client::export(&store.settings).ok();

    match ffxi_dat::DatRoot::from_env_or_default() {
        Ok(root) => {
            tracing::info!(root = %root.root().display(), "DAT gate: install accepted");
            commands.insert_resource(DatGateDone);
            go_back(ret, next);
        }
        Err(e) => {
            form.feedback = Some(Err(format!("Saved, but the loader rejected it: {e}")));
            dirty.0 = true;
        }
    }
}

fn continue_observer(
    _ev: On<Activate>,
    mut commands: Commands,
    mut form: ResMut<DatSetupForm>,
    mut ret: ResMut<DatSetupReturn>,
    mut next: ResMut<NextState<LauncherState>>,
    mut dirty: ResMut<DatSetupUiDirty>,
) {
    try_continue(&mut form, &mut commands, &mut ret, &mut next, &mut dirty);
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<DatSetupForm>,
    mut commands: Commands,
    mut ret: ResMut<DatSetupReturn>,
    mut next: ResMut<NextState<LauncherState>>,
    mut dirty: ResMut<DatSetupUiDirty>,
    job: Option<Res<ClientJob>>,
) {
    if job.is_some() {
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match ev.logical_key {
            Key::Enter => {
                try_continue(&mut form, &mut commands, &mut ret, &mut next, &mut dirty);
                return;
            }
            Key::Escape if ret.0.is_some() => {
                go_back(&mut ret, &mut next);
                return;
            }
            _ => {}
        }
    }
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<DatSetupRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}
