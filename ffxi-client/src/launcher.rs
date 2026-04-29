//! Interactive pre-TUI launcher. Drives auth + character selection from
//! plain stdin/stdout *before* the Bevy view enters alt-screen mode, so
//! authentication or "no such character" errors stay visible in the user's
//! normal terminal scrollback. Once a selection is made, the caller hands
//! off to the existing `spawn_session` + `view3d::run` flow.
//!
//! Stage A (this version) supports: prompt-driven login with on-demand
//! account creation, list characters, select one. `[c]reate` and
//! `[d]elete` print "not yet implemented" — both need wire-format work in
//! `lobby_client` (the existing `create_character` ships a wrong opcode
//! and would hang on a non-existent `chr_info2` push; delete has no
//! method at all yet).

use std::io::{self, BufRead, Write};

use anyhow::{Result, anyhow, bail};

use ffxi_client::auth_client::{AuthClient, AuthSession};
use ffxi_client::lobby_client::{CharSlot, LobbyClient};
use ffxi_client::session::InitialState;

/// What the launcher hands back to `main`. Carries the resolved CLI-arg
/// equivalents *plus* the fully-prepared `InitialState` so `session::run`
/// can skip auth and lobby entirely. This is the one path that reliably
/// avoids the same-hash / same-sockets collision the server is prone to
/// — see `session::Config::initial_state`.
pub struct Selection {
    pub user: String,
    pub password: String,
    pub char_id: u32,
    pub char_name: String,
    pub initial_state: InitialState,
}

/// Partial CLI args that should pre-fill launcher prompts. Anything `None`
/// is asked for interactively; anything `Some(...)` is used as the default
/// (Enter to accept, type to override). When `char_name` matches a slot
/// in the fetched list, we auto-select without showing the menu —
/// that's the "I just typo'd the password" recovery flow. Names on a
/// single account are unique by server constraint, so a single match is
/// sufficient.
#[derive(Default)]
pub struct Defaults {
    pub user: Option<String>,
    pub password: Option<String>,
    pub char_name: Option<String>,
}

/// Run the interactive flow. Loops on auth failure (retry / create account
/// / quit), then on the character menu (select / create / delete / quit).
/// Returns once the user has authenticated, selected, and the lobby has
/// completed its handoff — at which point `Selection::initial_state`
/// is ready to feed straight into `session::Config`.
pub async fn run(
    server: &str,
    auth: &AuthClient,
    lobby: &LobbyClient,
    defaults: Defaults,
) -> Result<Selection> {
    println!();
    println!("FFXI agent launcher — server {server}");
    println!();

    let (user, password, session) =
        login_loop(auth, defaults.user.as_deref(), defaults.password.as_deref()).await?;
    let handle = lobby
        .open(&session)
        .await
        .map_err(|e| anyhow!("opening lobby session: {e}"))?;

    // Resolve the desired slot — auto-select if the CLI gave us a name
    // that matches the live list, otherwise show the menu. On mismatch
    // (typo / wrong account) we deliberately fall through rather than
    // bailing out, so the user sees what's actually available.
    let slot = match defaults.char_name.as_deref() {
        Some(want) if handle.chars().iter().any(|c| c.name == want) => {
            let s = handle
                .chars()
                .iter()
                .find(|c| c.name == want)
                .unwrap()
                .clone();
            println!("Auto-selecting {} (charid {}).", s.name, s.char_id);
            s
        }
        Some(want) => {
            println!("No character named '{}' on this account; pick from the list:", want);
            character_menu(handle.chars())?
        }
        None => character_menu(handle.chars())?,
    };

    // Same key3 derivation the old `session::run` used; client-supplied
    // 20-byte blob ends up as the blowfish seed both lobby and map use.
    let mut key3 = [0u8; 20];
    for (i, b) in key3.iter_mut().enumerate() {
        *b = ((i as u8).wrapping_mul(0x37)) ^ 0x5a;
    }
    let handoff = handle
        .select(slot.char_id, &slot.name, key3)
        .await
        .map_err(|e| anyhow!("lobby select: {e}"))?;

    Ok(Selection {
        user,
        password,
        char_id: slot.char_id,
        char_name: slot.name,
        initial_state: InitialState {
            auth: session,
            handoff,
            key3,
        },
    })
}

