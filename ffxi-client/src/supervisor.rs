use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, mpsc};

use crate::goal_store::{is_persistable_goal, GoalStore};
use crate::reactor::{self, ReactorConfig};
use crate::session;
use crate::state::{AgentCommand, AgentEvent};

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub goal_store: Option<GoalStore>,

    pub initial_backoff: Duration,
    pub max_backoff: Duration,

    pub max_attempts: u32,

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

pub async fn run(
    cfg: session::Config,
    mut external_cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    sup_cfg: SupervisorConfig,
    reactor_cfg: ReactorConfig,
) -> Result<()> {
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

                        drop(inner_tx);
                        let res = (&mut reactor_handle).await;
                        break match res {
                            Ok(Ok(())) => AttemptOutcome::CallerDropped,
                            Ok(Err(e)) => AttemptOutcome::ReactorError(e),
                            Err(join_e) => AttemptOutcome::ReactorPanic(join_e.to_string()),
                        };
                    }
                    Some(cmd) => {

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
                return Ok(());
            }
            AttemptOutcome::ReactorError(e) => {
                if user_requested_disconnect {
                    return Err(e);
                }
                e
            }
            AttemptOutcome::ReactorPanic(msg) => anyhow!("reactor task panic: {msg}"),
        };

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
    CleanExit,

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
