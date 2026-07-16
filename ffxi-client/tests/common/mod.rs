#![allow(dead_code)]

pub mod mcp_client;

use anyhow::{anyhow, Context, Result};
use mysql_async::prelude::*;
use mysql_async::{Conn, Pool};

use ffxi_client::auth_client::AuthClient;

pub const DEFAULT_DB_URL: &str = "mysql://xiadmin:password@127.0.0.1:3306/xidb";

const FIXTURE_PASSWORD: &str = "TestPass!1234";

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

        let suffix: u32 = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            & 0xFFFFFF) as u32;

        let username = format!("it_{suffix:06x}");
        let charname = format!("It{suffix:06x}");

        let password = FIXTURE_PASSWORD.to_string();

        let pool = Pool::new(db_url.as_str());
        let mut conn = pool
            .get_conn()
            .await
            .with_context(|| format!("connecting to xidb at {db_url}"))?;

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

        const POS_ZONE: u32 = 230;
        const NATION: u8 = 0;
        const GMLEVEL: u8 = 5;

        const FACE: u8 = 0;
        const RACE: u8 = 1;
        const SIZE: u8 = 0;

        const MJOB: u8 = 1;

        let charid = run_inserts(
            &mut conn, accid, &charname, POS_ZONE, NATION, GMLEVEL, FACE, RACE, SIZE, MJOB,
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

    pub async fn cleanup(&self) -> Result<()> {
        let mut conn = self.pool.get_conn().await.context("DB conn for cleanup")?;

        for table in CHAR_TABLES.iter().rev() {
            let stmt = format!("DELETE FROM {table} WHERE charid = ?");
            stmt.with((self.charid,))
                .ignore(&mut conn)
                .await
                .with_context(|| format!("DELETE FROM {table}"))?;
        }

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

    Ok(charid)
}
