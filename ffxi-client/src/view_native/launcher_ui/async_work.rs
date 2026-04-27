use std::sync::Arc;

use anyhow::{anyhow, Result};
use bevy::prelude::*;
use ffxi_client::auth_client::AuthClient;
use ffxi_client::lobby_client::{LobbyClient, LobbyHandle, MapHandoff};
use ffxi_client::session::InitialState;
use tokio::sync::oneshot;

use crate::launcher::Selection;

use super::{
    ChangePasswordForm, CharCreateError, CharCreateForm, CharListData, CreateAccountErrorMsg,
    CreateAccountForm, Credentials, LauncherClients, LauncherState, LoginErrorMsg, LoginForm,
    OpenedLobby, PendingConnect, RuntimeHandle, SelectedChar, ServerSelectForm,
};

use ffxi_client::launcher_store::{self, keyring_account_key, SavedAccount, KEYRING_SERVICE};
use ffxi_client::secret_store::SecretStore;

fn save_on_success(server_name: &str, username: &str, password: &str, remember: bool) {
    let mut store = launcher_store::load();

    store
        .accounts
        .retain(|a| !(a.server_name == server_name && a.username == username));
    store.accounts.insert(
        0,
        SavedAccount {
            server_name: server_name.to_string(),
            username: username.to_string(),
            remember_password: remember,
        },
    );
    store.last_used = Some((server_name.to_string(), username.to_string()));
    if let Err(e) = launcher_store::save(&store) {
        tracing::warn!(error = %e, "launcher_store: save failed");
    }
    let key = keyring_account_key(server_name, username);
    if remember {
        SecretStore::set(KEYRING_SERVICE, &key, password);
    } else {
        SecretStore::delete(KEYRING_SERVICE, &key);
    }
}

struct AuthOk {
    handle: LobbyHandle,
    auth: ffxi_client::auth_client::AuthSession,
    user: String,
    pass: String,
}

#[derive(Resource)]
pub(super) struct AuthInFlightChan {
    rx: oneshot::Receiver<Result<AuthOk>>,
}

#[derive(Component)]
pub(super) struct AuthUiRoot;

pub(super) fn spawn_auth_task(
    mut commands: Commands,
    runtime: Res<RuntimeHandle>,
    clients: Res<LauncherClients>,
    form: Res<LoginForm>,
    mut creds: ResMut<Credentials>,
) {
    creds.user = form.user.clone();
    creds.pass = form.pass.clone();

    let (tx, rx) = oneshot::channel();
    let auth: Arc<AuthClient> = clients.auth.clone();
    let lobby: Arc<LobbyClient> = clients.lobby.clone();
    let user = form.user.clone();
    let pass = form.pass.clone();

    runtime.0.spawn(async move {
        let res = run_auth_then_open(&auth, &lobby, &user, &pass).await;
        let _ = tx.send(res);
    });

    commands.insert_resource(AuthInFlightChan { rx });
}

async fn run_auth_then_open(
    auth: &AuthClient,
    lobby: &LobbyClient,
    user: &str,
    pass: &str,
) -> Result<AuthOk> {
    tracing::debug!(user, "auth task: logging in");
    let session = auth
        .login(user, pass)
        .await
        .map_err(|e| anyhow!("login: {e}"))?;
    tracing::debug!("auth task: login succeeded, opening lobby");
    let handle = lobby
        .open(&session)
        .await
        .map_err(|e| anyhow!("opening lobby: {e}"))?;

    tracing::info!(
        char_count = handle.chars().len(),
        "auth task: lobby opened, sending result"
    );
    Ok(AuthOk {
        handle,
        auth: session,
        user: user.to_string(),
        pass: pass.to_string(),
    })
}

pub(super) fn poll_auth_system(
    mut commands: Commands,
    mut chan: ResMut<AuthInFlightChan>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<LoginErrorMsg>,
    mut chars: ResMut<CharListData>,
    opened: Res<OpenedLobby>,
    mut creds: ResMut<Credentials>,
    form: Res<LoginForm>,
    server_form: Res<ServerSelectForm>,
    server_info: Res<super::ServerInfo>,
) {
    match chan.rx.try_recv() {
        Ok(Ok(ok)) => {
            tracing::info!(
                char_count = ok.handle.chars().len(),
                "auth succeeded, transitioning to CharList"
            );
            chars.0 = ok.handle.chars().to_vec();

            if let Ok(mut slot) = opened.0.lock() {
                slot.handle = Some(ok.handle);
                slot.auth = Some(ok.auth);
            }
            let server_name = server_form
                .selected
                .clone()
                .unwrap_or_else(|| server_info.server.clone());
            save_on_success(&server_name, &ok.user, &ok.pass, form.remember_password);
            creds.user = ok.user;
            creds.pass = ok.pass;
            commands.remove_resource::<AuthInFlightChan>();
            next_state.set(LauncherState::CharList);
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "auth failed");
            err.0 = format!("{e:#}");
            commands.remove_resource::<AuthInFlightChan>();
            next_state.set(LauncherState::LoginError);
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            tracing::error!("auth task dropped its sender unexpectedly");
            err.0 = "auth task dropped its sender unexpectedly".into();
            commands.remove_resource::<AuthInFlightChan>();
            next_state.set(LauncherState::LoginError);
        }
    }
}

