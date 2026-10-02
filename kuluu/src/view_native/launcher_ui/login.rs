use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, checkbox_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::input_focus::{FocusCause, InputFocus, InputFocusVisible};
use bevy::prelude::*;
use bevy::ui::{Checked, ComputedNode, Overflow, ScrollPosition, UiGlobalTransform};
use bevy::ui_widgets::{Activate, ValueChange};

use crate::launcher_store::{self, keyring_account_key, keyring_square_enix_key, KEYRING_SERVICE};
use crate::secret_store::SecretStore;

use super::brand::{spawn_brand_mark, BrandMark};
use super::client_era_check::{ClientEraStatus, EraVerdict};
use super::common::{
    chip_group, hint, panel_node, pick_directional, row, screen_root, spawn_breadcrumb,
    spawn_settings_close_titlebar, Crumb, DefaultFocusTarget, ScrollRegion,
};
use super::server_edit::ver_lock_label;
use super::server_version_check::{ServerVersionStatus, VersionViolation};
use super::{
    Credentials, DatSetupReturn, LauncherClients, LauncherState, LoginErrorMsg, LoginErrorReturn,
    LoginField, LoginForm, ServerEditForm, ServerInfo, ServerSelectForm,
};
use crate::view_native::widgets::text_field::{text_field, TextField, TextFieldSubmitted};
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};
use kuluu_session::auth_client::AuthFlavor;

#[derive(Component)]
pub(super) struct LoginUiRoot;

#[derive(Component)]
pub(super) struct LoginCredentialField;

#[derive(Resource, Default)]
pub(super) struct LoginUiDirty(pub bool);

/// A saved account as the sign-in form needs it: the identity it is keyed on,
/// whether a password was kept, and the Square Enix id that rides with it.
type SavedEntry = (String, bool, String);

fn saved_accounts_for(form: &ServerSelectForm, info: &ServerInfo) -> (String, Vec<SavedEntry>) {
    let server_key = form.selected.clone().unwrap_or_else(|| info.server.clone());
    let accts = launcher_store::load()
        .accounts
        .into_iter()
        .filter(|a| a.server_name == server_key)
        .map(|a| (a.username, a.remember_password, a.square_enix_id))
        .collect();
    (server_key, accts)
}

pub(super) fn spawn_login_ui(
    mut commands: Commands,
    server: Res<ServerInfo>,
    form: Res<LoginForm>,
    server_form: Res<ServerSelectForm>,
    version: Res<ServerVersionStatus>,
    era: Res<ClientEraStatus>,
    mark: Res<BrandMark>,
    clients: Res<LauncherClients>,
) {
    build_login_ui(
        &mut commands,
        &server,
        &form,
        &server_form,
        &version,
        &era,
        &mark,
        clients.auth.flavor,
    );
}

pub(super) fn rebuild_login_ui_system(
    mut dirty: ResMut<LoginUiDirty>,
    mut commands: Commands,
    existing: Query<Entity, With<LoginUiRoot>>,
    server: Res<ServerInfo>,
    form: Res<LoginForm>,
    server_form: Res<ServerSelectForm>,
    version: Res<ServerVersionStatus>,
    era: Res<ClientEraStatus>,
    mark: Res<BrandMark>,
    clients: Res<LauncherClients>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in existing.iter() {
        commands.entity(e).despawn();
    }
    build_login_ui(
        &mut commands,
        &server,
        &form,
        &server_form,
        &version,
        &era,
        &mark,
        clients.auth.flavor,
    );
}

pub(super) fn mark_dirty_on_version_change(
    version: Res<ServerVersionStatus>,
    era: Res<ClientEraStatus>,
    mut dirty: ResMut<LoginUiDirty>,
) {
    if version.is_changed() || era.is_changed() {
        dirty.0 = true;
    }
}

fn login_blocked(version: &ServerVersionStatus, era: &ClientEraStatus) -> bool {
    version.violation == VersionViolation::BelowMinimum || era.blocks_login()
}

