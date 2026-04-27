use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, checkbox, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Activate, ValueChange};

use std::sync::Arc;

use ffxi_client::launcher_store::{self, EnvOverride, Settings};

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use super::{LauncherState, ServerInfo};
use crate::view_native::widgets::text_field::text_field;
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

#[derive(Component)]
pub(super) struct SettingsRoot;

#[derive(Resource, Default)]
pub(super) struct SettingsForm {
    pub dat_path: String,
    pub dat_override: bool,
    pub navmesh_dir: String,
    pub navmesh_override: bool,
    pub mac: String,
    pub mac_override: bool,

    pub feedback: Option<Result<String, String>>,
}

impl SettingsForm {
    fn to_settings(&self) -> Settings {
        let mk = |value: &str, override_env: bool| EnvOverride {
            value: value.trim().to_string(),
            override_env,
        };
        Settings {
            dat_path: mk(&self.dat_path, self.dat_override),
            navmesh_dir: mk(&self.navmesh_dir, self.navmesh_override),
            mac: mk(&self.mac, self.mac_override),
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct SettingsUiDirty(pub bool);

#[derive(Clone, Copy)]
enum SettingsField {
    DatPath,
    NavmeshDir,
    Mac,
}

pub(super) fn load_settings_form(mut form: ResMut<SettingsForm>) {
    let s = launcher_store::load().settings;
    form.dat_path = s.dat_path.value;
    form.dat_override = s.dat_path.override_env;
    form.navmesh_dir = s.navmesh_dir.value;
    form.navmesh_override = s.navmesh_dir.override_env;
    form.mac = s.mac.value;
    form.mac_override = s.mac.override_env;
    form.feedback = None;
}

fn current_effective(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => "(unset — using built-in default)".to_string(),
    }
}

pub(super) fn spawn_ui(mut commands: Commands, form: Res<SettingsForm>, server: Res<ServerInfo>) {
    build_ui(&mut commands, &form, &server);
}

pub(super) fn rebuild_settings_ui_system(
    mut dirty: ResMut<SettingsUiDirty>,
    mut commands: Commands,
    existing: Query<Entity, With<SettingsRoot>>,
    form: Res<SettingsForm>,
    server: Res<ServerInfo>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in existing.iter() {
        commands.entity(e).despawn();
    }
    build_ui(&mut commands, &form, &server);
}

fn build_ui(commands: &mut Commands, form: &SettingsForm, server: &ServerInfo) {
    let dat = form.dat_path.clone();
    let dat_ov = form.dat_override;
    let nav = form.navmesh_dir.clone();
    let nav_ov = form.navmesh_override;
    let mac = form.mac.clone();
    let mac_ov = form.mac_override;
    let feedback = form.feedback.clone();

    commands
        .spawn((SettingsRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(root, server, &[Crumb::Other("Settings".to_string())]);
            root.spawn(panel_node(640.0)).with_children(|panel| {
                panel.spawn(title("Settings"));
                panel.spawn(hint(
                    "Override the install paths the client reads from. Tick \
                     'Override' to win over a shell env var; leave it off to \
                     only fill in when the env var is unset. Esc cancels.",
                ));

                spawn_path_field(
                    panel,
                    "DAT install path",
                    "FFXI_DAT_PATH",
                    SettingsField::DatPath,
                    &dat,
                    dat_ov,
                    true,
                );
                spawn_path_field(
                    panel,
                    "Navmesh dir",
                    "FFXI_NAVMESH_DIR",
                    SettingsField::NavmeshDir,
                    &nav,
                    nav_ov,
                    true,
                );
                spawn_path_field(
                    panel,
                    "MAC override",
                    "FFXI_MAC",
                    SettingsField::Mac,
                    &mac,
                    mac_ov,
                    false,
                );

                if let Some(result) = &feedback {
                    let (text, color) = match result {
                        Ok(msg) => (msg.clone(), Color::srgb(0.35, 0.85, 0.40)),
                        Err(msg) => (msg.clone(), Color::srgb(0.95, 0.35, 0.30)),
                    };
                    panel.spawn((
                        Text::new(text),
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
                        Spawn((Text::new("Save & reload"), ThemedText)),
                    ))
                    .observe(save_observer);

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Back"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::ServerSelect);
                        },
                    );
                });
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn spawn_path_field(
    panel: &mut ChildSpawnerCommands,
    label: &str,
    var: &str,
    field: SettingsField,
    initial: &str,
    override_env: bool,
    browsable: bool,
) {
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
                    width: Val::Px(140.0),
                    ..default()
                },
                Text::new(label.to_string()),
                ThemedText,
            ));
            rowc.spawn(text_field(TextFieldProps {
                initial: initial.to_string(),
                placeholder: var.to_string(),
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<SettingsForm>| {
                    let v = ev.value.clone();
                    match field {
                        SettingsField::DatPath => form.dat_path = v,
                        SettingsField::NavmeshDir => form.navmesh_dir = v,
                        SettingsField::Mac => form.mac = v,
                    }
                },
            );

            if browsable {
                let browse_label = label.to_string();
                rowc.spawn(button(
                    ButtonProps::default(),
                    (),
                    Spawn((Text::new("Browse…"), ThemedText)),
                ))
                .observe(
                    move |_ev: On<Activate>,
                          mut form: ResMut<SettingsForm>,
                          mut dirty: ResMut<SettingsUiDirty>| {
                        pick_folder_into(field, &browse_label, &mut form);
                        dirty.0 = true;
                    },
                );
            }

            let mut cb = rowc.spawn(checkbox((), Spawn((Text::new("Override"), ThemedText))));
            if override_env {
                cb.insert(Checked);
            }
            cb.observe(
                move |ev: On<ValueChange<bool>>,
                      mut form: ResMut<SettingsForm>,
                      mut commands: Commands| {
                    match field {
                        SettingsField::DatPath => form.dat_override = ev.value,
                        SettingsField::NavmeshDir => form.navmesh_override = ev.value,
                        SettingsField::Mac => form.mac_override = ev.value,
                    }
                    if ev.value {
                        commands.entity(ev.source).insert(Checked);
                    } else {
                        commands.entity(ev.source).remove::<Checked>();
                    }
                },
            );
        });

    panel.spawn(hint(format!("Currently: {}", current_effective(var))));
}

fn pick_folder_into(field: SettingsField, title: &str, form: &mut SettingsForm) {
    let start = match field {
        SettingsField::DatPath => form.dat_path.clone(),
        SettingsField::NavmeshDir => form.navmesh_dir.clone(),
        SettingsField::Mac => String::new(),
    };
    let mut dialog = rfd::FileDialog::new().set_title(format!("Select {title}"));
    let start_dir = if !start.trim().is_empty() {
        std::path::PathBuf::from(start.trim())
    } else {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    };
    if start_dir.is_dir() {
        dialog = dialog.set_directory(start_dir);
    }
    let Some(path) = dialog.pick_folder() else {
        return;
    };
    let chosen = path.display().to_string();
    match field {
        SettingsField::DatPath => {
            form.dat_path = chosen;
            form.dat_override = true;
        }
        SettingsField::NavmeshDir => {
            form.navmesh_dir = chosen;
            form.navmesh_override = true;
        }
        SettingsField::Mac => {}
    }
    form.feedback = None;
}

fn save_observer(
    _ev: On<Activate>,
    mut commands: Commands,
    mut form: ResMut<SettingsForm>,
    mut cache: ResMut<ffxi_viewer_core::dat_mzb::ZoneGeomCache>,
    mut last_zone: ResMut<ffxi_viewer_core::dat_mzb::LastAutoLoadedZone>,
    mut dirty: ResMut<SettingsUiDirty>,
) {
    let result = persist_and_reload(&form, &mut commands, &mut cache, &mut last_zone);
    form.feedback = Some(result);
    dirty.0 = true;
}

fn persist_and_reload(
    form: &SettingsForm,
    commands: &mut Commands,
    cache: &mut ffxi_viewer_core::dat_mzb::ZoneGeomCache,
    last_zone: &mut ffxi_viewer_core::dat_mzb::LastAutoLoadedZone,
) -> Result<String, String> {
    let settings = form.to_settings();

    let mut store = launcher_store::load();
    store.settings = settings.clone();
    let persist_warn = match launcher_store::save(&store) {
        Ok(()) => None,
        Err(e) => {
            tracing::warn!(error = %e, "launcher_store: settings save failed");
            Some(format!(" (warning: couldn't write launcher.json: {e})"))
        }
    };

    settings.apply_to_env();

    let root = ffxi_dat::DatRoot::from_env_or_default()
        .map_err(|e| format!("DAT path rejected: {e}. Settings saved but assets not reloaded."))?;
    let root_path = root.root().display().to_string();
    let app_count = root.app_summary().len();
    let arc = Arc::new(root);

    commands.insert_resource(crate::view_native::DatRootRes(Some(arc.clone())));
    commands.insert_resource(ffxi_viewer_core::minimap::retail::MinimapDatRoot(Some(
        arc.clone(),
    )));
    commands.insert_resource(ffxi_viewer_core::hud::status_ribbon::StatusIconDatRoot(
        Some(arc.clone()),
    ));
    commands.insert_resource(ffxi_viewer_core::hud::item_dat_root::ItemDatRoot(Some(
        arc.clone(),
    )));

    cache.entries.clear();
    last_zone.zone_id = None;

    let mut msg = format!("Reloaded {app_count} archive(s) from {root_path}");
    if let Some(w) = persist_warn {
        msg.push_str(&w);
    }
    Ok(msg)
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<SettingsRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            next.set(LauncherState::ServerSelect);
            return;
        }
    }
}
