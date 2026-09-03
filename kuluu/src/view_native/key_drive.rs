//! Generic synthetic-key injection listener (`FFXI_KEY_DRIVE`) — same pattern as
//! `FFXI_STAIR_DRIVE` in [crate::view_native::input]. A TCP JSON-line driver
//! queues key messages; `key_drive_system` (PreUpdate) writes them into Bevy's
//! global `KeyboardInput` event stream so the launcher UI screens (login, char
//! select, ...) react to remote input with NO OS keystrokes and no window
//! focus required. The same mechanism works in-game: systems that read raw
//! keyboard events see the synthetic presses exactly like real key hits.
//!
//! Protocol — one JSON object per line:
//!   {"key":"Enter"}          tap (press + release) a named or single-char key
//!   {"key":"W","down":true}  press and HOLD (pair with up)
//!   {"key":"W","up":true}    release a held key
//!   {"text":"cowpass"}       type literal text; each character becomes a tap

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// One queued injection action.
#[derive(Debug)]
pub enum KeyMsg {
    Tap(String),
    Press(String),
    Release(String),
    Type(String),
}

impl KeyMsg {
    /// Decode one driver line into a message (see module docs for the protocol).
    pub fn from_json_line(line: &str) -> Option<Self> {
        let v = serde_json::from_str::<serde_json::Value>(line.trim()).ok()?;
        if let Some(text) = v.get("text").and_then(|x| x.as_str()) {
            return Some(KeyMsg::Type(text.to_string()));
        }
        let key = v.get("key")?.as_str()?;
        let down = v.get("down").and_then(|x| x.as_bool()).unwrap_or(false);
        let up = v.get("up").and_then(|x| x.as_bool()).unwrap_or(false);
        match (down, up) {
            // A lone "key" is a tap; explicit down/up give hold control.
            (_, true) => Some(KeyMsg::Release(key.to_ascii_lowercase())),
            (true, false) => Some(KeyMsg::Press(key.to_ascii_lowercase())),
            _ => Some(KeyMsg::Tap(key.to_ascii_lowercase())),
        }
    }

    /// Resolve a key name to its physical + logical pair. Letters and digits map
    /// to `Key*`/`Digit*`; everything else is a named special key. Unknown
    /// names are rejected (logged by the system) rather than mis-mapped.
    pub fn resolve(name: &str) -> Option<(KeyCode, Key)> {
        let n = name.to_ascii_lowercase();
        if n.len() == 1 {
            let c = n.chars().next()?;
            let char_lk = || Key::Character(n.chars().collect());
            return match c {
                'a'..='z' => Some((char_keycode(c)?, char_lk())),
                '0'..='9' => Some((digit_keycode(c)?, char_lk())),
                '/' => Some((KeyCode::Slash, char_lk())),
                ',' => Some((KeyCode::Comma, char_lk())),
                '.' => Some((KeyCode::Period, char_lk())),
                '-' => Some((KeyCode::Minus, char_lk())),
                _ => None,
            };
        }
        let (kc, lk) = match n.as_str() {
            "enter" | "return" => (KeyCode::Enter, Key::Enter),
            "escape" | "esc" => (KeyCode::Escape, Key::Escape),
            "tab" => (KeyCode::Tab, Key::Tab),
            "space" => (KeyCode::Space, Key::Space),
            "backspace" => (KeyCode::Backspace, Key::Backspace),
            "delete" | "del" => (KeyCode::Delete, Key::Delete),
            "home" => (KeyCode::Home, Key::Home),
            "end" => (KeyCode::End, Key::End),
            "pageup" | "pgup" => (KeyCode::PageUp, Key::PageUp),
            "pagedown" | "pgdn" => (KeyCode::PageDown, Key::PageDown),
            "up" => (KeyCode::ArrowUp, Key::ArrowUp),
            "down" => (KeyCode::ArrowDown, Key::ArrowDown),
            "left" => (KeyCode::ArrowLeft, Key::ArrowLeft),
            "right" => (KeyCode::ArrowRight, Key::ArrowRight),
            _ => return None,
        };
        Some((kc, lk))
    }
}

fn char_keycode(c: char) -> Option<KeyCode> {
    let kc = match c {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        _ => return None,
    };
    Some(kc)
}

fn digit_keycode(c: char) -> Option<KeyCode> {
    Some(match c {
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        _ => KeyCode::Digit9,
    })
}

/// Shared queue written by the listener task, drained by `key_drive_system`.
#[derive(Resource)]
pub struct KeyDriveQueue(pub Arc<Mutex<Vec<KeyMsg>>>);

impl Default for KeyDriveQueue {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

/// Bind and serve the `FFXI_STAIR_DRIVE`-style TCP listener. One JSON line per
/// connection; each valid line enqueues one [KeyMsg]. Malformed lines are
/// skipped (never drop the connection over a typo).
pub async fn serve_key_drive(addr: SocketAddr, queue: Arc<Mutex<Vec<KeyMsg>>>) {
    let Ok(listener) = tokio::net::TcpListener::bind(addr).await else {
        tracing::warn!(%addr, "FFXI_KEY_DRIVE bind failed");
        return;
    };
    tracing::info!(%addr, "FFXI_KEY_DRIVE listening");
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            break;
        };
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(tokio::io::BufWriter::new(sock)).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match KeyMsg::from_json_line(&line) {
                Some(msg) => {
                    if let Ok(mut q) = queue.lock() {
                        q.push(msg);
                    }
                }
                None => tracing::debug!(line = %line, "FFXI_KEY_DRIVE: unparseable line"),
            }
        }
    }
}

