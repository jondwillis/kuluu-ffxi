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
    /// At least one individual sky-realism feature has been toggled away
    /// from a named style's preset (via the menu sub-rows or `/sky`).
    /// This variant is *derived*, never stored — see
    /// [`GraphicsSettings::sky_style`]; `sky_realism` is the source of truth.
    Custom,
}

impl SkyStyle {
    pub const fn label(self) -> &'static str {
        match self {
            SkyStyle::Enhanced => "Enhanced",
            SkyStyle::Retail => "Retail",
            SkyStyle::Custom => "Custom",
        }
    }

    /// The [`crate::sky_realism::SkyRealism`] preset this style implies.
    /// `Custom` has no canonical preset (it *is* the off-preset state); it
    /// falls back to `enhanced()` so the `const fn` match stays total — the
    /// cycle path only ever calls this for the two named styles.
    pub const fn sky_realism(self) -> crate::sky_realism::SkyRealism {
        match self {
            SkyStyle::Enhanced | SkyStyle::Custom => crate::sky_realism::SkyRealism::enhanced(),
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

/// Which renderer draws animated character models (PCs + NPCs).
/// Orthogonal to the quality tier (preset cycles preserve it), like
/// [`SkyStyle`] / [`DynamicLights`]. Persisted in `graphics.json`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CharacterRenderPath {
    /// Existing path: Bevy `SkinnedMesh` + `StandardMaterial`. NPCs
    /// animate on the GPU; PCs render as static CPU-baked bind pose.
    BevyStandard,
    /// FFXI-faithful path: the custom [`crate::skinned_ffxi_material`]
    /// with per-frame bone matrices from [`crate::skeleton_instance`].
    /// Unifies PCs and NPCs and reproduces FFXI's dual-position skinning,
    /// symmetry, and `2*vertexColor*texel` shading.
    #[default]
    FfxiFaithful,
}

impl CharacterRenderPath {
    pub const fn label(self) -> &'static str {
        match self {
            CharacterRenderPath::BevyStandard => "Bevy",
            CharacterRenderPath::FfxiFaithful => "FFXI",
        }
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
    /// Coarse sky preset (Enhanced / Retail / derived Custom). The six
    /// `Sky*` rows below are its fine-grained sub-toggles (the `/sky` knobs).
    SkyStyle,
    SkyHorizonReddening,
    SkyHorizonDimming,
    SkyMoonIllusion,
    SkyEarthshine,
    /// Placeholder — backing feature is Stage-2/unwired; rendered greyed
    /// and inert in the GUIs (still flippable via `/sky realmoon`).
    SkyRealMoon,
    /// Placeholder — see [`GraphicsField::SkyRealMoon`].
    SkyEclipses,
    /// Coarse dynamic-light tier (Off / Few / Many / derived Custom). The
    /// four `Light*` rows below are its fine-grained sub-knobs (`/lights`).
    DynamicLights,
    LightThreshold,
    LightIntensity,
    LightRange,
    LightFlicker,
}

impl GraphicsField {
    /// Display label as it appears left of the bracketed value. Sky/light
    /// sub-rows are indented two spaces so they read as a group under their
    /// coarse parent in both GUIs.
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
            GraphicsField::SkyHorizonReddening => "  Reddening",
            GraphicsField::SkyHorizonDimming => "  Dimming",
            GraphicsField::SkyMoonIllusion => "  Moon Illusion",
            GraphicsField::SkyEarthshine => "  Earthshine",
            GraphicsField::SkyRealMoon => "  Real Moon",
            GraphicsField::SkyEclipses => "  Eclipses",
            GraphicsField::DynamicLights => "Dynamic Lights",
            GraphicsField::LightThreshold => "  Threshold",
            GraphicsField::LightIntensity => "  Intensity",
            GraphicsField::LightRange => "  Range",
            GraphicsField::LightFlicker => "  Flicker",
        }
    }

    /// The [`crate::sky_realism::SkyFeature`] a `Sky*` sub-row drives, or
    /// `None` for non-sky-feature fields (`SkyStyle` and everything else).
    pub fn sky_feature(self) -> Option<crate::sky_realism::SkyFeature> {
        use crate::sky_realism::SkyFeature;
        Some(match self {
            GraphicsField::SkyHorizonReddening => SkyFeature::HorizonReddening,
            GraphicsField::SkyHorizonDimming => SkyFeature::HorizonDimming,
            GraphicsField::SkyMoonIllusion => SkyFeature::MoonIllusion,
            GraphicsField::SkyEarthshine => SkyFeature::Earthshine,
            GraphicsField::SkyRealMoon => SkyFeature::PhysicalMoonOrbit,
            GraphicsField::SkyEclipses => SkyFeature::Eclipses,
            _ => return None,
        })
    }

    /// Rows the GUIs show but grey out and refuse to cycle: the backing
    /// feature isn't wired into rendering yet (Stage 2). They remain
    /// reachable via the `/sky` command for power users.
    pub const fn is_placeholder(self) -> bool {
        matches!(
            self,
            GraphicsField::SkyRealMoon | GraphicsField::SkyEclipses
        )
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
    /// Fine-grained sky/sun/moon realism toggles — the source of truth for
    /// the sky look (the coarse `Sky Style` row derives Enhanced/Retail/
    /// Custom from this; see [`GraphicsSettings::sky_style`]). Mutated by the
    /// menu sub-rows *and* the `/sky` command, persisted here, and mirrored
    /// onto the runtime [`crate::sky_realism::SkyRealism`] resource by
    /// `apply_sky_realism_system`. Orthogonal to the quality tier — preset
    /// cycles preserve it. `#[serde(default)]` so a `graphics.json` written
    /// before this field existed loads on the Enhanced preset (prior
    /// behavior). NOTE: a pre-existing `sky_style` key in such a file is
    /// ignored (serde drops unknown fields), so an older `Retail` save
    /// lands back on Enhanced once.
    #[serde(default = "default_sky_realism")]
    pub sky_realism: crate::sky_realism::SkyRealism,
    /// Synthesized environmental lights — coarse tier (enabled + emitter
    /// cap). Orthogonal to the quality tier (preset cycles preserve it).
    /// `#[serde(default)]` lands a pre-existing `graphics.json` on `Many`,
    /// the prior behavior. The four `light_*` knobs below are its
    /// fine-grained sub-settings (the `/lights` knobs).
    #[serde(default)]
    pub dynamic_lights: DynamicLights,
    /// `/lights threshold` — over-bright detection cutoff fed into
    /// [`crate::zone_lights::ZoneLightConfig::overbright_threshold`].
    #[serde(default = "default_light_threshold")]
    pub light_threshold: f32,
    /// `/lights intensity` — PointLight lumens before flicker.
    #[serde(default = "default_light_intensity")]
    pub light_intensity: f32,
    /// `/lights range` — PointLight range in metres.
    #[serde(default = "default_light_range")]
    pub light_range: f32,
    /// `/lights flicker` — animate emitter intensity/scale.
    #[serde(default = "default_light_flicker")]
    pub light_flicker: bool,
    /// Which renderer draws animated characters. `#[serde(default)]` lands
    /// a pre-existing `graphics.json` on `BevyStandard` (prior behavior).
    /// See [`GraphicsSettings::character_path`] for the runtime accessor
    /// (which honors the `FFXI_CHARACTER_PATH` dev override).
    #[serde(default)]
    pub character_render_path: CharacterRenderPath,
}

