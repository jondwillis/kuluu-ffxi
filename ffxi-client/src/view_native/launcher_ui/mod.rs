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

mod account_create;
mod async_work;
mod change_password;
mod char_create;
mod char_create_preview;
pub(crate) mod char_list;
mod char_preview;
mod common;
mod login;
mod server_edit;
mod server_select;
mod settings;

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ffxi_client::auth_client::{AuthClient, AuthFlavor};
use ffxi_client::launcher_store::{AuthFlavorKind, ServerProfile};
use ffxi_client::lobby_client::LobbyClient;
use tokio::runtime::Handle as RtHandle;

use crate::launcher::{Defaults, Selection};

use super::AppPhase;

/// Swap the live `LauncherClients` + `ServerInfo` to point at a freshly-
/// picked server profile. This is what makes ServerSelect's row click
/// actually change the network endpoint — without this, picking a
/// different profile only relabeled the keyring grouping and the next
/// login still hit whatever the CLI started us on. New in-flight tasks
/// (queued by AuthInFlight / ConnectInFlight) clone the Arc once at
/// dispatch, so any tasks already in flight against the old server
/// finish naturally on the old Arc; new tasks get the new endpoint.
pub(crate) fn apply_server_profile(commands: &mut Commands, profile: &ServerProfile) {
    let flavor = match profile.flavor {
        AuthFlavorKind::Json => AuthFlavor::Json,
        AuthFlavorKind::Binary => AuthFlavor::Binary,
    };
    let auth = Arc::new(AuthClient::with_flavor_and_version(
        profile.host.clone(),
        profile.auth_port,
        flavor,
        profile.xiloader_version.as_deref(),
    ));
    let lobby = Arc::new(LobbyClient::new(
        profile.host.clone(),
        profile.data_port,
        profile.view_port,
    ));
    commands.insert_resource(LauncherClients { auth, lobby });
    commands.insert_resource(ServerInfo {
        server: profile.host.clone(),
        profile_name: Some(profile.name.clone()),
    });
}

/// Mirror `ServerInfo` into the primary window's title bar. Was a
/// one-shot at startup (`ffxi-client — 127.0.0.1`); now reactive so
/// the title swaps when `ServerSelect` rebinds.
fn sync_window_title(
    server: Res<ServerInfo>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !server.is_changed() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let new_title = format!("ffxi-client — {}", server.display_label());
    if window.title != new_title {
        window.title = new_title;
    }
}