fn build_login_ui(
    commands: &mut Commands,
    server: &ServerInfo,
    form: &LoginForm,
    server_form: &ServerSelectForm,
    version: &ServerVersionStatus,
    era: &ClientEraStatus,
    mark: &BrandMark,
    flavor: AuthFlavor,
) {
    let user_initial = form.user.clone();
    let pass_initial = form.pass.clone();
    let pol_id_initial = form.pol_id.clone();
    let pol_pass_initial = form.pol_pass.clone();
    let remember = form.remember_password;
    let active_user = form
        .account_key(flavor == AuthFlavor::PlayOnline)
        .to_string();
    let (server_key, accts) = saved_accounts_for(server_form, server);
    let playonline = flavor == AuthFlavor::PlayOnline;

    commands
        .spawn((LoginUiRoot, screen_root()))
        .with_children(|root| {
            spawn_brand_mark(root, mark);
            spawn_breadcrumb(root, server, &[Crumb::Sign(None)]);
            root.spawn(panel_node(560.0)).with_children(|panel| {
                spawn_settings_close_titlebar(
                    panel,
                    format!("Sign in to {}", server.display_label()),
                );

                spawn_version_banner(panel, version);
                spawn_client_era_banner(panel, era);

                if playonline && !terms_acknowledged(server.profile_name.as_deref()) {
                    spawn_terms_gate(panel, server.profile_name.as_deref());
                    return;
                }

                {
                    spawn_saved_accounts_row(panel, &server_key, &active_user, &accts, playonline);

                    if playonline {
                        for line in POL_CREDENTIAL_HINTS {
                            panel.spawn((Text::new(line), ThemedText));
                        }
                        spawn_field(
                            panel,
                            "PlayOnline ID",
                            false,
                            &pol_id_initial,
                            LoginField::PolId,
                            playonline,
                        );
                        spawn_field(
                            panel,
                            "PlayOnline password",
                            true,
                            &pol_pass_initial,
                            LoginField::PolPassword,
                            playonline,
                        );
                    }

                    let user_label = if playonline {
                        "Square Enix ID"
                    } else {
                        "Username"
                    };
                    let pass_label = if playonline {
                        "Square Enix password"
                    } else {
                        "Password"
                    };
                    spawn_field(
                        panel,
                        user_label,
                        false,
                        &user_initial,
                        LoginField::User,
                        playonline,
                    );
                    spawn_field(
                        panel,
                        pass_label,
                        true,
                        &pass_initial,
                        LoginField::Password,
                        playonline,
                    );

                    {
                        let mut cb = panel.spawn(checkbox_bundle(
                            (),
                            Spawn((Text::new("Remember password"), ThemedText)),
                        ));
                        if remember {
                            cb.insert(Checked);
                        }
                        cb.observe(
                            |ev: On<ValueChange<bool>>,
                             mut form: ResMut<LoginForm>,
                             mut commands: Commands| {
                                form.remember_password = ev.value;
                                if ev.value {
                                    commands.entity(ev.source).insert(Checked);
                                } else {
                                    commands.entity(ev.source).remove::<Checked>();
                                }
                            },
                        );
                    }

                    let blocked = login_blocked(version, era);

                    panel.spawn(row()).with_children(|r| {
                        if !blocked {
                            r.spawn(button_bundle(
                            ButtonBundleProps {
                                variant: ButtonVariant::Primary,
                                ..default()
                            },
                            (),
                            Spawn((Text::new("Log in"), ThemedText)),
                        ))
                        .insert(DefaultFocusTarget)
                        .observe(
                            move |_ev: On<Activate>,
                                  form: Res<LoginForm>,
                                  mut next: ResMut<NextState<LauncherState>>| {
                                if form.is_complete(playonline) {
                                    next.set(LauncherState::AuthInFlight);
                                }
                            },
                        );
                        }
                        if playonline {
                            return;
                        }

                        r.spawn(button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new("Create account"), ThemedText)),
                        ))
                        .insert_if(DefaultFocusTarget, || blocked)
                        .observe(
                            |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                                next.set(LauncherState::CreateAccount);
                            },
                        );

                        r.spawn(button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new("Change password"), ThemedText)),
                        ))
                        .observe(
                            |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                                next.set(LauncherState::ChangePassword);
                            },
                        );
                    });
                }
            });
        });
}