/// Default fine dynamic-light knobs. Kept in lockstep with
/// [`crate::zone_lights::ZoneLightConfig`]'s defaults so the menu's "Many"
/// tier and a fresh `ZoneLightConfig` agree — `zone_lights` carries a test
/// pinning the two together (graphics_settings can't import ZoneLightConfig
/// without a dependency cycle).
pub const DEFAULT_LIGHT_THRESHOLD: f32 = 1.15;
pub const DEFAULT_LIGHT_INTENSITY: f32 = 25_000.0;
pub const DEFAULT_LIGHT_RANGE: f32 = 8.0;
pub const DEFAULT_LIGHT_FLICKER: bool = true;

fn default_sky_realism() -> crate::sky_realism::SkyRealism {
    crate::sky_realism::SkyRealism::enhanced()
}
fn default_light_threshold() -> f32 {
    DEFAULT_LIGHT_THRESHOLD
}
fn default_light_intensity() -> f32 {
    DEFAULT_LIGHT_INTENSITY
}
fn default_light_range() -> f32 {
    DEFAULT_LIGHT_RANGE
}
fn default_light_flicker() -> bool {
    DEFAULT_LIGHT_FLICKER
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

// Fine dynamic-light knob slots. Each default (1.15 / 25000 / 8.0) appears
// in its list so cycling from a fresh tier starts on a real slot.
const LIGHT_THRESHOLD_SLOTS: &[f32] = &[1.05, 1.15, 1.30, 1.50, 1.80];
const LIGHT_INTENSITY_SLOTS: &[f32] = &[5_000.0, 10_000.0, 25_000.0, 50_000.0, 100_000.0];
const LIGHT_RANGE_SLOTS: &[f32] = &[4.0, 6.0, 8.0, 12.0, 16.0];

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
                sky_realism: crate::sky_realism::SkyRealism::enhanced(),
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
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
                sky_realism: crate::sky_realism::SkyRealism::enhanced(),
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
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
                sky_realism: crate::sky_realism::SkyRealism::enhanced(),
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
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
                sky_realism: crate::sky_realism::SkyRealism::enhanced(),
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
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

    /// Active character render path, honoring the `FFXI_CHARACTER_PATH`
    /// dev override (`ffxi`/`faithful` or `bevy`/`standard`) so the two
    /// paths can be A/B'd at launch without editing `graphics.json`. The
    /// stored [`Self::character_render_path`] wins when the env var is unset.
    pub fn character_path(&self) -> CharacterRenderPath {
        match std::env::var("FFXI_CHARACTER_PATH").ok().as_deref() {
            Some("ffxi") | Some("faithful") => CharacterRenderPath::FfxiFaithful,
            Some("bevy") | Some("standard") => CharacterRenderPath::BevyStandard,
            _ => self.character_render_path,
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
            // Coarse sky row: derived Enhanced / Retail / Custom.
            GraphicsField::SkyStyle => self.sky_style().label().to_string(),
            GraphicsField::SkyHorizonReddening
            | GraphicsField::SkyHorizonDimming
            | GraphicsField::SkyMoonIllusion
            | GraphicsField::SkyEarthshine => {
                // `unwrap` is safe: each of these arms maps to a SkyFeature.
                bool_label(field.sky_feature().unwrap().get(&self.sky_realism)).into()
            }
            // Placeholders advertise the toggle but flag it as not-yet-live.
            GraphicsField::SkyRealMoon | GraphicsField::SkyEclipses => format!(
                "{} (soon)",
                bool_label(field.sky_feature().unwrap().get(&self.sky_realism))
            ),
            // Coarse lights row: named tier, or Custom when a fine knob moves.
            GraphicsField::DynamicLights => {
                if self.lights_fine_is_default() {
                    self.dynamic_lights.label().to_string()
                } else {
                    "Custom".to_string()
                }
            }
            GraphicsField::LightThreshold => format!("{:.2}", self.light_threshold),
            GraphicsField::LightIntensity => format!("{:.0}", self.light_intensity),
            GraphicsField::LightRange => format!("{:.0}m", self.light_range),
            GraphicsField::LightFlicker => bool_label(self.light_flicker).into(),
        }
    }

    /// Cycle the named field by `delta` (typically ±1) through its slot
    /// list. Touching any field other than `Preset` flips
    /// `self.preset = Custom` so persistence preserves the hand-tuned
    /// combination across restarts.
    pub fn cycle(&mut self, field: GraphicsField, delta: i32) {
        match field {
            GraphicsField::Preset => {
                // The sky and dynamic-light settings are orthogonal to the
                // quality tier; preserve them across a preset overwrite so
                // picking Low doesn't silently revert the user's Retail sky
                // to Enhanced or re-enable lights they turned off.
                let sky = self.sky_realism;
                let lights = self.dynamic_lights;
                let (lt, li, lr, lf) = (
                    self.light_threshold,
                    self.light_intensity,
                    self.light_range,
                    self.light_flicker,
                );
                let next =
                    cycle_slot(self.preset, PRESET_CYCLE, delta).unwrap_or(QualityPreset::High);
                *self = Self::for_preset(next);
                self.sky_realism = sky;
                self.dynamic_lights = lights;
                self.light_threshold = lt;
                self.light_intensity = li;
                self.light_range = lr;
                self.light_flicker = lf;
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
                // Cycle Enhanced <-> Retail and stamp the matching feature
                // preset onto `sky_realism`. From the derived `Custom` (not in
                // the cycle list) we fall back to the first slot. Orthogonal
                // to the quality tier — never flips `preset`.
                let next = cycle_slot(self.sky_style(), SKY_STYLE_CYCLE, delta)
                    .unwrap_or(SkyStyle::Enhanced);
                self.sky_realism = next.sky_realism();
            }
            GraphicsField::SkyHorizonReddening
            | GraphicsField::SkyHorizonDimming
            | GraphicsField::SkyMoonIllusion
            | GraphicsField::SkyEarthshine
            | GraphicsField::SkyRealMoon
            | GraphicsField::SkyEclipses => {
                // Placeholder rows (realmoon/eclipses) are inert in the GUI.
                if field.is_placeholder() {
                    return;
                }
                // Toggle the feature; the `Sky Style` row's derived label
                // follows automatically. Orthogonal to quality — no `preset`.
                if let Some(feat) = field.sky_feature() {
                    let cur = feat.get(&self.sky_realism);
                    feat.set(&mut self.sky_realism, !cur);
                }
            }
            GraphicsField::DynamicLights => {
                // Orthogonal to quality, like SkyStyle — cycle Off/Few/Many
                // without flipping the preset to Custom. Picking a named tier
                // resets the fine knobs to defaults (preset-overwrite
                // semantics) so the row leaves the derived "Custom" label.
                self.dynamic_lights = cycle_slot(self.dynamic_lights, DYNAMIC_LIGHTS_CYCLE, delta)
                    .unwrap_or(DynamicLights::Many);
                self.light_threshold = DEFAULT_LIGHT_THRESHOLD;
                self.light_intensity = DEFAULT_LIGHT_INTENSITY;
                self.light_range = DEFAULT_LIGHT_RANGE;
                self.light_flicker = DEFAULT_LIGHT_FLICKER;
            }
            GraphicsField::LightThreshold => {
                self.light_threshold =
                    cycle_slot_f32(self.light_threshold, LIGHT_THRESHOLD_SLOTS, delta);
            }
            GraphicsField::LightIntensity => {
                self.light_intensity =
                    cycle_slot_f32(self.light_intensity, LIGHT_INTENSITY_SLOTS, delta);
            }
            GraphicsField::LightRange => {
                self.light_range = cycle_slot_f32(self.light_range, LIGHT_RANGE_SLOTS, delta);
            }
            GraphicsField::LightFlicker => {
                self.light_flicker = !self.light_flicker;
            }
        }
    }

    /// Drop all overrides and snap back to High.
    pub fn reset_to_default(&mut self) {
        *self = Self::for_preset(QualityPreset::High);
    }

    /// The coarse sky style derived from the stored [`Self::sky_realism`]
    /// knobs: the named style when they match a preset exactly, else
    /// [`SkyStyle::Custom`]. This is the displayed `Sky Style` row value.
    pub fn sky_style(&self) -> SkyStyle {
        use crate::sky_realism::SkyRealism;
        if self.sky_realism == SkyRealism::retail() {
            SkyStyle::Retail
        } else if self.sky_realism == SkyRealism::enhanced() {
            SkyStyle::Enhanced
        } else {
            SkyStyle::Custom
        }
    }

    /// Whether the Enhanced-only embellishments — the procedural cloud dome
    /// ([`crate::skybox`]), the sun lens flare ([`crate::lens_flare`]), and
    /// the synthesized zone lights ([`crate::zone_lights`]) — should render.
    /// On for Enhanced *and* Custom; off only for full Retail (so flipping a
    /// single feature off doesn't strip the whole enhanced look).
    pub fn sky_embellishments_enabled(&self) -> bool {
        self.sky_realism != crate::sky_realism::SkyRealism::retail()
    }

    /// True when every fine dynamic-light knob is at its default. The
    /// `Dynamic Lights` row shows the named tier (Off/Few/Many) in that case
    /// and the derived "Custom" otherwise.
    fn lights_fine_is_default(&self) -> bool {
        (self.light_threshold - DEFAULT_LIGHT_THRESHOLD).abs() < 1e-3
            && (self.light_intensity - DEFAULT_LIGHT_INTENSITY).abs() < 1.0
            && (self.light_range - DEFAULT_LIGHT_RANGE).abs() < 1e-3
            && self.light_flicker == DEFAULT_LIGHT_FLICKER
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
    // Sky group: coarse style + its fine `/sky` toggles.
    GraphicsField::SkyStyle,
    GraphicsField::SkyHorizonReddening,
    GraphicsField::SkyHorizonDimming,
    GraphicsField::SkyMoonIllusion,
    GraphicsField::SkyEarthshine,
    GraphicsField::SkyRealMoon,
    GraphicsField::SkyEclipses,
    // Lights group: coarse tier + its fine `/lights` knobs.
    GraphicsField::DynamicLights,
    GraphicsField::LightThreshold,
    GraphicsField::LightIntensity,
    GraphicsField::LightRange,
    GraphicsField::LightFlicker,
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

/// Mirror the stored [`GraphicsSettings::sky_realism`] knobs onto the
/// runtime [`crate::sky_realism::SkyRealism`] resource that the sun / moon /
/// cloud renderers read. `GraphicsSettings` is the single source of truth —
/// the menu sub-rows, the coarse `Sky Style` row, and the `/sky` command all
/// mutate it — so this is a plain one-way copy. Shares the
/// `resource_changed::<GraphicsSettings>` run-condition with the other
/// reactors; the equality guard avoids an idle change-tick when an unrelated
/// field (bloom, FOV, …) moved.
pub fn apply_sky_realism_system(
    settings: Res<GraphicsSettings>,
    mut sky: ResMut<crate::sky_realism::SkyRealism>,
) {
    if *sky != settings.sky_realism {
        *sky = settings.sky_realism;
    }
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

    /// Sky style cycles Enhanced ↔ Retail without flipping the *quality*
    /// preset to Custom (it's orthogonal to the quality tier), and stamps
    /// the matching SkyRealism preset onto `sky_realism`.
    #[test]
    fn sky_style_cycles_without_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.sky_style(), SkyStyle::Enhanced);
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style(), SkyStyle::Retail);
        assert_eq!(s.sky_realism, crate::sky_realism::SkyRealism::retail());
        assert_eq!(s.preset, QualityPreset::High, "style must not flip preset");
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style(), SkyStyle::Enhanced, "two variants wrap");
    }

    /// Toggling an individual sky feature derives `Custom` for the coarse
    /// row but keeps the embellishment gate on (Custom ≠ full Retail) and
    /// never touches the quality preset.
    #[test]
    fn toggling_a_sky_feature_marks_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.sky_style(), SkyStyle::Enhanced);
        s.cycle(GraphicsField::SkyEarthshine, 1); // earthshine on -> off
        assert!(!s.sky_realism.earthshine);
        assert_eq!(s.sky_style(), SkyStyle::Custom);
        assert!(
            s.sky_embellishments_enabled(),
            "Custom keeps embellishments"
        );
        assert_eq!(s.value_label(GraphicsField::SkyStyle), "Custom");
        assert_eq!(s.preset, QualityPreset::High, "sky feature ⟂ quality tier");
    }

    /// Placeholder sky rows (realmoon/eclipses) are inert when cycled from
    /// the GUI and are flagged as placeholders.
    #[test]
    fn placeholder_sky_features_are_inert() {
        let mut s = GraphicsSettings::default();
        let before = s.sky_realism;
        s.cycle(GraphicsField::SkyRealMoon, 1);
        s.cycle(GraphicsField::SkyEclipses, 1);
        assert_eq!(s.sky_realism, before, "placeholder rows don't mutate state");
        assert!(GraphicsField::SkyRealMoon.is_placeholder());
        assert!(GraphicsField::SkyEclipses.is_placeholder());
        assert!(!GraphicsField::SkyEarthshine.is_placeholder());
    }

    /// Embellishments are gated off only for *full* Retail. Retail + one
    /// feature toggled back on is Custom, which re-enables the gate.
    #[test]
    fn embellishments_off_only_for_full_retail() {
        let mut s = GraphicsSettings::default();
        assert!(s.sky_embellishments_enabled());
        s.cycle(GraphicsField::SkyStyle, 1); // -> Retail
        assert!(!s.sky_embellishments_enabled());
        s.cycle(GraphicsField::SkyHorizonReddening, 1); // retail + reddening
        assert_eq!(s.sky_style(), SkyStyle::Custom);
        assert!(s.sky_embellishments_enabled());
    }

    /// Tuning a fine light knob derives `Custom` for the coarse row but
    /// leaves the cap tier and quality preset untouched; re-picking the
    /// tier resets the fine knobs.
    #[test]
    fn tuning_a_light_knob_marks_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Many");
        s.cycle(GraphicsField::LightIntensity, 1);
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Custom");
        assert_eq!(s.dynamic_lights, DynamicLights::Many, "cap tier unchanged");
        assert_eq!(s.preset, QualityPreset::High, "light knob ⟂ quality tier");
        s.cycle(GraphicsField::DynamicLights, 1); // Many -> Off, resets knobs
        assert!(s.lights_fine_is_default());
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Off");
    }

    /// Light-knob defaults must each sit on a real slot so cycling from a
    /// fresh tier starts cleanly.
    #[test]
    fn light_defaults_are_slot_aligned() {
        assert!(LIGHT_THRESHOLD_SLOTS
            .iter()
            .any(|x| (x - DEFAULT_LIGHT_THRESHOLD).abs() < 1e-3));
        assert!(LIGHT_INTENSITY_SLOTS
            .iter()
            .any(|x| (x - DEFAULT_LIGHT_INTENSITY).abs() < 1.0));
        assert!(LIGHT_RANGE_SLOTS
            .iter()
            .any(|x| (x - DEFAULT_LIGHT_RANGE).abs() < 1e-3));
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
        assert_eq!(s.sky_style(), SkyStyle::Retail);
        s.cycle(GraphicsField::Preset, 1); // High → Ultra
        assert_eq!(
            s.sky_style(),
            SkyStyle::Retail,
            "preset cycle kept the style"
        );
        assert_eq!(s.sky_realism, crate::sky_realism::SkyRealism::retail());
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

    /// Value labels render with the right shape — units/suffixes applied per
    /// field type. Each field is set explicitly so the test owns its inputs
    /// and stays decoupled from the preset default *values* (those change
    /// over time; the format strings shouldn't). Non-default values are
    /// chosen so a label accidentally hard-wired to a default still fails.
    #[test]
    fn value_label_smoke() {
        let mut s = GraphicsSettings::default();
        s.preset = QualityPreset::Ultra;
        s.shadow_map_size = 2048;
        s.shadow_cascade_count = 3;
        s.shadow_max_distance = 400.0;
        s.volumetric_fog = true;
        s.fov_deg = 90.0;
        assert_eq!(s.value_label(GraphicsField::Preset), "Ultra");
        assert_eq!(s.value_label(GraphicsField::ShadowMapSize), "2048px");
        assert_eq!(s.value_label(GraphicsField::ShadowCascadeCount), "3");
        assert_eq!(s.value_label(GraphicsField::ShadowMaxDistance), "400m");
        assert_eq!(s.value_label(GraphicsField::VolumetricFog), "On");
        assert_eq!(s.value_label(GraphicsField::Fov), "90°");
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