/// Bevy state driving the launcher UI. `SubStates` of
/// `AppPhase::Launcher` — only exists while the parent phase is
/// `Launcher`. Re-created at `#[default]` (`Login`) on every entry to
/// `AppPhase::Launcher`, including return-from-failure.
#[derive(SubStates, Default, Debug, Clone, Eq, PartialEq, Hash)]
#[source(super::AppPhase = super::AppPhase::Launcher)]
pub(crate) enum LauncherState {
    /// Persisted-server picker. Default when no CLI overrides AND no
    /// `last_used` entry — otherwise the launcher seeds straight into
    /// `Login` via `direct_mode_login_autostart` / the prefill systems.
    ServerSelect,
    /// Add or edit a `ServerProfile`. Reached from `ServerSelect` via
    /// the `+ Add server` or `Edit selected` buttons.
    ServerEdit,
    /// Global launcher settings — the DAT install path, navmesh dir, and
    /// MAC override (each an `EnvOverride` in `LauncherStore::settings`).
    /// Reached from the `Settings` button on `ServerSelect` / `Login`.
    /// Saving rebuilds the shared `DatRoot` and reloads the backdrop zone
    /// in place.
    Settings,
    /// Change password form (old / new / confirm). Reached from Login via
    /// the `Change password` button.
    ChangePassword,
    /// Sending the change-password command. Success → Login; failure →
    /// LoginError.
    ChangePasswordInFlight,
    #[default]
    Login,
    /// Account creation form. Reached from `Login` via the C key.
    /// Submit transitions to `CreateAccountInFlight`; Esc returns to
    /// `Login`.
    CreateAccount,
    /// Sending the `ensure_account` request. Success bounces back to
    /// `Login` with the new credentials prefilled (the user can then
    /// press Enter to authenticate). Failure routes to
    /// `CreateAccountError`.
    CreateAccountInFlight,
    /// Surface for unexpected ensure_account errors (network failures,
    /// server maintenance mode, etc.). Enter retries; Esc returns to
    /// the form.
    CreateAccountError,
    AuthInFlight,
    CharList,
    /// Character creation form. Reached from `CharList` via the "+ New
    /// character" entry. Submit transitions to `CharCreateInFlight`; Esc
    /// returns to `CharList`.
    CharCreate,
    /// Sending the two-packet name-check + register-char sequence to the
    /// view server. On success: reopen lobby, refresh char list, return
    /// to `CharList`. On failure: show error in `CharCreateError`.
    CharCreateInFlight,
    /// Server rejected the create (name in use, banned word, etc.). Esc
    /// returns to the form; Enter retries.
    CharCreateError,
    /// Inline confirmation modal for the `Delete selected` button on the
    /// char-list screen. Confirm dispatches `CharDeleteInFlight`; Cancel
    /// or Esc returns to `CharList`.
    /// (LSB's delete handler doesn't actually validate the `passwd`
    /// field — only `accountID` from the live session — so we use a
    /// simple Y/N confirm rather than re-prompting for the password.)
    CharDeleteConfirm,
    CharDeleteInFlight,
    ConnectInFlight,
    LoginError,
    /// Terminal for the launcher: triggers transition to
    /// `AppPhase::Connecting`. The bridge system there picks up
    /// `PendingConnect` and continues the flow.
    Done,
}

/// Which field on the char-create form has keyboard focus.
///
/// Retained as a tag enum even after the feathers rewrite — screens still
/// pass it to per-row spawn helpers to identify which form slot a
/// TextField/Button writes into. The `focus` field on the form
/// resource + the cycle helpers are dead with the new widget-driven
/// flow but cheap to keep around.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum CharCreateField {
    #[default]
    Name,
    Race,
    Job,
    Nation,
    Face,
    Size,
}

#[allow(dead_code)]
impl CharCreateField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Name => Self::Race,
            Self::Race => Self::Job,
            Self::Job => Self::Nation,
            Self::Nation => Self::Face,
            Self::Face => Self::Size,
            Self::Size => Self::Name,
        }
    }
    pub(crate) fn prev(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Race => Self::Name,
            Self::Job => Self::Race,
            Self::Nation => Self::Job,
            Self::Face => Self::Nation,
            Self::Size => Self::Face,
        }
    }
}

/// Char-creation form state. Race/job/nation/size are stored as the raw
/// LSB byte values; rendering looks them up against the tables in
/// `char_create.rs`.
#[derive(Resource)]
pub(crate) struct CharCreateForm {
    pub name: String,
    pub race: u8,
    pub job: u8,
    pub nation: u8,
    pub face: u8,
    pub size: u8,
    #[allow(dead_code)]
    pub focus: CharCreateField,
}

impl Default for CharCreateForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            race: 1,   // Hume M
            job: 1,    // Warrior
            nation: 0, // San d'Oria
            face: 0,
            size: 1, // Medium
            focus: CharCreateField::default(),
        }
    }
}