/// PreUpdate: drain the queue into global `KeyboardInput` events so every
/// Update-phase consumer (launcher screens, in-game input, text buffers) sees
/// the same frame's synthetic presses. A tap is a press+release pair queued
/// back-to-back; holds are explicit down/up messages from the driver.
pub fn key_drive_system(
    mut events: MessageWriter<KeyboardInput>,
    queue: Res<KeyDriveQueue>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return; // no window yet (first frame) — keep the queued messages
    };
    let mut batch = std::mem::take(&mut *match queue.0.lock() {
        Ok(mut q) => q,
        Err(_) => return,
    });

    for msg in batch.drain(..) {
        match msg {
            KeyMsg::Tap(name) => write_tap(&mut events, window, &name),
            KeyMsg::Press(name) => {
                if let Some((kc, lk)) = KeyMsg::resolve(&name) {
                    write_state(&mut events, window, kc, lk, ButtonState::Pressed);
                } else {
                    tracing::warn!(%name, "FFXI_KEY_DRIVE: unknown key name");
                }
            }
            KeyMsg::Release(name) => {
                if let Some((kc, lk)) = KeyMsg::resolve(&name) {
                    write_state(&mut events, window, kc, lk, ButtonState::Released);
                } else {
                    tracing::warn!(%name, "FFXI_KEY_DRIVE: unknown key name");
                }
            }
            KeyMsg::Type(text) => {
                for c in text.chars() {
                    write_tap_char(&mut events, window, c);
                }
            }
        }
    }
}

fn write_state(
    events: &mut MessageWriter<KeyboardInput>,
    window: Entity,
    kc: KeyCode,
    lk: Key,
    state: ButtonState,
) {
    events.write(KeyboardInput {
        key_code: kc,
        logical_key: lk,
        state,
        text: None,
        repeat: false,
        window,
    });
}

fn write_tap(events: &mut MessageWriter<KeyboardInput>, window: Entity, name: &str) {
    match KeyMsg::resolve(name) {
        Some(pair) => {
            write_state(events, window, pair.0, pair.1.clone(), ButtonState::Pressed);
            write_state(events, window, pair.0, pair.1, ButtonState::Released);
        }
        None => tracing::warn!(%name, "FFXI_KEY_DRIVE: unknown key name"),
    }
}

/// Type one character as a tap. Mapped letters/digits/symbols carry their real
/// physical `KeyCode`; anything else rides on an inert physical code while the
/// logical `Key::Character` + `text` fields deliver the glyph to raw-event
/// handlers (chat buffers, launcher text fields).
fn write_tap_char(events: &mut MessageWriter<KeyboardInput>, window: Entity, c: char) {
    let name = c.to_string();
    let mapped = KeyMsg::resolve(&name);
    let (kc, lk) = match mapped {
        Some((kc, lk)) => (kc, lk),
        None => (KeyCode::Backquote, Key::Character(name.chars().collect())),
    };
    write_state(events, window, kc, lk, ButtonState::Pressed);
    events.write(KeyboardInput {
        key_code: kc,
        logical_key: Key::Character(name.chars().collect()),
        state: ButtonState::Released,
        text: Some(std::iter::once(c).collect()),
        repeat: false,
        window,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_line_parses() {
        assert!(matches!(
            KeyMsg::from_json_line(r#"{"key":"Enter"}"#),
            Some(KeyMsg::Tap(k)) if k == "enter"
        ));
    }

    #[test]
    fn hold_lines_parse() {
        assert!(matches!(
            KeyMsg::from_json_line(r#"{"key":"W","down":true}"#),
            Some(KeyMsg::Press(_))
        ));
        assert!(matches!(
            KeyMsg::from_json_line(r#"{"key":"w","up":true}"#),
            Some(KeyMsg::Release(k)) if k == "w"
        ));
    }

    #[test]
    fn text_line_parses() {
        assert!(matches!(
            KeyMsg::from_json_line(r#"{"text":"cowpass"}"#),
            Some(KeyMsg::Type(t)) if t == "cowpass"
        ));
    }

    #[test]
    fn unknown_lines_rejected() {
        // is_none() (not assert_eq!(.., None)): rkyv's cross-type PartialEq
        // impls (via ffxi-nav-recast) break bare-None inference in assert_eq!
        assert!(KeyMsg::from_json_line("not json").is_none());
        assert!(KeyMsg::from_json_line(r#"{"foo":1}"#).is_none());
    }

    #[test]
    fn resolve_named_and_chars() {
        assert_eq!(KeyMsg::resolve("Enter").unwrap().0, KeyCode::Enter);
        let (kc, lk) = KeyMsg::resolve("w").unwrap();
        assert_eq!(kc, KeyCode::KeyW);
        match lk {
            Key::Character(s) => assert_eq!(s.as_str(), "w"),
            other => panic!("expected Character, got {other:?}"),
        }
        let (kc, _) = KeyMsg::resolve("7").unwrap();
        assert_eq!(kc, KeyCode::Digit7);
        assert_eq!(KeyMsg::resolve("Frobnicate"), None);
    }
}
