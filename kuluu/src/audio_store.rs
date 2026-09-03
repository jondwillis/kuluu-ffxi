use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;
use kuluu_render::audio::AudioMuteState;

/// Resource holding the on-disk audio settings store, so the persist system
/// can look up the target path when it needs to flush a change.
///
/// Mirrors [`crate::graphics_store::GraphicsStateRes`] deliberately — same
/// shape, same lifecycle. If you touch one, keep the other honest.
#[derive(Resource, Debug, Clone)]
pub struct AudioStateRes {
    pub store: AudioStore,
}

#[derive(Debug, Clone)]
pub struct AudioStore {
    path: PathBuf,
}

impl AudioStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Config file location — `audio.json` alongside `graphics.json` under the
    /// same player-facing config dir.
    pub fn default_path() -> Result<PathBuf> {
        kuluu_session::config_dir::config_file("audio.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<AudioMuteState>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let state: AudioMuteState = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    /// Atomic write via `write to .tmp -> rename`, matching the graphics store.
    /// A crash mid-save leaves the previous good file in place.
    pub fn save(&self, state: &AudioMuteState) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(state).context("serialize audio settings")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

/// Load persisted audio state or fall back to defaults. Same signature as
/// [`crate::graphics_store::load_or_default`].
pub fn load_or_default() -> (AudioMuteState, AudioStore) {
    let path = match AudioStore::default_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "audio: no config dir; using unmuted defaults");
            return (
                AudioMuteState::default(),
                AudioStore::new(std::env::temp_dir().join("ffxi-audio.json")),
            );
        }
    };
    let store = AudioStore::new(path);
    match store.load() {
        Ok(Some(state)) => (state, store),
        Ok(None) => (AudioMuteState::default(), store),
        Err(e) => {
            tracing::warn!(
                path = %store.path().display(),
                error = %e,
                "audio: parse failed; falling back to unmuted defaults",
            );
            (AudioMuteState::default(), store)
        }
    }
}

/// Persist system: flushes `AudioMuteState` to disk whenever it changes.
/// Registered on `Update` alongside `persist_graphics_on_change`. Bevy's
/// change detection fires exactly once per mutation regardless of how many
/// systems set the same value, so this is idempotent and cheap.
pub fn persist_audio_on_change(state: Res<AudioMuteState>, store: Res<AudioStateRes>) {
    if !state.is_changed() {
        return;
    }
    if let Err(e) = store.store.save(&state) {
        tracing::warn!(
            path = %store.store.path().display(),
            error = %e,
            "audio: failed to persist settings",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!(
            "ffxi-audio-store-{}-{:?}-{stamp}.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        p
    }

    #[test]
    fn default_path_uses_player_facing_dir() {
        let path = AudioStore::default_path().unwrap();
        assert!(path.ends_with("kuluu/audio.json"), "got {}", path.display());
    }

    #[test]
    fn roundtrips_mute_state() {
        let path = tmp_path();
        let store = AudioStore::new(&path);
        let s1 = AudioMuteState {
            bgm: true,
            sfx: false,
            ..Default::default()
        };
        store.save(&s1).unwrap();
        let s2 = store.load().unwrap().unwrap();
        assert_eq!(s1, s2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_is_none() {
        let store = AudioStore::new(tmp_path());
        assert!(store.load().unwrap().is_none());
    }
}
