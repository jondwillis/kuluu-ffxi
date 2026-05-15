//! Native windowed launcher: graphical login + character-select flow.
//!
//! Sibling to `crate::launcher` (the stdin/stdout TUI launcher). When the
//! `native` subcommand is invoked without all three positional args, we
//! show the login form in the same window the in-game viewer eventually
//! takes over.
//!
//! # Architecture: one Bevy `App`
//!
//! winit-0.30 enforces a process-singleton `EventLoop` (see
//! `winit-0.30.13/src/event_loop.rs:118`), so the launcher and the
//! in-game viewer **must** share a single `App`. The launcher is now a
//! set of state-driven systems registered via [`register`] onto the
//! caller's app. Entry to `LauncherState::Done` transitions
//! `AppPhase::Connecting`; the bridge there picks up [`PendingConnect`]
//! and continues into the viewer.
//!
//! `LauncherState` is a [`SubStates`] of `AppPhase::Launcher` — Bevy
//! creates and destroys the `State<LauncherState>` resource
//! automatically as `AppPhase` enters and leaves `Launcher`.
//!
//! # State machine
//!
//! ```text
//!                              ┌────────────────┐
//!                              │  LoginError    │◀─┐
//!                              └────────┬───────┘  │
//!                            Esc        │          │
//!     ┌─────────┐    Enter   ▼   error  │          │
//!     │  Login  │────────▶ AuthInFlight ┘          │
//!     └─────────┘             │                    │
//!         ▲                   │ ok                 │
//!         │ Esc               ▼                    │
//!         │             ┌──────────┐               │
//!         └─────────────│ CharList │               │
//!                       └──────┬───┘               │
//!                              │ click             │
//!                              ▼                   │
//!                       ┌────────────────┐         │
//!                       │ ConnectInFlight├─error ──┘
//!                       └──────┬─────────┘
//!                              │ ok
//!                              ▼
//!                       ┌────────────────┐
//!                       │     Done       │ → AppPhase::Connecting
//!                       └────────────────┘
//! ```

mod async_work;
mod char_list;
mod login;

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ffxi_client::auth_client::AuthClient;
use ffxi_client::lobby_client::LobbyClient;
use tokio::runtime::Handle as RtHandle;

use crate::launcher::{Defaults, Selection};

use super::AppPhase;

/// Bevy state driving the launcher UI. `SubStates` of
/// `AppPhase::Launcher` — only exists while the parent phase is
/// `Launcher`. Re-created at `#[default]` (`Login`) on every entry to
/// `AppPhase::Launcher`, including return-from-failure.
#[derive(SubStates, Default, Debug, Clone, Eq, PartialEq, Hash)]
#[source(super::AppPhase = super::AppPhase::Launcher)]
pub(crate) enum LauncherState {
    #[default]
    Login,
    AuthInFlight,
    CharList,
    ConnectInFlight,
    LoginError,
    /// Terminal for the launcher: triggers transition to
    /// `AppPhase::Connecting`. The bridge system there picks up
    /// `PendingConnect` and continues the flow.
    Done,
}

/// Where the focus lives in the login form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum LoginField {
    #[default]
    User,
    Password,
}

/// Login form contents, edited in place by `login::keyboard_input_system`
/// and read by `login::redraw_form_system`.
#[derive(Resource, Default)]
pub(crate) struct LoginForm {
    pub user: String,
    pub pass: String,
    pub focus: LoginField,
}

/// Carries the failure message displayed by the `LoginError` state.
/// Survives across `AppPhase` transitions: when `Connecting` fails it's
/// populated, then the bridge drops `AppPhase` back to `Launcher` and
/// the LoginError state reads this resource.
#[derive(Resource, Default)]
pub(crate) struct LoginErrorMsg(pub String);

/// The runtime handle systems use to spawn async auth/lobby work
/// without blocking the Bevy event loop.
#[derive(Resource, Clone)]
pub(crate) struct RuntimeHandle(pub RtHandle);

/// Server hostname (display only; the auth/lobby clients already hold it).
#[derive(Resource, Clone)]
pub(crate) struct ServerInfo {
    pub server: String,
}

/// Auth + lobby clients shared across launcher systems.
#[derive(Resource, Clone)]
pub(crate) struct LauncherClients {
    pub auth: Arc<AuthClient>,
    pub lobby: Arc<LobbyClient>,
}

/// State carried between `AuthInFlight` (which opens the lobby) and
/// `ConnectInFlight` (which selects a character on it). The
/// `LobbyHandle` is consumed by `select`, and the original
/// `AuthSession` must flow through into the final `InitialState.auth`
/// (re-logging in mid-handshake would produce a new `session_hash`
/// that doesn't match the lobby socket the server is tracking).
/// Wrapped in `Mutex` because `LobbyHandle` is not `Sync`.
#[derive(Default)]
pub(crate) struct OpenedLobbyInner {
    pub handle: Option<ffxi_client::lobby_client::LobbyHandle>,
    pub auth: Option<ffxi_client::auth_client::AuthSession>,
}

