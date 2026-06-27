mod account_create;
mod async_work;
mod change_password;
mod char_create;
mod char_create_preview;
pub(crate) mod char_list;
mod char_preview;
mod common;
mod dat_setup;
mod graphics;
mod login;
mod server_edit;
mod server_select;
mod settings;
mod updater;

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ffxi_client::auth_client::{AuthClient, AuthFlavor};
use ffxi_client::launcher_store::{AuthFlavorKind, ServerProfile};
use ffxi_client::lobby_client::LobbyClient;
use tokio::runtime::Handle as RtHandle;

use crate::launcher::{Defaults, Selection};

use super::AppPhase;

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

#[derive(SubStates, Default, Debug, Clone, Eq, PartialEq, Hash)]
#[source(super::AppPhase = super::AppPhase::Launcher)]
pub(crate) enum LauncherState {
    DatSetup,

    ServerSelect,

    ServerEdit,

    Settings,

    Graphics,

    ChangePassword,

    ChangePasswordInFlight,
    #[default]
    Login,

    CreateAccount,

    CreateAccountInFlight,

    CreateAccountError,
    AuthInFlight,
    CharList,

    CharCreate,

    CharCreateInFlight,

    CharCreateError,

    CharDeleteConfirm,
    CharDeleteInFlight,
    ConnectInFlight,
    LoginError,

    Done,
}

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
            race: 1,
            job: 1,
            nation: 0,
            face: 0,
            size: 1,
            focus: CharCreateField::default(),
        }
    }
}

