//! User-tunable graphics quality — resource, presets, cycle helpers,
//! hot-apply reactor systems.
//!
//! Loaded by the native client from
//! `$XDG_CONFIG_HOME/ffxi-mcp/graphics.json` (see
//! `ffxi-client::graphics_store`), or defaults to [`QualityPreset::High`]
//! on fresh install / wasm.
//!
//! # Ergonomics
//!
//! Every field is a fixed slot list — no sliders. Left/Right on the
//! in-game menu calls [`GraphicsSettings::cycle`], which advances the
//! highlighted field to the next/previous slot and flips
//! [`GraphicsSettings::preset`] to [`QualityPreset::Custom`] when any
//! non-preset field is touched. Cycling the `Preset` row itself
//! overwrites every field to that preset's table value.
//!
//! # Hot apply
//!
//! Each reactor system in this module reads the resource and writes
//! one piece of rendering state (shadow map size, MSAA/TAA on the
//! camera, etc.). All reactors are gated by `resource_changed::<GraphicsSettings>`
//! so they only fire on the frame the user touches a setting.

use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap, VolumetricFog,
};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::renderer::RenderAdapter;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};

use crate::camera::OperatorCamera;
use crate::sun_moon::IsSun;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// User-facing graphics tier. Default = `High` — a fresh install ships
/// visibly sharper shadows than the pre-settings hard-coded baseline.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QualityPreset {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
    /// The user has hand-tuned at least one field. Subsequent loads
    /// preserve the exact stored values rather than snapping back to
    /// any preset table.
    Custom,
}

impl QualityPreset {
    pub const fn label(self) -> &'static str {
        match self {
            QualityPreset::Low => "Low",
            QualityPreset::Medium => "Medium",
            QualityPreset::High => "High",
            QualityPreset::Ultra => "Ultra",
            QualityPreset::Custom => "Custom",
        }
    }
}

/// Anti-aliasing mode. Mutually exclusive (TAA forces MSAA off; see
/// `bevy_anti_alias/src/taa/mod.rs:164` for the runtime check).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AaMode {
    Off,
    Msaa2,
    #[default]
    Msaa4,
    Msaa8,
    /// Temporal anti-aliasing. Forces `Msaa::Off`. WASM clamps this to
    /// `Msaa4` in [`GraphicsSettings::cycle`] because the motion-vector
    /// prepass is heavy on WebGPU today — revisit when wgpu's
    /// motion-vector path matures.
    Taa,
}

impl AaMode {
    pub const fn label(self) -> &'static str {
        match self {
            AaMode::Off => "Off",
            AaMode::Msaa2 => "MSAA 2x",
            AaMode::Msaa4 => "MSAA 4x",
            AaMode::Msaa8 => "MSAA 8x",
            AaMode::Taa => "TAA",
        }
    }
}

/// Sky rendering style. Picks the overall *look* of the sky, sun, and
/// moon, independent of the quality tier. `Enhanced` is our modern,
/// embellished sky (horizon reddening, earthshine, moon illusion, sun
/// flare, procedural clouds); `Retail` reproduces the stylized 2002
/// client look (fixed antipodal moon, no embellishments). A reactor
/// (`apply_sky_style_system`) maps this onto the fine-grained
/// [`crate::sky_realism::SkyRealism`] knobs and gates the retail-only
/// render features.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkyStyle {
    #[default]
    Enhanced,
    Retail,
}

impl SkyStyle {
    pub const fn label(self) -> &'static str {
        match self {
            SkyStyle::Enhanced => "Enhanced",
            SkyStyle::Retail => "Retail",
        }
    }

    /// The [`crate::sky_realism::SkyRealism`] preset this style implies.
    pub const fn sky_realism(self) -> crate::sky_realism::SkyRealism {
        match self {
            SkyStyle::Enhanced => crate::sky_realism::SkyRealism::enhanced(),
            SkyStyle::Retail => crate::sky_realism::SkyRealism::retail(),
        }
    }
}

/// Dynamic environmental lights (lanterns/braziers) synthesized from
/// over-bright vertex clusters — an Enhanced-only embellishment (see
/// `crate::zone_lights`). This is a real frame-time lever, so it gets a
/// menu row: `Off` disables detection entirely, `Few`/`Many` cap the
/// live emitter count. Orthogonal to the quality tier (preset cycles
/// leave it untouched), like [`SkyStyle`].
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicLights {
    Off,
    Few,
    #[default]
    Many,
}

impl DynamicLights {
    pub const fn label(self) -> &'static str {
        match self {
            DynamicLights::Off => "Off",
            DynamicLights::Few => "Few",
            DynamicLights::Many => "Many",
        }
    }

    /// Hard cap on simultaneously live emitters this setting implies —
    /// fed into `ZoneLightConfig::max_total` by the reactor in
    /// `crate::zone_lights`. `Off` maps to 0 *and* flips `enabled` off.
    pub const fn max_total(self) -> u32 {
        match self {
            DynamicLights::Off => 0,
            DynamicLights::Few => 24,
            DynamicLights::Many => 48,
        }
    }

    /// Whether detection runs at all.
    pub const fn enabled(self) -> bool {
        !matches!(self, DynamicLights::Off)
    }
}

