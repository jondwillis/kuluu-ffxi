use std::{future::Future, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;

use crate::auth_binary::{self, BinaryAuthError, Command as BinCommand, PayloadBuilder};
use crate::tls::TofuVerifier;
use ffxi_proto::login::login_cmd::{LOGIN_ATTEMPT, LOGIN_CHANGE_PASSWORD, LOGIN_CREATE};
use ffxi_proto::login::login_result::*;

/// The auth server answered and said no. The same inputs get the same
/// answer, so a supervisor must surface it rather than retry it as a
/// transport blip.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AuthRejected(pub String);

pub fn is_auth_rejected(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AuthRejected>().is_some())
}

/// vendor/server/src/login/auth_session.cpp auth_session::read_func: a
/// JSON-only `error_message` (the loader-version text) is sent instead of a
/// `result`, so it is checked first; a `result` other than the success code
/// for the request is the server's verdict on the credentials.
fn json_result(resp: &Value, request: &str) -> Result<u8> {
    if let Some(msg) = resp.get("error_message").and_then(Value::as_str) {
        return Err(AuthRejected(format!("{request}: {}", msg.trim().replace('\n', " "))).into());
    }
    let result = resp
        .get("result")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("{request} response missing `result`: {resp}"))?;
    Ok(result as u8)
}

fn describe_login_result(code: u8) -> String {
    match code {
        LOGIN_FAIL => "account status does not permit login (banned or suspended)".into(),
        LOGIN_ERROR => "invalid username or password".into(),
        LOGIN_ERROR_ALREADY_LOGGED_IN => "account already logged in".into(),
        LOGIN_ERROR_VERSION_UNSUPPORTED => "loader version not supported by this server".into(),
        LOGIN_ERROR_TRUST_TOKEN_INVALID => "trust token rejected".into(),
        LOGIN_ERROR_CREATE_DISABLED => "account creation is disabled on this server".into(),
        LOGIN_ERROR_CREATE_TAKEN => "username already taken".into(),
        LOGIN_ERROR_CREATE => "account creation failed".into(),
        LOGIN_ERROR_CHANGE_PASSWORD => "password change failed".into(),
        other => format!("server result {other:#04x}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFlavor {
    Json,

    Binary,

    /// No auth server: the PlayOnline account services, signed in to
    /// in-house (`crate::pol_inhouse`), issue the lobby session.
    PlayOnline,
}

impl AuthFlavor {
    pub fn uses_auth_server(self) -> bool {
        !matches!(self, AuthFlavor::PlayOnline)
    }
}

impl std::str::FromStr for AuthFlavor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" | "lsb" => Ok(AuthFlavor::Json),
            "binary" | "hxi" | "horizon" => Ok(AuthFlavor::Binary),
            "playonline" | "pol" => Ok(AuthFlavor::PlayOnline),
            other => Err(format!(
                "unknown auth-flavor `{other}`; expected json|binary|playonline"
            )),
        }
    }
}

const AUTH_BUFFER_SIZE: usize = 4096;
// A complete-request limit also bounds servers that send one byte at a time.
const AUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const LEGACY_AUTH_HINT: &str = "The server returned a binary authentication response. The server may require Legacy LSB. Confirm the server type and loader version with the server operator. Changing the loader version does not change the connection protocol.";

const XILOADER_VERSION_ENV: &str = "FFXI_XILOADER_VERSION";

pub fn resolve_client_version(override_: Option<&str>) -> [u8; 3] {
    resolve_client_version_from(
        override_,
        std::env::var(XILOADER_VERSION_ENV).ok().as_deref(),
    )
}

fn resolve_client_version_from(override_: Option<&str>, env: Option<&str>) -> [u8; 3] {
    if let Some(s) = override_ {
        if let Some(v) = parse_version_triple(s) {
            return v;
        }
        tracing::warn!("--xiloader-version={s:?} invalid; falling back to env/default");
    }
    if let Some(s) = env {
        if let Some(v) = parse_version_triple(s) {
            return v;
        }
        tracing::warn!("{XILOADER_VERSION_ENV}={s:?} invalid; using default");
    }
    ffxi_proto::login::SUPPORTED_XILOADER_VERSION
}

