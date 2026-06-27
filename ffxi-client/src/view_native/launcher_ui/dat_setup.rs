use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};

use std::path::Path;

use ffxi_client::launcher_store::{self, EnvOverride};
use ffxi_dat::install_detect;

use super::common::{hint, panel_node, row, screen_root, title};
use super::{DatGateDone, LauncherState};
use crate::view_native::widgets::text_field::text_field;
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

const OK_COLOR: Color = Color::srgb(0.35, 0.85, 0.40);
const ERR_COLOR: Color = Color::srgb(0.95, 0.35, 0.30);

#[derive(Component)]
pub(super) struct DatSetupRoot;

#[derive(Resource, Default)]
pub(super) struct DatSetupForm {
    pub path: String,
    pub feedback: Option<Result<String, String>>,
    prefilled: bool,
}

#[derive(Resource, Default)]
pub(super) struct DatSetupUiDirty(pub bool);

fn is_valid(path: &str) -> bool {
    let p = path.trim();
    !p.is_empty() && install_detect::is_ffxi_root(Path::new(p))
}

pub(super) fn enter_prefill(mut form: ResMut<DatSetupForm>) {
    if form.prefilled {
        return;
    }
    form.prefilled = true;

    let persisted = launcher_store::load().settings.dat_path.value;
    if !persisted.trim().is_empty() {
        form.path = persisted;
        return;
    }
    if let Some(found) = install_detect::detect().into_iter().next() {
        form.path = found.display().to_string();
        form.feedback = Some(Ok("Found an install automatically — click Continue.".into()));
    }
}

pub(super) fn spawn_ui(mut commands: Commands, form: Res<DatSetupForm>) {
    build_ui(&mut commands, &form);
}

pub(super) fn rebuild_ui_system(
    mut dirty: ResMut<DatSetupUiDirty>,
    mut commands: Commands,
    existing: Query<Entity, With<DatSetupRoot>>,
    form: Res<DatSetupForm>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in existing.iter() {
        commands.entity(e).despawn();
    }
    build_ui(&mut commands, &form);
}

fn build_ui(commands: &mut Commands, form: &DatSetupForm) {
    let path = form.path.clone();
    let valid = is_valid(&path);

    let status = match &form.feedback {
        Some(Ok(msg)) => Some((msg.clone(), OK_COLOR)),
        Some(Err(msg)) => Some((msg.clone(), ERR_COLOR)),
        None if valid => Some(("Looks good — a valid FFXI install.".to_string(), OK_COLOR)),
        None if !path.trim().is_empty() => Some((
            "Not a FINAL FANTASY XI install (needs VTABLE.DAT and a ROM folder).".to_string(),
            ERR_COLOR,
        )),
        None => None,
    };

    commands
        .spawn((DatSetupRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(660.0)).with_children(|panel| {
                panel.spawn(title("Locate your FINAL FANTASY XI install"));
                panel.spawn(hint(
                    "Kuluu ships no game data — it reads geometry, textures, audio, and names \
                     from a retail FINAL FANTASY XI install you already own. Point it at that \
                     folder (the one containing VTABLE.DAT and a ROM directory). Press Enter or \
                     click Continue when it validates.",
                ));

                panel
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(8.0),
                        ..default()
                    })
                    .with_children(|rowc| {
                        rowc.spawn((
                            Node {
                                width: Val::Px(110.0),
                                ..default()
                            },
                            Text::new("Install folder"),
                            ThemedText,
                        ));
                        rowc.spawn(text_field(TextFieldProps {
                            initial: path.clone(),
                            placeholder: "/path/to/SquareEnix/FINAL FANTASY XI".to_string(),
                            submit_on_enter: false,
                            ..default()
                        }))
                        .with_children(|tf| {
                            tf.spawn((
                                Node {
                                    flex_grow: 1.0,
                                    ..default()
                                },
                                Text::new(String::new()),
                                TextColor(Color::srgb(0.92, 0.92, 0.95)),
                                TextFieldDisplay {
                                    owner: Entity::PLACEHOLDER,
                                },
                                ThemedText,
                            ));
                        })
                        .observe(
                            |ev: On<ValueChange<String>>, mut form: ResMut<DatSetupForm>| {
                                form.path = ev.value.clone();
                            },
                        );

                        rowc.spawn(button(
                            ButtonProps::default(),
                            (),
                            Spawn((Text::new("Browse…"), ThemedText)),
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

                if let Some((msg, color)) = status {
                    panel.spawn((
                        Text::new(msg),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(color),
                        ThemedText,
                    ));
                }

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Continue"), ThemedText)),
                    ))
                    .observe(continue_observer);
                });
            });
        });
}

fn pick_folder(form: &mut DatSetupForm) {
    let start = form.path.trim().to_string();
    let mut dialog = rfd::FileDialog::new().set_title("Select your FINAL FANTASY XI folder");
    let start_dir = if !start.is_empty() {
        std::path::PathBuf::from(&start)
    } else {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    };
    if start_dir.is_dir() {
        dialog = dialog.set_directory(start_dir);
    }
    let Some(picked) = dialog.pick_folder() else {
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
        Some(Ok("Looks good — a valid FFXI install.".into()))
    } else {
        Some(Err(
            "That folder doesn't contain a FINAL FANTASY XI install.".into(),
        ))
    };
    form.path = chosen;
}

fn try_continue(
    form: &mut DatSetupForm,
    commands: &mut Commands,
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
    store.settings.dat_path = EnvOverride {
        value: path,
        override_env: true,
    };
    if let Err(e) = launcher_store::save(&store) {
        tracing::warn!(error = %e, "launcher_store: dat_path save failed");
    }
    store.settings.apply_to_env();

    match ffxi_dat::DatRoot::from_env_or_default() {
        Ok(root) => {
            tracing::info!(root = %root.root().display(), "DAT gate: install accepted");
            commands.insert_resource(DatGateDone);
            next.set(LauncherState::Login);
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
    mut next: ResMut<NextState<LauncherState>>,
    mut dirty: ResMut<DatSetupUiDirty>,
) {
    try_continue(&mut form, &mut commands, &mut next, &mut dirty);
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<DatSetupForm>,
    mut commands: Commands,
    mut next: ResMut<NextState<LauncherState>>,
    mut dirty: ResMut<DatSetupUiDirty>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Enter) {
            try_continue(&mut form, &mut commands, &mut next, &mut dirty);
            return;
        }
    }
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<DatSetupRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}
