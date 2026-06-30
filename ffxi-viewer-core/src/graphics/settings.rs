use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap, VolumetricFog,
};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::renderer::RenderAdapter;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use bevy::core_pipeline::prepass::DepthPrepass;
#[cfg(not(target_arch = "wasm32"))]
use bevy::pbr::{Atmosphere, AtmosphereSettings, ScatteringMedium};
#[cfg(not(target_arch = "wasm32"))]
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};

use crate::camera::OperatorCamera;
use crate::sun_moon::IsSun;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QualityPreset {
    Low,
    Medium,
    #[default]
    High,
    Ultra,

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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AaMode {
    Off,
    Msaa2,
    #[default]
    Msaa4,
    Msaa8,

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

/// The two sky modes. `Enhanced` bundles every realism feature on plus Bevy's
/// procedural atmosphere (desktop) and the bloom sun glare; `Vanilla` is the
/// retail-faithful look — realism off, gradient skybox, painterly sun flare.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkyStyle {
    #[default]
    Enhanced,
    Vanilla,
}

impl SkyStyle {
    pub const fn label(self) -> &'static str {
        match self {
            SkyStyle::Enhanced => "Enhanced",
            SkyStyle::Vanilla => "Vanilla",
        }
    }

    pub const fn sky_realism(self) -> crate::sky_realism::SkyRealism {
        match self {
            SkyStyle::Enhanced => crate::sky_realism::SkyRealism::enhanced(),
            SkyStyle::Vanilla => crate::sky_realism::SkyRealism::retail(),
        }
    }

    /// Enhanced uses Bevy's procedural atmosphere (desktop only); Vanilla keeps
    /// the custom gradient skybox.
    pub const fn physical(self) -> bool {
        matches!(self, SkyStyle::Enhanced)
    }
}

/// How zone-line transition triggers are drawn. Retail shows nothing (you walk
/// into an invisible boundary), so `Off` is the faithful default. `Pillar` is a
/// debug glow column; `Gate` draws the real oriented trigger footprint
/// (`scale_x` × `scale_z`, yawed by `rotation`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoneLineDisplay {
    #[default]
    Off,
    Pillar,
    Gate,
}

