#![allow(dead_code)]

pub mod mcp_client;

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use mysql_async::prelude::*;
use mysql_async::{Conn, Pool};

use kuluu_session::auth_client::AuthClient;

pub const DEFAULT_DB_URL: &str = "mysql://xiadmin:password@127.0.0.1:3306/xidb";

// A half-up stack (something accepts on the port but mysqld never completes
// the handshake) must self-skip like an absent one, not hang the test binary.
const XIDB_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const FIXTURE_PASSWORD: &str = "TestPass!1234";

/// Southern San d'Oria: the fixture's default home zone; the 503 live test
/// logs in here.
const DEFAULT_POS_ZONE: u32 = 230;

// vendor/server/sql/triggers.sql `char_insert` (BEFORE INSERT ON chars).
// A leftover row in any of these makes the next COALESCE(MAX(charid),…)+1
// insert fail 1062 from inside the trigger, reported against `chars`.
const TRIGGER_CHILD_TABLES: &[&str] = &[
    "char_equip",
    "char_exp",
    "char_history",
    "char_inventory",
    "char_jobs",
    "char_pet",
    "char_points",
    "char_profile",
    "char_storage",
    "char_unlocks",
];

// Rows this fixture inserts on top of the trigger's. `char_flags` appears in
// neither trigger in vendor/server/sql/triggers.sql, so only this list frees it.
const FIXTURE_CHILD_TABLES: &[&str] = &["char_flags", "char_look", "char_stats"];

fn char_child_tables() -> impl Iterator<Item = &'static str> {
    TRIGGER_CHILD_TABLES
        .iter()
        .chain(FIXTURE_CHILD_TABLES)
        .copied()
}

// Fixture-owned name shape, emitted by `create` and matched by the tombstone
// sweep. Nothing else in xidb may look like this.
const FIXTURE_ACCOUNT_PREFIX: &str = "it_";
const FIXTURE_CHARNAME_PREFIX: &str = "It";
const FIXTURE_SUFFIX_HEX_DIGITS: usize = 6;

// vendor/server/settings/default/map.lua `MAX_TIME_LASTUPDATE = 60`: a map
// session — and the charid it pins — outlives the client's last packet by this
// many seconds (vendor/server/src/map/map_session_container.cpp MapSessionContainer::cleanupSessions). While it
// is resident, LSB answers the lobby's CharZone by refreshing that session
// instead of creating the pending session a fresh login needs
// (vendor/server/src/map/ipc_client.cpp IPCClient::handleMessage_CharZone session), so the new client's 0x00A is
// dropped (map_networking.cpp MapNetworking::recv_parse) and it never zones in. The fixture therefore
// parks its account as a tombstone rather than deleting it, so neither
// MAX(accounts.id)+1 nor COALESCE(MAX(chars.charid),…)+1 can hand the same ids
// to the next test while its session may still be resident.
const LSB_MAX_TIME_LASTUPDATE_SECS: u32 = 60;
// The tombstone has to outlive the session's *last packet*, not the account's
// creation, so it carries a budget for the longest a live test runs.
const FIXTURE_SESSION_BUDGET_SECS: u32 = 300;
const TOMBSTONE_TTL_SECS: u32 = LSB_MAX_TIME_LASTUPDATE_SECS + FIXTURE_SESSION_BUDGET_SECS;

// Local UDP port range the live tests pin `FFXI_MAP_LOCAL_PORT` into: high
// enough to stay clear of the server's ports (map 53230 / view 54001 /
// data 54230 / auth 54231) and the OS's low ephemeral allocations, wide
// enough that two runs don't land on the same port.
const LOCAL_PORT_BASE: u16 = 49_000;
const LOCAL_PORT_SPAN: u32 = 10_000;

fn fixture_name_suffix(nanos: u128) -> String {
    let mask = (1u128 << (4 * FIXTURE_SUFFIX_HEX_DIGITS)) - 1;
    format!(
        "{:0width$x}",
        nanos & mask,
        width = FIXTURE_SUFFIX_HEX_DIGITS
    )
}

