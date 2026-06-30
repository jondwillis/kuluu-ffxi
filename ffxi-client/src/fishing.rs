//! Client-side fishing mini-game state machine.
//!
//! Drives the FFXI fishing protocol from the player's side: cast → wait for a bite →
//! fight the fish → report the outcome → release. The wire contract is the c2s 0x110
//! GP_CLI_COMMAND_FISHING_2 modes and the s2c 0x037 animation byte / 0x115 fish stats.
//!
//! The mini-game itself plays out entirely client-side once the server hands over the
//! fish parameters (0x115); the server only validates the start, the hook check, and the
//! reported result. The state shape (await-hook → centre → left/right arrows) mirrors the
//! faithful reference in research/xim (FishingAttemptInstance.kt / ActorSubStates.kt),
//! parameterized by the real 0x115 values rather than XIM's single-player guesses.
//!
//! References:
//! - research/XiPackets/world/client/0x0110 (request modes + para/para2 encoding)
//! - research/XiPackets/world/server/0x0115 (fish stat semantics)
//! - vendor/server/src/map/utils/fishingutils.cpp (StartFishing / FishingAction)

use crate::state::{FishParams, FishingArrow, FishingInput, FishingMode};

/// Seconds the resolution animation (caught / break / stop) plays before the client asks
/// the server to release the fishing lock. research/xim FishingAttemptInstance.kt:133.
const FINISH_SECS: f32 = 3.25;

/// Seconds a single arrow stays on screen before it counts as a miss. Scaled from the
/// 0x115 `arrow_delay` (which is 0..=N); kept within a human-reactable band.
const ARROW_BASE_SECS: f32 = 1.5;

/// Brief settle between arrows while the rod is centred.
const CENTER_SECS: f32 = 0.6;

/// Below this many seconds left, warn the server once (0x110 mode 5). The LSB validator
/// clamps the reported value to 0..=10.
const TIMEOUT_WARN_SECS: f32 = 10.0;

