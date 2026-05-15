//! Per-test ephemeral LSB account + character fixture.
//!
//! Goal: each integration test gets a uniquely-named, freshly-stamped account
//! and character so two tests never collide on `accounts_sessions` (LSB's
//! 60-second post-disconnect map-session lockout is keyed on `accid`, not
//! `charid` — fresh accounts dodge it entirely).
//!
//! Strategy:
//!   1. `auth_client::ensure_account` — `LOGIN_CREATE` over the auth port so
//!      the password hash is computed by LSB itself. Avoids reimplementing
//!      LSB's password hashing in the test fixture.
//!   2. SQL `SELECT id FROM accounts WHERE login = ?` to find the new accid.
//!   3. SQL `INSERT` chain mirroring `server/src/login/login_helpers.cpp:140`
//!      `saveCharacter()` — eleven tables. We do this in SQL (not via the
//!      protocol-level char create) because the protocol path is half-built
//!      and not what we want to put in the test setup hot path.
//!   4. `cleanup()` — explicit teardown call from the test. Drop is *not*
//!      used because `#[tokio::test]` shutdown can deadlock async work in a
//!      synchronous Drop impl.

#![allow(dead_code)] // Used by integration test crates; rustc analyzes per-target.

pub mod mcp_client;

use anyhow::{anyhow, Context, Result};
use mysql_async::prelude::*;
use mysql_async::{Conn, Pool};

use ffxi_client::auth_client::AuthClient;

/// Default LSB dev-stack DB URL (matches `server/dev.docker-compose.yml`).
/// Override with `TEST_DB_URL` if your stack runs elsewhere.
pub const DEFAULT_DB_URL: &str = "mysql://xiadmin:password@127.0.0.1:3306/xidb";

/// Test password we stamp into every fresh account. Strength doesn't matter —
/// the account is deleted in `cleanup()`.
const FIXTURE_PASSWORD: &str = "TestPass!1234";

/// Char tables we INSERT into, mirroring `saveCharacter()` in
/// `server/src/login/login_helpers.cpp:140-214`. Order matters for cleanup
/// (cleanup walks this in reverse).
const CHAR_TABLES: &[&str] = &[
    "char_inventory",
    "char_storage",
    "char_profile",
    "char_unlocks",
    "char_points",
    "char_jobs",
    "char_flags",
    "char_exp",
    "char_stats",
    "char_look",
    "chars",
];

/// Owned fixture: holds the unique credentials and IDs the test should use,
/// and a connection pool kept open for `cleanup()`.
pub struct EphemeralChar {
    pub username: String,
    pub password: String,
    pub accid: u32,
    pub charid: u32,
    pub charname: String,
    pool: Pool,
}

impl EphemeralChar {
    pub async fn create(server_host: &str, auth_port: u16) -> Result<Self> {
        let db_url = std::env::var("TEST_DB_URL").unwrap_or_else(|_| DEFAULT_DB_URL.to_string());

        // 24-bit suffix from nanoseconds-since-epoch — collision space is
        // 16M, real-world collisions require sub-nanosecond test scheduling
        // which the OS won't grant.
        let suffix: u32 = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            & 0xFFFFFF) as u32;

        let username = format!("it_{suffix:06x}");
        let charname = format!("It{suffix:06x}");
        // High-namespace IDs: stays out of the way of human-friendly
        // hand-stamped accounts/chars (typically <1000) and — critically —
        // prevents LSB's in-memory `MapSession` cache (keyed on accid) from
        // shadowing a new account that recycled a deleted accid via MariaDB
        // AUTO_INCREMENT. Without this, two back-to-back test runs collide
        // on the cached session and the second hangs in lobby handshake.
        let target_accid = 1_000_000u32 + suffix;
        let charid = 1_000_000u32 + suffix;
        let password = FIXTURE_PASSWORD.to_string();

        // Step 1: open the DB pool early so we can bump AUTO_INCREMENT
        // *before* the account is created.
        let pool = Pool::new(db_url.as_str());
        let mut conn = pool
            .get_conn()
            .await
            .with_context(|| format!("connecting to xidb at {db_url}"))?;

        // Step 2: insert a sentinel row at `target_accid - 1` so LSB's
        // `MAX(id) + 1` arithmetic (auth_session.cpp:344-357) computes our
        // target as the next account id. AUTO_INCREMENT alone is *not*
        // sufficient — LSB ignores it and recomputes from MAX(id) on every
        // create. The sentinel is removed in step 5.
        let sentinel_login = format!("_fix_{suffix:06x}");
        "INSERT INTO accounts(id, login, password, timecreate, status, priv) \
         VALUES (?, ?, '', NOW(), 0, 0)"
            .with((target_accid - 1, &sentinel_login))
            .ignore(&mut conn)
            .await
            .context("inserting accid sentinel row")?;

        // Step 3: register the account through the auth protocol so password
        // hashing comes from LSB. The next available id is now `target_accid`
        // because of the sentinel.
        let auth = AuthClient::new(server_host.to_string(), auth_port);
        auth.ensure_account(&username, &password)
            .await
            .context("LOGIN_CREATE for ephemeral account")?;

