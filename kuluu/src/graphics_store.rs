use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;
use kuluu_render::GraphicsSettings;

#[derive(Resource, Debug, Clone)]
pub struct GraphicsStateRes {
    pub store: GraphicsStore,
}

#[derive(Debug, Clone)]
pub struct GraphicsStore {
    path: PathBuf,
}

impl GraphicsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        kuluu_session::config_dir::config_file("graphics.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<GraphicsSettings>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let settings = parse_graphics_settings(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(settings))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    pub fn save(&self, settings: &GraphicsSettings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(settings).context("serialize graphics settings")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

/// Deserialize one JSON value into `T`, or `None` when it is missing or
/// malformed — used by [`parse_graphics_settings`] to salvage field-by-field.
fn take<T: serde::de::DeserializeOwned>(v: &serde_json::Value, key: &str) -> Option<T> {
    v.get(key)
        .and_then(|j| serde_json::from_value(j.clone()).ok())
}

/// Parse `graphics.json` leniently, field by field: one malformed value (a
/// hand-edited typo) defaults just that field instead of resetting the whole
/// store — which the persist system would then overwrite on the next change,
/// destroying every other valid setting. A syntactically broken document still
/// fails outright; there is nothing to salvage from it.
///
/// `ui_scale` is clamped to the menu's slot range 0.5..=2.0 (and NaN rejected)
/// on the way in: a persisted `"ui_scale": 100` would otherwise scale the HUD
/// into illegibility while nothing downstream bounds it.
///
/// `dlss_supported` is deliberately NOT read back — it is `#[serde(skip)]`
/// runtime capability, set by update_dlss_availability_system at startup.
/// The Retail+ gates (`dlss_menu_enabled`, `job_display`, `mob_hp_under`) DO
/// round-trip: they are user choices that must survive a restart (default off
/// when absent).
fn parse_graphics_settings(bytes: &[u8]) -> Result<GraphicsSettings> {
    use kuluu_render::{
        AaMode, CharacterRenderPath, DlssQuality, DynamicLights, QualityPreset, TextureFiltering,
        ZoneLineDisplay,
    };

    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut s = GraphicsSettings::default();

    if let Some(x) = take::<QualityPreset>(&v, "preset") {
        s.preset = x;
    }
    if let Some(x) = take(&v, "shadow_map_size") {
        s.shadow_map_size = x;
    }
    if let Some(x) = take(&v, "shadow_cascade_count") {
        s.shadow_cascade_count = x;
    }
    if let Some(x) = take(&v, "shadow_max_distance") {
        s.shadow_max_distance = x;
    }
    if let Some(x) = take::<AaMode>(&v, "anti_aliasing") {
        s.anti_aliasing = x;
    }
    if let Some(x) = take::<DlssQuality>(&v, "dlss_quality") {
        s.dlss_quality = x;
    }
    if let Some(x) = take(&v, "neural_uplift") {
        s.neural_uplift = x;
    }
    if let Some(x) = take(&v, "nr_intensity") {
        s.nr_intensity = x;
    }
    if let Some(x) = take(&v, "nr_local_tone_strength") {
        s.nr_local_tone_strength = x;
    }
    if let Some(x) = take(&v, "nr_structure_strength") {
        s.nr_structure_strength = x;
    }
    if let Some(x) = take::<TextureFiltering>(&v, "texture_filtering") {
        s.texture_filtering = x;
    }
    if let Some(x) = take(&v, "bloom_intensity") {
        s.bloom_intensity = x;
    }
    if let Some(x) = take(&v, "volumetric_fog") {
        s.volumetric_fog = x;
    }
    if let Some(x) = take(&v, "fog_step_count") {
        s.fog_step_count = x;
    }
    if let Some(x) = take(&v, "view_distance") {
        s.view_distance = x;
    }
    if let Some(x) = take(&v, "vsync") {
        s.vsync = x;
    }
    if let Some(x) = take(&v, "fps_cap") {
        s.fps_cap = x;
    }
    if let Some(x) = take(&v, "fov_deg") {
        s.fov_deg = x;
    }
    if let Some(x) = take::<f32>(&v, "ui_scale").filter(|x| x.is_finite()) {
        s.ui_scale = x.clamp(0.5, 2.0);
    }
    if let Some(x) = take(&v, "camera_spring") {
        s.camera_spring = x;
    }
    if let Some(x) = take(&v, "menu_scale") {
        s.menu_scale = x;
    }
    if let Some(x) = take::<DynamicLights>(&v, "dynamic_lights") {
        s.dynamic_lights = x;
    }
    if let Some(x) = take(&v, "light_threshold") {
        s.light_threshold = x;
    }
    if let Some(x) = take(&v, "light_intensity") {
        s.light_intensity = x;
    }
    if let Some(x) = take(&v, "light_range") {
        s.light_range = x;
    }
    if let Some(x) = take(&v, "light_flicker") {
        s.light_flicker = x;
    }
    // Pre-rename files carry the old key; honour both.
    if let Some(x) = take(&v, "model_light_count").or_else(|| take(&v, "dynamic_light_count")) {
        s.model_light_count = x;
    }
    if let Some(x) = take::<CharacterRenderPath>(&v, "character_render_path") {
        s.character_render_path = x;
    }
    if let Some(x) = take(&v, "realistic_character_lighting") {
        s.realistic_character_lighting = x;
    }
    if let Some(x) = take(&v, "faithful_shadow_receive") {
        s.faithful_shadow_receive = x;
    }
    if let Some(x) = take(&v, "character_shadow_cast") {
        s.character_shadow_cast = x;
    }
    if let Some(x) = take(&v, "depth_of_field") {
        s.depth_of_field = x;
    }
    if let Some(x) = take(&v, "dof_aperture_f_stops") {
        s.dof_aperture_f_stops = x;
    }
    if let Some(x) = take::<ZoneLineDisplay>(&v, "zone_line_display") {
        s.zone_line_display = x;
    }
    if let Some(x) = take(&v, "render_scale") {
        s.render_scale = x;
    }
    if let Some(x) = take(&v, "fullscreen") {
        s.fullscreen = x;
    }
    if let Some(x) = take(&v, "windowed_fullscreen") {
        s.windowed_fullscreen = x;
    }
    // Retail+ gates (dev-only Debug menu): persisted user choices.
    if let Some(x) = take(&v, "dlss_menu_enabled") {
        s.dlss_menu_enabled = x;
    }
    if let Some(x) = take(&v, "job_display") {
        s.job_display = x;
    }
    if let Some(x) = take(&v, "mob_hp_under") {
        s.mob_hp_under = x;
    }

    Ok(s)
}

pub fn load_or_default() -> (GraphicsSettings, GraphicsStore) {
    let path = match GraphicsStore::default_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "graphics: no config dir; using High preset");
            return (
                GraphicsSettings::default(),
                GraphicsStore::new(std::env::temp_dir().join("ffxi-graphics.json")),
            );
        }
    };
    let store = GraphicsStore::new(path);
    match store.load() {
        Ok(Some(settings)) => (settings, store),
        Ok(None) => (GraphicsSettings::default(), store),
        Err(e) => {
            tracing::warn!(
                path = %store.path().display(),
                error = %e,
                "graphics: parse failed; falling back to High preset",
            );
            (GraphicsSettings::default(), store)
        }
    }
}

