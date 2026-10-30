use bevy::feathers::controls::{ButtonVariant, FeathersButton};
use bevy::feathers::font_styles::InheritableFont;
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};

use crate::ffxi_client;
use crate::launcher_store::{self, AuthFlavorKind, ServerProfile};
use ffxi_proto::login::{VerLock, LSB_CLIENT_VER, LSB_DEFAULT_VER_LOCK};
use kuluu_session::auth_client;

use super::common::{
    hint, panel_node_capped, row, screen_root, spawn_breadcrumb, title, Crumb, DefaultFocusTarget,
};
use crate::view_native::widgets::text_field::text_field;
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

use super::{LauncherState, ServerEditField, ServerEditForm, ServerInfo};

#[derive(Component)]
pub(super) struct ServerEditRoot;

#[derive(Component)]
pub(super) struct ConnectionSummary;

#[derive(Component)]
pub(super) struct ServerEditError;

#[derive(Component, Clone, Copy)]
pub(super) struct FlavorButton(AuthFlavorKind);

#[derive(Component, Clone, Copy)]
pub(super) struct VerLockButton(Option<u8>);

#[derive(Component, Clone)]
pub(super) struct PreferredClientButton(Option<String>);

const VER_LOCK_CHOICES: [(Option<u8>, &str); 4] = [
    (None, "Server default"),
    (Some(0), "Off"),
    (Some(1), "Exact"),
    (Some(2), "At least"),
];

pub(super) fn ver_lock_label(lock: VerLock) -> &'static str {
    match lock {
        VerLock::Off => "not enforced",
        VerLock::Exact => "exactly",
        VerLock::AtLeast => "or newer",
    }
}

#[derive(Resource)]
pub(super) struct ServerEditUiDirty(pub bool);

const PANEL_WIDTH_PX: f32 = 560.0;
const FIELD_LABEL_PX: f32 = 150.0;
const FORM_ROW_GAP_PX: f32 = 12.0;

pub(super) fn spawn_ui(mut commands: Commands, form: Res<ServerEditForm>, server: Res<ServerInfo>) {
    build_ui(&mut commands, &form, &server);
}

