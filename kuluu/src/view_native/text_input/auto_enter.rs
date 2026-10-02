//! Debug "Auto-Enter CS" row (enternity-style): while on, event-dialog
//! message frames advance themselves after their read time instead of
//! parking on Enter.
//!
//! Retail mechanism: an event line's stop is the 0x7F 0x31 marker in the
//! text stream while the VM parks at MESWAIT with the box open
//! (research/XiEvents/OpCodes/0x0023.md). The Windower addon enternity
//! strips that marker so the box scrolls and closes by itself; kuluu's
//! equivalent is client-side — after the read time it sends the same
//! `EndEventChoice` Enter would send.
//!
//! Enternity's exceptions are kept: choice frames (the addon "will not skip
//! choice dialog boxes"), item lines ("sentences that contain items will
//! not be skipped"), text-entry frames, server custom menus, and its two
//! blacklisted NPCs.

use std::time::{Duration, Instant};

use bevy::prelude::{Local, Res, Time};

use kuluu_render::hud::HudPanels;
use kuluu_render::SceneState;
use kuluu_session::state::AgentCommand;
use kuluu_snapshot::DialogState;

use crate::view_native::input::CommandTx;

/// Enternity's blacklist (addon `blist`): its lines are not skipped —
/// "Paintbrush of Souls" needs its timing, "Geomantic Reservoir" freezes the
/// dialogue.
const BLACKLIST: &[&str] = &["Paintbrush of Souls", "Geomantic Reservoir"];

/// Seconds of display per character: retail's dialog typing pace, so a line
/// is still readable before auto-enter advances it. Deliberate tuning.
const READ_TIME_PER_CHAR: f32 = 0.025;
/// Short lines still get a beat to land.
const READ_TIME_MIN: f32 = 1.5;
/// Long monologues do not drag the scene.
const READ_TIME_MAX: f32 = 12.0;
/// A frame outliving this after its advance command means the advance did
/// not land (the session round-trips in milliseconds); re-arm the clock.
const ADVANCE_GRACE: f32 = 0.5;
/// How long the player's own advance holds auto-enter's fire: the session
/// round-trip is milliseconds, so this only needs to cover the few frames
/// between the manual Enter and the snapshot showing the next frame; firing
/// inside that window would make the session dismiss the frame the manual
/// advance just opened.
const MANUAL_GUARD: Duration = Duration::from_millis(500);

/// Only plain event-VM message frames advance themselves. `prompt` being
/// `Some` excludes the raw-packet fallback path (those frames are not
/// MESWAIT gates); choices are branches the player must pick; text-entry and
/// custom-menu frames answer with different commands; item lines and
/// blacklisted speakers are enternity's exceptions (matched on the trigger's
/// name, the same `get_mob_by_target('t')` the addon checks).
pub fn eligible(d: &DialogState) -> bool {
    d.prompt.is_some()
        && !d.contains_item
        && d.choices.is_empty()
        && !d.text_entry
        && !d.custom_menu
        && d.npc_name
            .as_deref()
            .is_none_or(|name| !BLACKLIST.contains(&name))
}

/// How long a frame stays up before auto-enter advances it.
pub fn read_time(text: &str) -> f32 {
    (text.len() as f32 * READ_TIME_PER_CHAR).clamp(READ_TIME_MIN, READ_TIME_MAX)
}

/// Whether auto-enter holds its fire: true while the player's own dialog
/// advance is still inside the session round-trip window.
pub fn manual_advance_guard(last: Option<Instant>, window: Duration) -> bool {
    last.is_some_and(|at| at.elapsed() <= window)
}

/// The per-frame clock, held in the system's `Local`.
#[derive(Default)]
pub struct Clock {
    /// The frame the clock is running for.
    frame: Option<DialogState>,
    /// Seconds accumulated on `frame`.
    elapsed: f32,
    /// Time since the advance command for `frame` went out; while the
    /// session round-trips the same frame stays up, and a fresh read time
    /// would double-advance it.
    pending_since: Option<f32>,
}