impl CharCreateForm {
    /// Returns `None` if the form would be accepted by LSB's
    /// `createCharacter`; `Some(msg)` otherwise. Client-side mirror of
    /// the server validation in `login_helpers.cpp:216-244`.
    pub fn validation_msg(&self) -> Option<String> {
        if self.name.is_empty() {
            return Some("Enter a name (3–15 letters, A–Z only).".into());
        }
        if self.name.len() < 3 {
            return Some("Name is too short (minimum 3 letters).".into());
        }
        if self.name.len() > 15 {
            return Some("Name is too long (maximum 15 letters).".into());
        }
        if !self.name.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some("Letters only — server rejects digits and punctuation.".into());
        }
        None
    }

    /// Step the focused enum field. `delta` is +1 or -1.
    #[allow(dead_code)]
    pub fn cycle_focused(&mut self, delta: i32) {
        match self.focus {
            CharCreateField::Name => {}
            CharCreateField::Race => self.race = cycle_table(char_create::RACES, self.race, delta),
            CharCreateField::Job => self.job = cycle_table(char_create::JOBS, self.job, delta),
            CharCreateField::Nation => {
                self.nation = cycle_table(char_create::NATIONS, self.nation, delta)
            }
            CharCreateField::Face => {
                let next = (self.face as i32 + delta).rem_euclid(char_create::FACE_MAX as i32 + 1);
                self.face = next as u8;
            }
            CharCreateField::Size => self.size = cycle_table(char_create::SIZES, self.size, delta),
        }
    }
}

#[allow(dead_code)]
fn cycle_table(table: &[(u8, &str)], current: u8, delta: i32) -> u8 {
    let idx = table.iter().position(|(v, _)| *v == current).unwrap_or(0) as i32;
    let n = table.len() as i32;
    let next = (idx + delta).rem_euclid(n) as usize;
    table[next].0
}

/// Error message displayed on `CharCreateError`. Populated by the
/// in-flight task when the server (or local validation) rejects the
/// create. Cleared on entry to `CharList`.
#[derive(Resource, Default)]
pub(crate) struct CharCreateError(pub String);

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
    /// When true, a successful login persists the password to the OS
    /// keyring under `(KEYRING_SERVICE, server:user)`. Toggled with
    /// the `Remember password` checkbox on the login screen;
    /// pre-populated from `SavedAccount.remember_password` when
    /// the account-picker prefills this form.
    pub remember_password: bool,
}

/// Field focus on the server-edit form.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum ServerEditField {
    #[default]
    Name,
    Host,
    AuthPort,
    DataPort,
    ViewPort,
    Flavor,
    XiloaderVersion,
}

#[allow(dead_code)]
impl ServerEditField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Name => Self::Host,
            Self::Host => Self::AuthPort,
            Self::AuthPort => Self::DataPort,
            Self::DataPort => Self::ViewPort,
            Self::ViewPort => Self::Flavor,
            Self::Flavor => Self::XiloaderVersion,
            Self::XiloaderVersion => Self::Name,
        }
    }
}

/// Server-select form: just tracks which server name was picked so the
/// account-picker can filter by it.
#[derive(Resource, Default)]
pub(crate) struct ServerSelectForm {
    pub selected: Option<String>,
}

/// Keyboard cursor for the server-select list.
#[derive(Resource, Default)]
pub(crate) struct ServerSelectCursor(pub usize);

/// Server-edit form. `editing_index = Some(i)` overwrites the i-th
/// `ServerProfile` in `LauncherStore`; `None` appends.
#[derive(Resource)]
pub(crate) struct ServerEditForm {
    pub name: String,
    pub host: String,
    pub auth_port: String,
    pub data_port: String,
    pub view_port: String,
    pub flavor: ffxi_client::launcher_store::AuthFlavorKind,
    /// Empty string means "inherit the global default". Saved as `None`
    /// in [`ServerProfile`]; populated otherwise.
    pub xiloader_version: String,
    #[allow(dead_code)]
    pub focus: ServerEditField,
    pub editing_index: Option<usize>,
}

impl Default for ServerEditForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            auth_port: String::from("54231"),
            data_port: String::from("54230"),
            view_port: String::from("54001"),
            flavor: ffxi_client::launcher_store::AuthFlavorKind::Json,
            xiloader_version: String::new(),
            focus: ServerEditField::default(),
            editing_index: None,
        }
    }
}