/// Selector used by the menu row → cycle dispatch. One variant per
/// row on the Graphics tab; the row layout in `hud::menu` follows this
/// order top-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsField {
    Preset,
    ShadowMapSize,
    ShadowCascadeCount,
    ShadowMaxDistance,
    AntiAliasing,
    BloomIntensity,
    VolumetricFog,
    FogStepCount,
    ViewDistance,
    VSync,
    Fov,
    SkyStyle,
    DynamicLights,
}

impl GraphicsField {
    /// Display label as it appears left of the bracketed value.
    pub const fn label(self) -> &'static str {
        match self {
            GraphicsField::Preset => "Preset",
            GraphicsField::ShadowMapSize => "Shadow Quality",
            GraphicsField::ShadowCascadeCount => "Shadow Cascades",
            GraphicsField::ShadowMaxDistance => "Shadow Distance",
            GraphicsField::AntiAliasing => "Anti-Aliasing",
            GraphicsField::BloomIntensity => "Bloom",
            GraphicsField::VolumetricFog => "Volumetric Fog",
            GraphicsField::FogStepCount => "Fog Quality",
            GraphicsField::ViewDistance => "View Distance",
            GraphicsField::VSync => "VSync",
            GraphicsField::Fov => "FOV",
            GraphicsField::SkyStyle => "Sky Style",
            GraphicsField::DynamicLights => "Dynamic Lights",
        }
    }
}

/// Persistent graphics state. Held as a Bevy `Resource`; mutated by the
/// menu cycle handlers and by `apply_preset`; consumed by the reactor
/// systems at the bottom of this file.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GraphicsSettings {
    pub preset: QualityPreset,
    pub shadow_map_size: u32,
    pub shadow_cascade_count: u32,
    pub shadow_max_distance: f32,
    pub anti_aliasing: AaMode,
    pub bloom_intensity: f32,
    pub volumetric_fog: bool,
    pub fog_step_count: u32,
    pub view_distance: f32,
    pub vsync: bool,
    pub fov_deg: f32,
    /// Sky/sun/moon rendering style. Orthogonal to the quality tier —
    /// preset cycles leave it untouched. Defaults via `#[serde(default)]`
    /// so a `graphics.json` written before this field existed still
    /// loads (lands on `Enhanced`, the prior hard-coded behavior).
    #[serde(default)]
    pub sky_style: SkyStyle,
    /// Synthesized environmental lights. Orthogonal to the quality tier
    /// (preset cycles preserve it). `#[serde(default)]` lands a
    /// pre-existing `graphics.json` on `Many`, the prior behavior.
    #[serde(default)]
    pub dynamic_lights: DynamicLights,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self::for_preset(QualityPreset::High)
    }
}

// Slot tables — single source of truth for cycle ergonomics. Each
// preset's value must appear in its field's slot list, otherwise
// cycling from a preset value behaves as "snap to nearest, then move".
const SHADOW_MAP_SIZE_SLOTS: &[u32] = &[1024, 2048, 4096, 8192];
const SHADOW_CASCADE_COUNT_SLOTS: &[u32] = &[2, 3, 4];
const SHADOW_MAX_DISTANCE_SLOTS: &[f32] = &[200.0, 400.0, 600.0, 800.0, 1000.0];
const BLOOM_SLOTS: &[f32] = &[0.0, 0.04, 0.08, 0.12, 0.16];
const FOG_STEP_SLOTS: &[u32] = &[32, 64, 96, 128];
// Includes the exact values used by each preset (2000, 4000, 6000) so
// cycling from a preset starts on a real slot. SKY_RADIUS=4000 (see
// `sun_moon.rs:53`) — at view_distance ≤ 4000 the sun/moon discs are
// outside the far plane and disappear; that's an intentional trade for
// the Low/Medium tiers.
const VIEW_DISTANCE_SLOTS: &[f32] = &[1500.0, 2000.0, 3000.0, 4000.0, 4500.0, 6000.0];
const FOV_SLOTS: &[f32] = &[
    50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0, 95.0, 100.0,
];

#[cfg(not(target_arch = "wasm32"))]
const AA_SLOTS: &[AaMode] = &[
    AaMode::Off,
    AaMode::Msaa2,
    AaMode::Msaa4,
    AaMode::Msaa8,
    AaMode::Taa,
];

// WASM: TAA's motion-vector prepass is heavy on WebGPU and not
// production-ready in 0.17. Drop the TAA slot so cycling can't land
// there.
#[cfg(target_arch = "wasm32")]
const AA_SLOTS: &[AaMode] = &[AaMode::Off, AaMode::Msaa2, AaMode::Msaa4, AaMode::Msaa8];

