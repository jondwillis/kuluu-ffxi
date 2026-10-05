use std::sync::Arc;

use ffxi_proto::login::{LOGIN_AUTH_PORT, LOGIN_DATA_PORT, LOGIN_VIEW_PORT};
use kuluu::launcher::Defaults;
use kuluu::view_native::{NativeRunArgs, SessionPorts};
use kuluu_session::auth_client::AuthClient;
use kuluu_session::lobby_client::LobbyClient;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let server = "127.0.0.1";
    eprintln!("[room-no-install] ActionDatRoot=None; account autostart disabled");
    kuluu::view_native::run(NativeRunArgs {
        server: server.into(),
        ports: SessionPorts {
            auth_port: LOGIN_AUTH_PORT,
            data_port: LOGIN_DATA_PORT,
            view_port: LOGIN_VIEW_PORT,
            map_host_override: None,
        },
        auth: Arc::new(AuthClient::new(server, LOGIN_AUTH_PORT)),
        lobby: Arc::new(LobbyClient::new(server, LOGIN_DATA_PORT, LOGIN_VIEW_PORT)),
        defaults: Defaults {
            user: Some("room-no-install-fixture".into()),
            password: None,
            char_name: None,
        },
        direct_mode_autostart: false,
        runtime: runtime.handle().clone(),
        relay_listen: None,
        #[cfg(unix)]
        agent_listen: None,
        dat_root: None,
        unfocused: true,
        mute: true,
    })
}