/// The binary flavor's loader field is a fixed-width string with its own
/// default, so only an explicit override or the env var moves it off
/// auth_binary::DEFAULT_VERSION; the JSON default does not leak across.
pub fn resolve_binary_version(override_: Option<&str>) -> [u8; auth_binary::VERSION_FIELD_LEN] {
    resolve_binary_version_from(
        override_,
        std::env::var(XILOADER_VERSION_ENV).ok().as_deref(),
    )
}

fn resolve_binary_version_from(
    override_: Option<&str>,
    env: Option<&str>,
) -> [u8; auth_binary::VERSION_FIELD_LEN] {
    for (label, s) in [
        ("--xiloader-version", override_),
        (XILOADER_VERSION_ENV, env),
    ] {
        let Some(s) = s else {
            continue;
        };
        if let Some(v) = auth_binary::version_field(s) {
            return v;
        }
        tracing::warn!(
            "{label}={s:?} does not fit the binary loader's {}-byte single-digit x.y.z field; \
             ignoring it",
            auth_binary::VERSION_FIELD_LEN
        );
    }
    auth_binary::DEFAULT_VERSION
}

fn parse_version_triple(s: &str) -> Option<[u8; 3]> {
    let mut parts = s.trim().split('.');
    let mut out = [0u8; 3];
    for value in &mut out {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *value = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

pub fn validate_loader_version(flavor: AuthFlavor, value: &str) -> Result<()> {
    if value.trim().is_empty() || flavor == AuthFlavor::PlayOnline {
        return Ok(());
    }
    match flavor {
        AuthFlavor::Binary if auth_binary::version_field(value).is_none() => {
            bail!("Use a loader version such as 1.1.5, with one digit per number.");
        }
        AuthFlavor::Json if parse_version_triple(value).is_none() => {
            bail!("Use three loader version numbers from 0 to 255, separated by periods.");
        }
        _ => Ok(()),
    }
}

async fn auth_timeout<T>(
    limit: Duration,
    operation: &str,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(limit, future).await.with_context(|| {
        format!(
            "Authentication {operation} timed out after {} seconds.",
            limit.as_secs()
        )
    })?
}

// vendor/server/src/login/auth_session.cpp auth_session::read_func
async fn read_json_response(stream: &mut (impl AsyncRead + Unpin)) -> Result<Value> {
    let mut buf = vec![0u8; AUTH_BUFFER_SIZE];
    let mut total = 0usize;
    loop {
        match stream.read(&mut buf[total..]).await {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if matches!(buf[0], LOGIN_ERROR | LOGIN_ERROR_VERSION_UNSUPPORTED) {
                    return Err(AuthRejected(LEGACY_AUTH_HINT.into()).into());
                }
                if let Ok(value) =
                    serde_json::from_slice::<Value>(trim_trailing_zero(&buf[..total]))
                {
                    return Ok(value);
                }
                if total == buf.len() {
                    bail!("Authentication response exceeded {AUTH_BUFFER_SIZE} bytes.");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
    }
    serde_json::from_slice(trim_trailing_zero(&buf[..total]))
        .context("Authentication server returned an incomplete or invalid JSON response.")
}

// vendor/server/src/login/auth_session.cpp auth_session::read_func
async fn read_binary_response(
    stream: &mut (impl AsyncRead + Unpin),
    command: BinCommand<'_>,
) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; auth_binary::RESPONSE_LEN];
    let mut total = 0;
    let mut expected = auth_binary::RESPONSE_MIN_LEN;
    while total < expected {
        match stream.read(&mut buf[total..expected]).await {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                expected = command.response_len(buf[0]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
    }
    buf.truncate(total);
    Ok(buf)
}

/// What the lobby is opened with. LSB's auth server mints `session_hash`
/// and the client carries it as the 16-byte `identifer` of every lobby
/// packet header; retail's PlayOnline Viewer supplies the identifer and the
/// authCode instead (vendor/server/src/login/login_packets.h packet_t).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub account_id: u32,

    pub session_hash: [u8; SESSION_HASH_LEN],

    #[serde(default)]
    pub auth_code: LobbyAuthCode,
}

impl AuthSession {
    /// True when the session carries an authCode, which only a PlayOnline
    /// handoff supplies; both LSB auth flavors leave it zero.
    pub fn is_playonline(&self) -> bool {
        !self.auth_code.is_none()
    }
}

pub const SESSION_HASH_LEN: usize = 16;

pub const LOBBY_AUTH_CODE_LEN: usize = 64;

/// The authCode retail's lobby validates in the 0x26. LSB never reads the
/// field, so both of its auth flavors leave it zero; only a PlayOnline
/// handoff fills it. Debug output redacts it because it is a live credential.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LobbyAuthCode(pub [u8; LOBBY_AUTH_CODE_LEN]);

impl LobbyAuthCode {
    pub const NONE: Self = Self([0; LOBBY_AUTH_CODE_LEN]);

    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }
}

impl Default for LobbyAuthCode {
    fn default() -> Self {
        Self::NONE
    }
}

impl std::fmt::Debug for LobbyAuthCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_none() {
            "LobbyAuthCode(none)"
        } else {
            "LobbyAuthCode(set)"
        })
    }
}

