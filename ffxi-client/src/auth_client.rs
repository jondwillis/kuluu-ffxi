use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;

use crate::auth_binary::{self, BinaryAuthError, Command as BinCommand, PayloadBuilder};
use crate::tls::TofuVerifier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFlavor {
    Json,

    Binary,
}

impl std::str::FromStr for AuthFlavor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" | "lsb" => Ok(AuthFlavor::Json),
            "binary" | "hxi" | "horizon" => Ok(AuthFlavor::Binary),
            other => Err(format!(
                "unknown auth-flavor `{other}`; expected json|binary"
            )),
        }
    }
}

const AUTH_BUFFER_SIZE: usize = 4096;

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

fn parse_version_triple(s: &str) -> Option<[u8; 3]> {
    let mut out = [0u8; 3];
    let mut count = 0;
    for (i, part) in s.trim().split('.').enumerate().take(3) {
        out[i] = part.parse::<u8>().ok()?;
        count += 1;
    }
    if count == 3 {
        Some(out)
    } else {
        None
    }
}

const LOGIN_ATTEMPT: u8 = 0x10;
const LOGIN_CREATE: u8 = 0x20;
const LOGIN_CHANGE_PASSWORD: u8 = 0x30;

pub const LOGIN_FAIL: u8 = 0x00;
pub const LOGIN_SUCCESS: u8 = 0x01;
pub const LOGIN_ERROR: u8 = 0x02;
pub const LOGIN_SUCCESS_CREATE: u8 = 0x03;
pub const LOGIN_ERROR_CREATE_TAKEN: u8 = 0x04;
pub const LOGIN_SUCCESS_CHANGE_PASSWORD: u8 = 0x06;
pub const LOGIN_ERROR_CHANGE_PASSWORD: u8 = 0x07;
pub const LOGIN_ERROR_CREATE_DISABLED: u8 = 0x08;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub account_id: u32,

    pub session_hash: [u8; 16],
}

pub struct AuthClient {
    pub host: String,
    pub port: u16,
    pub verifier: Arc<TofuVerifier>,
    pub config: Arc<rustls::ClientConfig>,
    pub flavor: AuthFlavor,