impl ServerEditForm {
    pub fn from_profile(p: &ffxi_client::launcher_store::ServerProfile) -> Self {
        Self {
            name: p.name.clone(),
            host: p.host.clone(),
            auth_port: p.auth_port.to_string(),
            data_port: p.data_port.to_string(),
            view_port: p.view_port.to_string(),
            flavor: p.flavor,
            xiloader_version: p.xiloader_version.clone().unwrap_or_default(),
            focus: ServerEditField::default(),
            editing_index: None,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum ChangePasswordField {
    #[default]
    Old,
    New,
    Confirm,
}

#[allow(dead_code)]
impl ChangePasswordField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Old => Self::New,
            Self::New => Self::Confirm,
            Self::Confirm => Self::Old,
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct ChangePasswordForm {
    pub old: String,
    pub new_pw: String,
    pub confirm: String,
    #[allow(dead_code)]
    pub focus: ChangePasswordField,
    pub error: String,
}

/// Which field on the create-account form has keyboard focus.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum CreateAccountField {
    #[default]
    User,
    Password,
    PasswordConfirm,
}

#[allow(dead_code)]
impl CreateAccountField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::User => Self::Password,
            Self::Password => Self::PasswordConfirm,
            Self::PasswordConfirm => Self::User,
        }
    }
    pub(crate) fn prev(self) -> Self {
        match self {
            Self::User => Self::PasswordConfirm,
            Self::Password => Self::User,
            Self::PasswordConfirm => Self::Password,
        }
    }
}

/// Account-creation form contents. Mirrors `LoginForm` but with a
/// password-confirm field that the UI compares against `pass` before
/// allowing submit. Cleared on `OnExit(LauncherState::CreateAccount)`
/// to avoid leaking credentials across sessions if the user backs out.
#[derive(Resource, Default)]
pub(crate) struct CreateAccountForm {
    pub user: String,
    pub pass: String,
    pub pass_confirm: String,
    #[allow(dead_code)]
    pub focus: CreateAccountField,
}

impl CreateAccountForm {
    /// `None` if the form would be accepted; `Some(msg)` if it
    /// shouldn't submit yet. Drives the live validation hint AND gates
    /// the submit action (Enter does nothing while this returns Some).
    pub fn validation_msg(&self) -> Option<String> {
        if self.user.is_empty() {
            return Some("Enter a username.".into());
        }
        if self.pass.is_empty() {
            return Some("Enter a password.".into());
        }
        if self.pass_confirm.is_empty() {
            return Some("Re-enter the password to confirm.".into());
        }
        if self.pass != self.pass_confirm {
            return Some("Passwords don't match.".into());
        }
        None
    }
}

/// Error displayed on `CreateAccountError` — populated by the in-flight
/// task when `auth.ensure_account` fails with a non-validation error.
#[derive(Resource, Default)]
pub(crate) struct CreateAccountErrorMsg(pub String);

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
    /// Display name of the active server profile (when picked through
    /// `ServerSelect`). Falls back to `server` (the raw host) when only
    /// CLI args drove startup. Drives the persistent "Server: X" chip
    /// surfaced on every user-facing screen + the window title.
    pub profile_name: Option<String>,
}