pub(super) fn rebuild_ui_system(
    mut dirty: ResMut<ServerEditUiDirty>,
    mut commands: Commands,
    existing: Query<Entity, With<ServerEditRoot>>,
    form: Res<ServerEditForm>,
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

fn connection_summary(form: &ServerEditForm) -> String {
    if auth_client::validate_loader_version(form.flavor.auth_flavor(), &form.xiloader_version)
        .is_err()
    {
        return "Connection: invalid loader version".into();
    }
    let version =
        (!form.xiloader_version.trim().is_empty()).then_some(form.xiloader_version.as_str());
    match form.flavor {
        AuthFlavorKind::Json => {
            let [major, minor, patch] = auth_client::resolve_client_version(version);
            format!("Connection: JSON | loader {major}.{minor}.{patch}")
        }
        AuthFlavorKind::Binary => {
            let version = auth_client::resolve_binary_version(version);
            format!(
                "Connection: Binary | loader {}",
                String::from_utf8_lossy(&version)
            )
        }
        AuthFlavorKind::PlayOnline => "Connection: PlayOnline | no loader version".into(),
    }
}

fn build_ui(commands: &mut Commands, form: &ServerEditForm, server: &ServerInfo) {
    let editing = form.editing_index.is_some();
    let leaf = if editing {
        Crumb::Other(format!("Edit: {}", form.name))
    } else {
        Crumb::Other("New server".to_string())
    };
    commands.spawn((ServerEditRoot, screen_root())).with_children(|root| {
        spawn_breadcrumb(root, server, &[Crumb::Server, leaf]);
        root.spawn(panel_node_capped(PANEL_WIDTH_PX, Val::Percent(100.0))).with_children(|panel| {
            panel.spawn(title(if editing { "Edit server" } else { "New server" }));
            panel.spawn(Node {
                flex_direction: FlexDirection::Column,
                flex_shrink: 1.0,
                min_height: Val::Px(0.0),
                overflow: Overflow::scroll_y(),
                row_gap: Val::Px(FORM_ROW_GAP_PX),
                ..default()
            }).with_children(|body| {
                if !editing {
                    spawn_template_row(body);
                }
                spawn_field(body, "Name", &form.name, "", ServerEditField::Name);
                spawn_field(body, "Host", &form.host, "", ServerEditField::Host);
                body.spawn(hint("Server type:"));
                body.spawn(row()).with_children(|r| {
                    for kind in [AuthFlavorKind::Json, AuthFlavorKind::Binary, AuthFlavorKind::PlayOnline] {
                        spawn_flavor_button(r, kind.label(), kind, form.flavor);
                    }
                });
                body.spawn((hint(connection_summary(form)), ConnectionSummary));
                if form.flavor == AuthFlavorKind::PlayOnline {
                    for line in POL_FLAVOR_HINTS {
                        body.spawn(hint(line));
                    }
                }
                spawn_advanced_toggle(body, form.show_advanced);
                if form.show_advanced {
                    spawn_advanced_fields(body, form);
                }
            });
            panel.spawn((hint(&form.error), ServerEditError));
            panel.spawn(row()).with_children(|r| {
                r.spawn_empty().apply_scene(bsn! {
                    @FeathersButton {
                        @variant: ButtonVariant::Primary,
                        @caption: bsn! {
                            Text::new("Save")
                            ThemedText
                        },
                    }
                    Node { flex_grow: 1.0 }
                    InheritableFont { font: {Handle::<Font>::default()} }
                }).insert(DefaultFocusTarget).observe(|_ev: On<Activate>, mut form: ResMut<ServerEditForm>, mut next: ResMut<NextState<LauncherState>>| {
                    save_form(&mut form, &mut next);
                });
                r.spawn_empty().apply_scene(bsn! {
                    @FeathersButton {
                        @caption: bsn! {
                            Text::new("Cancel")
                            ThemedText
                        },
                    }
                    Node { flex_grow: 1.0 }
                    InheritableFont { font: {Handle::<Font>::default()} }
                }).observe(|_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                    next.set(LauncherState::ServerSelect);
                });
            });
        });
    });
}

fn spawn_template_row(panel: &mut ChildSpawnerCommands) {
    panel.spawn(hint("Start from:"));
    panel.spawn(row()).with_children(|r| {
        for template in launcher_store::server_templates() {
            let profile = template.profile;
            r.spawn_empty()
                .apply_scene(bsn! {
                    @FeathersButton {
                        @caption: bsn! {
                            Text::new(template.label)
                            ThemedText
                        },
                    }
                    Node { flex_grow: 1.0 }
                    InheritableFont { font: {Handle::<Font>::default()} }
                })
                .observe(
                    move |_ev: On<Activate>,
                          mut form: ResMut<ServerEditForm>,
                          mut dirty: ResMut<ServerEditUiDirty>| {
                        *form = ServerEditForm::from_profile(&profile);
                        dirty.0 = true;
                    },
                );
        }
    });
}

fn spawn_advanced_toggle(panel: &mut ChildSpawnerCommands, show_advanced: bool) {
    let label = if show_advanced {
        "Hide advanced settings"
    } else {
        "Advanced settings..."
    };
    panel
        .spawn_empty()
        .apply_scene(bsn! {
            @FeathersButton {
                @caption: bsn! {
                    Text::new(label)
                    ThemedText
                },
            }
            Node { flex_grow: 1.0 }
            InheritableFont { font: {Handle::<Font>::default()} }
        })
        .observe(
            |_ev: On<Activate>,
             mut form: ResMut<ServerEditForm>,
             mut dirty: ResMut<ServerEditUiDirty>| {
                form.show_advanced = !form.show_advanced;
                dirty.0 = true;
            },
        );
}

