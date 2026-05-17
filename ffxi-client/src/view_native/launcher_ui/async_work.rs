//! Async work scaffolding for the launcher Bevy app.
//!
//! Pattern (used for both `AuthInFlight` and `ConnectInFlight`):
//!
//! 1. On `OnEnter(state)`, the spawn system snapshots whatever inputs the
//!    async work needs (credentials, the lobby handle, key3, etc.),
//!    `runtime.spawn(...)`s a tokio task, and inserts a Resource holding
//!    the receiving end of a `oneshot`.
//! 2. A poll system runs every frame in `Update`, calls `try_recv()` on
//!    that oneshot. Empty → keep polling. Closed → shouldn't happen
//!    (would mean the task was dropped before sending), but treated as
//!    an error and routes to `LoginError`. Ready → consume the result
//!    and transition state accordingly.
//! 3. `OnExit(state)` removes the inflight Resource, ensuring nothing
//!    leaks.
//!
//! The Bevy event loop is never blocked: tasks run on the tokio runtime
//! threads, the main thread polls non-blockingly each frame.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use bevy::prelude::*;
use ffxi_client::auth_client::AuthClient;
use ffxi_client::lobby_client::{LobbyClient, LobbyHandle, MapHandoff};
use ffxi_client::session::InitialState;
use tokio::sync::oneshot;

use crate::launcher::Selection;

use super::{
    CharCreateError, CharCreateForm, CharListData, CreateAccountErrorMsg, CreateAccountForm,
    Credentials, LauncherClients, LauncherState, LoginErrorMsg, LoginForm, OpenedLobby,
    PendingConnect, RuntimeHandle, SelectedChar,
};

/// Auth + lobby-open in one task: succeeds when both auth and the lobby
/// `open()` returned (which is when we have the char list to render).
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
    // Carry the `AuthSession` forward — the final `InitialState.auth`
    // shipped to `session::run` must be the same one the lobby was
    // opened against (re-logging in produces a new `session_hash` that
    // doesn't match the lobby's view of the world).
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
) {
    match chan.rx.try_recv() {
        Ok(Ok(ok)) => {
            tracing::info!(
                char_count = ok.handle.chars().len(),
                "auth succeeded, transitioning to CharList"
            );
            chars.0 = ok.handle.chars().to_vec();
            // Stash the live LobbyHandle + AuthSession for the connect
            // step.
            if let Ok(mut slot) = opened.0.lock() {
                slot.handle = Some(ok.handle);
                slot.auth = Some(ok.auth);
            }
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
        Err(oneshot::error::TryRecvError::Empty) => {
            // Still in flight; come back next frame.
        }
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

// --- Connect-in-flight ----------------------------------------------------

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
        // Shouldn't happen — we only enter this state via a click that sets
        // `SelectedChar`. Bail out via an immediate error.
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
            // Lost the live lobby session somehow (resource race / extreme
            // timing). Recover by re-authenticating and reopening — same
            // shape as the stdin launcher's "open + select" fast path.
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

/// Run `select` on the live `LobbyHandle` using the `AuthSession` we
/// already hold. Mirrors the stdin launcher's behaviour: same session
/// hash flows through every step from auth → lobby open → select →
/// `InitialState`.
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

/// Fallback path: the stashed `LobbyHandle` was missing. Re-authenticate,
/// reopen the lobby, then select.
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
            // Stash the Selection for the OnEnter(AppPhase::Connecting)
            // bridge to pick up.
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

// --- Char-create in flight ------------------------------------------------

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

    // Take BOTH the live LobbyHandle and the AuthSession out of
    // OpenedLobby — we'll feed the handle straight into
    // `LobbyHandle::create_character` (which reuses its sockets, so no
    // second `open()` race). Putting fresh versions back happens in
    // the poll system on success.
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
        // Unreachable in practice: CharCreate is reached only via
        // CharList, which is reached only via a successful AuthInFlight
        // that populates both fields. Surface as a hard error rather
        // than silently retrying.
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
    // The refreshed handle is kept only so the success log can report
    // the updated char count; the poll system drops it immediately
    // (we route through AuthInFlight for a fresh session — see the
    // poll system's success branch for why).
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
            // Drop the post-create handle. Going back to CharList with
            // *this* handle and then selecting the new char would
            // trigger LSB's
            //   if (session.justCreatedNewChar) key3[16] += 6;
            // at data_session.cpp:336 — the server xors that magic +6
            // into the map UDP cipher key, but our client sends key3
            // unmodified, so every map UDP packet from the client
            // fails decryption ("bad packet from 172.18.0.1") and the
            // session GCs without zoning in. A fresh auth produces a
            // new session_t with justCreatedNewChar=false, sidestepping
            // the issue. Costs ~80ms extra; functionally required.
            if let Ok(mut slot) = opened.0.lock() {
                slot.handle = None;
                slot.auth = None;
            }
            // Repopulate LoginForm so AuthInFlight's spawn task picks
            // up the same credentials seamlessly.
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

// --- Account-create in flight --------------------------------------------

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
            // Prefill the login form so the user can press Enter
            // immediately to authenticate the new account.
            login_form.user = ok.user.clone();
            login_form.pass = ok.pass.clone();
            // Wipe the create form so backing out and re-entering
            // doesn't show stale state.
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