/// The label column has to hold the longest of the PlayOnline field names.
const FIELD_LABEL_WIDTH: f32 = 160.0;

/// A PlayOnline account carries both identities and the handshake uses both,
/// so the form says which is which in the Viewer's own words.
const POL_CREDENTIAL_HINTS: [&str; 3] = [
    "Sign in with both of the account's identities, as the PlayOnline Viewer",
    "asks for them. A PlayOnline ID is four capitals then four digits. The",
    "Square Enix ID is the login name you chose, not your email address.",
];

/// LEGAL.md section 7, said once per profile where the player signs in.
const POL_TERMS_NOTICE: [&str; 3] = [
    "Connecting with a third-party client may breach the terms of service of",
    "the server you connect to. Kuluu does not patch or inject into any Square",
    "Enix program; any consequence to your account is yours alone.",
];

fn terms_acknowledged(profile_name: Option<&str>) -> bool {
    let Some(name) = profile_name else {
        return false;
    };
    launcher_store::load()
        .servers
        .iter()
        .any(|p| p.name == name && p.terms_acknowledged)
}

fn acknowledge_terms(profile_name: Option<&str>) {
    let Some(name) = profile_name else {
        return;
    };
    let mut store = launcher_store::load();
    for profile in store.servers.iter_mut().filter(|p| p.name == name) {
        profile.terms_acknowledged = true;
    }
    if let Err(e) = launcher_store::save(&store) {
        tracing::warn!(error = %e, "launcher_store: save failed");
    }
}

/// A profile that signs in to the official service shows the notice once;
/// the sign-in form appears after the player has read it.
fn spawn_terms_gate(panel: &mut ChildSpawnerCommands, profile_name: Option<&str>) {
    for line in POL_TERMS_NOTICE {
        panel.spawn(hint(line));
    }
    let name = profile_name.map(str::to_string);
    panel.spawn(row()).with_children(|r| {
        r.spawn(button_bundle(
            ButtonBundleProps {
                variant: ButtonVariant::Primary,
                ..default()
            },
            (),
            Spawn((Text::new("I understand"), ThemedText)),
        ))
        .insert(DefaultFocusTarget)
        .observe(move |_ev: On<Activate>, mut dirty: ResMut<LoginUiDirty>| {
            acknowledge_terms(name.as_deref());
            dirty.0 = true;
        });
    });
}