fn fixture_login_pattern() -> String {
    format!("^{FIXTURE_ACCOUNT_PREFIX}[0-9a-f]{{{FIXTURE_SUFFIX_HEX_DIGITS}}}$")
}

/// Pin a unique local UDP port for this live-test run so the map client's
/// socket avoids reusing a port that is still resident on the map server. The
/// server matches sessions by source IP:port and keeps a session for 60 s
/// after the client's last packet (vendor/server/settings/default/map.lua
/// MAX_TIME_LASTUPDATE, vendor/server/src/map/map_session_container.cpp
/// MapSessionContainer::cleanupSessions); an ephemeral bind reuses the
/// just-freed port, so a back-to-back live test gets matched to the previous
/// run's session and its 0x00A is rejected with "Player ID mismatch"
/// (vendor/server/src/map/packets/c2s/0x00a_login.cpp). A random high port
/// avoids that. Windows may exclude random sub-ranges of the port space from
/// user binds (Hyper-V/WinNAT; `netsh int ipv4 show excludedportrange
/// protocol=udp`), so each candidate is probe-bound before it is pinned.
/// Sets FFXI_MAP_LOCAL_PORT, which MapClient::connect reads.
pub fn pin_unique_local_port() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    for offset in 0..LOCAL_PORT_SPAN {
        let port = LOCAL_PORT_BASE + ((nanos + offset) % LOCAL_PORT_SPAN) as u16;
        if std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok() {
            std::env::set_var("FFXI_MAP_LOCAL_PORT", port.to_string());
            eprintln!("[live] pinned local UDP port {port}");
            return;
        }
    }
    panic!(
        "no bindable local UDP port in {LOCAL_PORT_BASE}..={}",
        LOCAL_PORT_BASE + LOCAL_PORT_SPAN as u16
    );
}

pub struct EphemeralChar {
    pub username: String,
    pub password: String,
    pub accid: u32,
    pub charid: u32,
    pub charname: String,
    pool: Pool,
}

/// A saturated or shutting-down mysqld still accepts the TCP connection and
/// then refuses the handshake with an ERR packet, so it arrives here as
/// Error::Server, not Error::Io. These are the codes that mean "the daemon is
/// up but cannot serve anyone right now", as opposed to a credential/schema
/// mistake this fixture is responsible for (1045 access denied, 1049 unknown
/// database), which must still fail the test.
/// https://mariadb.com/kb/en/mariadb-error-code-reference/
const ER_CON_COUNT_ERROR: u16 = 1040;
const ER_SERVER_SHUTDOWN: u16 = 1053;
const ER_HOST_IS_BLOCKED: u16 = 1129;
const ER_TOO_MANY_USER_CONNECTIONS: u16 = 1203;
const ER_USER_LIMIT_REACHED: u16 = 1226;
const XIDB_UNAVAILABLE_SERVER_CODES: &[u16] = &[
    ER_CON_COUNT_ERROR,
    ER_SERVER_SHUTDOWN,
    ER_HOST_IS_BLOCKED,
    ER_TOO_MANY_USER_CONNECTIONS,
    ER_USER_LIMIT_REACHED,
];

fn xidb_unavailable(err: &mysql_async::Error) -> bool {
    match err {
        mysql_async::Error::Io(_) => true,
        mysql_async::Error::Server(server) => XIDB_UNAVAILABLE_SERVER_CODES.contains(&server.code),
        _ => false,
    }
}

/// Ok(None) = xidb cannot hand out a usable session (timed out mid-handshake,
/// the accept-then-drop / refused IO class, or a server that answered the
/// handshake with a capacity/availability error) and the caller should
/// self-skip; any other failure is a real provisioning error and still
/// propagates.
async fn xidb_conn(db_url: &str, connect_timeout: Duration) -> Result<Option<(Pool, Conn)>> {
    let pool = Pool::new(db_url);
    match tokio::time::timeout(connect_timeout, pool.get_conn()).await {
        Ok(Ok(conn)) => Ok(Some((pool, conn))),
        Ok(Err(err)) if xidb_unavailable(&err) => {
            eprintln!("xidb at {db_url}: handshake failed ({err}); treating as unreachable");
            let _ = pool.disconnect().await;
            Ok(None)
        }
        Ok(Err(err)) => {
            let _ = pool.disconnect().await;
            Err(err).with_context(|| format!("connecting to xidb at {db_url}"))
        }
        Err(_) => {
            eprintln!(
                "xidb at {db_url}: no handshake within {connect_timeout:?}; \
                 treating as unreachable"
            );
            let _ = pool.disconnect().await;
            Ok(None)
        }
    }
}