const PRESET_CYCLE: &[QualityPreset] = &[
    QualityPreset::Low,
    QualityPreset::Medium,
    QualityPreset::High,
    QualityPreset::Ultra,
];

const SKY_STYLE_CYCLE: &[SkyStyle] = &[SkyStyle::Enhanced, SkyStyle::Retail];

const DYNAMIC_LIGHTS_CYCLE: &[DynamicLights] =
    &[DynamicLights::Off, DynamicLights::Few, DynamicLights::Many];

impl GraphicsSettings {
    /// Concrete values for each preset tier. Touching this table changes
    /// what "fresh install" feels like.
    pub fn for_preset(preset: QualityPreset) -> Self {
        let aa_default = AaMode::Msaa4;
        match preset {
            QualityPreset::Low => Self {
                preset,
                shadow_map_size: 1024,
                shadow_cascade_count: 2,
                shadow_max_distance: 200.0,
                anti_aliasing: AaMode::Off,
                bloom_intensity: 0.0,
                volumetric_fog: false,
                fog_step_count: 32,
                view_distance: 2000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
            },
            QualityPreset::Medium => Self {
                preset,
                shadow_map_size: 2048,
                shadow_cascade_count: 3,
                shadow_max_distance: 400.0,
                anti_aliasing: aa_default,
                bloom_intensity: 0.08,
                volumetric_fog: true,
                fog_step_count: 64,
                view_distance: 4000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
            },
            QualityPreset::High => Self {
                preset,
                shadow_map_size: 4096,
                shadow_cascade_count: 4,
                shadow_max_distance: 600.0,
                anti_aliasing: aa_default,
                bloom_intensity: 0.08,
                volumetric_fog: true,
                fog_step_count: 64,
                view_distance: 6000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
            },
            QualityPreset::Ultra => Self {
                preset,
                shadow_map_size: 8192,
                shadow_cascade_count: 4,
                shadow_max_distance: 800.0,
                #[cfg(not(target_arch = "wasm32"))]
                anti_aliasing: AaMode::Taa,
                #[cfg(target_arch = "wasm32")]
                anti_aliasing: AaMode::Msaa4,
                bloom_intensity: 0.12,
                volumetric_fog: true,
                fog_step_count: 96,
                view_distance: 6000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
            },
            // Custom: identical to High at construction; the caller
            // mutates fields after. Used by the on-disk loader when
            // the file's `preset = Custom`.
            QualityPreset::Custom => Self {
                preset,
                ..Self::for_preset(QualityPreset::High)
            },
        }
    }

    /// Field renderer for the menu — returns the bracket text shown
    /// next to the label. Pure function of state.
    pub fn value_label(&self, field: GraphicsField) -> String {
        match field {
            GraphicsField::Preset => self.preset.label().to_string(),
            GraphicsField::ShadowMapSize => format!("{}px", self.shadow_map_size),
            GraphicsField::ShadowCascadeCount => format!("{}", self.shadow_cascade_count),
            GraphicsField::ShadowMaxDistance => format!("{:.0}m", self.shadow_max_distance),
            GraphicsField::AntiAliasing => self.anti_aliasing.label().to_string(),
            GraphicsField::BloomIntensity => {
                if self.bloom_intensity <= 1e-3 {
                    "Off".into()
                } else {
                    format!("{:.2}", self.bloom_intensity)
                }
            }
            GraphicsField::VolumetricFog => bool_label(self.volumetric_fog).into(),
            GraphicsField::FogStepCount => format!("{}", self.fog_step_count),
            GraphicsField::ViewDistance => format!("{:.0}m", self.view_distance),
            GraphicsField::VSync => bool_label(self.vsync).into(),
            GraphicsField::Fov => format!("{:.0}°", self.fov_deg),
            GraphicsField::SkyStyle => self.sky_style.label().to_string(),
            GraphicsField::DynamicLights => self.dynamic_lights.label().to_string(),
        }
    }