fn spawn_saved_accounts_row(
    panel: &mut ChildSpawnerCommands,
    server_key: &str,
    active_user: &str,
    accts: &[SavedEntry],
    playonline: bool,
) {
    if accts.is_empty() {
        return;
    }

    // Grows with content up to roughly five wrapped chip rows; beyond that,
    // accounts scroll inside the region. Matters most at narrow widths where
    // chips wrap onto many rows.
    const SAVED_ACCOUNTS_MAX_HEIGHT: f32 = 190.0;

    panel.spawn(hint("Saved accounts on this server:"));
    panel
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                row_gap: Val::Px(6.0),
                max_height: Val::Px(SAVED_ACCOUNTS_MAX_HEIGHT),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ScrollPosition::default(),
            ScrollRegion,
        ))
        .with_children(|r| {
            for (u, remember, square_enix_id) in accts.iter() {
                let label = if *remember {
                    format!("{u}  [saved]")
                } else {
                    u.clone()
                };
                let is_active = u == active_user;
                let variant = if is_active {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Normal
                };
                let pick_user = u.clone();
                let pick_server = server_key.to_string();
                let pick_remember = *remember;
                let pick_playonline = playonline;
                let pick_square_enix_id = square_enix_id.clone();

                let forget_user = u.clone();
                let forget_server = server_key.to_string();
                let forget_playonline = playonline;

                r.spawn(chip_group()).with_children(|chip| {
                    chip.spawn(button_bundle(
                        ButtonBundleProps {
                            variant,
                            ..default()
                        },
                        (),
                        Spawn((Text::new(label), ThemedText)),
                    ))
                    .observe(
                        move |_ev: On<Activate>,
                              mut login: ResMut<LoginForm>,
                              mut dirty: ResMut<LoginUiDirty>| {
                            // The saved account is keyed on whichever identity
                            // the flavor always has, so a PlayOnline chip names
                            // the PlayOnline id and carries the other beside it.
                            if pick_playonline {
                                login.pol_id = pick_user.clone();
                                login.user = pick_square_enix_id.clone();
                            } else {
                                login.user = pick_user.clone();
                            }
                            login.pass.clear();
                            login.pol_pass.clear();
                            login.remember_password = pick_remember;
                            if pick_remember {
                                let primary = SecretStore::get(
                                    KEYRING_SERVICE,
                                    &keyring_account_key(&pick_server, &pick_user),
                                );
                                let secondary = SecretStore::get(
                                    KEYRING_SERVICE,
                                    &keyring_square_enix_key(&pick_server, &pick_user),
                                );
                                if pick_playonline {
                                    login.pol_pass = primary.unwrap_or_default();
                                    login.pass = secondary.unwrap_or_default();
                                } else {
                                    login.pass = primary.unwrap_or_default();
                                }
                            }
                            login.focus = if login.pass.is_empty() {
                                LoginField::Password
                            } else {
                                LoginField::User
                            };

                            dirty.0 = true;
                        },
                    );

                    chip.spawn(button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new("X"), ThemedText)),
                    ))
                    .observe(
                        move |_ev: On<Activate>,
                              mut login: ResMut<LoginForm>,
                              mut dirty: ResMut<LoginUiDirty>| {
                            let mut store = launcher_store::load();
                            store.accounts.retain(|a| {
                                !(a.server_name == forget_server && a.username == forget_user)
                            });
                            if let Some((s, u)) = &store.last_used {
                                if *s == forget_server && *u == forget_user {
                                    store.last_used = None;
                                }
                            }
                            if let Err(e) = launcher_store::save(&store) {
                                tracing::warn!(error = %e, "launcher_store: save failed");
                            }
                            SecretStore::delete(
                                KEYRING_SERVICE,
                                &keyring_account_key(&forget_server, &forget_user),
                            );
                            SecretStore::delete(
                                KEYRING_SERVICE,
                                &keyring_square_enix_key(&forget_server, &forget_user),
                            );

                            if login.account_key(forget_playonline) == forget_user {
                                login.user.clear();
                                login.pass.clear();
                                login.pol_id.clear();
                                login.pol_pass.clear();
                                login.remember_password = false;
                                login.focus = LoginField::User;
                            }
                            dirty.0 = true;
                        },
                    );
                });
            }

            r.spawn(chip_group()).with_children(|chip| {
                chip.spawn(button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("+"), ThemedText)),
                ))
                .observe(
                    |_ev: On<Activate>,
                     mut login: ResMut<LoginForm>,
                     mut dirty: ResMut<LoginUiDirty>| {
                        login.user.clear();
                        login.pass.clear();
                        login.pol_id.clear();
                        login.pol_pass.clear();
                        login.remember_password = false;
                        login.focus = LoginField::User;
                        dirty.0 = true;
                    },
                );
            });
        });
}

fn spawn_version_banner(panel: &mut ChildSpawnerCommands, version: &ServerVersionStatus) {
    let (border, text_color, msg) = match version.violation {
        VersionViolation::Ok => return,
        VersionViolation::BelowRecommended => {
            let rec = version.recommended.clone().unwrap_or_default();
            (
                BANNER_WARN_BORDER,
                BANNER_WARN_TEXT,
                format!(
                    "This server recommends Kuluu {rec}; you are running Kuluu {}. Some features may not work.",
                    version.current
                ),
            )
        }
        VersionViolation::BelowMinimum => {
            let min = version.minimum.clone().unwrap_or_default();
            (
                BANNER_BLOCK_BORDER,
                BANNER_BLOCK_TEXT,
                format!(
                    "This server requires Kuluu {min}; you are running Kuluu {}. Update before logging in.",
                    version.current
                ),
            )
        }
    };

    panel
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            BorderColor::all(border),
        ))
        .with_children(|bar| {
            bar.spawn((
                Text::new(msg),
                TextFont {
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(text_color),
                ThemedText,
            ));
        });
}