impl Clock {
    /// Advance by `dt` seconds with `frame` up (`None` = box down). Returns
    /// the frame to advance when its read time has elapsed. A new frame
    /// starts its read clock; a frame change also clears any in-flight
    /// advance, since the session has moved on. When the advance outlives
    /// ADVANCE_GRACE it has not landed, and the clock re-arms with a fresh
    /// read time.
    pub fn tick(&mut self, frame: Option<&DialogState>, dt: f32) -> Option<DialogState> {
        let frame = frame.filter(|d| eligible(d));
        let Some(d) = frame else {
            *self = Self::default();
            return None;
        };
        if self.frame.as_ref() != Some(d) {
            self.frame = Some(d.clone());
            self.elapsed = 0.0;
            self.pending_since = None;
        }
        if let Some(since) = self.pending_since.as_mut() {
            *since += dt;
            if *since > ADVANCE_GRACE {
                self.pending_since = None;
                self.elapsed = 0.0;
            } else {
                return None;
            }
        } else {
            self.elapsed += dt;
        }
        if self.elapsed >= read_time(d.prompt.as_deref().unwrap_or_default()) {
            self.pending_since = Some(0.0);
            Some(d.clone())
        } else {
            None
        }
    }
}

/// The bevy side: one clock tick per frame while the Debug row is on.
pub fn auto_enter_cs_system(
    time: Res<Time>,
    panels: Res<HudPanels>,
    scene_state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut clock: Local<Clock>,
) {
    if !panels.auto_enter_cs {
        return;
    }
    let Some(d) = clock.tick(scene_state.snapshot.dialog.as_ref(), time.delta_secs()) else {
        return;
    };
    if manual_advance_guard(scene_state.last_manual_dialog_advance, MANUAL_GUARD) {
        return;
    }
    let _ = cmd_tx.0.try_send(AgentCommand::EndEventChoice {
        event_id: d.npc_id,
        act_index: d.act_index,
        event_num: d.event_para,
        choice: 0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(text: &str) -> DialogState {
        DialogState {
            prompt: Some(text.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn read_time_scales_with_length_within_bounds() {
        assert_eq!(read_time("hi"), READ_TIME_MIN);
        assert!((read_time(&"x".repeat(100)) - 2.5).abs() < 1e-6);
        assert_eq!(read_time(&"x".repeat(1000)), READ_TIME_MAX);
    }

    #[test]
    fn only_plain_message_frames_are_eligible() {
        assert!(eligible(&frame("hello")));

        let mut d = frame("hello");
        d.choices.push("A".into());
        assert!(!eligible(&d), "choice frames are branches, not gates");

        let mut d = frame("hello");
        d.text_entry = true;
        assert!(!eligible(&d));

        let mut d = frame("hello");
        d.custom_menu = true;
        assert!(!eligible(&d));

        let mut d = frame("hello");
        d.contains_item = true;
        assert!(!eligible(&d));

        let mut d = frame("hello");
        d.npc_name = Some("Paintbrush of Souls".into());
        assert!(!eligible(&d));

        let mut d = frame("hello");
        d.npc_name = Some("Geomantic Reservoir".into());
        assert!(!eligible(&d));

        let mut d = frame("hello");
        d.prompt = None;
        assert!(
            !eligible(&d),
            "raw-packet fallback frames are not MESWAIT gates"
        );
    }

    #[test]
    fn manual_advance_guard_holds_a_fresh_mark_and_ignores_absence() {
        assert!(manual_advance_guard(Some(Instant::now()), MANUAL_GUARD));
        assert!(!manual_advance_guard(None, MANUAL_GUARD));
    }

    #[test]
    fn clock_fires_once_per_frame_after_read_time() {
        let f = frame(&"x".repeat(100));
        let mut c = Clock::default();
        for _ in 0..12 {
            assert!(c.tick(Some(&f), 0.2).is_none());
        }
        assert_eq!(c.tick(Some(&f), 0.2), Some(f.clone()));
        for _ in 0..2 {
            assert!(c.tick(Some(&f), 0.2).is_none());
        }
        let g = frame("next line");
        assert!(c.tick(Some(&g), 0.2).is_none());
        assert!(c.tick(None, 0.2).is_none());
        assert!(c.tick(Some(&f), 0.2).is_none());
    }

    #[test]
    fn clock_rearms_when_the_advance_never_lands() {
        let f = frame("hi");
        let mut c = Clock::default();
        for _ in 0..7 {
            assert!(c.tick(Some(&f), 0.2).is_none());
        }
        assert_eq!(c.tick(Some(&f), 0.2), Some(f.clone()));
        for _ in 0..2 {
            assert!(c.tick(Some(&f), 0.2).is_none());
        }
        for _ in 0..8 {
            assert!(c.tick(Some(&f), 0.2).is_none());
        }
        assert_eq!(c.tick(Some(&f), 0.2), Some(f.clone()));
    }
}