impl EphemeralChar {
    pub async fn create(server_host: &str, auth_port: u16) -> Result<Option<Self>> {
        Self::create_in_zone(server_host, auth_port, DEFAULT_POS_ZONE).await
    }

    pub async fn create_in_zone(
        server_host: &str,
        auth_port: u16,
        pos_zone: u32,
    ) -> Result<Option<Self>> {
        let db_url = std::env::var("TEST_DB_URL").unwrap_or_else(|_| DEFAULT_DB_URL.to_string());

        let suffix = fixture_name_suffix(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );

        let username = format!("{FIXTURE_ACCOUNT_PREFIX}{suffix}");
        let charname = format!("{FIXTURE_CHARNAME_PREFIX}{suffix}");

        let password = FIXTURE_PASSWORD.to_string();

        let Some((pool, mut conn)) = xidb_conn(&db_url, XIDB_CONNECT_TIMEOUT).await? else {
            return Ok(None);
        };

        let auth = AuthClient::new(server_host.to_string(), auth_port);
        auth.ensure_account(&username, &password)
            .await
            .context("LOGIN_CREATE for ephemeral account")?;

        let accid: u32 = "SELECT id FROM accounts WHERE login = ?"
            .with((&username,))
            .first(&mut conn)
            .await
            .context("looking up accid for new ephemeral account")?
            .ok_or_else(|| anyhow!("ensure_account succeeded but accid {username:?} not found"))?;

        const NATION: u8 = 0;
        const GMLEVEL: u8 = 5;

        const FACE: u8 = 0;
        const RACE: u8 = 1;
        const SIZE: u8 = 0;

        const MJOB: u8 = 1;

        sweep_expired_tombstones(&mut conn)
            .await
            .context("sweeping expired fixture accounts before provisioning")?;
        sweep_orphaned_child_rows(&mut conn)
            .await
            .context("sweeping orphaned char_* rows before provisioning")?;

        let charid = run_inserts(
            &mut conn, accid, &charname, pos_zone, NATION, GMLEVEL, FACE, RACE, SIZE, MJOB,
        )
        .await
        .context("running LSB char-creation INSERT chain")?;

        drop(conn);

        Ok(Some(Self {
            username,
            password,
            accid,
            charid,
            charname,
            pool,
        }))
    }

    // Frees only the session row; the account (and the char + child rows it
    // cascades to) is left as the id-reuse tombstone that
    // `sweep_expired_tombstones` retires once TOMBSTONE_TTL_SECS have passed.
    pub async fn cleanup(&self) -> Result<()> {
        let mut conn = self.pool.get_conn().await.context("DB conn for cleanup")?;

        // Must go: LSB refuses the next login for an accid that still has a
        // session row (vendor/server/src/login/data_session.cpp data_session::read_func).
        "DELETE FROM accounts_sessions WHERE accid = ?"
            .with((self.accid,))
            .ignore(&mut conn)
            .await
            .context("DELETE FROM accounts_sessions")?;

        Ok(())
    }