/// Prompt for username + password, attempt login. On `LOGIN_FAIL` (wrong
/// password / unknown user) ask whether to create an account with the
/// supplied credentials, retry, or quit. `default_user` / `default_pw`
/// pre-fill the prompts on first iteration only — after a failed attempt
/// the user gets a fresh prompt with no defaults so they can correct
/// either field cleanly.
async fn login_loop(
    auth: &AuthClient,
    default_user: Option<&str>,
    default_pw: Option<&str>,
) -> Result<(String, String, AuthSession)> {
    let mut user_default = default_user.map(str::to_string);
    let mut pw_default = default_pw.map(str::to_string);
    loop {
        let user = prompt("Username", user_default.as_deref(), false)?;
        if user.is_empty() {
            println!("Username required.");
            continue;
        }
        let password = prompt("Password", pw_default.as_deref(), true)?;
        if password.is_empty() {
            println!("Password required.");
            continue;
        }
        // Only the first iteration uses CLI defaults; subsequent retries
        // start from blank so a corrected username doesn't drag a stale
        // password default back in.
        user_default = None;
        pw_default = None;

        match auth.login(&user, &password).await {
            Ok(session) => {
                println!("Login OK (account_id={}).\n", session.account_id);
                return Ok((user, password, session));
            }
            Err(e) => {
                println!("Login failed: {e}");
                let action = prompt(
                    "[r]etry, [c]reate account with this password, [q]uit",
                    Some("r"),
                    false,
                )?
                .to_lowercase();
                match action.as_str() {
                    "c" | "create" => {
                        auth.ensure_account(&user, &password)
                            .await
                            .map_err(|e| anyhow!("create account: {e}"))?;
                        println!("Account ensured. Re-attempting login...");
                        match auth.login(&user, &password).await {
                            Ok(session) => {
                                println!("Login OK (account_id={}).\n", session.account_id);
                                return Ok((user, password, session));
                            }
                            Err(e) => {
                                println!("Still failing after create: {e}");
                            }
                        }
                    }
                    "q" | "quit" | "exit" => {
                        bail!("user cancelled login");
                    }
                    _ => continue,
                }
            }
        }
    }
}

/// Present the character list and read a selection. Recognised inputs:
/// a 1-based slot number to select, `c` (create — stub), `d` (delete —
/// stub), `q` (quit). Loops until a valid character is chosen.
fn character_menu(chars: &[CharSlot]) -> Result<CharSlot> {
    if chars.is_empty() {
        println!("No characters on this account.");
        bail!(
            "no characters available (create flow not yet implemented — \
             use direct INSERT or wait for Stage B)"
        );
    }
    loop {
        println!("Characters:");
        for (i, c) in chars.iter().enumerate() {
            println!("  [{}] {}  (charid {})", i + 1, c.name, c.char_id);
        }
        let input = prompt("Select [1-N] / [c]reate / [d]elete / [q]uit", None, false)?;
        let lower = input.to_lowercase();
        match lower.as_str() {
            "c" | "create" => {
                println!(
                    "create: not yet implemented — `lobby_client::create_character` \
                     ships a broken wire format (opcode 0x01 instead of 0x21, plus \
                     hangs on a chr_info2 read). Stage B will fix this."
                );
                continue;
            }
            "d" | "delete" => {
                println!(
                    "delete: not yet implemented — server supports view-port \
                     opcode 0x14, but the client has no builder for `lpkt_deletechr` \
                     yet. Stage B will add it."
                );
                continue;
            }
            "q" | "quit" | "exit" => bail!("user cancelled selection"),
            _ => {}
        }
        if let Ok(idx) = input.parse::<usize>() {
            if (1..=chars.len()).contains(&idx) {
                return Ok(chars[idx - 1].clone());
            }
        }
        println!("Unrecognised input '{input}'. Try a number, or c/d/q.");
    }
}

/// Read one line from stdin, trim, return. The `[default]` hint shows the
/// pre-filled value if any (or `[***]` for secrets — the actual default
/// is still used when the user hits Enter, just not echoed). Empty input
/// returns the default; otherwise the typed input wins.
fn prompt(label: &str, default: Option<&str>, secret: bool) -> Result<String> {
    match (default, secret) {
        (Some(_), true) => print!("{label} [***]: "),
        (Some(d), false) => print!("{label} [{d}]: "),
        (None, _) => print!("{label}: "),
    }
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().lock().read_line(&mut buf)?;
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        if let Some(d) = default {
            return Ok(d.to_string());
        }
    }
    Ok(trimmed.to_string())
}