impl ServerInfo {
    pub fn display_label(&self) -> String {
        self.profile_name
            .clone()
            .unwrap_or_else(|| self.server.clone())
    }
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

/// Copy the launcher's `SelectedChar` data into the viewer-core
/// `SelfAppearance` override resource. Runs `OnEnter(ConnectInFlight)`
/// so the in-game look_resolver finds the self entity's outfit
/// once the player WorldEntity is spawned. LSB sends an empty
/// GrapIDTbl for the local PC's CHAR_PC packet (retail clients
/// reconstruct from local equipment state), so this is the only
/// path that gives our self entity a real `EntityLook::Equipped`.
fn populate_self_appearance(
    sel: Res<SelectedChar>,
    mut appearance: ResMut<ffxi_viewer_core::scene::SelfAppearance>,
) {
    use ffxi_viewer_wire::EntityLook;
    let Some(slot) = sel.0.as_ref() else {
        appearance.look = None;
        return;
    };
    if slot.race == 0 {
        // Empty / synthetic slot — leave override unset so the in-
        // game capsule remains until / unless the wire fills in.
        appearance.look = None;
        return;
    }
    appearance.look = Some(EntityLook::Equipped {
        face: slot.face,
        race: slot.race,
        head: slot.head,
        body: slot.body,
        hands: slot.hands,
        legs: slot.legs,
        feet: slot.feet,
        main: slot.main,
        sub: slot.sub,
        ranged: slot.ranged,
    });
    tracing::info!(
        char_id = slot.char_id,
        race = slot.race,
        face = slot.face,
        "self appearance: cached launcher slot for in-game look_resolver"
    );
}

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

/// Marker resource set when *any* CLI override (--server / --username /
/// --password / etc.) is present. Used to decide the launcher's initial
/// state: with overrides → straight to `Login`; without → `ServerSelect`
/// when a `LauncherStore` exists.
#[derive(Resource)]
pub(crate) struct CliOverridesPresent;

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
        .insert_resource(login::LoginUiDirty::default())
        .insert_resource(LoginErrorMsg::default())
        .insert_resource(RuntimeHandle(runtime))
        .insert_resource(ServerInfo {
            server: server.to_string(),
            profile_name: None,
        })
        .insert_resource(LauncherClients { auth, lobby })
        .insert_resource(OpenedLobby::default())
        .insert_resource(Credentials::default())
        .insert_resource(CharListData::default())
        .insert_resource(SelectedChar::default())
        .insert_resource(PendingConnect::default())
        .insert_resource(CharCreateForm::default())
        .insert_resource(CharCreateError::default())
        .insert_resource(CreateAccountForm::default())
        .insert_resource(CreateAccountErrorMsg::default())
        .insert_resource(ServerSelectForm::default())
        .insert_resource(ServerSelectCursor::default())
        .insert_resource(server_select::PendingServerDelete::default())
        .insert_resource(ServerEditForm::default())
        .insert_resource(settings::SettingsForm::default())
        .insert_resource(settings::SettingsUiDirty::default())
        .insert_resource(ChangePasswordForm::default())
        .insert_resource(DefaultCharName(defaults.char_name));

    // Launcher's 2D camera tracks the launcher phase exactly. The
    // in-game 3D camera spawns OnEnter(AppPhase::InGame) — see
    // `super::run`.
    app.add_systems(OnEnter(AppPhase::Launcher), spawn_launcher_camera)
        .add_systems(OnExit(AppPhase::Launcher), despawn_launcher_camera);

    // Window title + per-screen Server chip reflect the active profile.
    // Both run while the launcher is up — once the user is in-game,
    // the title is a static distraction and the gameplay HUD shows
    // connection state.
    app.add_systems(
        Update,
        (sync_window_title, common::update_server_chips).run_if(in_state(AppPhase::Launcher)),
    );

    // Re-entry hook: if returning to Launcher from a failed Connecting
    // bridge, jump straight to LoginError.
    app.add_systems(OnEnter(AppPhase::Launcher), restore_login_error_on_reentry);

    // First-frame decision: if no CLI overrides and no last_used pair,
    // jump from default Login → ServerSelect so the user manages
    // profiles before being prompted for creds. MUST run before
    // `login::spawn_login_ui` (registered below on the same OnEnter):
    // the prefill branch mutates `LoginForm` / `ServerSelectForm`, and
    // the UI builder reads those to seed the username field + saved-
    // account chips + active server. Both touch the same resources, so
    // Bevy serializes them — but in an unspecified order unless pinned.
    // Without `.before`, `spawn_login_ui` could (and did) win, building
    // the form from the still-empty `LoginForm` and stranding the user
    // on a blank login with no last-used prefill.
    app.add_systems(
        OnEnter(LauncherState::Login),
        decide_initial_screen.before(login::spawn_login_ui),
    );