const BANNER_WARN_BORDER: Color = Color::srgb(0.55, 0.45, 0.10);
const BANNER_WARN_TEXT: Color = Color::srgb(1.0, 0.85, 0.30);
const BANNER_BLOCK_BORDER: Color = Color::srgb(0.55, 0.15, 0.15);
const BANNER_BLOCK_TEXT: Color = Color::srgb(1.0, 0.40, 0.40);

/// Worded around the FINAL FANTASY XI install so it cannot be read as the
/// Kuluu app-version banner above it.
fn client_era_message(era: &ClientEraStatus) -> Option<String> {
    let lock = era.lock.map(ver_lock_label).unwrap_or("not enforced");
    let install = format!(
        "the selected install '{}' is era {}",
        era.install_name(),
        era.install_stamp()
    );
    let mut msg = match era.verdict {
        EraVerdict::Unchecked | EraVerdict::Ok => return None,
        EraVerdict::Refused => format!(
            "This server admits FINAL FANTASY XI clients from era {} ({lock}); {install} and \
             its lobby would refuse it.",
            era.expected
        ),
        EraVerdict::Warn if era.install_stamp() == "unknown" => format!(
            "The patch stamp of the selected install '{}' could not be read; this server \
             admits FINAL FANTASY XI clients from era {} ({lock}).",
            era.install_name(),
            era.expected
        ),
        EraVerdict::Warn if !era.configured => format!(
            "This server entry does not record which FINAL FANTASY XI client era it admits; \
             {install}. Current LandSandBoat servers expect {} ({lock}).",
            era.expected
        ),
        EraVerdict::Warn if era.preferred_mismatch().is_none() => format!(
            "This server admits FINAL FANTASY XI clients from era {} ({lock}); {install}. Zone \
             text and cast timing may come from the wrong era.",
            era.expected
        ),
        EraVerdict::Warn => String::new(),
    };
    if let Some(preferred) = era.preferred_mismatch() {
        if !msg.is_empty() {
            msg.push(' ');
        }
        msg.push_str(&format!(
            "This entry prefers the '{preferred}' install; '{}' is selected.",
            era.install_name()
        ));
    }
    Some(msg)
}

fn spawn_client_era_banner(panel: &mut ChildSpawnerCommands, era: &ClientEraStatus) {
    let Some(msg) = client_era_message(era) else {
        return;
    };
    let (border, text_color) = if era.blocks_login() {
        (BANNER_BLOCK_BORDER, BANNER_BLOCK_TEXT)
    } else {
        (BANNER_WARN_BORDER, BANNER_WARN_TEXT)
    };
    let server_name = era.server_name.clone();
    panel
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            BorderColor::all(border),
        ))
        .with_children(|bar| {
            bar.spawn((
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
                Text::new(msg),
                TextFont {
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(text_color),
                ThemedText,
            ));
            bar.spawn(row()).with_children(|r| {
                r.spawn(button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("Choose install..."), ThemedText)),
                ))
                .observe(
                    |_ev: On<Activate>,
                     mut ret: ResMut<DatSetupReturn>,
                     mut next: ResMut<NextState<LauncherState>>| {
                        ret.0 = Some(LauncherState::Login);
                        next.set(LauncherState::DatSetup);
                    },
                );
                if let Some(name) = server_name {
                    r.spawn(button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new("Edit server..."), ThemedText)),
                    ))
                    .observe(
                        move |_ev: On<Activate>,
                              mut form: ResMut<ServerEditForm>,
                              mut next: ResMut<NextState<LauncherState>>| {
                            let store = launcher_store::load();
                            let Some(idx) = store.servers.iter().position(|p| p.name == name)
                            else {
                                return;
                            };
                            *form = ServerEditForm::from_profile(&store.servers[idx]);
                            form.editing_index = Some(idx);
                            form.show_advanced = true;
                            next.set(LauncherState::ServerEdit);
                        },
                    );
                }
            });
        });
}

fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    mask: bool,
    initial: &str,
    binding: LoginField,
    playonline: bool,
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
                    width: Val::Px(FIELD_LABEL_WIDTH),
                    ..default()
                },
                Text::new(label.to_string()),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial: initial.to_string(),
                mask,
                submit_on_enter: true,
                ..default()
            }))
            .insert(LoginCredentialField)
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<LoginForm>| match binding {
                    LoginField::User => form.user = ev.value.clone(),
                    LoginField::Password => form.pass = ev.value.clone(),
                    LoginField::PolId => form.pol_id = ev.value.clone(),
                    LoginField::PolPassword => form.pol_pass = ev.value.clone(),
                },
            )
            .observe(
                move |_ev: On<TextFieldSubmitted>,
                      form: Res<LoginForm>,
                      version: Res<ServerVersionStatus>,
                      era: Res<ClientEraStatus>,
                      mut next: ResMut<NextState<LauncherState>>| {
                    if login_blocked(&version, &era) {
                        return;
                    }
                    if form.is_complete(playonline) {
                        next.set(LauncherState::AuthInFlight);
                    }
                },
            );
        });
}

pub(super) fn despawn_login_ui(mut commands: Commands, q: Query<Entity, With<LoginUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

/// Login is the root of the launcher's back tree - the default startup state
/// and the back target of every other screen - so Escape has nowhere to back
/// out to. The pad's Cancel lands here too; at the root it cancels the form.
/// A back hop to ServerSelect would loop: its Escape returns to Login.
///
/// Real keyboard Enter submits whenever BOTH fields are filled, regardless of
/// which widget (if any) holds UI focus. The per-field TextFieldSubmitted path
/// only fires for a FOCUSED field and stays silent when the other side is
/// still empty - that was the "pressing enter does nothing" dead end.
pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<LoginForm>,
    version: Res<ServerVersionStatus>,
    era: Res<ClientEraStatus>,
    mut next: ResMut<NextState<LauncherState>>,
    mut fields: Query<&mut TextField, With<LoginCredentialField>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match ev.logical_key {
            Key::Escape => {
                form.user.clear();
                form.pass.clear();
                for mut field in &mut fields {
                    field.value.clear();
                    field.cursor = 0;
                }
                return;
            }
            Key::Enter
                if !login_blocked(&version, &era)
                    && !form.user.is_empty()
                    && !form.pass.is_empty() =>
            {
                next.set(LauncherState::AuthInFlight);
                return;
            }
            _ => {}
        }
    }
}

/// Arrow-key navigation for the login form: move the blue focus outline between
/// tabbable widgets (saved-account chips, fields, remember checkbox, buttons)
/// in visual order, wrapping at the edges. While a text field holds focus,
/// Left/Right stay with the caret, not the selection; Up/Down still navigate.
/// The per-widget center estimate uses a uniform convention across all nodes;
/// only relative positions matter for scoring.
pub(super) fn arrow_nav_system(
    mut events: MessageReader<KeyboardInput>,
    mut input_focus: ResMut<InputFocus>,
    mut visible: ResMut<InputFocusVisible>,
    q_tabs: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<TabIndex>>,
    q_fields: Query<(), With<TextField>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let dir = match ev.logical_key {
            Key::ArrowUp => Vec2::new(0.0, -1.0),
            Key::ArrowDown => Vec2::new(0.0, 1.0),
            Key::ArrowLeft => Vec2::new(-1.0, 0.0),
            Key::ArrowRight => Vec2::new(1.0, 0.0),
            _ => continue,
        };

        let cur = input_focus.get();
        if dir.x != 0.0 && cur.is_some_and(|e| q_fields.contains(e)) {
            continue;
        }

        let cands: Vec<(Vec2, Entity)> = q_tabs
            .iter()
            .map(|(e, cn, gt)| (gt.affine().translation + cn.size * 0.5, e))
            .collect();
        let centers: Vec<Vec2> = cands.iter().map(|(p, _)| *p).collect();
        let current = cur.and_then(|c| cands.iter().position(|(_, e)| *e == c));
        let Some(i) = pick_directional(&centers, current, dir) else {
            continue;
        };
        input_focus.set(cands[i].1, FocusCause::Navigated);
        visible.0 = true;
    }
}

pub(super) fn redraw_login_form_system() {}

#[derive(Component)]
pub(super) struct ErrorUiRoot;

