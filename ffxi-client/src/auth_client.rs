//! xiloader-style auth over TLS-TCP.
//!
//! Speaks the protocol implemented by `server/src/login/auth_session.cpp`:
//! one JSON command per connection, server replies with one JSON object,
//! both sides close. Length is implicit (read until EOF / fixed buffer).

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;

use crate::tls::TofuVerifier;

/// Server-side max packet/buffer length. Server reads up to 4096 (see
/// `handler_session.h`); we keep parity here.
const AUTH_BUFFER_SIZE: usize = 4096;

/// Xiloader version this client claims. Must match
/// `SupportedXiloaderVersion` in `server/src/login/auth_session.h`
/// (currently `[2, 1, 0]`).
const CLIENT_VERSION: [u8; 3] = [2, 1, 0];

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
}

impl AuthClient {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let verifier = TofuVerifier::new();
        let config = crate::tls::make_client_config(verifier.clone());
        Self {
            host: host.into(),
            port,
            verifier,
            config,
        }
    }

    /// Best-effort: create the account if it doesn't exist. Returns Ok(()) for
    /// "created" and "already exists"; surfaces other errors.
    pub async fn ensure_account(&self, username: &str, password: &str) -> Result<()> {
        let payload = json!({
            "command": LOGIN_CREATE,
            "username": username,
            "password": password,
            "version": CLIENT_VERSION,
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
        let payload = json!({
            "command": LOGIN_ATTEMPT,
            "username": username,
            "password": password,
            "version": CLIENT_VERSION,
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

        let account_id = resp
            .get("account_id")
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
            session_hash[i] = v
                .as_u64()
                .ok_or_else(|| anyhow!("session_hash[{i}] not u8: {v}"))?
                as u8;
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
        // Read until EOF or buffer full or we have valid JSON.
        loop {
            let n = tls.read(&mut buf[total..]).await?;
            if n == 0 {
                break;
            }
            total += n;
            // Try to parse what we have.
            if let Ok(v) = serde_json::from_slice::<Value>(&buf[..total]) {
                return Ok(v);
            }
            if total == buf.len() {
                bail!("auth response exceeded {AUTH_BUFFER_SIZE} bytes");
            }
        }
        // EOF. Try one last parse.
        let trimmed = trim_trailing_zero(&buf[..total]);
        serde_json::from_slice(trimmed)
            .with_context(|| format!("auth server reply was not valid JSON: {trimmed:?}"))
    }
}

fn trim_trailing_zero(b: &[u8]) -> &[u8] {
    let end = b.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
    &b[..end]
}