pub(super) fn spawn_auth_ui(mut commands: Commands) {
    commands
        .spawn((
            AuthUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Authenticating..."),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                Text::new("Contacting auth + lobby servers."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
            ));
        });
}

pub(super) fn despawn_auth_ui(mut commands: Commands, q: Query<Entity, With<AuthUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

struct ConnectOk {
    handoff: MapHandoff,
    key3: [u8; 20],
    auth: ffxi_client::auth_client::AuthSession,
}

#[derive(Resource)]
pub(super) struct ConnectInFlightChan {
    rx: oneshot::Receiver<Result<ConnectOk>>,
}

#[derive(Component)]
pub(super) struct ConnectUiRoot;

pub(super) fn spawn_connect_task(
    mut commands: Commands,
    runtime: Res<RuntimeHandle>,
    clients: Res<LauncherClients>,
    creds: Res<Credentials>,
    sel: Res<SelectedChar>,
    opened: Res<OpenedLobby>,
) {
    let Some(slot) = sel.0.clone() else {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Err(anyhow!("no character selected")));
        commands.insert_resource(ConnectInFlightChan { rx });
        return;
    };

    let (handle, stored_auth) = match opened.0.lock() {
        Ok(mut g) => (g.handle.take(), g.auth.take()),
        Err(_) => (None, None),
    };

    match (handle, stored_auth) {
        (Some(handle), Some(auth_session)) => {
            let (tx, rx) = oneshot::channel();
            runtime.0.spawn(async move {
                let res = select_with_existing_handle(handle, &slot, auth_session).await;
                let _ = tx.send(res);
            });
            commands.insert_resource(ConnectInFlightChan { rx });
        }
        _ => {
            let (tx, rx) = oneshot::channel();
            let auth: Arc<AuthClient> = clients.auth.clone();
            let lobby: Arc<LobbyClient> = clients.lobby.clone();
            let user = creds.user.clone();
            let pass = creds.pass.clone();
            runtime.0.spawn(async move {
                let res = reopen_and_select(&auth, &lobby, &user, &pass, &slot).await;
                let _ = tx.send(res);
            });
            commands.insert_resource(ConnectInFlightChan { rx });
        }
    }
}

async fn select_with_existing_handle(
    handle: LobbyHandle,
    slot: &ffxi_client::lobby_client::CharSlot,
    auth_session: ffxi_client::auth_client::AuthSession,
) -> Result<ConnectOk> {
    let mut key3 = [0u8; 20];
    for (i, b) in key3.iter_mut().enumerate() {
        *b = ((i as u8).wrapping_mul(0x37)) ^ 0x5a;
    }
    let handoff = handle
        .select(slot.char_id, &slot.name, key3)
        .await
        .map_err(|e| anyhow!("lobby select: {e}"))?;
    Ok(ConnectOk {
        handoff,
        key3,
        auth: auth_session,
    })
}

async fn reopen_and_select(
    auth: &AuthClient,
    lobby: &LobbyClient,
    user: &str,
    pass: &str,
    slot: &ffxi_client::lobby_client::CharSlot,
) -> Result<ConnectOk> {
    let session = auth
        .login(user, pass)
        .await
        .map_err(|e| anyhow!("re-login: {e}"))?;
    let handle = lobby
        .open(&session)
        .await
        .map_err(|e| anyhow!("reopening lobby: {e}"))?;
    let mut key3 = [0u8; 20];
    for (i, b) in key3.iter_mut().enumerate() {
        *b = ((i as u8).wrapping_mul(0x37)) ^ 0x5a;
    }
    let handoff = handle
        .select(slot.char_id, &slot.name, key3)
        .await
        .map_err(|e| anyhow!("lobby select: {e}"))?;
    Ok(ConnectOk {
        handoff,
        key3,
        auth: session,
    })
}