#[derive(Resource, Default)]
pub(crate) struct OpenedLobby(pub Mutex<OpenedLobbyInner>);

/// Auth credentials carried from `Login` through the rest of the flow,
/// so the final `Selection` can echo them back to the bridge.
#[derive(Resource, Clone, Default)]
pub(crate) struct Credentials {
    pub user: String,
    pub pass: String,
}

/// Char list snapshot pulled from `LobbyHandle::chars()` once the lobby
/// is open. Stored separately so the menu UI doesn't have to reach
/// through the `Mutex`.
#[derive(Resource, Default)]
pub(crate) struct CharListData(pub Vec<ffxi_client::lobby_client::CharSlot>);

/// The character the user picked. Set by `char_list` on click; consumed
/// by the connect-in-flight system.
#[derive(Resource, Default)]
pub(crate) struct SelectedChar(pub Option<ffxi_client::lobby_client::CharSlot>);

/// The launcher's terminal output: the connecting bridge in
/// `super::run` consumes this on entering `AppPhase::Connecting`,
/// builds a `session::Config`, calls `spawn_session`, and inserts
/// `NativeSource`/`CommandTx` into the world.
#[derive(Resource, Default)]
pub(crate) struct PendingConnect(pub Option<Selection>);

/// Optional default char name pulled from CLI args. Used by `char_list`
/// to highlight a matching row, and by `direct_mode_charlist_autoselect`
/// to auto-click that row when present.
#[derive(Resource, Default)]
pub(crate) struct DefaultCharName(pub Option<String>);

/// Marker resource. When present (set by `main.rs` when all three CLI
/// args are supplied) the launcher auto-advances past both `Login` (if
/// creds are prefilled) and `CharList` (if the named char exists).
/// Removed at the natural ends of the auto-advance chain:
///   - by `direct_mode_charlist_autoselect` when the named char is
///     successfully picked (full auto-advance succeeded);
///   - on `OnEnter(LauncherState::LoginError)` (auth or lobby failed;
///     user must retype creds rather than enter a retry loop).
#[derive(Resource)]
pub(crate) struct DirectModeAutostart;

/// 2D-camera marker. Spawned `OnEnter(AppPhase::Launcher)`, despawned
/// `OnExit(AppPhase::Launcher)` so the in-game 3D camera (spawned by
/// `OnEnter(AppPhase::InGame)`) doesn't compete with this 2D one.
#[derive(Component)]
pub(crate) struct LauncherCamera;

/// Register the launcher's resources, sub-state, and systems on the
/// caller's app. Called from `view_native::run`. The caller is
/// responsible for `init_state::<AppPhase>()` and adding the
/// `OnEnter(AppPhase::Connecting)` bridge.
pub(crate) fn register(
    app: &mut App,
    server: &str,
    auth: Arc<AuthClient>,
    lobby: Arc<LobbyClient>,
    defaults: Defaults,
    runtime: RtHandle,
) {
    let mut form = LoginForm::default();
    if let Some(u) = defaults.user {
        form.user = u;
    }
    if let Some(p) = defaults.password {
        form.pass = p;
    }

    app.add_sub_state::<LauncherState>()
        .insert_resource(form)
        .insert_resource(LoginErrorMsg::default())
        .insert_resource(RuntimeHandle(runtime))
        .insert_resource(ServerInfo {
            server: server.to_string(),
        })
        .insert_resource(LauncherClients { auth, lobby })
        .insert_resource(OpenedLobby::default())
        .insert_resource(Credentials::default())
        .insert_resource(CharListData::default())
        .insert_resource(SelectedChar::default())
        .insert_resource(PendingConnect::default())
        .insert_resource(DefaultCharName(defaults.char_name));

    // Launcher's 2D camera tracks the launcher phase exactly. The
    // in-game 3D camera spawns OnEnter(AppPhase::InGame) — see
    // `super::run`.
    app.add_systems(OnEnter(AppPhase::Launcher), spawn_launcher_camera)
        .add_systems(OnExit(AppPhase::Launcher), despawn_launcher_camera);

    // Re-entry hook: if returning to Launcher from a failed Connecting
    // bridge, jump straight to LoginError.
    app.add_systems(OnEnter(AppPhase::Launcher), restore_login_error_on_reentry);

    // Login screen: builds UI on enter, eats keys, redraws on each frame
    // it's active.
    app.add_systems(OnEnter(LauncherState::Login), login::spawn_login_ui)
        .add_systems(OnExit(LauncherState::Login), login::despawn_login_ui)
        .add_systems(
            Update,
            (
                direct_mode_login_autostart,
                login::keyboard_input_system,
                login::redraw_login_form_system,
            )
                .run_if(in_state(LauncherState::Login)),
        );

    // Auth in flight: spawn task on enter, poll its oneshot every frame.
    app.add_systems(
        OnEnter(LauncherState::AuthInFlight),
        (async_work::spawn_auth_task, async_work::spawn_auth_ui),
    )
    .add_systems(
        OnExit(LauncherState::AuthInFlight),
        async_work::despawn_auth_ui,
    )
    .add_systems(
        Update,
        async_work::poll_auth_system.run_if(in_state(LauncherState::AuthInFlight)),
    );

    // Char list: spawn UI from the snapshot, dispatch on click.
    app.add_systems(
        OnEnter(LauncherState::CharList),
        char_list::spawn_char_list_ui,
    )
    .add_systems(
        OnExit(LauncherState::CharList),
        char_list::despawn_char_list_ui,
    )
    .add_systems(
        Update,
        (
            direct_mode_charlist_autoselect,
            char_list::handle_click_system,
            char_list::handle_keyboard_system,
        )
            .run_if(in_state(LauncherState::CharList)),
    );

    // Connect in flight: spawn task, poll oneshot.
    app.add_systems(
        OnEnter(LauncherState::ConnectInFlight),
        (async_work::spawn_connect_task, async_work::spawn_connect_ui),
    )
    .add_systems(
        OnExit(LauncherState::ConnectInFlight),
        async_work::despawn_connect_ui,
    )
    .add_systems(
        Update,
        async_work::poll_connect_system.run_if(in_state(LauncherState::ConnectInFlight)),
    );

    // Login error: simple message; Esc returns to Login. Also clears
    // any DirectModeAutostart marker so we don't auto-retry the same
    // failing credentials in a loop.
    app.add_systems(
        OnEnter(LauncherState::LoginError),
        (login::spawn_error_ui, clear_direct_mode_on_error),
    )
    .add_systems(OnExit(LauncherState::LoginError), login::despawn_error_ui)
    .add_systems(
        Update,
        login::error_keyboard_system.run_if(in_state(LauncherState::LoginError)),
    );

    // Done: hand off to AppPhase::Connecting. The launcher's camera
    // and any remaining UI are torn down by OnExit(AppPhase::Launcher).
    app.add_systems(OnEnter(LauncherState::Done), advance_to_connecting);
}

