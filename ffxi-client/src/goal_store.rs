use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::state::AgentCommand;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedGoal {
    pub command: AgentCommand,
    pub set_at_unix: u64,
}

#[derive(Debug, Clone)]
pub struct GoalStore {
    path: PathBuf,
}

impl GoalStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        crate::config_dir::config_file("goal.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<PersistedGoal>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let goal: PersistedGoal = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(goal))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    pub fn save(&self, cmd: &AgentCommand) -> Result<()> {
        if !is_persistable_goal(cmd) {
            return Err(anyhow!("not a persistable goal command: {cmd:?}"));
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let goal = PersistedGoal {
            command: cmd.clone(),
            set_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let bytes = serde_json::to_vec_pretty(&goal).context("serialize goal")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("remove {}", self.path.display())),
        }
    }
}

pub fn is_persistable_goal(cmd: &AgentCommand) -> bool {
    matches!(
        cmd,
        AgentCommand::Follow { .. } | AgentCommand::Engage { .. } | AgentCommand::PathTo { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!(
            "ffxi-goal-store-{}-{stamp}.json",
            std::process::id()
        ));
        p
    }

    #[test]
    fn default_path_uses_player_facing_dir() {
        let path = GoalStore::default_path().unwrap();
        assert!(path.ends_with("kuluu/goal.json"), "got {}", path.display());
    }

    #[test]
    fn load_missing_returns_none() {
        let store = GoalStore::new(tmp_path());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let store = GoalStore::new(tmp_path());
        let cmd = AgentCommand::Follow {
            target_id: 42,
            distance: 5.0,
        };
        store.save(&cmd).unwrap();

        let loaded = store.load().unwrap().expect("goal present after save");
        assert!(matches!(
            loaded.command,
            AgentCommand::Follow { target_id: 42, .. }
        ));
        assert!(loaded.set_at_unix > 0);

        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_rejects_non_goal_commands() {
        let store = GoalStore::new(tmp_path());
        let err = store.save(&AgentCommand::Snapshot).unwrap_err();
        assert!(err.to_string().contains("not a persistable goal"));
        let err = store.save(&AgentCommand::Cancel).unwrap_err();
        assert!(err.to_string().contains("not a persistable goal"));
    }

    #[test]
    fn clear_on_missing_is_idempotent() {
        let store = GoalStore::new(tmp_path());
        store.clear().unwrap();
        store.clear().unwrap();
    }

    #[test]
    fn save_overwrites_previous() {
        let path = tmp_path();
        let store = GoalStore::new(&path);
        store
            .save(&AgentCommand::Follow {
                target_id: 1,
                distance: 5.0,
            })
            .unwrap();
        store.save(&AgentCommand::Engage { target_id: 99 }).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(matches!(
            loaded.command,
            AgentCommand::Engage { target_id: 99 }
        ));
        store.clear().unwrap();
    }

    #[test]
    fn is_persistable_goal_classifies_correctly() {
        assert!(is_persistable_goal(&AgentCommand::Follow {
            target_id: 1,
            distance: 1.0
        }));
        assert!(is_persistable_goal(&AgentCommand::Engage { target_id: 1 }));
        assert!(is_persistable_goal(&AgentCommand::PathTo {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            force: false
        }));
        assert!(!is_persistable_goal(&AgentCommand::Cancel));
        assert!(!is_persistable_goal(&AgentCommand::Snapshot));
        assert!(!is_persistable_goal(&AgentCommand::Disconnect));
        assert!(!is_persistable_goal(&AgentCommand::Move {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            heading: 0
        }));
    }
}