pub fn persist_graphics_on_change(settings: Res<GraphicsSettings>, state: Res<GraphicsStateRes>) {
    if !settings.is_changed() {
        return;
    }
    if let Err(e) = state.store.save(&settings) {
        tracing::warn!(
            path = %state.store.path().display(),
            error = %e,
            "graphics: failed to persist settings",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::{GraphicsField, QualityPreset};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!(
            "ffxi-graphics-store-{}-{:?}-{stamp}.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        p
    }

    #[test]
    fn default_path_uses_player_facing_dir() {
        let path = GraphicsStore::default_path().unwrap();
        assert!(
            path.ends_with("kuluu/graphics.json"),
            "got {}",
            path.display()
        );
    }

    #[test]
    fn load_missing_returns_none() {
        let store = GraphicsStore::new(tmp_path());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let store = GraphicsStore::new(tmp_path());
        let mut settings = GraphicsSettings::for_preset(QualityPreset::Low);

        settings.cycle(GraphicsField::BloomIntensity, 1);
        assert_eq!(settings.preset, QualityPreset::Custom);

        store.save(&settings).unwrap();
        let loaded = store.load().unwrap().expect("present after save");
        assert_eq!(loaded, settings);
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn save_and_load_preset_only() {
        let store = GraphicsStore::new(tmp_path());
        let settings = GraphicsSettings::for_preset(QualityPreset::Ultra);
        store.save(&settings).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.preset, QualityPreset::Ultra);
        std::fs::remove_file(store.path()).ok();
    }

    /// One malformed field must default just that field — not reset the whole
    /// store (which persist would then overwrite, destroying valid settings) —
    /// and an out-of-range ui_scale must clamp into the menu slot range.
    #[test]
    fn load_salvages_one_bad_field_and_clamps_ui_scale() {
        let store = GraphicsStore::new(tmp_path());
        std::fs::write(
            store.path(),
            br#"{"preset": "Low", "ui_scale": 100, "vsync": "yes"}"#,
        )
        .unwrap();
        let loaded = store.load().unwrap().expect("present");
        assert_eq!(
            loaded.preset,
            QualityPreset::Low,
            "valid field survives a bad neighbour"
        );
        assert_eq!(
            loaded.vsync,
            GraphicsSettings::default().vsync,
            "malformed vsync falls back to default"
        );
        assert_eq!(
            loaded.ui_scale, 2.0,
            "ui_scale clamps into the menu slot range"
        );
    }

    #[test]
    fn load_rejects_nan_ui_scale() {
        let store = GraphicsStore::new(tmp_path());
        std::fs::write(store.path(), br#"{"ui_scale": NaN}"#).unwrap();
        let loaded = store.load().unwrap().expect("present");
        assert_eq!(loaded.ui_scale, 1.0, "NaN falls back to the default scale");
    }
}
