//! Disconnect-recovery supervisor — wraps `reactor::run` (or any
//! reactor-shaped runner) in a respawn loop with exponential backoff,
//! persists the active goal, and resumes that goal after reconnect.
//!
//! Architecturally, the supervisor sits *outermost*:
//!
//! ```text
//!   clients → supervisor → reactor → session
//!                ↓
//!         goal_store (disk)
//! ```
//!
//! Responsibilities:
//!  - Forward client commands to the inner reactor; transparently snoop
//!    on goal-setting commands and persist them.
//!  - When the reactor returns (clean or err), decide: was this user-
//!    requested disconnect (Ok exit), an error worth retrying (backoff),
//!    or a fatal failure (give up after `max_attempts`)?
//!  - On retry, after the reactor restarts and reconnects, replay the
//!    persisted goal and emit `Reconnected { downtime_ms }`.
//!
//! Authentication-fatal errors (bad password, banned account) are
//! distinguished by counting *consecutive* failures: a transient
//! network blip retries fine; a permanent error fails N times in a row
//! and we exit with the last error. Pragmatic over typed.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, mpsc};

use crate::goal_store::{is_persistable_goal, GoalStore};
use crate::reactor::{self, ReactorConfig};
use crate::session;
use crate::state::{AgentCommand, AgentEvent};

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Where to read/write the persisted goal. `None` disables
    /// persistence (useful for tests and stateless agents).
    pub goal_store: Option<GoalStore>,
    /// Initial backoff after a disconnect. Doubles on each retry up
    /// to `max_backoff`.
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// Maximum *consecutive* reconnect attempts before giving up. A
    /// successful run (which we detect as "reactor::run lasted at
    /// least `min_run_for_reset`") resets the counter.
    pub max_attempts: u32,
    /// A reactor run that lasts at least this long is considered
    /// successful enough to reset `max_attempts`. 30s comfortably
    /// covers the auth+lobby+map handshake (~3s typical).
    pub min_run_for_reset: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            goal_store: None,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            max_attempts: 8,
            min_run_for_reset: Duration::from_secs(30),
        }
    }
}