pub(super) fn poll_connect_system(
    mut commands: Commands,
    mut chan: ResMut<ConnectInFlightChan>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<LoginErrorMsg>,
    sel: Res<SelectedChar>,
    creds: Res<Credentials>,
    mut pending: ResMut<PendingConnect>,
) {
    match chan.rx.try_recv() {
        Ok(Ok(ok)) => {
            let slot = sel
                .0
                .clone()
                .expect("SelectedChar must be set when ConnectInFlight succeeds");
            pending.0 = Some(Selection {
                user: creds.user.clone(),
                password: creds.pass.clone(),
                char_id: slot.char_id,
                char_name: slot.name,
                initial_state: InitialState {
                    auth: ok.auth,
                    handoff: ok.handoff,
                    key3: ok.key3,
                },
            });
            commands.remove_resource::<ConnectInFlightChan>();
            next_state.set(LauncherState::Done);
        }
        Ok(Err(e)) => {
            err.0 = format!("{e:#}");
            commands.remove_resource::<ConnectInFlightChan>();
            next_state.set(LauncherState::LoginError);
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            err.0 = "connect task dropped its sender unexpectedly".into();
            commands.remove_resource::<ConnectInFlightChan>();
            next_state.set(LauncherState::LoginError);
        }
    }
}

pub(super) fn spawn_connect_ui(mut commands: Commands, sel: Res<SelectedChar>) {
    let name = sel.0.as_ref().map(|s| s.name.as_str()).unwrap_or("...");
    commands
        .spawn((
            ConnectUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(format!("Selecting {name}...")),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                Text::new("Lobby select + map handoff in progress."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
            ));
        });
}

pub(super) fn despawn_connect_ui(mut commands: Commands, q: Query<Entity, With<ConnectUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

struct CharCreateOk {
    handle: LobbyHandle,
    created_name: String,
}

#[derive(Resource)]
pub(super) struct CharCreateInFlightChan {
    rx: oneshot::Receiver<Result<CharCreateOk>>,
}

#[derive(Component)]
pub(super) struct CharCreateUiRoot;

pub(super) fn spawn_char_create_task(
    mut commands: Commands,
    runtime: Res<RuntimeHandle>,
    form: Res<CharCreateForm>,
    opened: Res<OpenedLobby>,
) {
    let (tx, rx) = oneshot::channel();

    let (handle, auth) = match opened.0.lock() {
        Ok(mut g) => (g.handle.take(), g.auth.take()),
        Err(_) => (None, None),
    };

    let spec = ffxi_client::lobby_client::CharCreateSpec {
        name: form.name.clone(),
        race: form.race,
        job: form.job,
        nation: form.nation,
        size: form.size,
        face: form.face,
    };

    let (Some(handle), Some(auth)) = (handle, auth) else {
        runtime.0.spawn(async move {
            let _ = tx.send(Err(anyhow!(
                "no live lobby session available — please log in again"
            )));
        });
        commands.insert_resource(CharCreateInFlightChan { rx });
        return;
    };

    runtime.0.spawn(async move {
        let res = run_char_create(handle, auth, spec).await;
        let _ = tx.send(res);
    });
    commands.insert_resource(CharCreateInFlightChan { rx });
}

async fn run_char_create(
    handle: LobbyHandle,
    auth: ffxi_client::auth_client::AuthSession,
    spec: ffxi_client::lobby_client::CharCreateSpec,
) -> Result<CharCreateOk> {
    let name = spec.name.clone();
    tracing::info!(name = %name, race = spec.race, job = spec.job, "char-create: sending");
    let refreshed = handle
        .create_character(&auth, &spec)
        .await
        .map_err(|e| anyhow!("character creation: {e}"))?;
    tracing::info!(
        name = %name,
        char_count = refreshed.chars().len(),
        "char-create: succeeded, lobby refreshed in-place"
    );

    Ok(CharCreateOk {
        handle: refreshed,
        created_name: name,
    })
}

pub(super) fn poll_char_create_system(
    mut commands: Commands,
    mut chan: ResMut<CharCreateInFlightChan>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<CharCreateError>,
    creds: Res<Credentials>,
    mut form: ResMut<LoginForm>,
    opened: Res<OpenedLobby>,
) {
    match chan.rx.try_recv() {
        Ok(Ok(ok)) => {
            tracing::info!(
                name = %ok.created_name,
                char_count = ok.handle.chars().len(),
                "char-create: success — bouncing through AuthInFlight to clear server's justCreatedNewChar flag"
            );

            if let Ok(mut slot) = opened.0.lock() {
                slot.handle = None;
                slot.auth = None;
            }

            form.user = creds.user.clone();
            form.pass = creds.pass.clone();
            err.0.clear();
            commands.remove_resource::<CharCreateInFlightChan>();
            next_state.set(LauncherState::AuthInFlight);
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "char-create failed");
            err.0 = format!("{e:#}");
            commands.remove_resource::<CharCreateInFlightChan>();
            next_state.set(LauncherState::CharCreateError);
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            err.0 = "char-create task dropped its sender unexpectedly".into();
            commands.remove_resource::<CharCreateInFlightChan>();
            next_state.set(LauncherState::CharCreateError);
        }
    }
}