fn spawn_advanced_fields(panel: &mut ChildSpawnerCommands, form: &ServerEditForm) {
    let install_names: Vec<String> = ffxi_client::installs()
        .into_iter()
        .map(|i| i.name)
        .collect();
    let default_lock = ver_lock_label(VerLock::from_setting(LSB_DEFAULT_VER_LOCK));

    spawn_field(
        panel,
        "Auth port",
        &form.auth_port,
        "",
        ServerEditField::AuthPort,
    );
    spawn_field(
        panel,
        "Data port",
        &form.data_port,
        "",
        ServerEditField::DataPort,
    );
    spawn_field(
        panel,
        "View port",
        &form.view_port,
        "",
        ServerEditField::ViewPort,
    );

    if form.flavor.uses_auth_server() {
        let placeholder = match form.flavor {
            AuthFlavorKind::Json => {
                let [major, minor, patch] = auth_client::resolve_client_version(None);
                format!("{major}.{minor}.{patch}")
            }
            _ => String::from_utf8_lossy(&auth_client::resolve_binary_version(None)).into_owned(),
        };
        spawn_field(
            panel,
            "Loader",
            &form.xiloader_version,
            &placeholder,
            ServerEditField::XiloaderVersion,
        );
    }
    spawn_field(
        panel,
        "Update URL",
        &form.version_check_url,
        "https://server/version.json",
        ServerEditField::VersionCheckUrl,
    );

    spawn_field(
        panel,
        "Client patch",
        &form.client_ver,
        LSB_CLIENT_VER,
        ServerEditField::ClientVer,
    );
    panel.spawn(hint(format!(
        "Version lock (server default: {default_lock}):"
    )));
    panel.spawn(row()).with_children(|r| {
        for (value, label) in VER_LOCK_CHOICES {
            spawn_ver_lock_button(r, label, value, form.ver_lock);
        }
    });

    panel.spawn(hint("Play from install:"));
    panel.spawn(row()).with_children(|r| {
        let current = form.preferred_client.as_deref();
        spawn_preferred_client_button(r, "Any", None, current);
        for name in &install_names {
            spawn_preferred_client_button(r, name, Some(name.clone()), current);
        }
        if let Some(saved) = current {
            if !install_names.iter().any(|n| n == saved) {
                spawn_preferred_client_button(
                    r,
                    &format!("{saved} (missing)"),
                    Some(saved.to_string()),
                    current,
                );
            }
        }
    });
}

const POL_FLAVOR_HINTS: [&str; 2] = [
    "Kuluu signs in to the PlayOnline account itself; no auth server, so the",
    "auth port is unused. Host is the FFXI lobby server the account plays on.",
];