impl CharCreateForm {
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

#[derive(Resource, Default)]
pub(crate) struct CharCreateError(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum LoginField {
    #[default]
    User,
    Password,
}

#[derive(Resource, Default)]
pub(crate) struct LoginForm {
    pub user: String,
    pub pass: String,
    pub focus: LoginField,

    pub remember_password: bool,
}

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

#[derive(Resource, Default)]
pub(crate) struct ServerSelectForm {
    pub selected: Option<String>,
}

#[derive(Resource, Default)]
pub(crate) struct ServerSelectCursor(pub usize);

#[derive(Resource)]
pub(crate) struct ServerEditForm {
    pub name: String,
    pub host: String,
    pub auth_port: String,
    pub data_port: String,
    pub view_port: String,
    pub flavor: ffxi_client::launcher_store::AuthFlavorKind,

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

#[derive(Resource, Default)]
pub(crate) struct CreateAccountForm {
    pub user: String,
    pub pass: String,
    pub pass_confirm: String,
    #[allow(dead_code)]
    pub focus: CreateAccountField,
}

impl CreateAccountForm {
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

#[derive(Resource, Default)]
pub(crate) struct CreateAccountErrorMsg(pub String);

#[derive(Resource, Default)]
pub(crate) struct LoginErrorMsg(pub String);

#[derive(Resource, Clone)]
pub(crate) struct RuntimeHandle(pub RtHandle);

#[derive(Resource, Clone)]
pub(crate) struct ServerInfo {
    pub server: String,

    pub profile_name: Option<String>,
}

impl ServerInfo {
    pub fn display_label(&self) -> String {
        self.profile_name
            .clone()
            .unwrap_or_else(|| self.server.clone())
    }
}

#[derive(Resource, Clone)]
pub(crate) struct LauncherClients {
    pub auth: Arc<AuthClient>,
    pub lobby: Arc<LobbyClient>,
}

#[derive(Default)]
pub(crate) struct OpenedLobbyInner {
    pub handle: Option<ffxi_client::lobby_client::LobbyHandle>,
    pub auth: Option<ffxi_client::auth_client::AuthSession>,
}

#[derive(Resource, Default)]
pub(crate) struct OpenedLobby(pub Mutex<OpenedLobbyInner>);

#[derive(Resource, Clone, Default)]
pub(crate) struct Credentials {
    pub user: String,
    pub pass: String,
}

#[derive(Resource, Default)]
pub(crate) struct CharListData(pub Vec<ffxi_client::lobby_client::CharSlot>);

#[derive(Resource, Default)]
pub(crate) struct SelectedChar(pub Option<ffxi_client::lobby_client::CharSlot>);

#[derive(Resource, Default)]
pub(crate) struct PendingConnect(pub Option<Selection>);

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

#[derive(Resource, Default)]
pub(crate) struct DefaultCharName(pub Option<String>);

#[derive(Resource)]
pub(crate) struct DirectModeAutostart;

#[derive(Resource)]
pub(crate) struct CliOverridesPresent;

#[derive(Resource)]
pub(crate) struct DatGateDone;

#[derive(Component)]
pub(crate) struct LauncherCamera;

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
        .insert_resource(dat_setup::DatSetupForm::default())
        .insert_resource(dat_setup::DatSetupUiDirty::default())
        .insert_resource(ChangePasswordForm::default())
        .insert_resource(DefaultCharName(defaults.char_name));

    app.add_systems(OnEnter(AppPhase::Launcher), spawn_launcher_camera)
        .add_systems(OnExit(AppPhase::Launcher), despawn_launcher_camera);

    app.add_systems(
        Update,
        (sync_window_title, common::update_server_chips).run_if(in_state(AppPhase::Launcher)),
    );

    app.add_systems(OnEnter(AppPhase::Launcher), restore_login_error_on_reentry);

    app.add_systems(
        OnEnter(LauncherState::Login),
        decide_initial_screen.before(login::spawn_login_ui),
    );

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

    app.add_systems(
        OnEnter(LauncherState::DatSetup),
        (dat_setup::enter_prefill, dat_setup::spawn_ui).chain(),
    )
    .add_systems(OnExit(LauncherState::DatSetup), dat_setup::despawn_ui)
    .add_systems(
        Update,
        (
            dat_setup::keyboard_input_system,
            dat_setup::rebuild_ui_system,
        )
            .run_if(in_state(LauncherState::DatSetup)),
    );

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

    app.init_resource::<graphics::GraphicsAdvancedOpen>()
        .add_systems(OnEnter(LauncherState::Graphics), graphics::spawn_ui)
        .add_systems(OnExit(LauncherState::Graphics), graphics::despawn_ui)
        .add_systems(
            Update,
            (
                graphics::keyboard_input_system,
                graphics::redraw_graphics_system,
                graphics::redraw_advanced_visibility,
            )
                .run_if(in_state(LauncherState::Graphics)),
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

    app.add_systems(
        Update,
        (
            char_preview::drive_preview_pose,
            ffxi_viewer_core::ffxi_actor_render::tick_ffxi_render_actors,
            char_preview::ensure_preview_render_layer,
            char_preview::relight_preview_actor
                .after(ffxi_viewer_core::ffxi_actor_render::update_ffxi_render_actor_lighting),
        )
            .chain()
            .run_if(in_state(LauncherState::CharList)),
    );

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

    app.add_systems(
        OnEnter(LauncherState::LoginError),
        (login::spawn_error_ui, clear_direct_mode_on_error),
    )
    .add_systems(OnExit(LauncherState::LoginError), login::despawn_error_ui)
    .add_systems(
        Update,
        login::error_keyboard_system.run_if(in_state(LauncherState::LoginError)),
    );

    char_create_preview::register(app);

    updater::register(app);

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

fn advance_to_connecting(mut next_phase: ResMut<NextState<AppPhase>>) {
    next_phase.set(AppPhase::Connecting);
}

fn restore_login_error_on_reentry(
    err: Res<LoginErrorMsg>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    if !err.0.is_empty() {
        next.set(LauncherState::LoginError);
    }
}

fn decide_initial_screen(
    mut commands: Commands,
    overrides: Option<Res<CliOverridesPresent>>,
    gate_done: Option<Res<DatGateDone>>,
    err: Res<LoginErrorMsg>,
    mut form: ResMut<LoginForm>,
    mut server_form: ResMut<ServerSelectForm>,
    mut server_info: ResMut<ServerInfo>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    if !err.0.is_empty() {
        return;
    }

    // No reachable DAT install → gate to DatSetup before login, else names
    // render as "?" and no geometry loads. Direct-mode keeps its --require-dat
    // path and is left to autostart.
    if gate_done.is_none()
        && overrides.is_none()
        && ffxi_dat::DatRoot::from_env_or_default().is_err()
    {
        next.set(LauncherState::DatSetup);
        return;
    }

    if !form.user.is_empty() {
        return;
    }

    if server_form.selected.is_some() {
        return;
    }
    if overrides.is_some() {
        return;
    }
    let store = ffxi_client::launcher_store::load();
    if let Some(prefill) = store.login_prefill() {
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

        if let Some(profile) = prefill.profile {
            apply_server_profile(&mut commands, profile);

            server_info.server = profile.host.clone();
            server_info.profile_name = Some(profile.name.clone());
        }
        server_form.selected = Some(prefill.account.server_name.clone());
        return;
    }

    next.set(LauncherState::ServerSelect);
}

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

fn clear_direct_mode_on_error(mut commands: Commands) {
    commands.remove_resource::<DirectModeAutostart>();
}

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