pub(super) fn spawn_char_create_ui(mut commands: Commands, form: Res<CharCreateForm>) {
    let name = form.name.clone();
    commands
        .spawn((
            CharCreateUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(format!("Creating {name}...")),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                Text::new("Name check + register-char in progress."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
            ));
        });
}

pub(super) fn despawn_char_create_ui(
    mut commands: Commands,
    q: Query<Entity, With<CharCreateUiRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

struct AccountCreateOk {
    user: String,
    pass: String,
}

#[derive(Resource)]
pub(super) struct AccountCreateInFlightChan {
    rx: oneshot::Receiver<Result<AccountCreateOk>>,
}

#[derive(Component)]
pub(super) struct AccountCreateUiRoot;

pub(super) fn spawn_account_create_task(
    mut commands: Commands,
    runtime: Res<RuntimeHandle>,
    clients: Res<LauncherClients>,
    form: Res<CreateAccountForm>,
) {
    let (tx, rx) = oneshot::channel();
    let auth: Arc<AuthClient> = clients.auth.clone();
    let user = form.user.clone();
    let pass = form.pass.clone();

    runtime.0.spawn(async move {
        tracing::info!(user = %user, "account-create: ensure_account starting");
        let res = match auth.ensure_account(&user, &pass).await {
            Ok(()) => {
                tracing::info!(user = %user, "account-create: ensure_account ok");
                Ok(AccountCreateOk { user, pass })
            }
            Err(e) => Err(anyhow!("ensure_account: {e}")),
        };
        let _ = tx.send(res);
    });
    commands.insert_resource(AccountCreateInFlightChan { rx });
}

pub(super) fn poll_account_create_system(
    mut commands: Commands,
    mut chan: ResMut<AccountCreateInFlightChan>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<CreateAccountErrorMsg>,
    mut login_form: ResMut<LoginForm>,
    mut create_form: ResMut<CreateAccountForm>,
) {
    match chan.rx.try_recv() {
        Ok(Ok(ok)) => {
            tracing::info!(user = %ok.user, "account-create: returning to Login with prefilled creds");

            login_form.user = ok.user.clone();
            login_form.pass = ok.pass.clone();

            create_form.user.clear();
            create_form.pass.clear();
            create_form.pass_confirm.clear();
            err.0.clear();
            commands.remove_resource::<AccountCreateInFlightChan>();
            next_state.set(LauncherState::Login);
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "account-create failed");
            err.0 = format!("{e:#}");
            commands.remove_resource::<AccountCreateInFlightChan>();
            next_state.set(LauncherState::CreateAccountError);
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            err.0 = "account-create task dropped its sender unexpectedly".into();
            commands.remove_resource::<AccountCreateInFlightChan>();
            next_state.set(LauncherState::CreateAccountError);
        }
    }
}

pub(super) fn spawn_account_create_ui(mut commands: Commands, form: Res<CreateAccountForm>) {
    let user = form.user.clone();
    commands
        .spawn((
            AccountCreateUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(format!("Creating account '{user}'...")),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.30, 1.0, 0.65)),
            ));
            parent.spawn((
                Text::new("Contacting connect-server."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
            ));
        });
}