fn save_form(form: &mut ServerEditForm, next: &mut NextState<LauncherState>) {
    form.error.clear();
    if let Err(error) =
        auth_client::validate_loader_version(form.flavor.auth_flavor(), &form.xiloader_version)
    {
        form.error = error.to_string();
        return;
    }
    if form.name.trim().is_empty() || form.host.trim().is_empty() {
        form.error = "Enter a server name and host.".into();
        return;
    }
    let auth_port = form.auth_port.parse().unwrap_or(0);
    let data_port = form.data_port.parse().unwrap_or(0);
    let view_port = form.view_port.parse().unwrap_or(0);
    let auth_port_missing = auth_port == 0 && form.flavor.uses_auth_server();
    if auth_port_missing || data_port == 0 || view_port == 0 {
        form.error = "Enter port numbers from 1 to 65535.".into();
        return;
    }
    let xiloader_version = {
        let trimmed = form.xiloader_version.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let version_check_url = {
        let trimmed = form.version_check_url.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let client_ver = {
        let trimmed = form.client_ver.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let mut store = launcher_store::load();
    let terms_acknowledged = form
        .editing_index
        .and_then(|idx| store.servers.get(idx))
        .is_some_and(|existing| existing.terms_acknowledged);
    let profile = ServerProfile {
        name: form.name.clone(),
        host: form.host.clone(),
        auth_port,
        data_port,
        view_port,
        flavor: form.flavor,
        xiloader_version,
        version_check_url,
        client_ver,
        ver_lock: form.ver_lock,
        preferred_client: form.preferred_client.clone(),
        terms_acknowledged,
    };
    match form.editing_index {
        Some(idx) if idx < store.servers.len() => store.servers[idx] = profile,
        _ => store.servers.push(profile),
    }
    if let Err(e) = launcher_store::save(&store) {
        tracing::warn!(error = %e, "launcher_store: save failed");
    }
    next.set(LauncherState::ServerSelect);
}

fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    initial: &str,
    placeholder: &str,
    binding: ServerEditField,
) {
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Px(32.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Node {
                    width: Val::Px(FIELD_LABEL_PX),
                    ..default()
                },
                Text::new(label.to_string()),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial: initial.to_string(),
                placeholder: placeholder.to_string(),
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<ServerEditForm>| match binding {
                    ServerEditField::Name => form.name = ev.value.clone(),
                    ServerEditField::Host => form.host = ev.value.clone(),
                    ServerEditField::AuthPort => {
                        if ev.value.chars().all(|c| c.is_ascii_digit()) {
                            form.auth_port = ev.value.clone();
                        }
                    }
                    ServerEditField::DataPort => {
                        if ev.value.chars().all(|c| c.is_ascii_digit()) {
                            form.data_port = ev.value.clone();
                        }
                    }
                    ServerEditField::ViewPort => {
                        if ev.value.chars().all(|c| c.is_ascii_digit()) {
                            form.view_port = ev.value.clone();
                        }
                    }
                    ServerEditField::XiloaderVersion => {
                        form.xiloader_version = ev.value.clone();
                    }
                    ServerEditField::VersionCheckUrl => {
                        form.version_check_url = ev.value.clone();
                    }
                    ServerEditField::ClientVer => {
                        form.client_ver = ev.value.clone();
                    }
                    ServerEditField::Flavor
                    | ServerEditField::VerLock
                    | ServerEditField::PreferredClient => {}
                },
            );
        });
}

fn spawn_flavor_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    kind: AuthFlavorKind,
    current: AuthFlavorKind,
) {
    let label = label.to_string();
    let variant = if kind == current {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Normal
    };
    parent
        .spawn_empty()
        .apply_scene(bsn! {
            @FeathersButton {
                @variant: variant,
                @caption: bsn! {
                    Text::new(label.clone())
                    ThemedText
                },
            }
            Node { flex_grow: 1.0 }
            InheritableFont { font: {Handle::<Font>::default()} }
        })
        .insert(FlavorButton(kind))
        .observe(
            move |_ev: On<Activate>,
                  mut form: ResMut<ServerEditForm>,
                  mut dirty: ResMut<ServerEditUiDirty>| {
                if form.flavor != kind {
                    form.select_flavor(kind);
                    dirty.0 = true;
                }
            },
        );
}

fn spawn_ver_lock_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    value: Option<u8>,
    current: Option<u8>,
) {
    let label = label.to_string();
    parent
        .spawn_empty()
        .apply_scene(bsn! {
            @FeathersButton {
                @variant: chip_variant(value == current),
                @caption: bsn! {
                    Text::new(label.clone())
                    ThemedText
                },
            }
            Node { flex_grow: 1.0 }
            InheritableFont { font: {Handle::<Font>::default()} }
        })
        .insert(VerLockButton(value))
        .observe(move |_ev: On<Activate>, mut form: ResMut<ServerEditForm>| {
            form.ver_lock = value;
        });
}