impl ZoneLineDisplay {
    pub const fn label(self) -> &'static str {
        match self {
            ZoneLineDisplay::Off => "Off",
            ZoneLineDisplay::Pillar => "Pillar",
            ZoneLineDisplay::Gate => "Gate",
        }
    }
}

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

    pub const fn max_total(self) -> u32 {
        match self {
            DynamicLights::Off => 0,
            DynamicLights::Few => 24,
            DynamicLights::Many => 48,
        }
    }

    pub const fn enabled(self) -> bool {
        !matches!(self, DynamicLights::Off)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CharacterRenderPath {
    BevyStandard,

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

/// Texture magnification/minification filtering for zone & model textures.
/// `Vanilla` is the retail-faithful look (bilinear + mipmaps, no anisotropy);
/// the `Aniso*` levels add anisotropic filtering, an enhancement gated behind
/// the quality preset.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextureFiltering {
    Vanilla,
    Aniso2x,
    #[default]
    Aniso4x,
    Aniso8x,
    Aniso16x,
}

impl TextureFiltering {
    pub const fn label(self) -> &'static str {
        match self {
            TextureFiltering::Vanilla => "Vanilla",
            TextureFiltering::Aniso2x => "Aniso 2x",
            TextureFiltering::Aniso4x => "Aniso 4x",
            TextureFiltering::Aniso8x => "Aniso 8x",
            TextureFiltering::Aniso16x => "Aniso 16x",
        }
    }

    /// Sampler `anisotropy_clamp` (1 disables anisotropic filtering).
    pub const fn anisotropy(self) -> u16 {
        match self {
            TextureFiltering::Vanilla => 1,
            TextureFiltering::Aniso2x => 2,
            TextureFiltering::Aniso4x => 4,
            TextureFiltering::Aniso8x => 8,
            TextureFiltering::Aniso16x => 16,
        }
    }

    /// `Vanilla` is bilinear with no mip chain (pixel-faithful to XIM); the
    /// anisotropic levels add mips so anisotropy has levels to sample.
    pub const fn mipmaps(self) -> bool {
        !matches!(self, TextureFiltering::Vanilla)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsField {
    Preset,
    ShadowMapSize,
    ShadowCascadeCount,
    ShadowMaxDistance,
    AntiAliasing,
    TextureFiltering,
    BloomIntensity,
    VolumetricFog,
    FogStepCount,
    ViewDistance,
    VSync,
    Fov,

    SkyStyle,

    DynamicLights,
    LightThreshold,
    LightIntensity,
    LightRange,
    LightFlicker,

    CharacterLighting,

    CharacterShadowReceive,
    CharacterShadowCast,

    DepthOfField,
    DofAperture,

    ZoneLineDisplay,

    RenderScale,
}

impl GraphicsField {
    pub const fn label(self) -> &'static str {
        match self {
            GraphicsField::Preset => "Preset",
            GraphicsField::ShadowMapSize => "Shadow Quality",
            GraphicsField::ShadowCascadeCount => "Shadow Cascades",
            GraphicsField::ShadowMaxDistance => "Shadow Distance",
            GraphicsField::AntiAliasing => "Anti-Aliasing",
            GraphicsField::TextureFiltering => "Texture Filtering",
            GraphicsField::BloomIntensity => "Bloom",
            GraphicsField::VolumetricFog => "Volumetric Fog",
            GraphicsField::FogStepCount => "Fog Quality",
            GraphicsField::ViewDistance => "View Distance",
            GraphicsField::VSync => "VSync",
            GraphicsField::Fov => "FOV",
            GraphicsField::SkyStyle => "Sky Style",
            GraphicsField::DynamicLights => "Dynamic Lights",
            GraphicsField::LightThreshold => "  Threshold",
            GraphicsField::LightIntensity => "  Intensity",
            GraphicsField::LightRange => "  Range",
            GraphicsField::LightFlicker => "  Flicker",
            GraphicsField::CharacterLighting => "Model Lighting",
            GraphicsField::CharacterShadowReceive => "Model Shadows",
            GraphicsField::CharacterShadowCast => "Model Shadow Casting",
            GraphicsField::DepthOfField => "Depth of Field",
            GraphicsField::DofAperture => "DoF Aperture",
            GraphicsField::ZoneLineDisplay => "Zone Lines",
            GraphicsField::RenderScale => "Render Scale",
        }
    }

    /// Fine-tuning knobs hidden behind the "Advanced" disclosure: the
    /// dynamic-light tuning knobs (children of Dynamic Lights). These are the
    /// indented "  …" rows — rarely touched, so collapsed by default.
    pub const fn is_advanced(self) -> bool {
        matches!(
            self,
            GraphicsField::LightThreshold
                | GraphicsField::LightIntensity
                | GraphicsField::LightRange
                | GraphicsField::LightFlicker
        )
    }
}

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GraphicsSettings {
    pub preset: QualityPreset,
    pub shadow_map_size: u32,
    pub shadow_cascade_count: u32,
    pub shadow_max_distance: f32,
    pub anti_aliasing: AaMode,

    #[serde(default)]
    pub texture_filtering: TextureFiltering,

    pub bloom_intensity: f32,
    pub volumetric_fog: bool,
    pub fog_step_count: u32,
    pub view_distance: f32,
    pub vsync: bool,
    pub fov_deg: f32,

    #[serde(default)]
    pub sky_style: SkyStyle,

    #[serde(default)]
    pub dynamic_lights: DynamicLights,

    #[serde(default = "default_light_threshold")]
    pub light_threshold: f32,

    #[serde(default = "default_light_intensity")]
    pub light_intensity: f32,

    #[serde(default = "default_light_range")]
    pub light_range: f32,

    #[serde(default = "default_light_flicker")]
    pub light_flicker: bool,

    #[serde(default)]
    pub character_render_path: CharacterRenderPath,

    #[serde(default)]
    pub realistic_character_lighting: bool,

    #[serde(default = "default_faithful_shadow_receive")]
    pub faithful_shadow_receive: bool,

    #[serde(default = "default_character_shadow_cast")]
    pub character_shadow_cast: bool,

    #[serde(default)]
    pub depth_of_field: bool,

    #[serde(default = "default_dof_aperture")]
    pub dof_aperture_f_stops: f32,

    #[serde(default)]
    pub zone_line_display: ZoneLineDisplay,

    #[serde(default = "default_render_scale")]
    pub render_scale: f32,
}

pub const DEFAULT_LIGHT_THRESHOLD: f32 = 1.15;
pub const DEFAULT_LIGHT_INTENSITY: f32 = 25_000.0;
pub const DEFAULT_LIGHT_RANGE: f32 = 8.0;
pub const DEFAULT_LIGHT_FLICKER: bool = true;

// Lower f-stop = wider aperture = stronger background blur. f/2.8 is a tasteful
// cinematic default once the user opts into DoF.
pub const DEFAULT_DOF_APERTURE: f32 = 2.8;

// 1.0 = render the 3D scene at the window's native resolution (the byte-identical
// default; no off-screen target, no composite camera). Below 1.0 downscales the
// 3D buffer for performance and upscales to the window; above 1.0 supersamples.
pub const DEFAULT_RENDER_SCALE: f32 = 1.0;

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
fn default_faithful_shadow_receive() -> bool {
    true
}
fn default_character_shadow_cast() -> bool {
    true
}
fn default_dof_aperture() -> f32 {
    DEFAULT_DOF_APERTURE
}
fn default_render_scale() -> f32 {
    DEFAULT_RENDER_SCALE
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self::for_preset(QualityPreset::High)
    }
}

const SHADOW_MAP_SIZE_SLOTS: &[u32] = &[1024, 2048, 4096, 8192];
const SHADOW_CASCADE_COUNT_SLOTS: &[u32] = &[2, 3, 4];
const SHADOW_MAX_DISTANCE_SLOTS: &[f32] = &[200.0, 400.0, 600.0, 800.0, 1000.0];
const BLOOM_SLOTS: &[f32] = &[0.0, 0.04, 0.08, 0.12, 0.16];
const FOG_STEP_SLOTS: &[u32] = &[32, 64, 96, 128];

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

#[cfg(target_arch = "wasm32")]
const AA_SLOTS: &[AaMode] = &[AaMode::Off, AaMode::Msaa2, AaMode::Msaa4, AaMode::Msaa8];