    /// Insert or update one char_vars row for this fixture char — quest vars
    /// like the hidden-quest notSeen flag that gate zone-in events.
    pub async fn set_char_var(&self, varname: &str, value: i32) -> Result<()> {
        let mut conn = self.pool.get_conn().await.context("DB conn for char var")?;

        "INSERT INTO char_vars(charid, varname, value) VALUES (?, ?, ?) \
         ON DUPLICATE KEY UPDATE value = VALUES(value)"
            .with((self.charid, varname, value))
            .ignore(&mut conn)
            .await
            .context("upserting char_vars row")?;

        let stored: i32 = "SELECT value FROM char_vars WHERE charid = ? AND varname = ?"
            .with((self.charid, varname))
            .first(&mut conn)
            .await
            .context("reading back the char_vars row")?
            .ok_or_else(|| anyhow!("char_vars {varname:?} missing after upsert"))?;

        if stored != value {
            return Err(anyhow!(
                "char_vars {varname:?} read back as {stored}, expected {value}"
            ));
        }

        Ok(())
    }

    /// Grant `amount` gil: gil is the currency item (id 0) of the main
    /// inventory (vendor/server/src/map/lua/lua_base_entity.cpp getGil reads
    /// getStorage(LOC_INVENTORY)->GetItem(0)). The fixture's char-creation
    /// trigger already inserts an empty (itemId 65535) row at the currency
    /// slot, so upsert it.
    pub async fn add_gil(&self, amount: u32) -> Result<()> {
        let mut conn = self.pool.get_conn().await.context("DB conn for gil")?;
        "INSERT INTO char_inventory(charid, location, slot, itemId, quantity) \
         VALUES (?, 0, 0, 0, ?) \
         ON DUPLICATE KEY UPDATE itemId = 0, quantity = VALUES(quantity)"
            .with((self.charid, amount))
            .ignore(&mut conn)
            .await
            .context("upserting gil into char_inventory")?;
        Ok(())
    }