    // New screens.
    app.add_systems(
        OnEnter(LauncherState::ServerSelect),
        server_select::spawn_ui,
    )
    .add_systems(
        OnExit(LauncherState::ServerSelect),
        server_select::despawn_ui,
    )
    .add_systems(
        Update,
        server_select::keyboard_input_system.run_if(in_state(LauncherState::ServerSelect)),
    );

    app.add_systems(OnEnter(LauncherState::ServerEdit), server_edit::spawn_ui)
        .add_systems(OnExit(LauncherState::ServerEdit), server_edit::despawn_ui)
        .add_systems(
            Update,
            (
                server_edit::keyboard_input_system,
                server_edit::redraw_system,
            )
                .run_if(in_state(LauncherState::ServerEdit)),
        );

    // Settings screen: load the form from the persisted store on enter
    // (before building the UI), tear it down on exit, and drive the
    // Browse picker + in-place rebuild every frame it's active.
    app.add_systems(
        OnEnter(LauncherState::Settings),
        (settings::load_settings_form, settings::spawn_ui).chain(),
    )
    .add_systems(OnExit(LauncherState::Settings), settings::despawn_ui)
    .add_systems(
        Update,
        (
            settings::keyboard_input_system,
            settings::rebuild_settings_ui_system,
        )
            .run_if(in_state(LauncherState::Settings)),
    );

    app.add_systems(
        OnEnter(LauncherState::ChangePassword),
        change_password::spawn_ui,
    )
    .add_systems(
        OnExit(LauncherState::ChangePassword),
        change_password::despawn_ui,
    )
    .add_systems(
        Update,
        (
            change_password::keyboard_input_system,
            change_password::redraw_system,
        )
            .run_if(in_state(LauncherState::ChangePassword)),
    );

    app.add_systems(
        OnEnter(LauncherState::ChangePasswordInFlight),
        (
            async_work::spawn_change_password_task,
            async_work::spawn_change_password_ui,
        ),
    )
    .add_systems(
        OnExit(LauncherState::ChangePasswordInFlight),
        async_work::despawn_change_password_ui,
    )
    .add_systems(
        Update,
        async_work::poll_change_password_system
            .run_if(in_state(LauncherState::ChangePasswordInFlight)),
    );

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
                login::rebuild_login_ui_system,
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
    // `char_preview::spawn_preview` runs *after* `spawn_char_list_ui`
    // so the `CharCursor` resource it depends on already exists.
    // Global observer tags any Mesh3d spawned under a
    // CharPreviewParent with the preview render layer — keeps the PC
    // model out of the backdrop zone's render pass.
    app.add_observer(char_preview::tag_preview_meshes);
    app.add_systems(
        OnEnter(LauncherState::CharList),
        (char_list::spawn_char_list_ui, char_preview::spawn_preview).chain(),
    )
    .add_systems(
        OnExit(LauncherState::CharList),
        (
            char_list::despawn_char_list_ui,
            char_preview::despawn_preview,
        ),
    )
    .add_systems(
        Update,
        (
            direct_mode_charlist_autoselect,
            char_list::handle_click_system,
            char_list::handle_keyboard_system,
            char_list::keyboard_nav_system,
            char_list::redraw_char_list_system,
            char_preview::refresh_preview_on_cursor_change,
            char_preview::poll_pending_preview,
        )
            .run_if(in_state(LauncherState::CharList)),
    );