    pub version: [u8; 3],

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
            binary_builder: std::sync::OnceLock::new(),
        }
    }

    fn binary_builder(&self) -> Result<&PayloadBuilder> {
        let res = self.binary_builder.get_or_init(PayloadBuilder::new);
        match res {
            Ok(b) => Ok(b),
            Err(e) => Err(anyhow!("binary auth builder unavailable: {e}")),
        }
    }

    pub async fn ensure_account(&self, username: &str, password: &str) -> Result<()> {
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
        let result = resp
            .get("result")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("LOGIN_CREATE response missing `result`: {resp}"))?
            as u8;
        match result {
            LOGIN_SUCCESS_CREATE => Ok(()),
            LOGIN_ERROR_CREATE_TAKEN => Ok(()),
            LOGIN_ERROR_CREATE_DISABLED => bail!("server disabled account creation"),
            other => bail!("LOGIN_CREATE failed with result {other:#x}: {resp}"),
        }
    }

    pub async fn change_password(
        &self,
        username: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<()> {
        if self.flavor == AuthFlavor::Binary {
            bail!("change_password unsupported in binary auth flavor");
        }
        let payload = json!({
            "command": LOGIN_CHANGE_PASSWORD,
            "username": username,
            "password": current_password,
            "new_password": new_password,
            "version": self.version,
        });
        let resp = self.exchange(&payload).await?;
        let result = resp
            .get("result")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("LOGIN_CHANGE_PASSWORD response missing `result`: {resp}"))?
            as u8;
        match result {
            LOGIN_SUCCESS_CHANGE_PASSWORD => Ok(()),
            other => bail!("LOGIN_CHANGE_PASSWORD failed with result {other:#x}: {resp}"),
        }
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<AuthSession> {
        if self.flavor == AuthFlavor::Binary {
            return self.login_binary(username, password).await;
        }
        let payload = json!({
            "command": LOGIN_ATTEMPT,
            "username": username,
            "password": password,
            "version": self.version,
        });
        let resp = self.exchange(&payload).await?;
        let result = resp
            .get("result")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("LOGIN_ATTEMPT response missing `result`: {resp}"))?
            as u8;
        if result != LOGIN_SUCCESS {
            bail!("LOGIN_ATTEMPT failed with result {result:#x}: {resp}");
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
        })
    }

    async fn exchange(&self, payload: &Value) -> Result<Value> {
        let connector = TlsConnector::from(self.config.clone());
        let server_name = ServerName::try_from(self.host.clone())
            .map_err(|_| anyhow!("invalid server name: {}", self.host))?;
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("TCP connect to {}:{}", self.host, self.port))?;
        let mut tls = connector.connect(server_name, tcp).await?;

        let body = payload.to_string();
        tls.write_all(body.as_bytes()).await?;

        tls.flush().await?;

        let mut buf = vec![0u8; AUTH_BUFFER_SIZE];
        let mut total = 0usize;

        loop {
            match tls.read(&mut buf[total..]).await {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if let Ok(v) = serde_json::from_slice::<Value>(&buf[..total]) {
                        return Ok(v);
                    }
                    if total == buf.len() {
                        bail!("auth response exceeded {AUTH_BUFFER_SIZE} bytes");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }

        let trimmed = trim_trailing_zero(&buf[..total]);
        serde_json::from_slice(trimmed)
            .with_context(|| format!("auth server reply was not valid JSON: {trimmed:?}"))
    }

    async fn login_binary(&self, username: &str, password: &str) -> Result<AuthSession> {
        let builder = self.binary_builder()?;
        let payload = builder
            .build(username, password, BinCommand::Login)
            .map_err(|e| anyhow!("build binary login payload: {e}"))?;
        let reply = self.exchange_binary(&payload).await?;
        let (account_id, session_hash) = auth_binary::parse_response(&reply, builder.version)
            .with_context(|| {
                format!(
                    "binary login (server={}:{}, user={username})",
                    self.host, self.port
                )
            })?;
        Ok(AuthSession {
            account_id,
            session_hash,
        })
    }

    async fn ensure_account_binary(&self, username: &str, password: &str) -> Result<()> {
        let builder = self.binary_builder()?;
        let payload = builder
            .build(username, password, BinCommand::Create)
            .map_err(|e| anyhow!("build binary create payload: {e}"))?;
        let reply = self.exchange_binary(&payload).await?;
        match auth_binary::parse_create_response(&reply) {
            Ok(()) => Ok(()),
            Err(auth_binary::BinaryAuthError::CreateTaken) => Ok(()),
            Err(e) => Err(anyhow!("binary create: {e}")),
        }
    }

    async fn exchange_binary(&self, payload: &[u8; auth_binary::PAYLOAD_LEN]) -> Result<Vec<u8>> {
        let connector = TlsConnector::from(self.config.clone());
        let server_name = ServerName::try_from(self.host.clone())
            .map_err(|_| anyhow!("invalid server name: {}", self.host))?;
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("TCP connect to {}:{}", self.host, self.port))?;
        let mut tls = connector.connect(server_name, tcp).await?;
        if std::env::var_os("FFXI_AUTH_TRACE").is_some() {
            eprintln!("[auth-trace] TX {} bytes:", payload.len());
            eprintln!("{}", hex_dump(payload));
        }
        tls.write_all(payload).await?;
        tls.flush().await?;

        let mut buf = vec![0u8; auth_binary::RESPONSE_LEN];
        let mut total = 0;
        while total < buf.len() {
            match tls.read(&mut buf[total..]).await {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        buf.truncate(total);
        if std::env::var_os("FFXI_AUTH_TRACE").is_some() {
            eprintln!("[auth-trace] RX {} bytes:", buf.len());
            eprintln!("{}", hex_dump(&buf));
        }
        Ok(buf)
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
    const LSB_SUPPORTED_XILOADER_VERSION: [u8; 3] = [2, 1, 0];

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