    /// Grant a key item by its id (vendor/server/scripts/enum/key_item.lua),
    /// e.g. 138 = CHOCOBO_LICENSE. The keyitems column is a fixed blob of
    /// little-endian uint16 ids; an empty slot is 0.
    pub async fn add_key_item(&self, id: u16) -> Result<()> {
        let mut conn = self.pool.get_conn().await.context("DB conn for key item")?;
        // A fresh fixture char's keyitems is NULL; start from an empty blob of
        // the column's width (512 uint16s) in that case.
        let blob: Option<Vec<u8>> = "SELECT keyitems FROM chars WHERE charid = ?"
            .with((self.charid,))
            .first(&mut conn)
            .await
            .context("reading keyitems blob")?
            .ok_or_else(|| anyhow!("chars row {charid} not found", charid = self.charid))?;
        let blob = blob.unwrap_or_else(|| vec![0u8; 512 * 2]);
        let mut ids: Vec<u16> = blob
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        // Find a free slot (0) or reuse the last one; the blob is large enough
        // that a free slot always exists for a fresh fixture char.
        let slot = ids.iter().position(|&v| v == 0).unwrap_or(ids.len() - 1);
        ids[slot] = id;
        let mut new_blob = Vec::with_capacity(ids.len() * 2);
        for &v in &ids {
            new_blob.extend_from_slice(&v.to_le_bytes());
        }
        "UPDATE chars SET keyitems = ? WHERE charid = ?"
            .with((&new_blob, self.charid))
            .ignore(&mut conn)
            .await
            .context("UPDATE chars keyitems")?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_inserts(
    conn: &mut Conn,
    accid: u32,
    charname: &str,
    pos_zone: u32,
    nation: u8,
    gmlevel: u8,
    face: u8,
    race: u8,
    size: u8,
    mjob: u8,
) -> Result<u32> {
    // Mirror LSB's own char creation (MAX(charid)+1) inside a single
    // INSERT ... SELECT, then read the actual created row back. Precomputing
    // an id from a sentinel scheme is unsound: nothing guarantees the next id,
    // and a charid that doesn't match the created row makes the lobby reject
    // char select with "mismatched character name".
    "INSERT INTO chars(charid, accid, charname, pos_zone, nation, gmlevel) \
     SELECT COALESCE(MAX(c.charid), 1000000) + 1, ?, ?, ?, ?, ? FROM chars AS c"
        .with((accid, charname, pos_zone, nation, gmlevel))
        .ignore(&mut *conn)
        .await
        .context("INSERT INTO chars")?;

    let charid: u32 = "SELECT charid FROM chars WHERE accid = ? AND charname = ? \
                       ORDER BY charid DESC LIMIT 1"
        .with((accid, charname))
        .first(&mut *conn)
        .await
        .context("reading back created charid")?
        .ok_or_else(|| {
            anyhow!("chars row for accid {accid} / charname {charname:?} not found after insert")
        })?;

    "INSERT INTO char_look(charid, face, race, size) VALUES (?, ?, ?, ?)"
        .with((charid, face, race, size))
        .ignore(&mut *conn)
        .await
        .context("INSERT INTO char_look")?;

    "INSERT INTO char_stats(charid, mjob) VALUES (?, ?)"
        .with((charid, mjob))
        .ignore(&mut *conn)
        .await
        .context("INSERT INTO char_stats")?;

    for table in [
        "char_exp",
        "char_flags",
        "char_jobs",
        "char_points",
        "char_unlocks",
        "char_profile",
        "char_storage",
    ] {
        let stmt = format!(
            "INSERT INTO {table}(charid) VALUES (?) ON DUPLICATE KEY UPDATE charid = charid"
        );
        stmt.with((charid,))
            .ignore(&mut *conn)
            .await
            .with_context(|| format!("INSERT INTO {table}"))?;
    }

    Ok(charid)
}

// Retires tombstones whose map session cannot still be resident. Scoped by the
// login shape this fixture emits, so a real account is never a candidate. One
// DELETE frees the whole identity: LSB's `account_delete` trigger cascades to
// `chars`, whose `char_delete` trigger cascades to the child tables
// (vendor/server/sql/triggers.sql).
async fn sweep_expired_tombstones(conn: &mut Conn) -> Result<()> {
    "DELETE FROM accounts \
     WHERE login REGEXP ? \
       AND UNIX_TIMESTAMP(timecreate) < UNIX_TIMESTAMP() - ?"
        .with((fixture_login_pattern(), TOMBSTONE_TTL_SECS))
        .ignore(&mut *conn)
        .await
        .context("tombstone sweep on accounts")?;

    let swept = conn.affected_rows();
    if swept > 0 {
        eprintln!("fixture: swept {swept} expired fixture account tombstone(s)");
    }
    Ok(())
}

// LSB's map server REPLACEs into char_history on save well after the client
// drops (vendor/server/src/map/utils/charutils.cpp db::preparedStmt), and a panicking test
// never reaches cleanup() at all, so a previous run can leave child rows whose
// `chars` row is gone. Sweeping them here is what keeps the `char_insert`
// trigger from colliding when their charid comes back around.
async fn sweep_orphaned_child_rows(conn: &mut Conn) -> Result<()> {
    for table in char_child_tables() {
        let stmt = format!(
            "DELETE t FROM {table} AS t \
             LEFT JOIN chars AS c ON c.charid = t.charid \
             WHERE c.charid IS NULL"
        );
        conn.query_drop(&stmt)
            .await
            .with_context(|| format!("orphan sweep on {table}"))?;
        let swept = conn.affected_rows();
        if swept > 0 {
            eprintln!("fixture: swept {swept} orphaned {table} row(s) with no chars row");
        }
    }
    Ok(())
}

#[cfg(test)]
mod xidb_conn_tests {
    use super::*;

    use tokio::io::AsyncWriteExt;

    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

    fn db_url(port: u16) -> String {
        format!("mysql://user:pass@127.0.0.1:{port}/xidb")
    }

    // MySQL/MariaDB wire packet: 3-byte LE payload length, 1-byte sequence id
    // (0 for the server's first packet). A server that refuses before the
    // handshake sends an ERR packet - 0xFF, LE u16 code, message - with no
    // SQL-state field, because no capabilities have been negotiated yet.
    // https://mariadb.com/docs/server/reference/clientserver-protocol/0-packet
    // https://mariadb.com/docs/server/reference/clientserver-protocol/4-server-response-packets/err_packet
    // The pre-handshake SQL-state omission is what mysql_async 0.37
    // src/conn/mod.rs handle_packet parses against (empty capabilities).
    const PACKET_LENGTH_BYTES: usize = 3;
    const SERVER_FIRST_PACKET_SEQ: u8 = 0;
    const ERR_PACKET_TAG: u8 = 0xFF;

    fn err_packet(code: u16, message: &str) -> Vec<u8> {
        let mut payload = vec![ERR_PACKET_TAG];
        payload.extend_from_slice(&code.to_le_bytes());
        payload.extend_from_slice(message.as_bytes());

        let mut framed = (payload.len() as u32).to_le_bytes()[..PACKET_LENGTH_BYTES].to_vec();
        framed.push(SERVER_FIRST_PACKET_SEQ);
        framed.extend_from_slice(&payload);
        framed
    }

    async fn spawn_refusing_server(packet: Vec<u8>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let packet = packet.clone();
                tokio::spawn(async move {
                    let _ = sock.write_all(&packet).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn accept_then_drop_self_skips() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });

        let got = xidb_conn(&db_url(port), HANDSHAKE_TIMEOUT)
            .await
            .expect("accept-then-drop must self-skip, not error");
        assert!(got.is_none());
    }

    /// A TCP-reachability gate reads a saturated mysqld as healthy and fails
    /// the live test; only the handshake result distinguishes the two.
    #[tokio::test]
    async fn saturated_server_self_skips() {
        let port =
            spawn_refusing_server(err_packet(ER_CON_COUNT_ERROR, "Too many connections")).await;

        let got = xidb_conn(&db_url(port), HANDSHAKE_TIMEOUT)
            .await
            .expect("a server-side connection-limit refusal must self-skip, not error");
        assert!(got.is_none());
    }

    /// The other half of the gate: a handshake refusal this fixture caused is
    /// not an unhealthy server, and must still fail the test loudly.
    #[tokio::test]
    async fn access_denied_still_errors() {
        const ER_ACCESS_DENIED_ERROR: u16 = 1045;
        assert!(!XIDB_UNAVAILABLE_SERVER_CODES.contains(&ER_ACCESS_DENIED_ERROR));

        let port = spawn_refusing_server(err_packet(
            ER_ACCESS_DENIED_ERROR,
            "Access denied for user 'user'@'localhost' (using password: YES)",
        ))
        .await;

        let err = xidb_conn(&db_url(port), HANDSHAKE_TIMEOUT)
            .await
            .expect_err("a credential failure must propagate, not self-skip");
        assert!(
            format!("{err:#}").contains(&ER_ACCESS_DENIED_ERROR.to_string()),
            "expected the server error code in the propagated chain, got: {err:#}"
        );
    }

    #[tokio::test]
    async fn refused_port_self_skips() {
        let reserved_unlistened_socket = tokio::net::TcpSocket::new_v4().unwrap();
        reserved_unlistened_socket
            .bind(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                0,
            )))
            .unwrap();
        let port = reserved_unlistened_socket.local_addr().unwrap().port();

        let got = xidb_conn(&db_url(port), HANDSHAKE_TIMEOUT)
            .await
            .expect("connect-refused must self-skip, not error");
        assert!(got.is_none());
        drop(reserved_unlistened_socket);
    }
}

#[cfg(test)]
mod fixture_name_tests {
    use super::*;

    // The sweep matches names with a SQL REGEXP built from these same consts;
    // this pins the emitter to the character class that pattern accepts.
    #[test]
    fn emitted_names_match_the_sweep_pattern() {
        for nanos in [0u128, 1, 0x0f_ff_ff, u128::MAX] {
            let suffix = fixture_name_suffix(nanos);
            assert_eq!(suffix.len(), FIXTURE_SUFFIX_HEX_DIGITS);
            assert!(suffix
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));

            let login = format!("{FIXTURE_ACCOUNT_PREFIX}{suffix}");
            assert!(login.starts_with(FIXTURE_ACCOUNT_PREFIX));
            assert_eq!(
                login.len(),
                FIXTURE_ACCOUNT_PREFIX.len() + FIXTURE_SUFFIX_HEX_DIGITS
            );
        }
    }
}