/// One thing the machine wants the surrounding runtime to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FishingOut {
    /// Send the start action (c2s 0x1A, ActionID Fish) targeting self.
    StartCast,
    /// Send a c2s 0x110 GP_CLI_COMMAND_FISHING_2 with these fields.
    Request {
        mode: FishingMode,
        para: i32,
        para2: i32,
    },
    /// Publish the current mini-game HUD/pose view.
    Progress {
        fish_hp: u16,
        arrow: Option<FishingArrow>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FightSub {
    /// Fish is on the line; the player must set the hook (Enter) to begin reeling.
    AwaitHook,
    /// Rod centred between arrow prompts.
    Center { remaining: f32 },
    /// An arrow is shown; the player must press the matching direction in time.
    Arrow { remaining: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Fight {
    fish: FishParams,
    fish_hp: f32,
    elapsed: f32,
    sub: FightSub,
    timeout_warned: bool,
    next_arrow_left: bool,
    arrow_golden: bool,
}

impl Fight {
    fn time_limit(&self) -> f32 {
        // 0x115 `time` is in seconds (the client scales by 60 to frames). A 0 limit would
        // strand the fight, so floor it.
        (self.fish.time as f32).max(1.0)
    }

    fn remaining_time(&self) -> f32 {
        (self.time_limit() - self.elapsed).max(0.0)
    }

    fn current_arrow(&self) -> Option<FishingArrow> {
        matches!(self.sub, FightSub::Arrow { .. }).then_some(FishingArrow {
            left: self.next_arrow_left,
            golden: self.arrow_golden,
        })
    }

    fn hp_u16(&self) -> u16 {
        self.fish_hp.round().clamp(0.0, f32::from(u16::MAX)) as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Idle,
    /// Sent the start action; awaiting the server's FISHING_START confirmation.
    Casting,
    /// Cast confirmed; counting the hook delay down before requesting a hook check.
    Waiting {
        remaining: f32,
    },
    /// Sent mode 2 (CheckHook); awaiting 0x115 (bite) or a STOP animation (nothing).
    CheckingHook,
    /// Reeling: the local mini-game is running.
    Fighting(Fight),
    /// Sent mode 3 (EndMiniGame); awaiting the server's resolution animation.
    Ending,
    /// Resolution animation playing; counts down, then sends mode 4 (Release).
    Finishing {
        remaining: f32,
    },
    /// Sent mode 4 (Release); awaiting the server clearing the fishing animation.
    Releasing,
}

/// The fishing protocol driver. Fed server events (`on_cast`, `on_hooked`, `on_phase`),
/// player input (`input`), and time (`tick`); emits [`FishingOut`] actions.
#[derive(Debug, Clone)]
pub struct FishingMachine {
    auto_play: bool,
    state: State,
}

impl FishingMachine {
    pub fn new(auto_play: bool) -> Self {
        Self {
            auto_play,
            state: State::Idle,
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// The self pose phase (0..=6) the renderer should display, or `None` when idle.
    pub fn phase(&self) -> Option<u8> {
        match self.state {
            State::Idle => None,
            State::Casting | State::Waiting { .. } | State::CheckingHook => Some(0),
            State::Fighting(_) => Some(1),
            // Resolution phases (2..=6) are dictated by the server's animation byte, which
            // is reflected straight to the renderer via FishingPhaseChanged, so the
            // machine does not need to second-guess which one here.
            State::Ending | State::Finishing { .. } | State::Releasing => Some(1),
        }
    }

    /// Player requested `/fish`. No-op if already fishing.
    pub fn start(&mut self) -> Vec<FishingOut> {
        if self.is_active() {
            return Vec::new();
        }
        self.state = State::Casting;
        vec![FishingOut::StartCast]
    }

    /// Server confirmed the cast (0x037 FISHING_START + hook delay, in seconds).
    pub fn on_cast(&mut self, hook_delay: u8) {
        if matches!(self.state, State::Casting | State::Idle) {
            self.state = State::Waiting {
                remaining: f32::from(hook_delay),
            };
        }
    }

    /// A fish bit (0x115). Begins the reeling mini-game.
    pub fn on_hooked(&mut self, params: FishParams) -> Vec<FishingOut> {
        if !matches!(self.state, State::CheckingHook) {
            return Vec::new();
        }
        let fight = Fight {
            fish: params,
            fish_hp: f32::from(params.stamina),
            elapsed: 0.0,
            sub: FightSub::AwaitHook,
            timeout_warned: false,
            next_arrow_left: true,
            // bit0 of angler_sense unlocks golden arrows / better timing; intuition refines
            // it. Treat a set bit plus non-trivial intuition as golden-capable.
            arrow_golden: (params.angler_sense & 1) == 1 && params.intuition > 0,
        };
        let progress = FishingOut::Progress {
            fish_hp: fight.hp_u16(),
            arrow: None,
        };
        self.state = State::Fighting(fight);
        vec![progress]
    }

    /// The server's fishing animation byte changed (phase 0..=6, or `None` once cleared).
    /// Drives the resolution → release handshake and the no-bite path.
    ///
    /// `None` only resets the machine once it is past the active phases. The 0x037 status
    /// packet is sent for many unrelated reasons (HP changes, status icons) carrying the
    /// current animation byte, so a `None` arriving while we are still Casting/Waiting/
    /// Fighting is an unrelated update, not the end of fishing — ignore it there.
    pub fn on_phase(&mut self, phase: Option<u8>) {
        match (phase, &self.state) {
            (
                None,
                State::CheckingHook | State::Ending | State::Finishing { .. } | State::Releasing,
            ) => self.state = State::Idle,
            // Nothing bit (server sent STOP straight after our hook check), or the
            // resolution animation for a completed fight: play it out, then release.
            (Some(p), State::CheckingHook | State::Ending) if (2..=6).contains(&p) => {
                self.state = State::Finishing {
                    remaining: FINISH_SECS,
                };
            }
            _ => {}
        }
    }

    /// Player/agent input during the mini-game.
    pub fn input(&mut self, input: FishingInput) -> Vec<FishingOut> {
        // Snapshot the bits we need so the borrow on `self.state` ends before we mutate it.
        let (sub, next_arrow_left) = match &self.state {
            State::Fighting(f) => (f.sub, f.next_arrow_left),
            _ => return Vec::new(),
        };

        match input {
            FishingInput::Cancel => {
                // Force-exit: para=200, para2=0. XiPackets 0x0110 mode 3.
                self.state = State::Ending;
                vec![FishingOut::Request {
                    mode: FishingMode::EndMiniGame,
                    para: 200,
                    para2: 0,
                }]
            }
            FishingInput::Hook => {
                if let (FightSub::AwaitHook, State::Fighting(f)) = (sub, &mut self.state) {
                    f.sub = FightSub::Center {
                        remaining: CENTER_SECS,
                    };
                }
                Vec::new()
            }
            FishingInput::Left | FishingInput::Right => {
                if matches!(sub, FightSub::Arrow { .. }) {
                    let pressed_left = matches!(input, FishingInput::Left);
                    self.resolve_arrow(pressed_left == next_arrow_left)
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Advance `dt` seconds.
    pub fn tick(&mut self, dt: f32) -> Vec<FishingOut> {
        match &mut self.state {
            State::Waiting { remaining } => {
                *remaining -= dt;
                if *remaining <= 0.0 {
                    self.state = State::CheckingHook;
                    // mode 2: para and para2 are both 0.
                    vec![FishingOut::Request {
                        mode: FishingMode::CheckHook,
                        para: 0,
                        para2: 0,
                    }]
                } else {
                    Vec::new()
                }
            }
            State::Fighting(_) => self.tick_fight(dt),
            State::Finishing { remaining } => {
                *remaining -= dt;
                if *remaining <= 0.0 {
                    self.state = State::Releasing;
                    // mode 4: para and para2 are both 0.
                    vec![FishingOut::Request {
                        mode: FishingMode::Release,
                        para: 0,
                        para2: 0,
                    }]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn tick_fight(&mut self, dt: f32) -> Vec<FishingOut> {
        let mut out = Vec::new();

        // Advance timers/regen under a scoped borrow, then decide what to do once the
        // borrow on `self.state` is released (arrow resolution needs `&mut self`).
        let decision = {
            let State::Fighting(fight) = &mut self.state else {
                return out;
            };

            fight.elapsed += dt;

            // Regen biased by 128 server-side; applied per second, clamped to the max.
            let regen_per_s = (f32::from(fight.fish.regen) - 128.0).max(0.0);
            fight.fish_hp = (fight.fish_hp + regen_per_s * dt).min(f32::from(fight.fish.stamina));

            // Warn the server once as time runs low (mode 5).
            if !fight.timeout_warned && fight.remaining_time() <= TIMEOUT_WARN_SECS {
                fight.timeout_warned = true;
                let para = fight.remaining_time().round().clamp(0.0, 10.0) as i32;
                out.push(FishingOut::Request {
                    mode: FishingMode::PotentialTimeout,
                    para,
                    para2: 0,
                });
            }

            if fight.remaining_time() <= 0.0 {
                FightDecision::Timeout
            } else {
                match fight.sub {
                    FightSub::AwaitHook => {
                        if self.auto_play {
                            fight.sub = FightSub::Center {
                                remaining: CENTER_SECS,
                            };
                        }
                        FightDecision::Idle
                    }
                    FightSub::Center { remaining } => {
                        let remaining = remaining - dt;
                        if remaining <= 0.0 {
                            fight.next_arrow_left = !fight.next_arrow_left;
                            fight.sub = FightSub::Arrow {
                                remaining: arrow_window(&fight.fish),
                            };
                        } else {
                            fight.sub = FightSub::Center { remaining };
                        }
                        FightDecision::Idle
                    }
                    FightSub::Arrow { remaining } => {
                        let remaining = remaining - dt;
                        if self.auto_play {
                            // A skilled angler reacts correctly to every arrow.
                            FightDecision::Resolve { hit: true }
                        } else if remaining <= 0.0 {
                            FightDecision::Resolve { hit: false }
                        } else {
                            fight.sub = FightSub::Arrow { remaining };
                            FightDecision::Idle
                        }
                    }
                }
            }
        };

        match decision {
            FightDecision::Idle => out.push(self.progress()),
            FightDecision::Resolve { hit } => out.extend(self.resolve_arrow(hit)),
            FightDecision::Timeout => {
                self.state = State::Ending;
                out.push(FishingOut::Request {
                    mode: FishingMode::EndMiniGame,
                    para: 300,
                    para2: 0,
                });
            }
        }
        out
    }

    /// Apply an arrow result. `hit` true = the player pressed the correct direction in
    /// time (deal arrow_damage); false = miss/timeout (fish recovers arrow_regen).
    fn resolve_arrow(&mut self, hit: bool) -> Vec<FishingOut> {
        let State::Fighting(fight) = &mut self.state else {
            return Vec::new();
        };
        if hit {
            // arrow_damage, doubled with good angler sense (XiPackets 0x0115).
            let mut dmg = f32::from(fight.fish.arrow_damage).max(1.0);
            if fight.arrow_golden {
                dmg *= 2.0;
            }
            fight.fish_hp -= dmg;
        } else {
            fight.fish_hp = (fight.fish_hp + f32::from(fight.fish.arrow_regen))
                .min(f32::from(fight.fish.stamina));
        }
        fight.sub = FightSub::Center {
            remaining: CENTER_SECS,
        };

        if fight.fish_hp <= 0.0 {
            // Caught: mode 3 para=0, para2=special (the reflected intuition value).
            let special = fight.fish.intuition as i32;
            self.state = State::Ending;
            return vec![FishingOut::Request {
                mode: FishingMode::EndMiniGame,
                para: 0,
                para2: special,
            }];
        }
        vec![self.progress()]
    }

    fn progress(&self) -> FishingOut {
        if let State::Fighting(f) = &self.state {
            FishingOut::Progress {
                fish_hp: f.hp_u16(),
                arrow: f.current_arrow(),
            }
        } else {
            FishingOut::Progress {
                fish_hp: 0,
                arrow: None,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FightDecision {
    Idle,
    Resolve { hit: bool },
    Timeout,
}

/// Arrow reaction window in seconds, derived from 0x115 `arrow_delay`.
fn arrow_window(fish: &FishParams) -> f32 {
    let delay = fish.arrow_delay.max(1) as f32;
    (ARROW_BASE_SECS + delay * 0.05).clamp(0.5, 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fish(stamina: u16, intuition: u32) -> FishParams {
        FishParams {
            stamina,
            arrow_delay: 5,
            // regen below the 128 bias → no passive recovery, keeps the sim deterministic.
            regen: 128,
            move_frequency: 3,
            arrow_damage: 5,
            arrow_regen: 2,
            time: 30,
            angler_sense: 0,
            intuition,
        }
    }

    fn requests(out: &[FishingOut]) -> Vec<(FishingMode, i32, i32)> {
        out.iter()
            .filter_map(|o| match o {
                FishingOut::Request { mode, para, para2 } => Some((*mode, *para, *para2)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn start_emits_cast_and_is_idempotent() {
        let mut m = FishingMachine::new(true);
        assert_eq!(m.start(), vec![FishingOut::StartCast]);
        assert!(m.is_active());
        assert_eq!(m.phase(), Some(0));
        // Already fishing: a second /fish does nothing.
        assert!(m.start().is_empty());
    }

    #[test]
    fn waits_hook_delay_then_requests_check_hook() {
        let mut m = FishingMachine::new(true);
        m.start();
        m.on_cast(3); // 3-second hook delay
        assert!(m.tick(1.0).is_empty());
        assert!(m.tick(1.0).is_empty());
        let out = m.tick(1.5);
        assert_eq!(
            requests(&out),
            vec![(FishingMode::CheckHook, 0, 0)],
            "check-hook fires once the delay elapses"
        );
    }

    #[test]
    fn no_bite_stop_animation_then_release() {
        let mut m = FishingMachine::new(true);
        m.start();
        m.on_cast(0);
        m.tick(0.1); // -> CheckingHook
                     // Server reports nothing bit by sending the STOP animation (phase 6).
        m.on_phase(Some(6));
        // The stop animation plays out, then we ask to be released.
        let mut released = false;
        for _ in 0..200 {
            if requests(&m.tick(0.1)).contains(&(FishingMode::Release, 0, 0)) {
                released = true;
                break;
            }
        }
        assert!(released, "stop animation must lead to a Release request");
        m.on_phase(None);
        assert!(!m.is_active());
    }

    #[test]
    fn auto_play_catches_fish_and_reports_intuition() {
        let mut m = FishingMachine::new(true);
        m.start();
        m.on_cast(0);
        m.tick(0.1); // -> CheckingHook
        m.on_hooked(fish(20, 0x64));
        assert_eq!(m.phase(), Some(1));

        // Drive the auto-played mini-game to completion.
        let mut caught = None;
        for _ in 0..2000 {
            for r in requests(&m.tick(0.1)) {
                if r.0 == FishingMode::EndMiniGame {
                    caught = Some(r);
                }
            }
            if caught.is_some() {
                break;
            }
        }
        assert_eq!(
            caught,
            Some((FishingMode::EndMiniGame, 0, 0x64)),
            "a caught fish reports para=0 and para2=intuition"
        );
    }

    #[test]
    fn caught_fish_resolution_then_release_then_idle() {
        let mut m = FishingMachine::new(true);
        m.start();
        m.on_cast(0);
        m.tick(0.1);
        m.on_hooked(fish(10, 7));
        for _ in 0..2000 {
            let out = m.tick(0.1);
            if requests(&out)
                .iter()
                .any(|r| r.0 == FishingMode::EndMiniGame)
            {
                break;
            }
        }
        // Server animates the catch (phase 2 = caught fish).
        m.on_phase(Some(2));
        let mut release = false;
        for _ in 0..200 {
            if requests(&m.tick(0.1)).contains(&(FishingMode::Release, 0, 0)) {
                release = true;
                break;
            }
        }
        assert!(release);
        m.on_phase(None);
        assert!(!m.is_active());
    }

    #[test]
    fn cancel_force_exits_with_para_200() {
        let mut m = FishingMachine::new(false);
        m.start();
        m.on_cast(0);
        m.tick(0.1);
        m.on_hooked(fish(50, 0));
        let out = m.input(FishingInput::Cancel);
        assert_eq!(requests(&out), vec![(FishingMode::EndMiniGame, 200, 0)]);
    }

    #[test]
    fn manual_correct_arrow_damages_fish() {
        let mut m = FishingMachine::new(false);
        m.start();
        m.on_cast(0);
        m.tick(0.1);
        m.on_hooked(fish(10, 0));
        m.input(FishingInput::Hook);
        // Advance past the centre settle so an arrow appears.
        let mut hp_before = None;
        for _ in 0..50 {
            for o in m.tick(0.1) {
                if let FishingOut::Progress {
                    fish_hp,
                    arrow: Some(_),
                } = o
                {
                    hp_before = Some(fish_hp);
                }
            }
            if hp_before.is_some() {
                break;
            }
        }
        let hp_before = hp_before.expect("an arrow should appear");
        // Press both directions; one of them matches and deals damage.
        m.input(FishingInput::Left);
        m.input(FishingInput::Right);
        // After resolving, hp should have dropped by the arrow damage.
        let out = m.tick(0.1);
        let hp_after = out.iter().find_map(|o| match o {
            FishingOut::Progress { fish_hp, .. } => Some(*fish_hp),
            _ => None,
        });
        if let Some(after) = hp_after {
            assert!(
                after <= hp_before,
                "a correct arrow must not increase fish hp"
            );
        }
    }

    #[test]
    fn timeout_fails_with_para_300() {
        let mut m = FishingMachine::new(false); // no auto-play: never presses, runs out the clock
        m.start();
        m.on_cast(0);
        m.tick(0.1);
        m.on_hooked(fish(9999, 0)); // unbeatable without input
        m.input(FishingInput::Hook);
        let mut failed = false;
        for _ in 0..1000 {
            if requests(&m.tick(0.5)).contains(&(FishingMode::EndMiniGame, 300, 0)) {
                failed = true;
                break;
            }
        }
        assert!(failed, "running out the timer reports para=300");
    }

    #[test]
    fn unrelated_none_status_does_not_cancel_pending_cast() {
        // A 0x037 carrying NONE arriving while we are still Casting/Waiting is an unrelated
        // status update, not the end of fishing.
        let mut m = FishingMachine::new(true);
        m.start();
        m.on_phase(None); // still Casting (awaiting FISHING_START)
        assert!(m.is_active());
        m.on_cast(5);
        m.on_phase(None); // Waiting
        assert!(m.is_active());
    }

    #[test]
    fn none_resets_once_releasing() {
        let mut m = FishingMachine::new(true);
        m.start();
        m.on_cast(0);
        m.tick(0.1); // CheckingHook
        m.on_phase(Some(6)); // Finishing
        for _ in 0..200 {
            if requests(&m.tick(0.1)).contains(&(FishingMode::Release, 0, 0)) {
                break;
            }
        }
        m.on_phase(None);
        assert!(!m.is_active());
        assert_eq!(m.phase(), None);
    }
}