const PRESET_CYCLE: &[QualityPreset] = &[
    QualityPreset::Low,
    QualityPreset::Medium,
    QualityPreset::High,
    QualityPreset::Ultra,
];

const TEXTURE_FILTERING_CYCLE: &[TextureFiltering] = &[
    TextureFiltering::Vanilla,
    TextureFiltering::Aniso2x,
    TextureFiltering::Aniso4x,
    TextureFiltering::Aniso8x,
    TextureFiltering::Aniso16x,
];

const LIGHT_THRESHOLD_SLOTS: &[f32] = &[1.05, 1.15, 1.30, 1.50, 1.80];
const LIGHT_INTENSITY_SLOTS: &[f32] = &[5_000.0, 10_000.0, 25_000.0, 50_000.0, 100_000.0];
const LIGHT_RANGE_SLOTS: &[f32] = &[4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0];

const DYNAMIC_LIGHTS_CYCLE: &[DynamicLights] =
    &[DynamicLights::Off, DynamicLights::Few, DynamicLights::Many];

const ZONE_LINE_DISPLAY_CYCLE: &[ZoneLineDisplay] = &[
    ZoneLineDisplay::Off,
    ZoneLineDisplay::Pillar,
    ZoneLineDisplay::Gate,
];

const DOF_APERTURE_SLOTS: &[f32] = &[1.4, 2.0, 2.8, 4.0, 5.6, 8.0];

// Sub-1.0 downscales the 3D buffer (perf); 1.0 is native; >1.0 supersamples (SSAA).
// 1.0 must stay in this list — it's the default and the no-op path.
const RENDER_SCALE_SLOTS: &[f32] = &[0.5, 0.67, 0.75, 0.85, 1.0, 1.25, 1.5, 2.0];

impl GraphicsSettings {
    pub fn for_preset(preset: QualityPreset) -> Self {
        let aa_default = AaMode::Msaa4;
        match preset {
            QualityPreset::Low => Self {
                preset,
                shadow_map_size: 1024,
                shadow_cascade_count: 2,
                shadow_max_distance: 200.0,
                anti_aliasing: AaMode::Off,
                texture_filtering: TextureFiltering::Vanilla,
                bloom_intensity: 0.0,
                volumetric_fog: false,
                fog_step_count: 32,
                view_distance: 2000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: false,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
            },
            QualityPreset::Medium => Self {
                preset,
                shadow_map_size: 2048,
                shadow_cascade_count: 3,
                shadow_max_distance: 400.0,
                anti_aliasing: aa_default,
                texture_filtering: TextureFiltering::Vanilla,
                bloom_intensity: 0.08,
                volumetric_fog: true,
                fog_step_count: 64,
                view_distance: 4000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: false,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
            },
            QualityPreset::High => Self {
                preset,
                shadow_map_size: 4096,
                shadow_cascade_count: 4,
                shadow_max_distance: 600.0,
                anti_aliasing: aa_default,
                texture_filtering: TextureFiltering::Aniso4x,
                bloom_intensity: 0.08,
                volumetric_fog: true,
                fog_step_count: 64,
                view_distance: 6000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: true,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
            },
            QualityPreset::Ultra => Self {
                preset,
                shadow_map_size: 8192,
                shadow_cascade_count: 4,
                shadow_max_distance: 800.0,
                // MSAA8 rather than TAA: TAA needs a motion-vector camera prepass,
                // which forces every zone/character draw through a second geometry
                // pass. No preset should silently pay that — the prepass is opt-in
                // via Depth of Field only. TAA stays available as a manual choice.
                anti_aliasing: AaMode::Msaa8,
                texture_filtering: TextureFiltering::Aniso8x,
                bloom_intensity: 0.12,
                volumetric_fog: true,
                fog_step_count: 96,
                view_distance: 6000.0,
                vsync: true,
                fov_deg: 50.0,
                sky_style: SkyStyle::Enhanced,
                dynamic_lights: DynamicLights::Many,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: true,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
            },

            QualityPreset::Custom => Self {
                preset,
                ..Self::for_preset(QualityPreset::High)
            },
        }
    }

    pub fn character_path(&self) -> CharacterRenderPath {
        match std::env::var("FFXI_CHARACTER_PATH").ok().as_deref() {
            Some("ffxi") | Some("faithful") => CharacterRenderPath::FfxiFaithful,
            Some("bevy") | Some("standard") => CharacterRenderPath::BevyStandard,
            _ => self.character_render_path,
        }
    }

    pub fn value_label(&self, field: GraphicsField) -> String {
        match field {
            GraphicsField::Preset => self.preset.label().to_string(),
            GraphicsField::ShadowMapSize => format!("{}px", self.shadow_map_size),
            GraphicsField::ShadowCascadeCount => format!("{}", self.shadow_cascade_count),
            GraphicsField::ShadowMaxDistance => format!("{:.0}m", self.shadow_max_distance),
            GraphicsField::AntiAliasing => self.anti_aliasing.label().to_string(),
            GraphicsField::TextureFiltering => self.texture_filtering.label().to_string(),
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

            GraphicsField::SkyStyle => self.sky_style().label().to_string(),

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

            GraphicsField::CharacterLighting => if self.realistic_character_lighting {
                "Realistic"
            } else {
                "FFXI"
            }
            .into(),
            GraphicsField::CharacterShadowReceive => {
                bool_label(self.faithful_shadow_receive).into()
            }
            GraphicsField::CharacterShadowCast => bool_label(self.character_shadow_cast).into(),
            GraphicsField::DepthOfField => bool_label(self.depth_of_field).into(),
            GraphicsField::DofAperture => format!("f/{:.1}", self.dof_aperture_f_stops),
            GraphicsField::ZoneLineDisplay => self.zone_line_display.label().to_string(),
            GraphicsField::RenderScale => format!("{:.0}%", self.render_scale * 100.0),
        }
    }