pub(super) fn spawn_error_ui(
    mut commands: Commands,
    msg: Res<LoginErrorMsg>,
    ret: Res<LoginErrorReturn>,
) {
    let body = msg.0.clone();
    let (heading, back_label) = match *ret {
        // The account is still signed in and the character list is intact -
        // a lobby-select / map-handoff failure only needs another pick.
        LoginErrorReturn::CharList => ("Couldn't enter world", "Back to characters"),
        LoginErrorReturn::Login => ("Login failed", "Back to login"),
    };
    commands
        .spawn((ErrorUiRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(520.0)).with_children(|panel| {
                panel.spawn((
                    Text::new(heading),
                    TextFont {
                        font_size: 22.0.into(),
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.20, 0.20)),
                    ThemedText,
                ));
                panel.spawn((
                    Text::new(body),
                    TextFont {
                        font_size: 14.0.into(),
                        ..default()
                    },
                    TextColor(Color::srgb(0.85, 0.85, 0.85)),
                    ThemedText,
                ));
                panel
                    .spawn(button_bundle(
                        ButtonBundleProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        DefaultFocusTarget,
                        Spawn((Text::new(back_label), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         ret: Res<LoginErrorReturn>,
                         mut err: ResMut<LoginErrorMsg>,
                         mut form: ResMut<LoginForm>,
                         mut creds: ResMut<Credentials>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            back_from_error(*ret, &mut err, &mut form, &mut creds, &mut next);
                        },
                    );
            });
        });
}

fn back_from_error(
    ret: LoginErrorReturn,
    err: &mut LoginErrorMsg,
    form: &mut LoginForm,
    creds: &mut Credentials,
    next: &mut NextState<LauncherState>,
) {
    // Dismissing the error consumes the message; a fresh disconnect/failure
    // re-sets it, so leaving it would only re-trigger a phantom error later.
    err.0.clear();
    match ret {
        // Keep the live credentials so a re-pick can reopen the lobby.
        LoginErrorReturn::CharList => next.set(LauncherState::CharList),
        LoginErrorReturn::Login => {
            form.pass.clear();
            creds.user.clear();
            creds.pass.clear();
            next.set(LauncherState::Login);
        }
    }
}

pub(super) fn despawn_error_ui(mut commands: Commands, q: Query<Entity, With<ErrorUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn error_keyboard_system(
    mut events: MessageReader<KeyboardInput>,
    ret: Res<LoginErrorReturn>,
    mut err: ResMut<LoginErrorMsg>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut form: ResMut<LoginForm>,
    mut creds: ResMut<Credentials>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            back_from_error(*ret, &mut err, &mut form, &mut creds, &mut next_state);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::PrimaryWindow;

    fn escape_app() -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .init_resource::<NextState<LauncherState>>()
            .insert_resource(LoginForm {
                user: "cow".into(),
                pass: "moo".into(),
                ..Default::default()
            })
            .insert_resource(ServerVersionStatus::default())
            .insert_resource(ClientEraStatus::default())
            .add_systems(Update, keyboard_input_system);
        app
    }

    fn press_escape(app: &mut App, window: Entity) {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: Key::Escape,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
    }

    #[test]
    fn escape_at_login_wipes_credentials_without_leaving_the_screen() {
        let mut app = escape_app();
        let window = app.world_mut().spawn(PrimaryWindow).id();
        let fields: Vec<_> = ["cow", "moo"]
            .into_iter()
            .map(|value| {
                app.world_mut()
                    .spawn((
                        LoginCredentialField,
                        TextField {
                            value: value.into(),
                            cursor: value.len(),
                            ..Default::default()
                        },
                    ))
                    .id()
            })
            .collect();
        press_escape(&mut app, window);
        app.update();
        let form = app.world().resource::<LoginForm>();
        assert!(form.user.is_empty(), "Escape must clear the user field");
        assert!(form.pass.is_empty(), "Escape must clear the password field");
        for field in fields {
            let editor = app.world().get::<TextField>(field).unwrap();
            assert!(editor.value.is_empty());
            assert_eq!(editor.cursor, 0);
        }
        assert!(
            matches!(
                *app.world().resource::<NextState<LauncherState>>(),
                NextState::Unchanged
            ),
            "Login is the back-tree root: Escape must not request a screen change"
        );
    }
}
