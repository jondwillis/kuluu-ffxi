//! Native windowed launcher: graphical login + character-select flow.
//!
//! Sibling to `crate::launcher` (the stdin/stdout TUI launcher). When the
//! `native` subcommand is invoked without all three positional args, we'd
//! like the user to *see* the login form in the same window the game
//! eventually opens — not in their terminal. This module owns that flow.
//!
//! # Architecture: separate Bevy `App`s
//!
//! Rather than tangle [`ffxi_viewer_core::ViewerCorePlugin`] with state
//! machinery (its `Startup` systems unconditionally spawn the world,
//! camera, and HUD), we run the launcher as its own short-lived Bevy
//! `App`. When the user has finished selecting a character, the launcher
//! `App` exits via `AppExit` and the caller (`main.rs::run_native_main_thread`)
//! continues into the existing `view_native::run` — same pattern the TUI
//! launcher used: complete pre-flight first, then enter the game viewer.
//!
//! The cost is one window-close + one window-open between launcher and
//! game; the upside is `ViewerCorePlugin` stays untouched and the
//! launcher's UI doesn't have to fight the HUD's `Startup`-spawned
//! nodes for z-order.
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
//!                       │     Done       │ → AppExit
//!                       └────────────────┘
//! ```
//!
//! `Done` triggers `AppExit::Success`. The result is plucked from a
//! [`LauncherOutcome`] resource on the way out.

mod async_work;
mod char_list;
mod login;

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use bevy::log::LogPlugin;
use bevy::prelude::*;
use ffxi_client::auth_client::AuthClient;
use ffxi_client::lobby_client::LobbyClient;
use tokio::runtime::Handle as RtHandle;

use crate::launcher::{Defaults, Selection};

/// Top-level Bevy state driving the launcher UI.
#[derive(States, Default, Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) enum LauncherState {
    #[default]
    Login,
    AuthInFlight,
    CharList,
    ConnectInFlight,
    LoginError,
    /// Terminal: triggers `AppExit::Success`. The result is in
    /// `LauncherOutcome`.
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
/// so the final `Selection` can echo them back to the caller (matching
/// the stdin launcher's contract).
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

/// What gets handed back to the caller when the launcher exits cleanly.
/// Wrapped in `Mutex` so the `Done`-on-enter system can fill it from a
/// `&mut` system param while the outer caller (which holds an `Arc`) can
/// pluck the result after `app.run()` returns.
#[derive(Resource, Default, Clone)]
pub(crate) struct LauncherOutcome(pub Arc<Mutex<Option<Result<Selection>>>>);