        // Step 4: confirm the accid we got. If MariaDB had a higher counter
        // than we asked for (concurrent INSERTs from another process), the
        // bump is a no-op and we'd get a different accid — better to fail
        // loudly than to silently mismatch with later assumptions.
        let accid: u32 = "SELECT id FROM accounts WHERE login = ?"
            .with((&username,))
            .first(&mut conn)
            .await
            .context("looking up accid for new ephemeral account")?
            .ok_or_else(|| anyhow!("ensure_account succeeded but accid {username:?} not found"))?;
        if accid != target_accid {
            return Err(anyhow!(
                "accid mismatch: requested {target_accid} but got accid={accid} \
                 (concurrent inserts on accounts table during fixture setup?)"
            ));
        }

        // Step 5: drop the sentinel — its only job was to influence the
        // next-id arithmetic in ensure_account.
        "DELETE FROM accounts WHERE login = ?"
            .with((&sentinel_login,))
            .ignore(&mut conn)
            .await
            .context("dropping accid sentinel row")?;

        // Step 6: gmlevel=5 because zone_change's !zone needs gmlevel >= 1;
        // we pick 5 to leave headroom for any future GM-only command tests.
        // pos_zone=230 (East Ronfaure) is a real starting zone, distinct from
        // the zone_change test's TARGET_ZONE_ID=100 (West Ronfaure) so the
        // !zone command actually moves us.
        const POS_ZONE: u32 = 230;
        const NATION: u8 = 0; // San d'Oria
        const GMLEVEL: u8 = 5;
        // char_look defaults: face=0, race=1 (HumeM), size=0
        const FACE: u8 = 0;
        const RACE: u8 = 1;
        const SIZE: u8 = 0;
        // char_stats: mjob=1 (Warrior, valid starting job per
        // login_helpers.cpp:248).
        const MJOB: u8 = 1;

        run_inserts(
            &mut conn, accid, charid, &charname, POS_ZONE, NATION, GMLEVEL, FACE, RACE, SIZE, MJOB,
        )
        .await
        .context("running LSB char-creation INSERT chain")?;

        drop(conn);

        Ok(Self {
            username,
            password,
            accid,
            charid,
            charname,
            pool,
        })
    }

    /// Tear down everything we created. Call before the test's assertions so
    /// failure paths still clean up — we do this by `&self` so the caller
    /// keeps the fixture alive for diagnostics if cleanup itself errors.
    pub async fn cleanup(&self) -> Result<()> {
        let mut conn = self.pool.get_conn().await.context("DB conn for cleanup")?;

        // Walk char tables in reverse insert order. `IF EXISTS` would be
        // ideal but MariaDB doesn't support it on DELETE; instead, every
        // statement is unconditionally `DELETE ... WHERE charid = ?` which
        // is a no-op if the row was never inserted (e.g., earlier insert
        // failed mid-chain).
        for table in CHAR_TABLES.iter().rev() {
            let stmt = format!("DELETE FROM {table} WHERE charid = ?");
            stmt.with((self.charid,))
                .ignore(&mut conn)
                .await
                .with_context(|| format!("DELETE FROM {table}"))?;
        }

        // accounts_sessions can linger up to 60s after disconnect; deleting
        // it explicitly is what makes per-test isolation actually idempotent
        // even within the lockout window.
        "DELETE FROM accounts_sessions WHERE accid = ?"
            .with((self.accid,))
            .ignore(&mut conn)
            .await
            .context("DELETE FROM accounts_sessions")?;

        "DELETE FROM accounts WHERE id = ?"
            .with((self.accid,))
            .ignore(&mut conn)
            .await
            .context("DELETE FROM accounts")?;

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_inserts(
    conn: &mut Conn,
    accid: u32,
    charid: u32,
    charname: &str,
    pos_zone: u32,
    nation: u8,
    gmlevel: u8,
    face: u8,
    race: u8,
    size: u8,
    mjob: u8,
) -> Result<()> {
    // chars: explicit gmlevel because the default is 0 and zone_change.rs's
    // !zone command requires gmlevel >= 1.
    "INSERT INTO chars(charid, accid, charname, pos_zone, nation, gmlevel) \
     VALUES (?, ?, ?, ?, ?, ?)"
        .with((charid, accid, charname, pos_zone, nation, gmlevel))
        .ignore(&mut *conn)
        .await
        .context("INSERT INTO chars")?;

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

    // The remaining tables only need `charid` — defaults handle everything
    // else. ON DUPLICATE KEY makes them safe against a partial earlier run.
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

    // char_inventory has a DELETE-then-INSERT pattern in LSB's saveCharacter.
    "DELETE FROM char_inventory WHERE charid = ?"
        .with((charid,))
        .ignore(&mut *conn)
        .await
        .context("DELETE FROM char_inventory (pre-insert)")?;
    "INSERT INTO char_inventory(charid) VALUES (?)"
        .with((charid,))
        .ignore(&mut *conn)
        .await
        .context("INSERT INTO char_inventory")?;

    Ok(())
}