/// Run the supervisor. Public surface mirrors `reactor::run` /
/// `session::run` — same `(cfg, cmd_rx, event_tx)` shape — so binaries
/// that already call one of those can swap in the supervisor without
/// other changes.
pub async fn run(
    cfg: session::Config,
    mut external_cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    sup_cfg: SupervisorConfig,
    reactor_cfg: ReactorConfig,
) -> Result<()> {
    // Replay any goal persisted from a previous run.
    let mut last_goal: Option<AgentCommand> = sup_cfg
        .goal_store
        .as_ref()
        .and_then(|s| s.load().ok().flatten().map(|p| p.command));

    let mut backoff = sup_cfg.initial_backoff;
    let mut consecutive_failures = 0u32;
    let mut user_requested_disconnect = false;

    loop {
        let attempt_id = consecutive_failures + 1;
        tracing::info!(
            attempt = attempt_id,
            replaying_goal = last_goal.is_some(),
            "supervisor.attempt.start"
        );

        let (inner_tx, inner_rx) = mpsc::channel::<AgentCommand>(64);

        // Replay persisted goal first thing on reconnect so the reactor
        // re-arms before any new command arrives.
        if let Some(g) = &last_goal {
            let _ = inner_tx.send(g.clone()).await;
        }

        let reactor_event_tx = event_tx.clone();
        let reactor_cfg_clone = reactor_cfg;
        let cfg_clone = cfg.clone();
        let mut reactor_handle = tokio::spawn(async move {
            reactor::run(cfg_clone, inner_rx, reactor_event_tx, reactor_cfg_clone).await
        });

        let attempt_started = Instant::now();
        let goal_store = sup_cfg.goal_store.clone();

        // Forwarding loop — runs while the reactor is alive.
        let attempt_result: AttemptOutcome = loop {
            tokio::select! {
                biased;
                res = &mut reactor_handle => {
                    break match res {
                        Ok(Ok(())) => AttemptOutcome::CleanExit,
                        Ok(Err(e)) => AttemptOutcome::ReactorError(e),
                        Err(join_e) => AttemptOutcome::ReactorPanic(join_e.to_string()),
                    };
                }
                cmd = external_cmd_rx.recv() => match cmd {
                    None => {
                        // Caller dropped — clean shutdown.
                        drop(inner_tx);
                        let res = (&mut reactor_handle).await;
                        break match res {
                            Ok(Ok(())) => AttemptOutcome::CallerDropped,
                            Ok(Err(e)) => AttemptOutcome::ReactorError(e),
                            Err(join_e) => AttemptOutcome::ReactorPanic(join_e.to_string()),
                        };
                    }
                    Some(cmd) => {
                        // Snoop goal commands; persist & remember.
                        if is_persistable_goal(&cmd) {
                            if let Some(s) = &goal_store {
                                if let Err(e) = s.save(&cmd) {
                                    let _ = event_tx.send(AgentEvent::Error {
                                        message: format!("goal_store.save: {e}"),
                                    });
                                }
                            }
                            last_goal = Some(cmd.clone());
                        } else if matches!(cmd, AgentCommand::Cancel) {
                            if let Some(s) = &goal_store {
                                let _ = s.clear();
                            }
                            last_goal = None;
                        } else if matches!(cmd, AgentCommand::Disconnect) {
                            user_requested_disconnect = true;
                        }
                        if inner_tx.send(cmd).await.is_err() {
                            // Reactor closed; let next iteration capture join.
                            let res = (&mut reactor_handle).await;
                            break match res {
                                Ok(Ok(())) => AttemptOutcome::CleanExit,
                                Ok(Err(e)) => AttemptOutcome::ReactorError(e),
                                Err(join_e) => AttemptOutcome::ReactorPanic(join_e.to_string()),
                            };
                        }
                    }
                }
            }
        };

        let attempt_duration = attempt_started.elapsed();
        tracing::info!(
            attempt = attempt_id,
            duration_ms = attempt_duration.as_millis() as u64,
            outcome = ?attempt_result,
            "supervisor.attempt.end"
        );

        let attempt_error: anyhow::Error = match attempt_result {
            AttemptOutcome::CleanExit | AttemptOutcome::CallerDropped => {
                // Deliberate shutdown — do not respawn.
                return Ok(());
            }
            AttemptOutcome::ReactorError(e) => {
                if user_requested_disconnect {
                    // User asked to quit and the reactor errored on the
                    // way out — surface the error but don't loop.
                    return Err(e);
                }
                e
            }
            AttemptOutcome::ReactorPanic(msg) => anyhow!("reactor task panic: {msg}"),
        };

        // A "long-enough" attempt resets the failure counter and backoff —
        // a single transient drop after hours of play shouldn't poison
        // future retries.
        if attempt_duration >= sup_cfg.min_run_for_reset {
            consecutive_failures = 0;
            backoff = sup_cfg.initial_backoff;
        } else {
            consecutive_failures += 1;
        }

        if consecutive_failures >= sup_cfg.max_attempts {
            return Err(attempt_error);
        }

        let _ = event_tx.send(AgentEvent::Error {
            message: format!(
                "session ended ({attempt_error}); attempt {consecutive_failures}/{}; \
                 retrying in {}ms",
                sup_cfg.max_attempts,
                backoff.as_millis()
            ),
        });

        let downtime_started = Instant::now();
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(sup_cfg.max_backoff);

        // Emit Reconnected *before* the next loop iteration spawns the
        // reactor — clients listening on the broadcast see "we're trying
        // again now" with the cumulative downtime so far.
        let downtime_ms = downtime_started.elapsed().as_millis() as u64;
        tracing::info!(
            attempt = attempt_id + 1,
            downtime_ms,
            "supervisor.reconnected"
        );
        let _ = event_tx.send(AgentEvent::Reconnected { downtime_ms });
    }
}

#[derive(Debug)]
enum AttemptOutcome {
    /// Reactor returned `Ok(())` — either user issued Disconnect or the
    /// session terminated cleanly without error.
    CleanExit,
    /// External command channel was dropped by the caller.
    CallerDropped,
    ReactorError(anyhow::Error),
    ReactorPanic(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_config_default_sane() {
        let cfg = SupervisorConfig::default();
        assert!(cfg.initial_backoff < cfg.max_backoff);
        assert!(cfg.min_run_for_reset > Duration::ZERO);
        assert!(cfg.max_attempts > 0);
    }

    #[test]
    fn backoff_grows_then_caps() {
        // Pure-math sanity: doubling 1s capped at 60s reaches the cap
        // around attempt 7 (1, 2, 4, 8, 16, 32, 60).
        let mut b = Duration::from_secs(1);
        let cap = Duration::from_secs(60);
        let sequence: Vec<u64> = (0..8)
            .map(|_| {
                let cur = b.as_secs();
                b = (b * 2).min(cap);
                cur
            })
            .collect();
        assert_eq!(sequence, vec![1, 2, 4, 8, 16, 32, 60, 60]);
    }
}
