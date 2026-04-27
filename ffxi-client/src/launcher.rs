use std::io::{self, BufRead, Write};

use anyhow::{anyhow, bail, Result};

use ffxi_client::auth_client::{AuthClient, AuthSession};
use ffxi_client::lobby_client::{CharSlot, LobbyClient};
use ffxi_client::session::InitialState;

pub struct Selection {
    pub user: String,
    pub password: String,
    pub char_id: u32,
    pub char_name: String,
    pub initial_state: InitialState,
}

#[derive(Default)]
pub struct Defaults {
    pub user: Option<String>,
    pub password: Option<String>,
    pub char_name: Option<String>,
}

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
            println!(
                "No character named '{}' on this account; pick from the list:",
                want
            );
            character_menu(handle.chars())?
        }
        None => character_menu(handle.chars())?,
    };

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
