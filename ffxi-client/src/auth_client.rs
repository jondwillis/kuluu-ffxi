//! xiloader-style auth over TLS-TCP.
//!
//! Speaks the protocol implemented by `server/src/login/auth_session.cpp`:
//! one JSON command per connection, server replies with one JSON object,
//! both sides close. Length is implicit (read until EOF / fixed buffer).

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

/// Wire encoding of the auth handshake. Selected per-deployment because
/// xiloader-family servers diverged: LSB rewrote auth to JSON in PR #7
/// (WinterSolstice8), while HorizonXI's hxiloader fork retained a binary
/// payload (102-byte write / 21-byte read) and added a MAC + version
/// slot. See `auth_binary` for the exact byte layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFlavor {
    /// LSB JSON rewrite (single JSON object → JSON reply → EOF).
    Json,
    /// hxiloader v1.3-style binary handshake. Reads the local MAC at
    /// construction time; reuses it for every login attempt.
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

/// Server-side max packet/buffer length. Server reads up to 4096 (see
/// `handler_session.h`); we keep parity here.
const AUTH_BUFFER_SIZE: usize = 4096;

/// Default xiloader version sent in JSON-flavor auth. Matches LSB
/// upstream's `SupportedXiloaderVersion` in
/// `server/src/login/auth_session.h`. **Override with the
/// `FFXI_XILOADER_VERSION=major.minor.patch` env var** when targeting a
/// fork on a different pin — e.g. HorizonXI currently requires
/// `2.0.x` and rejects `2.1.0` as "too old". The server only compares
/// `major.minor`, so the patch value is free.
const DEFAULT_CLIENT_VERSION: [u8; 3] = [2, 1, 0];

/// Resolve the version triple to send. Precedence:
/// 1. `override_` arg (e.g. from a CLI flag) — beats env so users can
///    override a sticky shell var per-invocation.
/// 2. `FFXI_XILOADER_VERSION` env var.
/// 3. [`DEFAULT_CLIENT_VERSION`].
///
/// Malformed values are logged and fall through to the next layer.
pub fn resolve_client_version(override_: Option<&str>) -> [u8; 3] {
    if let Some(s) = override_ {
        if let Some(v) = parse_version_triple(s) {
            return v;
        }
        tracing::warn!("--xiloader-version={s:?} invalid; falling back to env/default");
    }
    if let Ok(s) = std::env::var("FFXI_XILOADER_VERSION") {
        if let Some(v) = parse_version_triple(&s) {
            return v;
        }
        tracing::warn!("FFXI_XILOADER_VERSION={s:?} invalid; using default");
    }
    DEFAULT_CLIENT_VERSION
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

// Command IDs from server/src/login/auth_session.h.
const LOGIN_ATTEMPT: u8 = 0x10;
const LOGIN_CREATE: u8 = 0x20;

// Result IDs used by v1.
pub const LOGIN_FAIL: u8 = 0x00;
pub const LOGIN_SUCCESS: u8 = 0x01;
pub const LOGIN_ERROR: u8 = 0x02;
pub const LOGIN_SUCCESS_CREATE: u8 = 0x03;
pub const LOGIN_ERROR_CREATE_TAKEN: u8 = 0x04;
pub const LOGIN_ERROR_CREATE_DISABLED: u8 = 0x08;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub account_id: u32,
    /// 16-byte session_hash returned by the server. Used to correlate this
    /// auth session with the data and view sockets.
    pub session_hash: [u8; 16],
}

pub struct AuthClient {
    pub host: String,
    pub port: u16,
    pub verifier: Arc<TofuVerifier>,
    pub config: Arc<rustls::ClientConfig>,
    pub flavor: AuthFlavor,
    /// Resolved at construction, beats env. JSON flavor only.
    pub version: [u8; 3],
    /// Lazily-constructed binary-payload builder. `None` until first use
    /// in `Binary` flavor; failure to read the local MAC is deferred so
    /// JSON-flavor users don't pay the syscall.
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

    /// Best-effort: create the account if it doesn't exist. Returns Ok(()) for
    /// "created" and "already exists"; surfaces other errors.
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
            LOGIN_ERROR_CREATE_TAKEN => Ok(()), // pre-existing account is fine
            LOGIN_ERROR_CREATE_DISABLED => bail!("server disabled account creation"),
            other => bail!("LOGIN_CREATE failed with result {other:#x}: {resp}"),
        }
    }

    /// Authenticate and return the session metadata.
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

    /// One JSON request → one JSON response → close. Per
    /// `auth_session.cpp::read_func`, the server writes its response back into
    /// the connection-scoped buffer and does not close on success — but on the
    /// next read it will get our EOF, so we close from the client side after
    /// reading.
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
        // No length prefix; the server reads the whole buffer at once. Flush
        // and half-close so the server's read returns and it sends a reply.
        tls.flush().await?;

        let mut buf = vec![0u8; AUTH_BUFFER_SIZE];
        let mut total = 0usize;
        // Read until EOF or buffer full or we have valid JSON. The LSB
        // auth server (and hxiloader on the binary path) closes the TCP
        // socket without a TLS close_notify; since rustls 0.23 that
        // surfaces as `io::ErrorKind::UnexpectedEof` on the next read
        // instead of n=0. Treat both as graceful end-of-stream so we can
        // still parse the bytes that already arrived. See
        // <https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html#unexpected-eof>.
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
        // EOF. Try one last parse.
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

        // hxiloader explicitly `shutdown(SD_BOTH); closesocket(...)` after
        // writing its 21-byte reply — no TLS close_notify. rustls 0.23
        // turns that into `UnexpectedEof`; treat it as EOF so we still
        // return whatever bytes already arrived (typically the full
        // 21-byte reply).
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

/// Canonical xxd-style 16-byte-per-row hex dump. Used only by the auth
/// trace path; cheap enough to compile in unconditionally.
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