pub(super) fn despawn_account_create_ui(
    mut commands: Commands,
    q: Query<Entity, With<AccountCreateUiRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

#[derive(Resource)]
pub(super) struct ChangePasswordChan {
    rx: oneshot::Receiver<Result<()>>,
}

#[derive(Component)]
pub(super) struct ChangePasswordUiRoot;

pub(super) fn spawn_change_password_task(
    mut commands: Commands,
    runtime: Res<RuntimeHandle>,
    clients: Res<LauncherClients>,
    form: Res<ChangePasswordForm>,
    login: Res<LoginForm>,
) {
    let (tx, rx) = oneshot::channel();
    let auth: Arc<AuthClient> = clients.auth.clone();
    let user = login.user.clone();
    let old = form.old.clone();
    let new_pw = form.new_pw.clone();
    runtime.0.spawn(async move {
        let res = auth
            .change_password(&user, &old, &new_pw)
            .await
            .map_err(|e| anyhow!("change_password: {e}"));
        let _ = tx.send(res);
    });
    commands.insert_resource(ChangePasswordChan { rx });
}

pub(super) fn poll_change_password_system(
    mut commands: Commands,
    mut chan: ResMut<ChangePasswordChan>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<LoginErrorMsg>,
    mut form: ResMut<ChangePasswordForm>,
    mut login: ResMut<LoginForm>,
) {
    match chan.rx.try_recv() {
        Ok(Ok(())) => {
            login.pass = form.new_pw.clone();
            form.old.clear();
            form.new_pw.clear();
            form.confirm.clear();
            form.error.clear();
            commands.remove_resource::<ChangePasswordChan>();
            next_state.set(LauncherState::Login);
        }
        Ok(Err(e)) => {
            err.0 = format!("{e:#}");
            commands.remove_resource::<ChangePasswordChan>();
            next_state.set(LauncherState::LoginError);
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            err.0 = "change-password task dropped its sender unexpectedly".into();
            commands.remove_resource::<ChangePasswordChan>();
            next_state.set(LauncherState::LoginError);
        }
    }
}

pub(super) fn spawn_change_password_ui(mut commands: Commands) {
    commands
        .spawn((
            ChangePasswordUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Changing password..."),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
        });
}

pub(super) fn despawn_change_password_ui(
    mut commands: Commands,
    q: Query<Entity, With<ChangePasswordUiRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

struct CharDeleteOk {
    handle: LobbyHandle,
}

#[derive(Resource)]
pub(super) struct CharDeleteChan {
    rx: oneshot::Receiver<Result<CharDeleteOk>>,
}

#[derive(Component)]
pub(super) struct CharDeleteUiRoot;

pub(super) fn spawn_char_delete_task(
    mut commands: Commands,
    runtime: Res<RuntimeHandle>,
    sel: Res<SelectedChar>,
    opened: Res<OpenedLobby>,
) {
    let (tx, rx) = oneshot::channel();
    let Some(slot) = sel.0.clone() else {
        runtime.0.spawn(async move {
            let _ = tx.send(Err(anyhow!("no character selected for delete")));
        });
        commands.insert_resource(CharDeleteChan { rx });
        return;
    };
    let (handle, auth) = match opened.0.lock() {
        Ok(mut g) => (g.handle.take(), g.auth.take()),
        Err(_) => (None, None),
    };
    let (Some(handle), Some(auth)) = (handle, auth) else {
        runtime.0.spawn(async move {
            let _ = tx.send(Err(anyhow!(
                "no live lobby session available — please log in again"
            )));
        });
        commands.insert_resource(CharDeleteChan { rx });
        return;
    };
    runtime.0.spawn(async move {
        let res = handle
            .delete_character(&auth, slot.char_id)
            .await
            .map(|h| CharDeleteOk { handle: h })
            .map_err(|e| anyhow!("delete_character: {e}"));
        let _ = tx.send(res);
    });
    commands.insert_resource(CharDeleteChan { rx });
}

pub(super) fn poll_char_delete_system(
    mut commands: Commands,
    mut chan: ResMut<CharDeleteChan>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<LoginErrorMsg>,
    mut chars: ResMut<CharListData>,
    opened: Res<OpenedLobby>,
    creds: Res<Credentials>,
    mut form: ResMut<LoginForm>,
) {
    match chan.rx.try_recv() {
        Ok(Ok(ok)) => {
            chars.0 = ok.handle.chars().to_vec();

            if let Ok(mut slot) = opened.0.lock() {
                slot.handle = None;
                slot.auth = None;
            }
            form.user = creds.user.clone();
            form.pass = creds.pass.clone();
            commands.remove_resource::<CharDeleteChan>();
            next_state.set(LauncherState::AuthInFlight);
        }
        Ok(Err(e)) => {
            err.0 = format!("{e:#}");
            commands.remove_resource::<CharDeleteChan>();
            next_state.set(LauncherState::LoginError);
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
        Err(oneshot::error::TryRecvError::Closed) => {
            err.0 = "char-delete task dropped its sender unexpectedly".into();
            commands.remove_resource::<CharDeleteChan>();
            next_state.set(LauncherState::LoginError);
        }
    }
}

pub(super) fn spawn_char_delete_ui(mut commands: Commands, sel: Res<SelectedChar>) {
    let name = sel
        .0
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "?".into());
    commands
        .spawn((
            CharDeleteUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(format!("Deleting {name}...")),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.20, 0.20)),
            ));
        });
}

pub(super) fn despawn_char_delete_ui(
    mut commands: Commands,
    q: Query<Entity, With<CharDeleteUiRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}