    pub fn cycle(&mut self, field: GraphicsField, delta: i32) {
        match field {
            GraphicsField::Preset => {
                // Sky style is orthogonal to the quality tier, so carry it
                // across a preset change.
                let sky = self.sky_style;
                let lights = self.dynamic_lights;
                let (lt, li, lr, lf) = (
                    self.light_threshold,
                    self.light_intensity,
                    self.light_range,
                    self.light_flicker,
                );
                let realistic = self.realistic_character_lighting;
                let receive = self.faithful_shadow_receive;
                let zld = self.zone_line_display;
                let next =
                    cycle_slot(self.preset, PRESET_CYCLE, delta).unwrap_or(QualityPreset::High);
                *self = Self::for_preset(next);
                self.sky_style = sky;
                self.dynamic_lights = lights;
                self.light_threshold = lt;
                self.light_intensity = li;
                self.light_range = lr;
                self.light_flicker = lf;
                self.realistic_character_lighting = realistic;
                self.faithful_shadow_receive = receive;
                self.zone_line_display = zld;
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
            GraphicsField::TextureFiltering => {
                self.texture_filtering =
                    cycle_slot(self.texture_filtering, TEXTURE_FILTERING_CYCLE, delta)
                        .unwrap_or(TextureFiltering::Aniso4x);
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
                self.sky_style = match self.sky_style {
                    SkyStyle::Enhanced => SkyStyle::Vanilla,
                    SkyStyle::Vanilla => SkyStyle::Enhanced,
                };
            }
            GraphicsField::DynamicLights => {
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
            GraphicsField::CharacterLighting => {
                self.realistic_character_lighting = !self.realistic_character_lighting;
            }
            GraphicsField::CharacterShadowReceive => {
                self.faithful_shadow_receive = !self.faithful_shadow_receive;
            }
            GraphicsField::CharacterShadowCast => {
                self.character_shadow_cast = !self.character_shadow_cast;
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::DepthOfField => {
                self.depth_of_field = !self.depth_of_field;
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::DofAperture => {
                self.dof_aperture_f_stops =
                    cycle_slot_f32(self.dof_aperture_f_stops, DOF_APERTURE_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::ZoneLineDisplay => {
                self.zone_line_display =
                    cycle_slot(self.zone_line_display, ZONE_LINE_DISPLAY_CYCLE, delta)
                        .unwrap_or(ZoneLineDisplay::Off);
            }
            GraphicsField::RenderScale => {
                self.render_scale = cycle_slot_f32(self.render_scale, RENDER_SCALE_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
        }
    }

    pub fn reset_to_default(&mut self) {
        *self = Self::for_preset(QualityPreset::High);
    }

    pub fn sky_style(&self) -> SkyStyle {
        self.sky_style
    }

    pub fn sky_embellishments_enabled(&self) -> bool {
        self.sky_style == SkyStyle::Enhanced
    }

    /// Lamp reach as a multiple of the default range, so the GUI "Range" knob
    /// scales every lamp feed (faithful Generator lights + `/lights` emitters)
    /// uniformly rather than only the emitters.
    pub fn light_reach_scale(&self) -> f32 {
        self.light_range / DEFAULT_LIGHT_RANGE
    }

    fn lights_fine_is_default(&self) -> bool {
        (self.light_threshold - DEFAULT_LIGHT_THRESHOLD).abs() < 1e-3
            && (self.light_intensity - DEFAULT_LIGHT_INTENSITY).abs() < 1.0
            && (self.light_range - DEFAULT_LIGHT_RANGE).abs() < 1e-3
            && self.light_flicker == DEFAULT_LIGHT_FLICKER
    }

    pub fn msaa(&self) -> Msaa {
        match self.anti_aliasing {
            AaMode::Off | AaMode::Taa => Msaa::Off,
            AaMode::Msaa2 => Msaa::Sample2,
            AaMode::Msaa4 => Msaa::Sample4,
            AaMode::Msaa8 => Msaa::Sample8,
        }
    }

    pub fn wants_taa(&self) -> bool {
        matches!(self.anti_aliasing, AaMode::Taa)
    }

    /// Clamped render-scale factor (3D-buffer resolution ÷ window resolution).
    pub fn render_scale(&self) -> f32 {
        self.render_scale.clamp(0.25, 2.0)
    }

    /// True when the 3D buffer should be rendered off-window and (up/down)scaled.
    /// At exactly 1.0 the camera renders straight to the window (no extra passes).
    pub fn wants_render_scale(&self) -> bool {
        (self.render_scale() - 1.0).abs() > 1e-3
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct MsaaCaps {
    pub mask: u32,
}

impl Default for MsaaCaps {
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

    pub fn clamp(self, want: Msaa) -> Msaa {
        for n in [want.samples(), 4, 2, 1] {
            if n <= want.samples() && self.supports(n) {
                return Msaa::from_samples(n);
            }
        }
        Msaa::Off
    }
}

pub fn init_msaa_caps_system(
    adapter: Option<Res<RenderAdapter>>,
    mut settings: ResMut<GraphicsSettings>,
    mut commands: Commands,
) {
    let caps = if let Some(adapter) = adapter {
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
        mask |= (1 << 1) | (1 << 4);
        MsaaCaps { mask }
    } else {
        MsaaCaps::default()
    };
    commands.insert_resource(caps);

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

pub const GRAPHICS_FIELDS: &[GraphicsField] = &[
    GraphicsField::Preset,
    GraphicsField::ShadowMapSize,
    GraphicsField::ShadowCascadeCount,
    GraphicsField::ShadowMaxDistance,
    GraphicsField::AntiAliasing,
    GraphicsField::TextureFiltering,
    GraphicsField::BloomIntensity,
    GraphicsField::VolumetricFog,
    GraphicsField::FogStepCount,
    GraphicsField::ViewDistance,
    GraphicsField::VSync,
    GraphicsField::Fov,
    GraphicsField::SkyStyle,
    GraphicsField::DynamicLights,
    GraphicsField::LightThreshold,
    GraphicsField::LightIntensity,
    GraphicsField::LightRange,
    GraphicsField::LightFlicker,
    GraphicsField::CharacterLighting,
    GraphicsField::CharacterShadowReceive,
    GraphicsField::DepthOfField,
    GraphicsField::DofAperture,
    GraphicsField::ZoneLineDisplay,
    GraphicsField::RenderScale,
];

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

pub fn cascade_config_from_settings(s: &GraphicsSettings) -> CascadeShadowConfig {
    CascadeShadowConfigBuilder {
        num_cascades: s.shadow_cascade_count as usize,
        minimum_distance: 0.1,
        maximum_distance: s.shadow_max_distance,
        first_cascade_far_bound: 8.0,
        overlap_proportion: 0.15,
    }
    .build()
}

pub fn apply_shadow_map_size_system(settings: Res<GraphicsSettings>, mut commands: Commands) {
    commands.insert_resource(DirectionalLightShadowMap {
        size: settings.shadow_map_size as usize,
    });
}

pub fn apply_cascade_config_system(
    settings: Res<GraphicsSettings>,
    mut q_sun: Query<&mut CascadeShadowConfig, With<IsSun>>,
) {
    for mut cfg in q_sun.iter_mut() {
        *cfg = cascade_config_from_settings(&settings);
    }
}

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

pub fn apply_bloom_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    mut q_cam: Query<(Entity, Option<&mut Bloom>), With<OperatorCamera>>,
) {
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

pub fn apply_sky_realism_system(
    settings: Res<GraphicsSettings>,
    mut sky: ResMut<crate::sky_realism::SkyRealism>,
) {
    let want = settings.sky_style.sky_realism();
    if *sky != want {
        *sky = want;
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn apply_depth_of_field_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    q_cam: Query<(Entity, Option<&DepthOfField>), With<OperatorCamera>>,
) {
    let Ok((entity, current)) = q_cam.single() else {
        return;
    };
    match (settings.depth_of_field, current.is_some()) {
        (true, false) => {
            commands.entity(entity).insert(DepthOfField {
                mode: DepthOfFieldMode::Bokeh,
                aperture_f_stops: settings.dof_aperture_f_stops,
                // Seed only; focal distance + aperture are kept live by
                // update_depth_of_field_focus_system.
                focal_distance: 18.0,
                ..DepthOfField::default()
            });
        }
        (false, true) => {
            commands.entity(entity).remove::<DepthOfField>();
        }
        _ => {}
    }
}

/// Yalms ahead of the eye to focus in first person when nothing is targeted.
#[cfg(not(target_arch = "wasm32"))]
const DOF_FIRST_PERSON_FOCUS: f32 = 6.0;

/// Keep the focal plane on the current target (sharp subject, bokeh-blurred
/// background), falling back to: a few yalms ahead in first person, or the
/// player in chase view, when nothing is targeted. Also syncs the GUI-tunable
/// aperture onto the live component. Only runs while DoF is present.
#[cfg(not(target_arch = "wasm32"))]
pub fn update_depth_of_field_focus_system(
    settings: Res<GraphicsSettings>,
    target: Res<crate::scene::Target>,
    mode: Option<Res<crate::camera::CameraMode>>,
    q_self: Query<&Transform, (With<crate::components::IsSelf>, Without<OperatorCamera>)>,
    q_world: Query<(&Transform, &crate::components::WorldEntity), Without<OperatorCamera>>,
    mut q_cam: Query<(&Transform, &mut DepthOfField), With<OperatorCamera>>,
) {
    let Ok((cam_t, mut dof)) = q_cam.single_mut() else {
        return;
    };

    let target_dist = target.id.and_then(|tid| {
        q_world
            .iter()
            .find(|(_, w)| w.id == tid)
            .map(|(t, _)| cam_t.translation.distance(t.translation))
    });

    let first_person = matches!(
        mode.as_deref(),
        Some(crate::camera::CameraMode::FirstPerson)
    );
    let focal = match target_dist {
        Some(d) => d.max(2.0),
        // No target: first person focuses a few yalms ahead; chase focuses the
        // player (the camera→self distance ≈ the chase zoom).
        None if first_person => DOF_FIRST_PERSON_FOCUS,
        None => q_self
            .single()
            .ok()
            .map(|t| cam_t.translation.distance(t.translation).max(2.0))
            .unwrap_or(DOF_FIRST_PERSON_FOCUS),
    };

    if (dof.focal_distance - focal).abs() > 0.05 {
        dof.focal_distance = focal;
    }
    if (dof.aperture_f_stops - settings.dof_aperture_f_stops).abs() > 1e-4 {
        dof.aperture_f_stops = settings.dof_aperture_f_stops;
    }
}

/// Owns the camera `DepthPrepass`, shared by every consumer that needs scene
/// depth: Depth of Field, and the Vanilla sun flare (which samples the prepass
/// to occlude itself behind terrain). TAA also requires it via
/// `#[require(DepthPrepass, …)]`, so we never strip it while TAA is on. Runs
/// every frame (not just on settings change) because the flare's need rises and
/// falls with the sun; the `match` self-heals across the AA camera respawn.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_camera_prepass_system(
    settings: Res<GraphicsSettings>,
    sky: Res<crate::sun_moon::VanaSky>,
    mut commands: Commands,
    q_cam: Query<(Entity, Option<&DepthPrepass>), With<OperatorCamera>>,
) {
    let Ok((entity, depth)) = q_cam.single() else {
        return;
    };
    let flare_wants = settings.sky_style == SkyStyle::Vanilla && sky.sun_altitude > 0.0;
    let keep_depth = settings.depth_of_field || settings.wants_taa() || flare_wants;
    // try_* so we no-op (not panic) if apply_anti_aliasing_system queued a
    // despawn+respawn of this same camera earlier in the frame.
    match (keep_depth, depth.is_some()) {
        (true, false) => {
            commands.entity(entity).try_insert(DepthPrepass);
        }
        (false, true) => {
            commands.entity(entity).try_remove::<DepthPrepass>();
        }
        _ => {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
pub struct PhysicalSkyMedium(pub Handle<ScatteringMedium>);

#[cfg(not(target_arch = "wasm32"))]
pub fn init_physical_sky_medium_system(
    mut commands: Commands,
    media: Option<ResMut<Assets<ScatteringMedium>>>,
) {
    let Some(mut media) = media else {
        return;
    };
    let handle = media.add(ScatteringMedium::default());
    commands.insert_resource(PhysicalSkyMedium(handle));
}

#[cfg(not(target_arch = "wasm32"))]
pub fn apply_physical_sky_system(
    settings: Res<GraphicsSettings>,
    medium: Option<Res<PhysicalSkyMedium>>,
    mut commands: Commands,
    q_cam: Query<(Entity, Option<&Atmosphere>), With<OperatorCamera>>,
    mut q_sky: Query<&mut Visibility, With<crate::skybox::SkyboxSphere>>,
) {
    let physical = settings.sky_style.physical();
    if let Ok((entity, atmo)) = q_cam.single() {
        match (physical, atmo.is_some()) {
            (true, false) => {
                if let Some(medium) = medium {
                    commands.entity(entity).insert((
                        Atmosphere::earthlike(medium.0.clone()),
                        AtmosphereSettings::default(),
                    ));
                }
            }
            (false, true) => {
                commands
                    .entity(entity)
                    .remove::<Atmosphere>()
                    .remove::<AtmosphereSettings>();
            }
            _ => {}
        }
    }

    let want = if physical {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut vis in q_sky.iter_mut() {
        if *vis != want {
            *vis = want;
        }
    }
}

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
            assert!(TEXTURE_FILTERING_CYCLE.contains(&s.texture_filtering));
            assert!(
                RENDER_SCALE_SLOTS
                    .iter()
                    .any(|x| (x - s.render_scale).abs() < 1e-3),
                "preset {preset:?} render_scale {} not in slot list",
                s.render_scale
            );
        }
    }

    #[test]
    fn json_roundtrip_preserves_all_fields() {
        let s = GraphicsSettings::for_preset(QualityPreset::Ultra);
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicsSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn cycling_a_lever_marks_preset_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.preset, QualityPreset::High);
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.preset, QualityPreset::Custom);

        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::VolumetricFog, 1);
        assert_eq!(s.preset, QualityPreset::Custom);
        assert!(!s.volumetric_fog, "toggled off");
    }

    #[test]
    fn cycling_preset_overwrites_all_fields() {
        let mut s = GraphicsSettings::for_preset(QualityPreset::High);
        s.shadow_map_size = 1024;
        s.preset = QualityPreset::Custom;

        s.cycle(GraphicsField::Preset, 1);
        let medium = GraphicsSettings::for_preset(QualityPreset::Medium);
        assert_eq!(s, medium);
    }

    #[test]
    fn sky_style_toggles_two_modes() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.sky_style(), SkyStyle::Enhanced);
        assert!(s.sky_embellishments_enabled());
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style(), SkyStyle::Vanilla);
        assert!(!s.sky_embellishments_enabled());
        assert!(!s.sky_style.physical(), "Vanilla keeps the gradient skybox");
        assert_eq!(s.preset, QualityPreset::High, "style must not flip preset");
        // Cycling in either direction just flips the two modes.
        s.cycle(GraphicsField::SkyStyle, -1);
        assert_eq!(s.sky_style(), SkyStyle::Enhanced);
        assert!(s.sky_style.physical(), "Enhanced drives the atmosphere");
    }

    #[test]
    fn embellishments_off_only_for_vanilla() {
        let mut s = GraphicsSettings::default();
        assert!(s.sky_embellishments_enabled());
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style(), SkyStyle::Vanilla);
        assert!(!s.sky_embellishments_enabled());
    }

    #[test]
    fn tuning_a_light_knob_marks_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Many");
        s.cycle(GraphicsField::LightIntensity, 1);
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Custom");
        assert_eq!(s.dynamic_lights, DynamicLights::Many, "cap tier unchanged");
        assert_eq!(s.preset, QualityPreset::High, "light knob ⟂ quality tier");
        s.cycle(GraphicsField::DynamicLights, 1);
        assert!(s.lights_fine_is_default());
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Off");
    }

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

    #[test]
    fn sky_style_maps_to_realism() {
        use crate::sky_realism::SkyRealism;
        assert_eq!(SkyStyle::Vanilla.sky_realism(), SkyRealism::retail());
        assert_eq!(SkyStyle::Enhanced.sky_realism(), SkyRealism::enhanced());
        assert!(!SkyRealism::retail().earthshine);
        assert!(SkyRealism::enhanced().earthshine);
    }

    #[test]
    fn preset_cycle_preserves_sky_style() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::SkyStyle, 1);
        assert_eq!(s.sky_style(), SkyStyle::Vanilla);
        s.cycle(GraphicsField::Preset, 1);
        assert_eq!(
            s.sky_style(),
            SkyStyle::Vanilla,
            "preset cycle kept the style"
        );
    }

    #[test]
    fn dynamic_lights_cycles_without_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.dynamic_lights, DynamicLights::Many);
        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Off);
        assert_eq!(s.preset, QualityPreset::High, "lights must not flip preset");
        assert!(!s.dynamic_lights.enabled());
        assert_eq!(s.dynamic_lights.max_total(), 0);
        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Few);
        assert_eq!(s.dynamic_lights.max_total(), 24);
        assert!(s.dynamic_lights.enabled());
    }

    #[test]
    fn preset_cycle_preserves_dynamic_lights() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Off);
        s.cycle(GraphicsField::Preset, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Off, "preset cycle kept it");
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        let mut s = GraphicsSettings::default();

        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.shadow_map_size, 8192);
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.shadow_map_size, 1024, "wrapped past 8192");
        s.cycle(GraphicsField::ShadowMapSize, -1);
        assert_eq!(s.shadow_map_size, 8192, "wrapped back");
    }

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

    #[test]
    fn value_label_smoke() {
        let s = GraphicsSettings {
            preset: QualityPreset::Ultra,
            shadow_map_size: 2048,
            shadow_cascade_count: 3,
            shadow_max_distance: 400.0,
            volumetric_fog: true,
            fov_deg: 90.0,
            ..Default::default()
        };
        assert_eq!(s.value_label(GraphicsField::Preset), "Ultra");
        assert_eq!(s.value_label(GraphicsField::ShadowMapSize), "2048px");
        assert_eq!(s.value_label(GraphicsField::ShadowCascadeCount), "3");
        assert_eq!(s.value_label(GraphicsField::ShadowMaxDistance), "400m");
        assert_eq!(s.value_label(GraphicsField::VolumetricFog), "On");
        assert_eq!(s.value_label(GraphicsField::Fov), "90°");
    }

    #[test]
    fn model_shadows_default_on_for_all_presets() {
        for &preset in PRESET_CYCLE {
            assert!(
                GraphicsSettings::for_preset(preset).faithful_shadow_receive,
                "preset {preset:?} should default to receiving shadows"
            );
        }
        assert!(GraphicsSettings::default().faithful_shadow_receive);
    }

    #[test]
    fn model_shadows_toggle_is_orthogonal() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::CharacterShadowReceive), "On");
        s.cycle(GraphicsField::CharacterShadowReceive, 1);
        assert!(!s.faithful_shadow_receive);
        assert_eq!(s.value_label(GraphicsField::CharacterShadowReceive), "Off");
        assert_eq!(
            s.preset,
            QualityPreset::High,
            "shadow receipt ⟂ quality tier"
        );

        s.cycle(GraphicsField::Preset, 1);
        assert!(!s.faithful_shadow_receive, "preset cycle kept receipt off");
    }

    #[test]
    fn reset_returns_to_high() {
        let mut s = GraphicsSettings::for_preset(QualityPreset::Low);
        s.bloom_intensity = 0.16;
        s.preset = QualityPreset::Custom;
        s.reset_to_default();
        assert_eq!(s, GraphicsSettings::for_preset(QualityPreset::High));
    }

    #[test]
    fn presets_are_dof_and_taa_free_by_default() {
        // Depth of Field and TAA are the settings-level prepass forcers; no
        // preset turns either on, so the prepass is paid only at runtime (DoF,
        // or the Vanilla sun flare while the sun is up).
        for &preset in PRESET_CYCLE {
            let s = GraphicsSettings::for_preset(preset);
            assert!(!s.depth_of_field, "{preset:?} must not auto-enable DoF");
            assert_ne!(
                s.anti_aliasing,
                AaMode::Taa,
                "{preset:?} must not default to TAA (forces a prepass)"
            );
        }
    }

    #[test]
    fn depth_of_field_flips_tier_sky_style_does_not() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DepthOfField, 1);
        assert!(s.depth_of_field);
        assert_eq!(s.preset, QualityPreset::Custom, "DoF is a quality knob");

        // Toggling Sky Style is orthogonal to the quality tier.
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::SkyStyle, 1); // Enhanced -> Vanilla
        assert_eq!(s.sky_style(), SkyStyle::Vanilla);
        assert_eq!(s.preset, QualityPreset::High, "sky style ⟂ quality tier");
    }

    #[test]
    fn depth_of_field_toggles() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::DepthOfField), "Off");
        s.cycle(GraphicsField::DepthOfField, 1);
        assert!(s.depth_of_field);
        assert_eq!(s.value_label(GraphicsField::DepthOfField), "On");
    }

    #[test]
    fn dof_aperture_cycles_through_f_stops() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::DofAperture), "f/2.8");
        assert!(DOF_APERTURE_SLOTS.contains(&DEFAULT_DOF_APERTURE));
        s.cycle(GraphicsField::DofAperture, 1);
        assert_eq!(s.dof_aperture_f_stops, 4.0);
        assert_eq!(
            s.preset,
            QualityPreset::Custom,
            "aperture is a quality knob"
        );
        s.cycle(GraphicsField::DofAperture, -1);
        assert_eq!(s.dof_aperture_f_stops, 2.8);
    }

    #[test]
    fn preset_cycle_resets_dof_keeps_sky_style() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DepthOfField, 1); // on -> Custom
        s.cycle(GraphicsField::SkyStyle, 1); // Enhanced -> Vanilla
        assert_eq!(s.sky_style(), SkyStyle::Vanilla);

        s.cycle(GraphicsField::Preset, 1); // Custom -> Medium
        assert_eq!(s.preset, QualityPreset::Medium);
        assert!(
            !s.depth_of_field,
            "preset cycle reset DoF to the tier default"
        );
        assert_eq!(
            s.sky_style(),
            SkyStyle::Vanilla,
            "preset cycle kept the sky style"
        );
    }

    #[test]
    fn advanced_fields_are_exactly_the_indented_knobs() {
        let advanced: Vec<_> = GRAPHICS_FIELDS
            .iter()
            .copied()
            .filter(|f| f.is_advanced())
            .collect();
        // The 4 light tuning knobs.
        assert_eq!(advanced.len(), 4, "advanced set drifted: {advanced:?}");
        // Every advanced field is an indented child row ("  …"); no basic field is.
        for &f in GRAPHICS_FIELDS {
            assert_eq!(
                f.is_advanced(),
                f.label().starts_with("  "),
                "{f:?}: is_advanced disagrees with its indented label"
            );
        }
    }

    #[test]
    fn zone_line_display_cycles_three_modes_orthogonal_to_tier() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.zone_line_display, ZoneLineDisplay::Off);
        assert_eq!(s.value_label(GraphicsField::ZoneLineDisplay), "Off");

        s.cycle(GraphicsField::ZoneLineDisplay, 1);
        assert_eq!(s.zone_line_display, ZoneLineDisplay::Pillar);
        assert_eq!(s.preset, QualityPreset::High, "display ⟂ quality tier");

        s.cycle(GraphicsField::ZoneLineDisplay, 1);
        assert_eq!(s.zone_line_display, ZoneLineDisplay::Gate);
        s.cycle(GraphicsField::ZoneLineDisplay, 1);
        assert_eq!(s.zone_line_display, ZoneLineDisplay::Off, "wrapped");
        s.cycle(GraphicsField::ZoneLineDisplay, -1);
        assert_eq!(s.zone_line_display, ZoneLineDisplay::Gate, "wrapped back");
    }

    #[test]
    fn preset_cycle_preserves_zone_line_display() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::ZoneLineDisplay, 1);
        assert_eq!(s.zone_line_display, ZoneLineDisplay::Pillar);
        s.cycle(GraphicsField::Preset, 1);
        assert_eq!(
            s.zone_line_display,
            ZoneLineDisplay::Pillar,
            "preset cycle kept the zone-line mode"
        );
    }

    #[test]
    fn render_scale_defaults_to_full_and_is_a_quality_lever() {
        let mut s = GraphicsSettings::default();
        assert!((s.render_scale - 1.0).abs() < 1e-6);
        assert_eq!(s.value_label(GraphicsField::RenderScale), "100%");
        assert!(!s.wants_render_scale(), "100% is the no-op native path");

        s.cycle(GraphicsField::RenderScale, -1);
        assert!(s.render_scale < 1.0, "stepped below native");
        assert!(s.wants_render_scale());
        assert_eq!(
            s.preset,
            QualityPreset::Custom,
            "render scale is a quality knob"
        );

        // Wraps and remains slot-aligned in both directions.
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::RenderScale, 1);
        assert!(s.render_scale > 1.0, "stepped into supersampling");
        assert_eq!(s.value_label(GraphicsField::RenderScale), "125%");
    }

    #[test]
    fn effect_fields_survive_json_roundtrip() {
        let s = GraphicsSettings {
            depth_of_field: true,
            sky_style: SkyStyle::Vanilla,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicsSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