fn spawn_preferred_client_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    value: Option<String>,
    current: Option<&str>,
) {
    let label = label.to_string();
    let selected = value.as_deref() == current;
    parent
        .spawn_empty()
        .apply_scene(bsn! {
            @FeathersButton {
                @variant: chip_variant(selected),
                @caption: bsn! {
                    Text::new(label.clone())
                    ThemedText
                },
            }
            Node { flex_grow: 1.0 }
            InheritableFont { font: {Handle::<Font>::default()} }
        })
        .insert(PreferredClientButton(value.clone()))
        .observe(move |_ev: On<Activate>, mut form: ResMut<ServerEditForm>| {
            form.preferred_client = value.clone();
        });
}

fn chip_variant(selected: bool) -> ButtonVariant {
    if selected {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Normal
    }
}

pub(super) fn redraw_flavor_buttons(
    form: Res<ServerEditForm>,
    flavors: Query<(Entity, &FlavorButton)>,
    locks: Query<(Entity, &VerLockButton)>,
    clients: Query<(Entity, &PreferredClientButton)>,
    mut commands: Commands,
) {
    if !form.is_changed() {
        return;
    }
    for (e, fb) in flavors.iter() {
        commands.entity(e).insert(chip_variant(fb.0 == form.flavor));
    }
    for (e, lb) in locks.iter() {
        commands
            .entity(e)
            .insert(chip_variant(lb.0 == form.ver_lock));
    }
    for (e, pb) in clients.iter() {
        commands
            .entity(e)
            .insert(chip_variant(pb.0 == form.preferred_client));
    }
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ServerEditRoot>>) {
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

pub(super) fn redraw_system(
    form: Res<ServerEditForm>,
    flavors: Query<(Entity, &FlavorButton)>,
    locks: Query<(Entity, &VerLockButton)>,
    clients: Query<(Entity, &PreferredClientButton)>,
    commands: Commands,
    mut summary: Query<&mut Text, (With<ConnectionSummary>, Without<ServerEditError>)>,
    mut errors: Query<&mut Text, (With<ServerEditError>, Without<ConnectionSummary>)>,
) {
    if form.is_changed() {
        for mut text in &mut summary {
            text.0 = connection_summary(&form);
        }
        for mut text in &mut errors {
            text.0.clone_from(&form.error);
        }
    }
    redraw_flavor_buttons(form, flavors, locks, clients, commands);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_server_type_updates_the_loader_without_changing_the_endpoint() {
        let mut form =
            ServerEditForm::from_profile(&ServerProfile::lsb_defaults("Test", "localhost"));
        form.editing_index = Some(0);
        form.select_flavor(AuthFlavorKind::Binary);
        assert_eq!(
            form.xiloader_version,
            launcher_store::LEGACY_LSB_XILOADER_VERSION
        );
        assert_eq!(form.host, "localhost");
        assert_eq!(form.editing_index, Some(0));
        form.select_flavor(AuthFlavorKind::Json);
        assert!(form.xiloader_version.is_empty());
    }

    #[test]
    fn opening_an_existing_binary_profile_preserves_its_override() {
        let profile = ServerProfile {
            flavor: AuthFlavorKind::Binary,
            xiloader_version: Some("1.0.9".into()),
            ..ServerProfile::lsb_defaults("Test", "localhost")
        };
        let mut form = ServerEditForm::from_profile(&profile);
        form.select_flavor(AuthFlavorKind::Binary);
        assert_eq!(form.xiloader_version, profile.xiloader_version.unwrap());
    }

    #[test]
    fn invalid_loader_does_not_save_or_leave_the_form() {
        let mut form =
            ServerEditForm::from_profile(&ServerProfile::lsb_defaults("Test", "localhost"));
        form.xiloader_version = "1.1.5.9".into();
        let mut next = NextState::Unchanged;
        save_form(&mut form, &mut next);
        assert!(!form.error.is_empty());
        assert!(matches!(next, NextState::Unchanged));
    }
}