/// Run the launcher Bevy app. Returns the user's selection (matching the
/// stdin launcher's `Selection` shape) or an error if the user closed
/// the window before completing the flow.
pub fn run(
    server: &str,
    auth: Arc<AuthClient>,
    lobby: Arc<LobbyClient>,
    defaults: Defaults,
    runtime: RtHandle,
) -> Result<Selection> {
    let outcome = LauncherOutcome::default();
    let outcome_for_app = outcome.clone();

    let mut form = LoginForm::default();
    if let Some(u) = defaults.user {
        form.user = u;
    }
    if let Some(p) = defaults.password {
        form.pass = p;
    }
    // If both fields prefilled and we're missing only the char_name, the
    // user should still see the login screen — the simplest behaviour is
    // to start in `Login` always. Auto-advance is left to the user
    // pressing Enter; that mirrors the stdin launcher (which also
    // re-prompts even with `Some` defaults).

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("ffxi-client — login [{server}]"),
                    resolution: (800u32, 500u32).into(),
                    ..default()
                }),
                ..default()
            })
            .build()
            .disable::<LogPlugin>(),
    );

    app.init_state::<LauncherState>()
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
        .insert_resource(outcome_for_app)
        .insert_resource(DefaultCharName(defaults.char_name));

    // Login screen: builds UI on enter, eats keys, redraws on each frame
    // it's active.
    app.add_systems(OnEnter(LauncherState::Login), login::spawn_login_ui)
        .add_systems(OnExit(LauncherState::Login), login::despawn_login_ui)
        .add_systems(
            Update,
            (
                login::keyboard_input_system,
                login::redraw_login_form_system,
            )
                .run_if(in_state(LauncherState::Login)),
        );

    // Auth in flight: spawn task on enter, poll its oneshot every frame.
    app.add_systems(
        OnEnter(LauncherState::AuthInFlight),
        async_work::spawn_auth_task,
    )
    .add_systems(OnEnter(LauncherState::AuthInFlight), async_work::spawn_auth_ui)
    .add_systems(OnExit(LauncherState::AuthInFlight), async_work::despawn_auth_ui)
    .add_systems(
        Update,
        async_work::poll_auth_system.run_if(in_state(LauncherState::AuthInFlight)),
    );

    // Char list: spawn UI from the snapshot, dispatch on click.
    app.add_systems(OnEnter(LauncherState::CharList), char_list::spawn_char_list_ui)
        .add_systems(OnExit(LauncherState::CharList), char_list::despawn_char_list_ui)
        .add_systems(
            Update,
            (
                char_list::handle_click_system,
                char_list::handle_keyboard_system,
            )
                .run_if(in_state(LauncherState::CharList)),
        );

    // Connect in flight: spawn task, poll oneshot.
    app.add_systems(
        OnEnter(LauncherState::ConnectInFlight),
        async_work::spawn_connect_task,
    )
    .add_systems(
        OnEnter(LauncherState::ConnectInFlight),
        async_work::spawn_connect_ui,
    )
    .add_systems(
        OnExit(LauncherState::ConnectInFlight),
        async_work::despawn_connect_ui,
    )
    .add_systems(
        Update,
        async_work::poll_connect_system.run_if(in_state(LauncherState::ConnectInFlight)),
    );

    // Login error: simple message; Esc returns to Login.
    app.add_systems(OnEnter(LauncherState::LoginError), login::spawn_error_ui)
        .add_systems(OnExit(LauncherState::LoginError), login::despawn_error_ui)
        .add_systems(
            Update,
            login::error_keyboard_system.run_if(in_state(LauncherState::LoginError)),
        );

    // Done: write outcome (already populated) + emit AppExit.
    app.add_systems(OnEnter(LauncherState::Done), exit_on_done);

    // 2D camera so UI nodes render. Spawned once at Startup; no per-state
    // teardown needed since the launcher app lives only as long as this
    // flow.
    app.add_systems(Startup, spawn_camera);

    // Window-close: write a Cancelled result if the user shut the window
    // before completing the flow.
    app.add_systems(Update, handle_close_request);

    app.run();

    // Pluck the result out from the shared slot. If the user closed the
    // window before any state landed an outcome, we report that as a
    // user-cancelled error.
    let mut slot = outcome
        .0
        .lock()
        .map_err(|_| anyhow!("launcher outcome mutex poisoned"))?;
    slot.take()
        .unwrap_or_else(|| Err(anyhow!("launcher window closed before selection completed")))
        .context("launcher Bevy app")
}

/// Optional default char name pulled from CLI args. Used by the char_list
/// system to highlight a row matching the name (UX nicety; not required
/// for the flow to work).
#[derive(Resource, Default)]
pub(crate) struct DefaultCharName(pub Option<String>);

fn spawn_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// Final-state hook: emit `AppExit`. The `LauncherOutcome` resource has
/// already been populated by the system that transitioned us into `Done`
/// (either `poll_connect_system`, which writes the success path, or
/// `handle_close_request`, which writes the cancellation path).
fn exit_on_done(mut exit: MessageWriter<AppExit>) {
    exit.write_default();
}

/// If the window-close-requested message fires before we've reached
/// `Done`, write a cancellation outcome and exit. Without this the
/// launcher would close cleanly but the caller would receive an
/// ambiguous "outcome slot empty" error.
fn handle_close_request(
    mut close: MessageReader<bevy::window::WindowCloseRequested>,
    state: Res<State<LauncherState>>,
    outcome: Res<LauncherOutcome>,
    mut exit: MessageWriter<AppExit>,
) {
    if close.read().next().is_none() {
        return;
    }
    if *state.get() != LauncherState::Done {
        if let Ok(mut slot) = outcome.0.lock() {
            if slot.is_none() {
                *slot = Some(Err(anyhow!("launcher window closed by user")));
            }
        }
    }
    exit.write_default();
}