impl Serialize for LobbyAuthCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for LobbyAuthCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let mut code = [0u8; LOBBY_AUTH_CODE_LEN];
        hex::decode_to_slice(text.trim(), &mut code).map_err(|e| {
            serde::de::Error::custom(format!(
                "authCode must be {LOBBY_AUTH_CODE_LEN} bytes as hex: {e}"
            ))
        })?;
        Ok(Self(code))
    }
}

pub struct AuthClient {
    pub host: String,
    pub port: u16,
    pub verifier: Arc<TofuVerifier>,
    pub config: Arc<rustls::ClientConfig>,
    pub flavor: AuthFlavor,

    pub version: [u8; 3],
    pub binary_version: [u8; auth_binary::VERSION_FIELD_LEN],

    binary_builder: std::sync::OnceLock<Result<PayloadBuilder, BinaryAuthError>>,
}

impl AuthClient {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self::with_flavor(host, port, AuthFlavor::Json)
    }

    pub fn with_flavor(host: impl Into<String>, port: u16, flavor: AuthFlavor) -> Self {
        Self::with_flavor_and_version(host, port, flavor, None)
    }

    pub fn with_flavor_and_version(
        host: impl Into<String>,
        port: u16,
        flavor: AuthFlavor,
        version_override: Option<&str>,
    ) -> Self {
        let verifier = TofuVerifier::new();
        let config = crate::tls::make_client_config(verifier.clone());
        Self {
            host: host.into(),
            port,
            verifier,
            config,
            flavor,
            version: resolve_client_version(version_override),
            binary_version: resolve_binary_version(version_override),
            binary_builder: std::sync::OnceLock::new(),
        }
    }

    fn binary_builder(&self) -> Result<&PayloadBuilder> {
        let res = self
            .binary_builder
            .get_or_init(|| PayloadBuilder::with_version(self.binary_version));
        match res {
            Ok(b) => Ok(b),
            Err(e) => Err(anyhow!("binary auth builder unavailable: {e}")),
        }
    }

    pub async fn ensure_account(&self, username: &str, password: &str) -> Result<()> {
        if self.flavor == AuthFlavor::PlayOnline {
            bail!("a PlayOnline account is registered with Square Enix, not through Kuluu");
        }
        if self.flavor == AuthFlavor::Binary {
            return self.ensure_account_binary(username, password).await;
        }
        let payload = json!({
            "command": LOGIN_CREATE,
            "username": username,
            "password": password,
            "version": self.version,
        });
        let resp = self.exchange(&payload).await?;
        match json_result(&resp, "LOGIN_CREATE")? {
            LOGIN_SUCCESS_CREATE | LOGIN_ERROR_CREATE_TAKEN => Ok(()),
            other => {
                Err(AuthRejected(format!("LOGIN_CREATE: {}", describe_login_result(other))).into())
            }
        }
    }

    pub async fn change_password(
        &self,
        username: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<()> {
        if self.flavor != AuthFlavor::Json {
            bail!("change_password is only offered by the JSON auth flavor");
        }
        let payload = json!({
            "command": LOGIN_CHANGE_PASSWORD,
            "username": username,
            "password": current_password,
            "new_password": new_password,
            "version": self.version,
        });
        let resp = self.exchange(&payload).await?;
        match json_result(&resp, "LOGIN_CHANGE_PASSWORD")? {
            LOGIN_SUCCESS_CHANGE_PASSWORD => Ok(()),
            other => Err(AuthRejected(format!(
                "LOGIN_CHANGE_PASSWORD: {}",
                describe_login_result(other)
            ))
            .into()),
        }
    }

    /// Sign in to a PlayOnline account. The flavor authenticates two
    /// identities, which `login` has no room for.
    pub async fn login_playonline(
        &self,
        creds: crate::pol_inhouse::Credentials,
    ) -> Result<AuthSession> {
        if self.flavor != AuthFlavor::PlayOnline {
            bail!("this server does not use PlayOnline accounts");
        }
        crate::pol_inhouse::login(creds).await
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<AuthSession> {
        match self.flavor {
            AuthFlavor::Json => {}
            AuthFlavor::Binary => return self.login_binary(username, password).await,
            AuthFlavor::PlayOnline => {
                bail!(
                    "a PlayOnline sign-in needs both the PlayOnline and the Square Enix \
                     identity; call login_playonline"
                )
            }
        }
        let payload = json!({
            "command": LOGIN_ATTEMPT,
            "username": username,
            "password": password,
            "version": self.version,
        });
        let resp = self.exchange(&payload).await?;
        let result = json_result(&resp, "LOGIN_ATTEMPT")?;
        if result != LOGIN_SUCCESS {
            return Err(
                AuthRejected(format!("LOGIN_ATTEMPT: {}", describe_login_result(result))).into(),
            );
        }

        let account_id =
            resp.get("account_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| anyhow!("missing account_id in {resp}"))? as u32;

        let hash_arr = resp
            .get("session_hash")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("missing session_hash array in {resp}"))?;
        if hash_arr.len() != 16 {
            bail!(
                "session_hash has {} elements, expected 16: {resp}",
                hash_arr.len()
            );
        }
        let mut session_hash = [0u8; 16];
        for (i, v) in hash_arr.iter().enumerate() {
            session_hash[i] =
                v.as_u64()
                    .ok_or_else(|| anyhow!("session_hash[{i}] not u8: {v}"))? as u8;
        }

        Ok(AuthSession {
            account_id,
            session_hash,
            auth_code: LobbyAuthCode::NONE,
        })
    }

    async fn connect(&self) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        auth_timeout(AUTH_CONNECT_TIMEOUT, "connection", async {
            let connector = TlsConnector::from(self.config.clone());
            let server_name = ServerName::try_from(self.host.clone())
                .map_err(|_| anyhow!("invalid server name: {}", self.host))?;
            let tcp = TcpStream::connect((self.host.as_str(), self.port))
                .await
                .with_context(|| format!("TCP connect to {}:{}", self.host, self.port))?;
            Ok(connector.connect(server_name, tcp).await?)
        })
        .await
    }

    async fn exchange(&self, payload: &Value) -> Result<Value> {
        let mut tls = self.connect().await?;
        auth_timeout(AUTH_RESPONSE_TIMEOUT, "response", async {
            tls.write_all(payload.to_string().as_bytes()).await?;
            tls.flush().await?;
            read_json_response(&mut tls).await
        })
        .await
    }

    async fn login_binary(&self, username: &str, password: &str) -> Result<AuthSession> {
        let builder = self.binary_builder()?;
        let payload = builder
            .build(username, password, BinCommand::Login)
            .map_err(|e| anyhow!("build binary login payload: {e}"))?;
        let reply = self.exchange_binary(&payload, BinCommand::Login).await?;
        let (account_id, session_hash) = auth_binary::parse_response(&reply, builder.version)
            .map_err(|e| match e {
                BinaryAuthError::LoginFailed
                | BinaryAuthError::AlreadyLoggedIn
                | BinaryAuthError::VersionMismatch { .. } => {
                    anyhow::Error::from(AuthRejected(format!("binary login: {e}")))
                }
                other => anyhow::Error::from(other),
            })
            .with_context(|| {
                format!(
                    "binary login (server={}:{}, user={username})",
                    self.host, self.port
                )
            })?;
        Ok(AuthSession {
            account_id,
            session_hash,
            auth_code: LobbyAuthCode::NONE,
        })
    }

    async fn ensure_account_binary(&self, username: &str, password: &str) -> Result<()> {
        let builder = self.binary_builder()?;
        let payload = builder
            .build(username, password, BinCommand::Create)
            .map_err(|e| anyhow!("build binary create payload: {e}"))?;
        let reply = self.exchange_binary(&payload, BinCommand::Create).await?;
        match auth_binary::parse_create_response(&reply) {
            Ok(()) => Ok(()),
            Err(auth_binary::BinaryAuthError::CreateTaken) => Ok(()),
            Err(e) => Err(anyhow!("binary create: {e}")),
        }
    }

    async fn exchange_binary(
        &self,
        payload: &[u8; auth_binary::PAYLOAD_LEN],
        command: BinCommand<'_>,
    ) -> Result<Vec<u8>> {
        let mut tls = self.connect().await?;
        auth_timeout(AUTH_RESPONSE_TIMEOUT, "response", async {
            if std::env::var_os("FFXI_AUTH_TRACE").is_some() {
                eprintln!("[auth-trace] TX {} bytes:", payload.len());
                eprintln!("{}", hex_dump(payload));
            }
            tls.write_all(payload).await?;
            tls.flush().await?;
            let reply = read_binary_response(&mut tls, command).await?;
            if std::env::var_os("FFXI_AUTH_TRACE").is_some() {
                eprintln!("[auth-trace] RX {} bytes:", reply.len());
                eprintln!("{}", hex_dump(&reply));
            }
            Ok(reply)
        })
        .await
    }
}

