//! Launcher Settings screen — edit the global `EnvOverride` values that
//! live in [`launcher_store::Settings`] (the DAT install path, the
//! navmesh dir, and the MAC override) without dropping to a shell.
//!
//! # Why this screen can hot-reload assets
//!
//! The launcher zone backdrop loads geometry through viewer-core's
//! `auto_load_zone_geometry_system`, which resolves the install via
//! `DatRoot::from_env_or_default()` *at load time* — it reads
//! `FFXI_DAT_PATH` from the process env every zone change rather than
//! holding a fixed handle. So once Save writes the new path into the env
//! (and clears the geometry cache so the old-path parse can't be reused),
//! a single reset of `LastAutoLoadedZone` re-fires the current zone's
//! load against the new install and the backdrop swaps in place.
//!
//! The in-game consumers (minimap, status-icon ribbon, item detail,
//! NPC-name scene) instead hold an `Arc<DatRoot>` snapshot taken at
//! startup, so Save also rebuilds that Arc and overwrites every resource
//! that carries one. A path change made on the launcher screen is then
//! consistent for the session the user is about to start.

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

/// Editable mirror of [`launcher_store::Settings`]. Loaded from the store
/// on [`load_settings_form`] (OnEnter), mutated in place by the field
/// observers / Browse picker, and written back on Save. `feedback` holds
/// the result line from the last Save / Browse action (empty = none).
#[derive(Resource, Default)]
pub(super) struct SettingsForm {
    pub dat_path: String,
    pub dat_override: bool,
    pub navmesh_dir: String,
    pub navmesh_override: bool,
    pub mac: String,
    pub mac_override: bool,
    /// `Ok` (green) success or `Err` (red) failure copy from the last
    /// Save. `None` until the user saves at least once this visit.
    pub feedback: Option<Result<String, String>>,
}

impl SettingsForm {
    /// Reassemble a [`launcher_store::Settings`] from the editable fields.
    /// The on-disk schema only stores the trimmed value + the override
    /// flag, so trimming here keeps "  " from masquerading as a real
    /// path (see [`EnvOverride::resolved`]).
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

/// In-place rebuild flag, mirroring `login::LoginUiDirty`. The Browse
/// picker and Save observers mutate `SettingsForm` and need the panel to
/// re-render (a `next.set(Settings)` self-transition is a silent no-op).
#[derive(Resource, Default)]
pub(super) struct SettingsUiDirty(pub bool);

/// Which `EnvOverride` a field/checkbox writes into.
#[derive(Clone, Copy)]
enum SettingsField {
    DatPath,
    NavmeshDir,
    Mac,
}

/// Copy the persisted settings into the editable form and clear any stale
/// feedback. Runs `OnEnter(Settings)` *before* `spawn_ui` so the fields
/// render seeded.
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

/// The value the process is *currently* resolving for `var` — the live
/// env (set at startup from these same settings, or by the shell), shown
/// so the user can see what's in effect before they change it.
fn current_effective(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => "(unset — using built-in default)".to_string(),
    }
}

pub(super) fn spawn_ui(mut commands: Commands, form: Res<SettingsForm>, server: Res<ServerInfo>) {
    build_ui(&mut commands, &form, &server);
}

/// In-place rebuild: when Browse / Save flips [`SettingsUiDirty`], tear
/// down the panel and rebuild from the (now-mutated) form.
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

                // Result line from the last Save — green on success, red
                // on failure (e.g. the new DAT path has no VTABLE.DAT).
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

/// One settings row: label + text field, an optional Browse button (for
/// directory values), the "Override" checkbox, and a hint echoing the
/// currently-effective env value.
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

            // "Override" checkbox.
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

/// Open the native folder picker, seeded at the field's current value (or
/// `$HOME`), and write the chosen path into the form. Picking a folder
/// implies the user wants it to win, so we also tick `override_env`.
/// Blocking call on the winit main thread — fine for a modal dialog.
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

/// Save observer: persist the form, apply it to the env, then rebuild the
/// shared `DatRoot` and reload the backdrop zone in place. Kept as a
/// named fn (not a closure) so the long param list reads cleanly.
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

/// The work behind Save: write `launcher.json`, push the values into the
/// process env, rebuild the `DatRoot` Arc into every consumer, and reset
/// the backdrop loader so the login zone re-parses from the new install.
/// Returns the user-facing result line (`Ok` = green, `Err` = red).
fn persist_and_reload(
    form: &SettingsForm,
    commands: &mut Commands,
    cache: &mut ffxi_viewer_core::dat_mzb::ZoneGeomCache,
    last_zone: &mut ffxi_viewer_core::dat_mzb::LastAutoLoadedZone,
) -> Result<String, String> {
    let settings = form.to_settings();

    // 1. Persist. A save failure is worth surfacing — the user's change
    //    won't survive a relaunch — but we still apply it to the live
    //    session below so this run reflects the edit.
    let mut store = launcher_store::load();
    store.settings = settings.clone();
    let persist_warn = match launcher_store::save(&store) {
        Ok(()) => None,
        Err(e) => {
            tracing::warn!(error = %e, "launcher_store: settings save failed");
            Some(format!(" (warning: couldn't write launcher.json: {e})"))
        }
    };

    // 2. Push into the process env so `DatRoot::from_env_or_default()` —
    //    used by the backdrop's zone loader — picks up the new path.
    settings.apply_to_env();

    // 3. Rebuild the shared DatRoot. On failure, leave every existing Arc
    //    untouched (a half-applied path that loads nothing is worse than
    //    the prior good one) and report the error.
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

    // 4. Force the backdrop to re-parse the current zone from the new
    //    install: drop cached geometry (so the old-path parse can't be
    //    reused) and reset the auto-loader's "last zone" so it sees a
    //    transition next frame and re-fires the load.
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

/// Esc returns to the server picker (the screen Settings is reached from).
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