    // Connect in flight: spawn task, poll oneshot. Also: copy the
    // selected character's appearance into `SelfAppearance` so the
    // in-game look_resolver has something to render once the player
    // entity arrives (LSB zeros self GrapIDTbl, so this is the only
    // source of truth for the local PC's outfit).
    app.add_systems(
        OnEnter(LauncherState::ConnectInFlight),
        (
            populate_self_appearance,
            async_work::spawn_connect_task,
            async_work::spawn_connect_ui,
        ),
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

    // Live 3D preview overlay for the char-create form.
    char_create_preview::register(app);

    // Character creation: form UI on enter, eats keys, redraws on each
    // frame, submits to CharCreateInFlight.
    app.add_systems(OnEnter(LauncherState::CharCreate), char_create::spawn_ui)
        .add_systems(OnExit(LauncherState::CharCreate), char_create::despawn_ui)
        .add_systems(
            Update,
            (
                char_create::keyboard_input_system,
                char_create::redraw_form_system,
            )
                .run_if(in_state(LauncherState::CharCreate)),
        );

    // Char-create in flight: spawn task on enter, poll its oneshot every
    // frame. Success refreshes char list and bounces back to CharList;
    // failure routes to CharCreateError.
    app.add_systems(
        OnEnter(LauncherState::CharCreateInFlight),
        (
            async_work::spawn_char_create_task,
            async_work::spawn_char_create_ui,
        ),
    )
    .add_systems(
        OnExit(LauncherState::CharCreateInFlight),
        async_work::despawn_char_create_ui,
    )
    .add_systems(
        Update,
        async_work::poll_char_create_system.run_if(in_state(LauncherState::CharCreateInFlight)),
    );

    // Char delete: confirm then in-flight.
    app.add_systems(
        OnEnter(LauncherState::CharDeleteConfirm),
        char_list::spawn_delete_confirm_ui,
    )
    .add_systems(
        OnExit(LauncherState::CharDeleteConfirm),
        char_list::despawn_delete_confirm_ui,
    )
    .add_systems(
        Update,
        char_list::delete_confirm_keyboard_system
            .run_if(in_state(LauncherState::CharDeleteConfirm)),
    );

    app.add_systems(
        OnEnter(LauncherState::CharDeleteInFlight),
        (
            async_work::spawn_char_delete_task,
            async_work::spawn_char_delete_ui,
        ),
    )
    .add_systems(
        OnExit(LauncherState::CharDeleteInFlight),
        async_work::despawn_char_delete_ui,
    )
    .add_systems(
        Update,
        async_work::poll_char_delete_system.run_if(in_state(LauncherState::CharDeleteInFlight)),
    );

    // Char-create error: simple message; Esc back to form, Enter retry.
    app.add_systems(
        OnEnter(LauncherState::CharCreateError),
        char_create::spawn_error_ui,
    )
    .add_systems(
        OnExit(LauncherState::CharCreateError),
        char_create::despawn_error_ui,
    )
    .add_systems(
        Update,
        char_create::error_keyboard_system.run_if(in_state(LauncherState::CharCreateError)),
    );

    // Account creation: form UI on enter, eats keys, redraws on each
    // frame, submits to CreateAccountInFlight.
    app.add_systems(
        OnEnter(LauncherState::CreateAccount),
        account_create::spawn_ui,
    )
    .add_systems(
        OnExit(LauncherState::CreateAccount),
        account_create::despawn_ui,
    )
    .add_systems(
        Update,
        (
            account_create::keyboard_input_system,
            account_create::redraw_form_system,
        )
            .run_if(in_state(LauncherState::CreateAccount)),
    );

    // Account-create in flight: spawn task on enter, poll its oneshot
    // every frame. Success returns to Login with creds prefilled (user
    // hits Enter to authenticate); failure routes to CreateAccountError.
    app.add_systems(
        OnEnter(LauncherState::CreateAccountInFlight),
        (
            async_work::spawn_account_create_task,
            async_work::spawn_account_create_ui,
        ),
    )
    .add_systems(
        OnExit(LauncherState::CreateAccountInFlight),
        async_work::despawn_account_create_ui,
    )
    .add_systems(
        Update,
        async_work::poll_account_create_system
            .run_if(in_state(LauncherState::CreateAccountInFlight)),
    );

    // Account-create error: simple message; Esc back to form, Enter retry.
    app.add_systems(
        OnEnter(LauncherState::CreateAccountError),
        account_create::spawn_error_ui,
    )
    .add_systems(
        OnExit(LauncherState::CreateAccountError),
        account_create::despawn_error_ui,
    )
    .add_systems(
        Update,
        account_create::error_keyboard_system.run_if(in_state(LauncherState::CreateAccountError)),
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

/// On the first OnEnter(Login), check the persisted store: if no CLI
/// overrides + no last_used → ServerSelect. If last_used + empty form
/// → prefill from store + keyring. Runs every Login entry but the
/// already-filled-form branch is a no-op, so re-entries (e.g. from
/// clicking a saved-account chip) don't clobber what was just picked.
fn decide_initial_screen(
    mut commands: Commands,
    overrides: Option<Res<CliOverridesPresent>>,
    err: Res<LoginErrorMsg>,
    mut form: ResMut<LoginForm>,
    mut server_form: ResMut<ServerSelectForm>,
    mut server_info: ResMut<ServerInfo>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    // Don't preempt an in-flight error screen — that path goes
    // Login -> LoginError -> Login via the error_keyboard_system.
    if !err.0.is_empty() {
        return;
    }
    // If a chip / prefill already populated the form, don't touch.
    if !form.user.is_empty() {
        return;
    }
    // If the user has explicitly picked a server (via ServerSelect or a
    // prior prefill), STAY on Login — its form is the entry point for a
    // brand-new account, even when the picked server has zero saved
    // accounts. Without this guard, picking a fresh server bounces
    // straight back to ServerSelect every `OnEnter(Login)`, stranding
    // the user with no way to type a new login.
    if server_form.selected.is_some() {
        return;
    }
    if overrides.is_some() {
        return;
    }
    let store = ffxi_client::launcher_store::load();
    if let Some(prefill) = store.login_prefill() {
        // Prefill from the most-recent login. Password comes from the
        // keyring iff the matching SavedAccount has `remember_password`.
        form.user = prefill.account.username.clone();
        form.remember_password = prefill.account.remember_password;
        if prefill.account.remember_password {
            if let Some(pw) = ffxi_client::secret_store::SecretStore::get(
                ffxi_client::launcher_store::KEYRING_SERVICE,
                &ffxi_client::launcher_store::keyring_account_key(
                    &prefill.account.server_name,
                    &prefill.account.username,
                ),
            ) {
                form.pass = pw;
            }
        }
        // Fully restore the last-used server, not just its name: apply the
        // matching profile so the network endpoint, the "Server:" chip,
        // and the window title all point at it. `main.rs` seeds the
        // launcher from CLI defaults (127.0.0.1) and never reads the store,
        // so without this the prefilled username would authenticate against
        // the wrong host.
        if let Some(profile) = prefill.profile {
            apply_server_profile(&mut commands, profile);
            // `apply_server_profile` swaps `ServerInfo` via a *command*,
            // which doesn't flush until after this OnEnter schedule — but
            // `login::spawn_login_ui` runs later in this *same* schedule
            // (it's `.before`-ordered after us) and reads `ServerInfo` to
            // build the "Sign in to X" title. Write the resource directly
            // too so that first paint shows the real server, not the stale
            // CLI default. The deferred insert lands the identical value.
            server_info.server = profile.host.clone();
            server_info.profile_name = Some(profile.name.clone());
        }
        server_form.selected = Some(prefill.account.server_name.clone());
        return;
    }
    // No CLI overrides + no matching last_used → always go through
    // ServerSelect. An empty server list is fine: ServerSelect's
    // `+ Add server` button is the only way to add the first entry,
    // so gating this on `!servers.is_empty()` would strand a
    // fresh install with no path to a server.
    next.set(LauncherState::ServerSelect);
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