    /// Cycle the named field by `delta` (typically ±1) through its slot
    /// list. Touching any field other than `Preset` flips
    /// `self.preset = Custom` so persistence preserves the hand-tuned
    /// combination across restarts.
    pub fn cycle(&mut self, field: GraphicsField, delta: i32) {
        match field {
            GraphicsField::Preset => {
                // Sky style and dynamic lights are orthogonal to the
                // quality tier; preserve them across a preset overwrite so
                // picking Low doesn't silently revert the user's Retail sky
                // to Enhanced or re-enable lights they turned off.
                let sky = self.sky_style;
                let lights = self.dynamic_lights;
                let next =
                    cycle_slot(self.preset, PRESET_CYCLE, delta).unwrap_or(QualityPreset::High);
                *self = Self::for_preset(next);
                self.sky_style = sky;
                self.dynamic_lights = lights;
            }
            GraphicsField::ShadowMapSize => {
                self.shadow_map_size =
                    cycle_slot_u32(self.shadow_map_size, SHADOW_MAP_SIZE_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::ShadowCascadeCount => {
                self.shadow_cascade_count =
                    cycle_slot_u32(self.shadow_cascade_count, SHADOW_CASCADE_COUNT_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::ShadowMaxDistance => {
                self.shadow_max_distance =
                    cycle_slot_f32(self.shadow_max_distance, SHADOW_MAX_DISTANCE_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::AntiAliasing => {
                self.anti_aliasing =
                    cycle_slot(self.anti_aliasing, AA_SLOTS, delta).unwrap_or(AaMode::Msaa4);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::BloomIntensity => {
                self.bloom_intensity = cycle_slot_f32(self.bloom_intensity, BLOOM_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::VolumetricFog => {
                self.volumetric_fog = !self.volumetric_fog;
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::FogStepCount => {
                self.fog_step_count = cycle_slot_u32(self.fog_step_count, FOG_STEP_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::ViewDistance => {
                self.view_distance = cycle_slot_f32(self.view_distance, VIEW_DISTANCE_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::VSync => {
                self.vsync = !self.vsync;
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::Fov => {
                self.fov_deg = cycle_slot_f32(self.fov_deg, FOV_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::SkyStyle => {
                // Orthogonal to quality — does NOT flip the preset to
                // Custom. Just cycles the style enum.
                self.sky_style = cycle_slot(self.sky_style, SKY_STYLE_CYCLE, delta)
                    .unwrap_or(SkyStyle::Enhanced);
            }
            GraphicsField::DynamicLights => {
                // Orthogonal to quality, like SkyStyle — cycle Off/Few/Many
                // without flipping the preset to Custom.
                self.dynamic_lights = cycle_slot(self.dynamic_lights, DYNAMIC_LIGHTS_CYCLE, delta)
                    .unwrap_or(DynamicLights::Many);
            }
        }
    }

    /// Drop all overrides and snap back to High.
    pub fn reset_to_default(&mut self) {
        *self = Self::for_preset(QualityPreset::High);
    }

    /// Map `AaMode` → Bevy `Msaa` component value. TAA forces MSAA off
    /// regardless of the user's prior MSAA slot.
    pub fn msaa(&self) -> Msaa {
        match self.anti_aliasing {
            AaMode::Off | AaMode::Taa => Msaa::Off,
            AaMode::Msaa2 => Msaa::Sample2,
            AaMode::Msaa4 => Msaa::Sample4,
            AaMode::Msaa8 => Msaa::Sample8,
        }
    }

    /// True when TAA should be present on the camera as a component.
    pub fn wants_taa(&self) -> bool {
        matches!(self.anti_aliasing, AaMode::Taa)
    }
}

/// Adapter-reported MSAA capability for the HDR color attachment
/// (`Rgba16Float`, which Bevy's core_3d pipeline uses whenever the
/// camera has `Hdr`). The WebGPU spec only guarantees `{1, 4}`; many
/// Apple Silicon adapters cap at 4, integrated Intel parts often allow
/// 2 but not 8, etc. Asking for an unsupported count makes wgpu panic
/// inside the pipeline cache, so we clamp before writing `Msaa` to the
/// camera. Populated once at startup by [`init_msaa_caps_system`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct MsaaCaps {
    /// Bitmask of supported sample counts. Bit `n` set means the
    /// adapter advertises N-sample MSAA on `Rgba16Float`.
    pub mask: u32,
}

impl Default for MsaaCaps {
    /// WebGPU-spec floor: every conformant device supports 1 and 4.
    /// Used until [`init_msaa_caps_system`] queries the real adapter.
    fn default() -> Self {
        Self {
            mask: (1 << 1) | (1 << 4),
        }
    }
}

impl MsaaCaps {
    pub fn supports(self, samples: u32) -> bool {
        samples > 0 && samples < 32 && (self.mask & (1 << samples)) != 0
    }

    /// Round `want` down to the highest supported sample count `<= want`,
    /// falling back to `Msaa::Off` if nothing matches.
    pub fn clamp(self, want: Msaa) -> Msaa {
        for n in [want.samples(), 4, 2, 1] {
            if n <= want.samples() && self.supports(n) {
                return Msaa::from_samples(n);
            }
        }
        Msaa::Off
    }
}

/// Startup system: query the wgpu adapter for the MSAA counts it allows
/// on `Rgba16Float` and store the result. Runs once; no-op on subsequent
/// startups (only the first installs the resource).
pub fn init_msaa_caps_system(
    adapter: Option<Res<RenderAdapter>>,
    mut settings: ResMut<GraphicsSettings>,
    mut commands: Commands,
) {
    let caps = if let Some(adapter) = adapter {
        // Intersect Rgba16Float (HDR color attachment) with Depth32Float
        // (view depth target) — both must support the chosen sample
        // count or Bevy's `prepare_view_targets` / `prepare_core_3d_depth_textures`
        // will panic at create_texture time, before any Update reactor runs.
        let color = adapter
            .get_texture_format_features(TextureFormat::Rgba16Float)
            .flags;
        let depth = adapter
            .get_texture_format_features(TextureFormat::Depth32Float)
            .flags;
        let mut mask = 0u32;
        for n in [1u32, 2, 4, 8, 16] {
            if color.sample_count_supported(n) && depth.sample_count_supported(n) {
                mask |= 1 << n;
            }
        }
        mask |= (1 << 1) | (1 << 4); // spec floor
        MsaaCaps { mask }
    } else {
        MsaaCaps::default()
    };
    commands.insert_resource(caps);

    // Clamp the persisted AA mode *now*, before `spawn_camera` reads
    // it. Settings are not re-saved — bumping back to an unsupported
    // GPU later (or fixing the driver) restores the user's original
    // request from `graphics.json`.
    let want = settings.msaa();
    let got = caps.clamp(want);
    if got != want {
        settings.anti_aliasing = match got {
            Msaa::Off => AaMode::Off,
            Msaa::Sample2 => AaMode::Msaa2,
            Msaa::Sample4 => AaMode::Msaa4,
            Msaa::Sample8 => AaMode::Msaa8,
        };
        warn!(
            "MSAA {}x unsupported on this adapter (color+depth intersection); clamped to {}x",
            want.samples(),
            got.samples()
        );
    }
}

/// Menu row order — used by `hud::menu` to size the row pool. Each
/// entry is one row on the Graphics tab, top-down.
pub const GRAPHICS_FIELDS: &[GraphicsField] = &[
    GraphicsField::Preset,
    GraphicsField::ShadowMapSize,
    GraphicsField::ShadowCascadeCount,
    GraphicsField::ShadowMaxDistance,
    GraphicsField::AntiAliasing,
    GraphicsField::BloomIntensity,
    GraphicsField::VolumetricFog,
    GraphicsField::FogStepCount,
    GraphicsField::ViewDistance,
    GraphicsField::VSync,
    GraphicsField::Fov,
    GraphicsField::SkyStyle,
    GraphicsField::DynamicLights,
];

// ---------------------------------------------------------------------------
// Cycle slot helpers
// ---------------------------------------------------------------------------

fn cycle_slot<T: PartialEq + Copy>(current: T, slots: &[T], delta: i32) -> Option<T> {
    if slots.is_empty() {
        return None;
    }
    let n = slots.len() as i32;
    let i = slots.iter().position(|x| *x == current).unwrap_or(0) as i32;
    let next = (i + delta).rem_euclid(n);
    Some(slots[next as usize])
}

fn cycle_slot_u32(current: u32, slots: &[u32], delta: i32) -> u32 {
    cycle_slot(current, slots, delta).unwrap_or(current)
}

fn cycle_slot_f32(current: f32, slots: &[f32], delta: i32) -> f32 {
    if slots.is_empty() {
        return current;
    }
    let n = slots.len() as i32;
    let i = slots
        .iter()
        .position(|x| (x - current).abs() < 1e-3)
        .unwrap_or(0) as i32;
    let next = (i + delta).rem_euclid(n);
    slots[next as usize]
}

fn bool_label(b: bool) -> &'static str {
    if b {
        "On"
    } else {
        "Off"
    }
}

/// Build the cascade config from settings. Used at spawn (in
/// `sun_moon::spawn_sun_and_moon`) and by `apply_cascade_config_system`
/// on hot apply. First cascade stays tight at 12m for sharp character
/// self-shadowing; the rest stretch out to `shadow_max_distance`.
pub fn cascade_config_from_settings(s: &GraphicsSettings) -> CascadeShadowConfig {
    CascadeShadowConfigBuilder {
        num_cascades: s.shadow_cascade_count as usize,
        minimum_distance: 0.1,
        maximum_distance: s.shadow_max_distance,
        first_cascade_far_bound: 12.0,
        overlap_proportion: 0.2,
    }
    .build()
}

// ---------------------------------------------------------------------------
// Reactor systems — all gated by `resource_changed::<GraphicsSettings>`.
// ---------------------------------------------------------------------------

/// Resize the directional-light shadow map. Bevy validates pow2 and
/// hot-resizes the GPU texture on the next prepare-lights pass — no
/// entity respawn required.
pub fn apply_shadow_map_size_system(settings: Res<GraphicsSettings>, mut commands: Commands) {
    commands.insert_resource(DirectionalLightShadowMap {
        size: settings.shadow_map_size as usize,
    });
}

/// Rebuild the sun's cascade config. `CascadeShadowConfig` is read every
/// prepare-lights frame, so overwriting it triggers a one-frame
/// resnapping flicker — acceptable for user-initiated changes.
pub fn apply_cascade_config_system(
    settings: Res<GraphicsSettings>,
    mut q_sun: Query<&mut CascadeShadowConfig, With<IsSun>>,
) {
    for mut cfg in q_sun.iter_mut() {
        *cfg = cascade_config_from_settings(&settings);
    }
}

/// Despawn + respawn the OperatorCamera when the AA mode changes.
///
/// In-place `Msaa` writes were racy: Bevy resizes the view-target's
/// sample count on the next frame, but the pipeline cache keeps the
/// previously-specialized pipelines around for one render pass —
/// `main_opaque_pass_3d` then binds a 1-sample pipeline against a
/// 2-sample render pass and wgpu panics. Rebuilding the camera entity
/// forces the pipeline cache to compile fresh pipelines for the new
/// sample count before any render pass references the new target.
///
/// `Local<Option<Msaa>>` remembers the last applied value so we only
/// pay the respawn cost on actual AA changes, not on every
/// `GraphicsSettings` mutation (bloom, fog, FOV, etc. all share the
/// `resource_changed::<GraphicsSettings>` run-condition).
pub fn apply_anti_aliasing_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    q_cam: Query<(Entity, &Transform), With<OperatorCamera>>,
    caps: Option<Res<MsaaCaps>>,
    mut last_applied: Local<Option<(Msaa, bool)>>,
) {
    let target_msaa = caps
        .map(|c| c.clamp(settings.msaa()))
        .unwrap_or_else(|| settings.msaa());
    let want_taa = settings.wants_taa();
    let next = (target_msaa, want_taa);

    if *last_applied == Some(next) {
        return;
    }

    let Ok((entity, transform)) = q_cam.single() else {
        // Camera hasn't spawned yet (PreStartup ran but Startup
        // hasn't); record nothing and let the next change retry.
        return;
    };

    commands.entity(entity).despawn();
    let mut settings_for_respawn = settings.clone();
    let aa = if want_taa {
        AaMode::Taa
    } else {
        match target_msaa {
            Msaa::Off => AaMode::Off,
            Msaa::Sample2 => AaMode::Msaa2,
            Msaa::Sample4 => AaMode::Msaa4,
            Msaa::Sample8 => AaMode::Msaa8,
        }
    };
    settings_for_respawn.anti_aliasing = aa;
    crate::camera::build_operator_camera(&mut commands, &settings_for_respawn, Some(*transform));
    *last_applied = Some(next);
}

/// Mutate Bloom in place — never insert/remove (the camera always has
/// Bloom; we just dial the intensity, including to ~0 for "off").
pub fn apply_bloom_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    mut q_cam: Query<(Entity, Option<&mut Bloom>), With<OperatorCamera>>,
) {
    // Profiling (May 2026): when `Bloom` is present, Bevy runs the full
    // mip-chain downsample + upsample + composite render-pass set every
    // frame regardless of intensity — wgpu render-pass dispatch was
    // measured at ~30% of all hot-thread CPU time. Treat near-zero
    // intensity as "off" and actually remove the component so the
    // passes don't get scheduled. Matches the volumetric-fog pattern
    // just below.
    let want = settings.bloom_intensity;
    let on = want > 1e-3;
    for (entity, bloom) in q_cam.iter_mut() {
        match (on, bloom) {
            (true, Some(mut b)) => {
                if (b.intensity - want).abs() > 1e-4 {
                    b.intensity = want;
                }
            }
            (true, None) => {
                commands.entity(entity).insert(Bloom {
                    intensity: want,
                    ..Default::default()
                });
            }
            (false, Some(_)) => {
                commands.entity(entity).remove::<Bloom>();
            }
            (false, None) => {}
        }
    }
}

/// Insert/remove `VolumetricFog` and mutate `step_count`. The
/// `FogVolume` itself is a separate entity in `scene::setup_world` —
/// the camera-side `VolumetricFog` component is what gates the
/// raymarch pass.
pub fn apply_volumetric_fog_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    q_cam: Query<(Entity, Option<&VolumetricFog>), With<OperatorCamera>>,
) {
    for (entity, fog) in q_cam.iter() {
        match (settings.volumetric_fog, fog) {
            (true, Some(_)) | (true, None) => {
                commands.entity(entity).insert(VolumetricFog {
                    step_count: settings.fog_step_count,
                    ambient_intensity: 0.1,
                    ambient_color: Color::srgb(0.85, 0.88, 1.0),
                    jitter: 0.0,
                });
            }
            (false, Some(_)) => {
                commands.entity(entity).remove::<VolumetricFog>();
            }
            (false, None) => {}
        }
    }
}

/// Mutate the perspective projection's far plane + FOV.
pub fn apply_projection_system(
    settings: Res<GraphicsSettings>,
    mut q_cam: Query<&mut Projection, With<OperatorCamera>>,
) {
    for mut proj in q_cam.iter_mut() {
        if let Projection::Perspective(p) = proj.as_mut() {
            p.far = settings.view_distance;
            p.fov = settings.fov_deg.to_radians();
        }
    }
}

/// Mutate the primary window's `PresentMode`. Mirrors the pattern at
/// `ffxi-client/src/view_native/text_input.rs:743/760` — `Fifo` is
/// vsync-on, `AutoVsync` is vsync-off (auto-pick AutoNoVsync when the
/// driver supports it; falls back to Mailbox otherwise).
pub fn apply_vsync_system(
    settings: Res<GraphicsSettings>,
    mut q_window: Query<&mut Window, With<PrimaryWindow>>,
) {
    for mut window in q_window.iter_mut() {
        let target = if settings.vsync {
            PresentMode::Fifo
        } else {
            PresentMode::AutoNoVsync
        };
        if window.present_mode != target {
            window.present_mode = target;
        }
    }
}

/// Push the chosen [`SkyStyle`] onto the [`crate::sky_realism::SkyRealism`]
/// knobs whenever the style actually changes. Shares the
/// `resource_changed::<GraphicsSettings>` run-condition with the other
/// reactors, but the `Local` guard means an unrelated change (bloom,
/// FOV, …) won't re-derive — so a `/sky <feature>` runtime override,
/// which mutates `SkyRealism` directly and never touches
/// `GraphicsSettings`, survives until the user picks a different style.
pub fn apply_sky_style_system(
    settings: Res<GraphicsSettings>,
    mut sky: ResMut<crate::sky_realism::SkyRealism>,
    mut last: Local<Option<SkyStyle>>,
) {
    if *last == Some(settings.sky_style) {
        return;
    }
    *sky = settings.sky_style.sky_realism();
    *last = Some(settings.sky_style);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_high_preset() {
        let s = GraphicsSettings::default();
        assert_eq!(s.preset, QualityPreset::High);
        assert_eq!(s.shadow_map_size, 4096);
        assert_eq!(s.shadow_cascade_count, 4);
        assert!((s.shadow_max_distance - 600.0).abs() < 1e-6);
    }

    /// Every preset value must appear in its field's slot list so
    /// cycling from a preset doesn't first need to snap.
    #[test]
    fn preset_values_are_slot_aligned() {
        for &preset in PRESET_CYCLE {
            let s = GraphicsSettings::for_preset(preset);
            assert!(
                SHADOW_MAP_SIZE_SLOTS.contains(&s.shadow_map_size),
                "preset {:?} shadow_map_size {} not in slot list",
                preset,
                s.shadow_map_size
            );
            assert!(SHADOW_CASCADE_COUNT_SLOTS.contains(&s.shadow_cascade_count));
            assert!(SHADOW_MAX_DISTANCE_SLOTS
                .iter()
                .any(|x| (x - s.shadow_max_distance).abs() < 1e-3));
            assert!(BLOOM_SLOTS
                .iter()
                .any(|x| (x - s.bloom_intensity).abs() < 1e-3));
            assert!(FOG_STEP_SLOTS.contains(&s.fog_step_count));
            assert!(VIEW_DISTANCE_SLOTS
                .iter()
                .any(|x| (x - s.view_distance).abs() < 1e-3));
            assert!(FOV_SLOTS.iter().any(|x| (x - s.fov_deg).abs() < 1e-3));
            assert!(AA_SLOTS.contains(&s.anti_aliasing));
        }
    }

    /// JSON round-trip preserves every field, including the preset tag.
    #[test]
    fn json_roundtrip_preserves_all_fields() {
        let s = GraphicsSettings::for_preset(QualityPreset::Ultra);
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicsSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    /// Cycling any non-preset field once flips the preset tag to Custom.
    #[test]
    fn cycling_a_lever_marks_preset_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.preset, QualityPreset::High);
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.preset, QualityPreset::Custom);

        // Same for a binary field.
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::VolumetricFog, 1);
        assert_eq!(s.preset, QualityPreset::Custom);
        assert!(!s.volumetric_fog, "toggled off");
    }

    /// Cycling the Preset row overwrites every field to that preset's
    /// values — it does NOT flip to Custom (that's the whole point).
    #[test]
    fn cycling_preset_overwrites_all_fields() {
        let mut s = GraphicsSettings::for_preset(QualityPreset::High);
        s.shadow_map_size = 1024;
        s.preset = QualityPreset::Custom;

        // Snap to Low via Preset cycle: Custom isn't in PRESET_CYCLE,
        // so the position lookup falls back to index 0 (Low), then
        // delta=+1 → Medium.
        s.cycle(GraphicsField::Preset, 1);
        let medium = GraphicsSettings::for_preset(QualityPreset::Medium);
        assert_eq!(s, medium);
    }

    /// Sky style cycles Enhanced ↔ Retail without flipping the preset
    /// tag to Custom (it's orthogonal to the quality tier), and maps
    /// onto the matching SkyRealism preset.
    #[test]
    fn sky_style_cycles_without_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.sky_style, SkyStyle::Enhanced);
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style, SkyStyle::Retail);
        assert_eq!(s.preset, QualityPreset::High, "style must not flip preset");
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style, SkyStyle::Enhanced, "two variants wrap");
    }

    /// `SkyStyle::sky_realism()` returns the expected presets — Retail
    /// is the all-off stylized look, Enhanced keeps the embellishments.
    #[test]
    fn sky_style_maps_to_realism() {
        use crate::sky_realism::SkyRealism;
        assert_eq!(SkyStyle::Retail.sky_realism(), SkyRealism::retail());
        assert_eq!(SkyStyle::Enhanced.sky_realism(), SkyRealism::enhanced());
        assert!(!SkyRealism::retail().earthshine);
        assert!(SkyRealism::enhanced().earthshine);
    }

    /// Picking a quality preset must preserve the sky style — it's an
    /// independent axis, so cycling Preset can't silently revert it.
    #[test]
    fn preset_cycle_preserves_sky_style() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style, SkyStyle::Retail);
        s.cycle(GraphicsField::Preset, 1); // High → Ultra
        assert_eq!(s.sky_style, SkyStyle::Retail, "preset cycle kept the style");
    }

    /// Dynamic lights cycle Off → Few → Many without flipping the preset
    /// tag to Custom (orthogonal to the quality tier), and the slot maps
    /// onto the right max_total / enabled values.
    #[test]
    fn dynamic_lights_cycles_without_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.dynamic_lights, DynamicLights::Many);
        s.cycle(GraphicsField::DynamicLights, 1); // Many -> Off (wrap)
        assert_eq!(s.dynamic_lights, DynamicLights::Off);
        assert_eq!(s.preset, QualityPreset::High, "lights must not flip preset");
        assert!(!s.dynamic_lights.enabled());
        assert_eq!(s.dynamic_lights.max_total(), 0);
        s.cycle(GraphicsField::DynamicLights, 1); // Off -> Few
        assert_eq!(s.dynamic_lights, DynamicLights::Few);
        assert_eq!(s.dynamic_lights.max_total(), 24);
        assert!(s.dynamic_lights.enabled());
    }

    /// Picking a quality preset preserves the dynamic-lights slot — it's
    /// an independent axis, like the sky style.
    #[test]
    fn preset_cycle_preserves_dynamic_lights() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DynamicLights, 1); // Many -> Off
        assert_eq!(s.dynamic_lights, DynamicLights::Off);
        s.cycle(GraphicsField::Preset, 1); // High -> Ultra
        assert_eq!(s.dynamic_lights, DynamicLights::Off, "preset cycle kept it");
    }

    /// Cycle wraps with rem_euclid — last slot + 1 → first slot, and
    /// vice versa.
    #[test]
    fn cycle_wraps_in_both_directions() {
        let mut s = GraphicsSettings::default();
        // shadow_map_size starts at 4096 = slot index 2 of [1024, 2048, 4096, 8192].
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.shadow_map_size, 8192);
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.shadow_map_size, 1024, "wrapped past 8192");
        s.cycle(GraphicsField::ShadowMapSize, -1);
        assert_eq!(s.shadow_map_size, 8192, "wrapped back");
    }

    /// `msaa()` resolves Taa to MSAA::Off (the TAA render node requires
    /// it; see comment on `apply_anti_aliasing_system`).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn taa_implies_msaa_off() {
        let s = GraphicsSettings {
            anti_aliasing: AaMode::Taa,
            ..Default::default()
        };
        assert_eq!(s.msaa(), Msaa::Off);
        assert!(s.wants_taa());
    }

    /// Value labels are human-readable — used by the menu row renderer.
    #[test]
    fn value_label_smoke() {
        let s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::Preset), "High");
        assert_eq!(s.value_label(GraphicsField::ShadowMapSize), "4096px");
        assert_eq!(s.value_label(GraphicsField::ShadowCascadeCount), "4");
        assert_eq!(s.value_label(GraphicsField::ShadowMaxDistance), "600m");
        assert_eq!(s.value_label(GraphicsField::VolumetricFog), "On");
        assert_eq!(s.value_label(GraphicsField::Fov), "75°");
    }

    /// reset_to_default snaps any custom state back to the High preset.
    #[test]
    fn reset_returns_to_high() {
        let mut s = GraphicsSettings::for_preset(QualityPreset::Low);
        s.bloom_intensity = 0.16;
        s.preset = QualityPreset::Custom;
        s.reset_to_default();
        assert_eq!(s, GraphicsSettings::for_preset(QualityPreset::High));
    }
}