fn hex_dump(b: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in b.chunks(16).enumerate() {
        out.push_str(&format!("  {:04x}  ", i * 16));
        for (j, byte) in chunk.iter().enumerate() {
            out.push_str(&format!("{:02x} ", byte));
            if j == 7 {
                out.push(' ');
            }
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push_str(" |");
        for &byte in chunk {
            out.push(if (0x20..0x7f).contains(&byte) {
                byte as char
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out
}

fn trim_trailing_zero(b: &[u8]) -> &[u8] {
    let end = b.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
    &b[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    // vendor/server/src/login/auth_session.h SupportedXiloaderVersion
    const LSB_SUPPORTED_XILOADER_VERSION: [u8; 3] = [2, 2, 0];

    const TEST_RESPONSE_LIMIT: Duration = Duration::from_secs(1);
    const TEST_STALL_LIMIT: Duration = Duration::from_millis(20);

    #[tokio::test]
    async fn binary_rejections_return_while_the_server_keeps_the_connection_open() {
        for result in [
            auth_binary::RESULT_LOGIN_FAIL,
            auth_binary::RESULT_VERSION_MISMATCH,
            auth_binary::RESULT_ALREADY_LOGGED_IN,
        ] {
            let (mut client, mut server) = tokio::io::duplex(AUTH_BUFFER_SIZE);
            server.write_all(&[result]).await.unwrap();
            let reply = auth_timeout(
                TEST_RESPONSE_LIMIT,
                "test",
                read_binary_response(&mut client, BinCommand::Login),
            )
            .await
            .unwrap();
            assert_eq!(reply, vec![result]);
            assert!(auth_binary::parse_response(&reply, auth_binary::DEFAULT_VERSION).is_err());
        }
    }

    #[tokio::test]
    async fn binary_create_success_returns_without_waiting_for_a_session() {
        let (mut client, mut server) = tokio::io::duplex(AUTH_BUFFER_SIZE);
        server
            .write_all(&[auth_binary::RESULT_CREATE_OK])
            .await
            .unwrap();
        let reply = auth_timeout(
            TEST_RESPONSE_LIMIT,
            "test",
            read_binary_response(&mut client, BinCommand::Create),
        )
        .await
        .unwrap();
        auth_binary::parse_create_response(&reply).unwrap();
    }

    #[tokio::test]
    async fn binary_success_collects_fragmented_account_and_session_data() {
        let mut expected = vec![0; auth_binary::RESPONSE_LEN];
        expected[0] = auth_binary::RESULT_LOGIN_OK;
        let (mut client, mut server) = tokio::io::duplex(auth_binary::RESPONSE_MIN_LEN);
        let bytes = expected.clone();
        let writer = tokio::spawn(async move {
            for byte in bytes {
                server.write_all(&[byte]).await.unwrap();
            }
            server
        });
        let reply = auth_timeout(
            TEST_RESPONSE_LIMIT,
            "test",
            read_binary_response(&mut client, BinCommand::Login),
        )
        .await
        .unwrap();
        let _open_server = writer.await.unwrap();
        assert_eq!(reply, expected);
        auth_binary::parse_response(&reply, auth_binary::DEFAULT_VERSION).unwrap();
    }

    #[tokio::test]
    async fn truncated_binary_success_is_not_accepted() {
        let (mut client, mut server) = tokio::io::duplex(AUTH_BUFFER_SIZE);
        server
            .write_all(&[auth_binary::RESULT_LOGIN_OK])
            .await
            .unwrap();
        drop(server);
        let reply = read_binary_response(&mut client, BinCommand::Login)
            .await
            .unwrap();
        assert!(matches!(
            auth_binary::parse_response(&reply, auth_binary::DEFAULT_VERSION),
            Err(BinaryAuthError::TruncatedSuccess(_))
        ));
    }

    #[tokio::test]
    async fn json_mode_reports_binary_rejection_without_waiting_for_close() {
        for result in [LOGIN_ERROR, LOGIN_ERROR_VERSION_UNSUPPORTED] {
            let (mut client, mut server) = tokio::io::duplex(AUTH_BUFFER_SIZE);
            server.write_all(&[result]).await.unwrap();
            let error = auth_timeout(TEST_RESPONSE_LIMIT, "test", read_json_response(&mut client))
                .await
                .unwrap_err();
            assert!(is_auth_rejected(&error));
            assert_eq!(error.to_string(), LEGACY_AUTH_HINT);
        }
    }

    #[tokio::test]
    async fn json_response_can_arrive_one_byte_at_a_time() {
        let response = json!({"result": LOGIN_ERROR});
        let bytes = response.to_string().into_bytes();
        let (mut client, mut server) = tokio::io::duplex(auth_binary::RESPONSE_MIN_LEN);
        let writer = tokio::spawn(async move {
            for byte in bytes {
                server.write_all(&[byte]).await.unwrap();
            }
            server
        });
        let reply = auth_timeout(TEST_RESPONSE_LIMIT, "test", read_json_response(&mut client))
            .await
            .unwrap();
        let _open_server = writer.await.unwrap();
        assert_eq!(reply, response);
    }

    #[tokio::test]
    async fn silent_and_partial_responses_hit_the_response_limit() {
        let (mut client, mut server) = tokio::io::duplex(AUTH_BUFFER_SIZE);
        let error = auth_timeout(
            TEST_STALL_LIMIT,
            "response",
            read_json_response(&mut client),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        server
            .write_all(&[auth_binary::RESULT_LOGIN_OK])
            .await
            .unwrap();
        let error = auth_timeout(
            TEST_STALL_LIMIT,
            "response",
            read_binary_response(&mut client, BinCommand::Login),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn loader_validation_rejects_extra_components_and_invalid_numbers() {
        for value in ["1.1.5.9", "+1.1.5", "256.1.5", "1..5", "1.1"] {
            assert!(
                validate_loader_version(AuthFlavor::Json, value).is_err(),
                "{value}"
            );
        }
        assert!(validate_loader_version(AuthFlavor::Binary, "10.1.5").is_err());
        assert!(validate_loader_version(AuthFlavor::Binary, " 1.1.5 ").is_ok());
        assert!(validate_loader_version(AuthFlavor::Json, "255.10.0").is_ok());
        assert!(validate_loader_version(AuthFlavor::Json, "").is_ok());
    }

    #[test]
    fn default_version_matches_lsb_supported_xiloader() {
        assert_eq!(
            resolve_client_version_from(None, None),
            LSB_SUPPORTED_XILOADER_VERSION
        );
        assert_eq!(
            ffxi_proto::login::SUPPORTED_XILOADER_VERSION,
            LSB_SUPPORTED_XILOADER_VERSION
        );
    }

    #[test]
    fn error_message_is_a_rejection_even_without_a_result() {
        let resp = json!({"error_message": "Your xiloader is too old.\nPlease update to version '2.1.x'."});
        let err = json_result(&resp, "LOGIN_ATTEMPT").unwrap_err();
        assert!(is_auth_rejected(&err));
        assert_eq!(
            err.to_string(),
            "LOGIN_ATTEMPT: Your xiloader is too old. Please update to version '2.1.x'."
        );
    }

    #[test]
    fn missing_result_is_not_a_rejection() {
        let err = json_result(&json!({"junk": 1}), "LOGIN_ATTEMPT").unwrap_err();
        assert!(!is_auth_rejected(&err));
    }

    #[test]
    fn result_codes_describe_the_server_verdict() {
        assert_eq!(
            json_result(&json!({"result": 1}), "x").unwrap(),
            LOGIN_SUCCESS
        );
        assert_eq!(
            describe_login_result(LOGIN_ERROR),
            "invalid username or password"
        );
        assert_eq!(describe_login_result(0x7F), "server result 0x7f");
    }

    #[test]
    fn rejection_survives_a_context_chain() {
        let err = anyhow::Error::from(AuthRejected("no".into())).context("auth login");
        assert!(is_auth_rejected(&err));
        assert!(!is_auth_rejected(&anyhow!("connection reset")));
    }

    #[test]
    fn binary_version_defaults_to_the_loader_field_not_the_json_triple() {
        assert_eq!(
            resolve_binary_version_from(None, None),
            auth_binary::DEFAULT_VERSION
        );
        assert_eq!(resolve_binary_version_from(Some("2.1.0"), None), *b"2.1.0");
        assert_eq!(resolve_binary_version_from(None, Some("3.0.1")), *b"3.0.1");
        assert_eq!(
            resolve_binary_version_from(Some("10.0.0"), Some("3.0.1")),
            *b"3.0.1"
        );
        assert_eq!(
            resolve_binary_version_from(Some("bad"), Some("worse")),
            auth_binary::DEFAULT_VERSION
        );
    }

    #[test]
    fn binary_client_threads_the_override_into_its_builder() {
        let c =
            AuthClient::with_flavor_and_version("127.0.0.1", 1, AuthFlavor::Binary, Some("2.1.0"));
        assert_eq!(c.binary_version, *b"2.1.0");
        if let Ok(b) = c.binary_builder() {
            assert_eq!(b.version, *b"2.1.0");
        }
    }

    #[test]
    fn override_wins_over_env() {
        assert_eq!(
            resolve_client_version_from(Some("3.4.5"), Some("9.9.9")),
            [3, 4, 5]
        );
    }

    #[test]
    fn env_wins_over_default() {
        assert_eq!(resolve_client_version_from(None, Some("2.9.1")), [2, 9, 1]);
    }

    #[test]
    fn invalid_override_falls_back_to_env() {
        assert_eq!(
            resolve_client_version_from(Some("not-a-version"), Some("2.1.5")),
            [2, 1, 5]
        );
    }

    #[test]
    fn invalid_override_and_env_fall_back_to_default() {
        assert_eq!(
            resolve_client_version_from(Some("x"), Some("1.2")),
            LSB_SUPPORTED_XILOADER_VERSION
        );
    }
}