fn spawn_launcher_camera(mut commands: Commands) {
    commands.spawn((Camera2d, LauncherCamera));
}

fn despawn_launcher_camera(mut commands: Commands, q: Query<Entity, With<LauncherCamera>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

/// On entering `LauncherState::Done`, advance the top-level phase to
/// `Connecting`. The bridge there consumes `PendingConnect`.
fn advance_to_connecting(mut next_phase: ResMut<NextState<AppPhase>>) {
    next_phase.set(AppPhase::Connecting);
}

/// If we re-enter `AppPhase::Launcher` after a `Connecting` failure,
/// the bridge has populated `LoginErrorMsg.0`. Skip past `Login` and
/// show the error directly so the user sees what happened.
fn restore_login_error_on_reentry(
    err: Res<LoginErrorMsg>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    if !err.0.is_empty() {
        next.set(LauncherState::LoginError);
    }
}

/// Direct-mode helper: if creds are prefilled and `DirectModeAutostart`
/// is set, jump straight to AuthInFlight on the first frame in `Login`.
/// We do NOT remove the marker here — it has to survive through to
/// `direct_mode_charlist_autoselect`. Cleanup happens at the natural
/// ends of the chain (charlist pick or LoginError).
fn direct_mode_login_autostart(
    autostart: Option<Res<DirectModeAutostart>>,
    form: Res<LoginForm>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    if autostart.is_none() {
        return;
    }
    if form.user.is_empty() || form.pass.is_empty() {
        return;
    }
    next.set(LauncherState::AuthInFlight);
}

/// On entering `LauncherState::LoginError`, drop the `DirectModeAutostart`
/// marker (if any). Without this, a user who hits Esc from LoginError
/// back to Login would auto-advance into the same failing creds — an
/// infinite-retry loop.
fn clear_direct_mode_on_error(mut commands: Commands) {
    commands.remove_resource::<DirectModeAutostart>();
}

/// Direct-mode helper: if `DirectModeAutostart` is still set when the
/// char list lands and `DefaultCharName` matches a row, auto-pick it.
fn direct_mode_charlist_autoselect(
    mut commands: Commands,
    autostart: Option<Res<DirectModeAutostart>>,
    chars: Res<CharListData>,
    default_name: Res<DefaultCharName>,
    mut sel: ResMut<SelectedChar>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    if autostart.is_none() {
        return;
    }
    let Some(name) = default_name.0.as_deref() else {
        return;
    };
    if let Some(slot) = chars.0.iter().find(|c| c.name == name) {
        sel.0 = Some(slot.clone());
        next.set(LauncherState::ConnectInFlight);
        commands.remove_resource::<DirectModeAutostart>();
    }
}
