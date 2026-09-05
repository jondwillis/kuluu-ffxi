use bevy::core_pipeline::tonemapping::Tonemapping;
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
    #[default]
    Off,
    Msaa2,
    Msaa4,
    Msaa8,

    Taa,

    /// NVIDIA DLSS Super Resolution: upscaling + anti-aliasing in one. Only
    /// reachable through the Graphics menu's explicit DLSS on/off row, and
    /// only while `dlss_selectable()` (runtime capability AND the Retail+
    /// gate); the variant itself is unconditional so a graphics.json written
    /// with DLSS on still deserializes on any build (where it behaves as Off).
    Dlss,
}

impl AaMode {
    pub const fn label(self) -> &'static str {
        match self {
            AaMode::Off => "Off",
            AaMode::Msaa2 => "MSAA 2x",
            AaMode::Msaa4 => "MSAA 4x",
            AaMode::Msaa8 => "MSAA 8x",
            AaMode::Taa => "TAA",
            AaMode::Dlss => "DLSS",
        }
    }
}

/// DLSS Super Resolution performance/quality tier. Mirrors
/// `dlss_wgpu::DlssPerfQualityMode` one-to-one (mapping lives in
/// `graphics::dlss`, dlss-feature builds only); defined unconditionally so the
/// persisted graphics.json round-trips on default builds too.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DlssQuality {
    /// Let DLSS pick a tier from the output resolution (NVIDIA's
    /// recommendation and dlss_wgpu's default).
    #[default]
    Auto,
    /// Anti-aliasing only, no upscaling (native-res DLSS).
    Dlaa,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl DlssQuality {
    pub const fn label(self) -> &'static str {
        match self {
            DlssQuality::Auto => "Auto",
            DlssQuality::Dlaa => "DLAA",
            DlssQuality::Quality => "Quality",
            DlssQuality::Balanced => "Balanced",
            DlssQuality::Performance => "Performance",
            DlssQuality::UltraPerformance => "Ultra Perf",
        }
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

/// `Vanilla` renders only the faithful DAT Generator lights; `Enhanced` adds
/// the heuristic over-bright-vertex emitters on top; `Off` disables both.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicLights {
    Off,
    #[serde(alias = "Few", alias = "Many")]
    #[default]
    Vanilla,
    Enhanced,
}

/// Clustered-lighting budget for the heuristic emitters.
pub const DYNAMIC_LIGHTS_MAX_TOTAL: u32 = 48;

impl DynamicLights {
    pub const fn label(self) -> &'static str {
        match self {
            DynamicLights::Off => "Off",
            DynamicLights::Vanilla => "Vanilla",
            DynamicLights::Enhanced => "Enhanced",
        }
    }

    pub const fn faithful_enabled(self) -> bool {
        !matches!(self, DynamicLights::Off)
    }

    pub const fn emitters_enabled(self) -> bool {
        matches!(self, DynamicLights::Enhanced)
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
    #[default]
    Vanilla,
    Aniso2x,
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
    FrameRateCap,
    Fov,
    UiScale,
    CameraSpring,
    MenuScale,
    Fullscreen,
    Windowed,

    DynamicLights,
    LightThreshold,
    LightIntensity,
    LightRange,
    LightFlicker,
    ModelLightCount,

    CharacterLighting,

    CharacterShadowReceive,
    CharacterShadowCast,

    DepthOfField,
    DofAperture,

    ZoneLineDisplay,

    RenderScale,

    /// DLSS on/off toggle in the main graphics list (a mirror of
    /// `anti_aliasing == AaMode::Dlss`; N/A while unsupported).
    Dlss,

    // --- DLSS Config submenu rows (DLSS_CONFIG_FIELDS, not GRAPHICS_FIELDS) ---
    /// SR performance/quality tier — the one live knob.
    DlssQuality,
    /// Ray Reconstruction preset. Inert placeholder: bevy_anti_alias 0.19
    /// exposes SR only, no RR plumbing. Always "N/A".
    DlssRrPreset,
    /// Super Resolution model preset (RenoDX-style J/K/L/M). Inert:
    /// dlss_wgpu 4.0 doesn't surface preset selection. Always "N/A".
    DlssSrPreset,
    /// RR responsivity bias. Inert (RR itself unavailable). Always "N/A".
    DlssRrResponsivity,
    /// DLSS 5 Neural Uplift (NR) master toggle. Live on dlss builds with an
    /// RTX GPU + `nvngx_dlssnr.dll` staged next to the exe; N/A otherwise.
    /// Drives the NR pipeline in `graphics/dlss_nr.rs`.
    DlssNeuralUplift,
    /// NR enhancement strength (the addon's "NR Intensity"; 1.0 reads as
    /// "no visible effect", kuluu default 1.01). Live while supported.
    DlssNrIntensity,
    /// NR local tone-mapping strength. Live while supported.
    DlssNrLocalTone,
    /// NR local structure (edge) strength. Live while supported.
    DlssNrStructure,
    /// Post-upscale sharpening. Wireable later via bevy's
    /// ContrastAdaptiveSharpening; shipped inert for now. Always "N/A".
    DlssSharpness,
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
            GraphicsField::FrameRateCap => "Frame Rate Cap",
            GraphicsField::Fov => "FOV",
            GraphicsField::UiScale => "UI Scale",
            GraphicsField::CameraSpring => "Camera Spring",
            GraphicsField::MenuScale => "Menu Scale",
            GraphicsField::Fullscreen => "Fullscreen",
            GraphicsField::Windowed => "Windowed",
            GraphicsField::DynamicLights => "Dynamic Lights",
            GraphicsField::LightThreshold => "  Emitter Threshold",
            GraphicsField::LightIntensity => "  Emitter Intensity",
            GraphicsField::LightRange => "  Emitter Range",
            GraphicsField::LightFlicker => "  Flicker",
            GraphicsField::ModelLightCount => "  Lights per Model",
            GraphicsField::CharacterLighting => "Shading",
            GraphicsField::CharacterShadowReceive => "Model Shadow Receiving",
            GraphicsField::CharacterShadowCast => "Model Shadow Casting",
            GraphicsField::DepthOfField => "Depth of Field",
            GraphicsField::DofAperture => "DoF Aperture",
            GraphicsField::ZoneLineDisplay => "Zone Lines",
            GraphicsField::RenderScale => "Render Scale",
            GraphicsField::Dlss => "DLSS",
            GraphicsField::DlssQuality => "DLSS Quality",
            GraphicsField::DlssRrPreset => "RR Preset",
            GraphicsField::DlssSrPreset => "SR Preset",
            GraphicsField::DlssRrResponsivity => "RR Responsivity",
            GraphicsField::DlssNeuralUplift => "Neural Uplift",
            GraphicsField::DlssNrIntensity => "NR Intensity",
            GraphicsField::DlssNrLocalTone => "Local Tone Strength",
            GraphicsField::DlssNrStructure => "Structure Strength",
            GraphicsField::DlssSharpness => "Sharpness",
        }
    }

    /// Rows that live in the DLSS Config surface (a pushed submenu in-game, a
    /// disclosure in the launcher) rather than the main graphics list.
    pub const fn is_dlss_config(self) -> bool {
        matches!(
            self,
            GraphicsField::DlssQuality
                | GraphicsField::DlssRrPreset
                | GraphicsField::DlssSrPreset
                | GraphicsField::DlssRrResponsivity
                | GraphicsField::DlssNeuralUplift
                | GraphicsField::DlssNrIntensity
                | GraphicsField::DlssNrLocalTone
                | GraphicsField::DlssNrStructure
                | GraphicsField::DlssSharpness
        )
    }

    /// The inert RenoDX-parity placeholders: visible so the config surface
    /// shows what's planned, but nothing behind them until the SDK plumbing
    /// (RR / presets / CAS) exists. value_label = "N/A", cycle = no-op, on
    /// every build. Neural Uplift left this set when kuluu-dlss-nr landed.
    pub const fn is_dlss_placeholder(self) -> bool {
        matches!(
            self,
            GraphicsField::DlssRrPreset
                | GraphicsField::DlssSrPreset
                | GraphicsField::DlssRrResponsivity
                | GraphicsField::DlssSharpness
        )
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
                | GraphicsField::ModelLightCount
        )
    }
}

fn default_ui_scale() -> f32 {
    1.0
}
fn default_menu_scale_on() -> bool {
    true
}
/// The RenoDX addon's NR Intensity default: 1.0 is the parser default and can
/// read as "no visible effect", so kuluu starts one notch above it.
fn default_nr_intensity() -> f32 {
    1.01
}
fn default_nr_local_tone() -> f32 {
    1.0
}
fn default_nr_structure() -> f32 {
    1.0
}

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GraphicsSettings {
    pub preset: QualityPreset,
    pub shadow_map_size: u32,
    pub shadow_cascade_count: u32,
    pub shadow_max_distance: f32,
    pub anti_aliasing: AaMode,

    /// DLSS SR quality tier, applied whenever `anti_aliasing == AaMode::Dlss`.
    /// Persisted independently of the on/off state so toggling DLSS off and on
    /// keeps the chosen tier. NOT owned by quality presets — preset cycling
    /// carries it over untouched.
    #[serde(default)]
    pub dlss_quality: DlssQuality,

    /// DLSS 5 Neural Uplift (NR) master toggle. Live only while `dlss_supported`
    /// AND the NR runtime DLL is staged next to the exe; N/A otherwise. NOT
    /// owned by quality presets — preset cycling carries it over untouched.
    #[serde(default)]
    pub neural_uplift: bool,

    /// NR enhancement strength (addon default 1.01; see `default_nr_intensity`).
    #[serde(default = "default_nr_intensity")]
    pub nr_intensity: f32,

    /// NR local tone-mapping strength.
    #[serde(default = "default_nr_local_tone")]
    pub nr_local_tone_strength: f32,

    /// NR local structure (edge) strength.
    #[serde(default = "default_nr_structure")]
    pub nr_structure_strength: f32,

    /// Runtime capability: true only when the dlss cargo feature is compiled
    /// in AND the renderer initialized DLSS on this machine (RTX GPU, Vulkan,
    /// and the NVIDIA snippet DLLs present). Set once at startup by
    /// `graphics::dlss::update_dlss_availability_system`; never persisted.
    /// Combined with [`Self::dlss_menu_enabled`] it gates everything
    /// user-facing: the DLSS rows only read/cycle while both are true.
    #[serde(skip)]
    pub dlss_supported: bool,

    /// Retail+ menu gate (dev-only Debug menu): when true, the Graphics menu
    /// exposes DLSS — the DLSS row cycles and a persisted `AaMode::Dlss`
    /// takes effect. OFF by default: vanilla parity (retail has no DLSS), so
    /// one dlss-capable build serves both audiences instead of shipping a
    /// separate DLSS release. This does NOT turn DLSS on — it only makes it
    /// selectable; the Graphics menu's own DLSS row still owns on/off.
    /// Persisted here so the choice sticks across runs (camera_spring class).
    #[serde(default)]
    pub dlss_menu_enabled: bool,

    /// Retail+ gate for the party-frame Job column. OFF by default — retail's
    /// party frame shows no job abbreviations; ours is an enhancement, so it
    /// stays hidden unless explicitly enabled in the dev-only Debug menu. The
    /// `enhanced-job-display` feature is its compile-time half: without it this
    /// field can never light the column (the row doesn't exist either).
    /// Persisted here so the choice sticks across runs.
    #[serde(default)]
    pub job_display: bool,

    /// Retail+ gate for the mob/pet HP readout on nameplates — both the green
    /// bar under the billboard plate and the "{name} {pct}%" suffix in the UI
    /// plates. OFF by default: retail shows no mob HP, so ours stays hidden
    /// unless explicitly enabled in the dev-only Debug menu. The
    /// `enhanced-mob-hp-under` feature is its compile-time half: without it this
    /// field can never light either (the row doesn't exist either). Persisted
    /// here so the choice sticks across runs.
    #[serde(default)]
    pub mob_hp_under: bool,

    #[serde(default)]
    pub texture_filtering: TextureFiltering,

    pub bloom_intensity: f32,
    // Retail has no volumetric light shafts, so vanilla parity means off at every
    // tier; the toggle stays as an opt-in embellishment.
    pub volumetric_fog: bool,
    pub fog_step_count: u32,
    pub view_distance: f32,
    pub vsync: bool,
    /// 0 disables the cap (framepace Auto); RETAIL_FPS-adjacent slots otherwise.
    #[serde(default)]
    pub fps_cap: u32,
    pub fov_deg: f32,
    /// HUD size multiplier on top of the resolution-relative base (1080p =
    /// 1.0x). Applied via bevy's UiScale by apply_ui_scale_system.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Camera position-spring + boom easing. OFF by default while the
    /// accel-driven UI jitter is under investigation (2026-08-27: disabling
    /// this empirically killed the every-other-frame HUD jitter). Toggled in
    /// the Debug menu; persisted here so the choice sticks.
    #[serde(default)]
    pub camera_spring: bool,
    /// Menu-only UI scale multiplier. Applied on top of the resolution-relative
    /// base and the global UI Scale, so "Menu Scale off" holds menu panels at
    /// the 1080p baseline while HUD widgets still track the window. Toggled in
    /// the Graphics menu; persisted here.
    #[serde(default = "default_menu_scale_on")]
    pub menu_scale: bool,

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

    /// How many nearby point lights each skinned model samples. Zone surfaces
    /// are lit by clustered forward binning instead and ignore this.
    #[serde(default = "default_model_light_count", alias = "dynamic_light_count")]
    pub model_light_count: u32,

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

    /// Whether the window is fullscreen at all. Display preference like VSync,
    /// doesn't touch the quality preset. `FFXI_FULLSCREEN` env var still wins at
    /// startup. When on, `windowed_fullscreen` picks exclusive vs borderless.
    #[serde(default)]
    pub fullscreen: bool,

    /// When `fullscreen` is on, choose borderless windowed-fullscreen (true)
    /// instead of exclusive/true fullscreen (false). Ignored while windowed.
    #[serde(default)]
    pub windowed_fullscreen: bool,
}

pub const DEFAULT_LIGHT_THRESHOLD: f32 = 1.15;
pub const DEFAULT_LIGHT_INTENSITY: f32 = 25_000.0;
pub const DEFAULT_LIGHT_RANGE: f32 = 8.0;
pub const DEFAULT_LIGHT_FLICKER: bool = true;
// How many dynamic point lights illuminate zone/actor surfaces at once (the
// nearest N to the viewer/actor). The old fixed cap was 4; more lights let a
// wider spread of lamps light their surroundings before you reach them. Capped
// by MAX_POINT_LIGHTS (the shader array length).
pub const DEFAULT_MODEL_LIGHT_COUNT: u32 = 8;
pub const MODEL_LIGHT_COUNT_SLOTS: &[u32] = &[4, 8, 12, 16];

// Lower f-stop = wider aperture = stronger background blur. f/2.8 is a tasteful
// cinematic default once the user opts into DoF.
pub const DEFAULT_DOF_APERTURE: f32 = 2.8;

// 1.0 = render the 3D scene at the window's native resolution (the byte-identical
// default; no off-screen target, no composite camera). Below 1.0 downscales the
// 3D buffer for performance and upscales to the window; above 1.0 supersamples.
pub const DEFAULT_RENDER_SCALE: f32 = 1.0;

// Retail derives its vertical fov from a projection focal length over a fixed
// half-height: fovy = 2*atan2f(192, ProjectionFocalLength)
// (research/XIClient/src/XIClient/source/World/Generator/Effects/CMoElem.cpp:274).
pub const RETAIL_PROJECTION_HALF_HEIGHT: f32 = 192.0;
// research/XIClient/src/XIClient/source/World/Camera/CameraManager.cpp:318:
// SetProjectionFocalLength(350.0f) is the default; cutscene/zoom effects animate it.
pub const RETAIL_DEFAULT_FOCAL_LENGTH: f32 = 350.0;
// = retail_default_fov_deg(); f32::atan is not const fn, so the derived value is
// pinned by the default_fov_derives_from_retail_focal_length guard test.
pub const DEFAULT_FOV_DEG: f32 = 57.495_83;

pub fn retail_default_fov_deg() -> f32 {
    (2.0 * (RETAIL_PROJECTION_HALF_HEIGHT / RETAIL_DEFAULT_FOCAL_LENGTH).atan()).to_degrees()
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
fn default_model_light_count() -> u32 {
    DEFAULT_MODEL_LIGHT_COUNT
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

// wgpu/Bevy require power-of-two DirectionalLightShadowMap sizes; a non-PoT slot
// (e.g. 3072) is silently rounded up by bevy_light, so the menu value would lie.
const SHADOW_MAP_SIZE_SLOTS: &[u32] = &[1024, 2048, 4096];
const SHADOW_CASCADE_COUNT_SLOTS: &[u32] = &[2, 3, 4];
const SHADOW_MAX_DISTANCE_SLOTS: &[f32] = &[100.0, 200.0, 300.0, 700.0, 1100.0];
const BLOOM_SLOTS: &[f32] = &[0.0, 0.04, 0.08, 0.12, 0.16];
const FOG_STEP_SLOTS: &[u32] = &[32, 64, 96, 128];

const VIEW_DISTANCE_SLOTS: &[f32] = &[200.0, 500.0, 700.0, 1100.0, 2300.0, 6100.0];
const FOV_SLOTS: &[f32] = &[
    50.0,
    55.0,
    DEFAULT_FOV_DEG,
    60.0,
    65.0,
    70.0,
    75.0,
    80.0,
    85.0,
    90.0,
    95.0,
    100.0,
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

const DLSS_QUALITY_SLOTS: &[DlssQuality] = &[
    DlssQuality::Auto,
    DlssQuality::Dlaa,
    DlssQuality::Quality,
    DlssQuality::Balanced,
    DlssQuality::Performance,
    DlssQuality::UltraPerformance,
];

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

const DYNAMIC_LIGHTS_CYCLE: &[DynamicLights] = &[
    DynamicLights::Off,
    DynamicLights::Vanilla,
    DynamicLights::Enhanced,
];

const ZONE_LINE_DISPLAY_CYCLE: &[ZoneLineDisplay] = &[
    ZoneLineDisplay::Off,
    ZoneLineDisplay::Pillar,
    ZoneLineDisplay::Gate,
];

const DOF_APERTURE_SLOTS: &[f32] = &[1.4, 2.0, 2.8, 4.0, 5.6, 8.0];

// NR menu slots (RenoDX addon parity): intensity starts at the parser's no-op
// value and one notch above it; tone/structure span off..double.
const NR_INTENSITY_SLOTS: &[f32] = &[0.5, 0.75, 1.0, 1.01, 1.25, 1.5, 2.0];
const NR_TONE_STRUCTURE_SLOTS: &[f32] = &[0.0, 0.5, 1.0, 1.5, 2.0];

// Sub-1.0 downscales the 3D buffer (perf); 1.0 is native; >1.0 supersamples (SSAA).
// 1.0 must stay in this list — it's the default and the no-op path.
// 30 matches retail's native cadence; 0 = Off (framepace Auto). A flat cap
// stabilizes pacing on ProMotion panels (measured 2026-07-31: /fps 60 smoothed
// crowd judder even with vsync headroom above 60).
const FPS_CAP_SLOTS: &[u32] = &[0, 30, 60, 120];

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
                dlss_quality: DlssQuality::Auto,
                neural_uplift: false,
                nr_intensity: default_nr_intensity(),
                nr_local_tone_strength: default_nr_local_tone(),
                nr_structure_strength: default_nr_structure(),
                dlss_supported: false,
                dlss_menu_enabled: false,
                job_display: false,
                mob_hp_under: false,
                texture_filtering: TextureFiltering::Vanilla,
                bloom_intensity: 0.0,
                volumetric_fog: false,
                fog_step_count: 32,
                view_distance: 500.0,
                vsync: true,
                fps_cap: 0,
                fov_deg: DEFAULT_FOV_DEG,
                ui_scale: 1.0,
                camera_spring: false,
                menu_scale: true,
                dynamic_lights: DynamicLights::Vanilla,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                model_light_count: DEFAULT_MODEL_LIGHT_COUNT,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: false,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
                fullscreen: false,
                windowed_fullscreen: false,
            },
            QualityPreset::Medium => Self {
                preset,
                shadow_map_size: 2048,
                shadow_cascade_count: 3,
                shadow_max_distance: 300.0,
                anti_aliasing: aa_default,
                dlss_quality: DlssQuality::Auto,
                neural_uplift: false,
                nr_intensity: default_nr_intensity(),
                nr_local_tone_strength: default_nr_local_tone(),
                nr_structure_strength: default_nr_structure(),
                dlss_supported: false,
                dlss_menu_enabled: false,
                job_display: false,
                mob_hp_under: false,
                texture_filtering: TextureFiltering::Vanilla,
                bloom_intensity: 0.08,
                volumetric_fog: false,
                fog_step_count: 64,
                view_distance: 1100.0,
                vsync: true,
                fps_cap: 0,
                fov_deg: DEFAULT_FOV_DEG,
                ui_scale: 1.0,
                camera_spring: false,
                menu_scale: true,
                dynamic_lights: DynamicLights::Vanilla,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                model_light_count: DEFAULT_MODEL_LIGHT_COUNT,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: false,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
                fullscreen: false,
                windowed_fullscreen: false,
            },
            QualityPreset::High => Self {
                preset,
                shadow_map_size: 4096,
                shadow_cascade_count: 4,
                shadow_max_distance: 700.0,
                anti_aliasing: aa_default,
                dlss_quality: DlssQuality::Auto,
                neural_uplift: false,
                nr_intensity: default_nr_intensity(),
                nr_local_tone_strength: default_nr_local_tone(),
                nr_structure_strength: default_nr_structure(),
                dlss_supported: false,
                dlss_menu_enabled: false,
                job_display: false,
                mob_hp_under: false,
                texture_filtering: TextureFiltering::Aniso4x,
                bloom_intensity: 0.08,
                volumetric_fog: false,
                fog_step_count: 64,
                view_distance: 6100.0,
                vsync: true,
                fps_cap: 0,
                fov_deg: DEFAULT_FOV_DEG,
                ui_scale: 1.0,
                camera_spring: false,
                menu_scale: true,
                dynamic_lights: DynamicLights::Vanilla,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                model_light_count: DEFAULT_MODEL_LIGHT_COUNT,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: true,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
                fullscreen: false,
                windowed_fullscreen: false,
            },
            QualityPreset::Ultra => Self {
                preset,
                shadow_map_size: 4096,
                shadow_cascade_count: 4,
                shadow_max_distance: 1100.0,
                // MSAA8 rather than TAA: TAA needs a motion-vector camera prepass,
                // which forces every zone/character draw through a second geometry
                // pass. No preset should silently pay that — the prepass is opt-in
                // via Depth of Field only. TAA stays available as a manual choice.
                anti_aliasing: AaMode::Msaa8,
                dlss_quality: DlssQuality::Auto,
                neural_uplift: false,
                nr_intensity: default_nr_intensity(),
                nr_local_tone_strength: default_nr_local_tone(),
                nr_structure_strength: default_nr_structure(),
                dlss_supported: false,
                dlss_menu_enabled: false,
                job_display: false,
                mob_hp_under: false,
                texture_filtering: TextureFiltering::Aniso8x,
                bloom_intensity: 0.12,
                volumetric_fog: false,
                fog_step_count: 96,
                view_distance: 6100.0,
                vsync: true,
                fps_cap: 0,
                fov_deg: DEFAULT_FOV_DEG,
                ui_scale: 1.0,
                camera_spring: false,
                menu_scale: true,
                dynamic_lights: DynamicLights::Vanilla,
                light_threshold: DEFAULT_LIGHT_THRESHOLD,
                light_intensity: DEFAULT_LIGHT_INTENSITY,
                light_range: DEFAULT_LIGHT_RANGE,
                light_flicker: DEFAULT_LIGHT_FLICKER,
                model_light_count: DEFAULT_MODEL_LIGHT_COUNT,
                character_render_path: CharacterRenderPath::FfxiFaithful,
                realistic_character_lighting: false,
                faithful_shadow_receive: true,
                character_shadow_cast: true,
                depth_of_field: false,
                dof_aperture_f_stops: DEFAULT_DOF_APERTURE,
                zone_line_display: ZoneLineDisplay::Off,
                render_scale: DEFAULT_RENDER_SCALE,
                fullscreen: false,
                windowed_fullscreen: false,
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
            GraphicsField::AntiAliasing => {
                // A json written with DLSS on can land us on Dlss while this
                // machine/build can't run it — or the Retail+ gate is closed;
                // say so instead of a bare "DLSS".
                if matches!(self.anti_aliasing, AaMode::Dlss) && !self.dlss_selectable() {
                    "DLSS (N/A)".to_string()
                } else {
                    self.anti_aliasing.label().to_string()
                }
            }
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
            GraphicsField::Fullscreen => bool_label(self.fullscreen).into(),
            GraphicsField::Windowed => bool_label(self.windowed_fullscreen).into(),
            GraphicsField::FrameRateCap => match self.fps_cap {
                0 => "Off".into(),
                n => format!("{n} fps"),
            },
            GraphicsField::Fov => format!("{:.0}°", self.fov_deg),
            GraphicsField::UiScale => format!("{:.0}%", self.ui_scale * 100.0),
            GraphicsField::CameraSpring => {
                (if self.camera_spring { "on" } else { "off" }).to_string()
            }
            GraphicsField::MenuScale => (if self.menu_scale { "on" } else { "off" }).to_string(),

            GraphicsField::DynamicLights => {
                if self.dynamic_lights == DynamicLights::Enhanced && !self.lights_fine_is_default()
                {
                    "Custom".to_string()
                } else {
                    self.dynamic_lights.label().to_string()
                }
            }
            GraphicsField::LightThreshold => format!("{:.2}", self.light_threshold),
            GraphicsField::LightIntensity => format!("{:.0}", self.light_intensity),
            GraphicsField::LightRange => format!("{:.0}m", self.light_range),
            GraphicsField::LightFlicker => bool_label(self.light_flicker).into(),
            GraphicsField::ModelLightCount => format!("{}", self.model_light_count),

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
            GraphicsField::RenderScale => {
                if self.dlss_active() {
                    // DLSS owns internal resolution (the quality tier picks
                    // it); the manual scale is parked until DLSS is off.
                    "DLSS".to_string()
                } else {
                    format!("{:.0}%", self.render_scale * 100.0)
                }
            }
            GraphicsField::Dlss => {
                // N/A while the runtime can't run it OR the Retail+ gate is
                // closed (the default) — one dlss build serves both audiences.
                if !self.dlss_selectable() {
                    "N/A".to_string()
                } else if matches!(self.anti_aliasing, AaMode::Dlss) {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            GraphicsField::DlssQuality => {
                if self.dlss_selectable() {
                    self.dlss_quality.label().to_string()
                } else {
                    "N/A".to_string()
                }
            }
            // Live NR rows: N/A while not selectable, values otherwise (the
            // knobs stay adjustable before the toggle is flipped on).
            GraphicsField::DlssNeuralUplift => {
                if !self.dlss_selectable() {
                    "N/A".to_string()
                } else {
                    bool_label(self.neural_uplift).into()
                }
            }
            GraphicsField::DlssNrIntensity => {
                if self.dlss_selectable() {
                    format!("{:.2}", self.nr_intensity)
                } else {
                    "N/A".to_string()
                }
            }
            GraphicsField::DlssNrLocalTone => {
                if self.dlss_selectable() {
                    format!("{:.2}", self.nr_local_tone_strength)
                } else {
                    "N/A".to_string()
                }
            }
            GraphicsField::DlssNrStructure => {
                if self.dlss_selectable() {
                    format!("{:.2}", self.nr_structure_strength)
                } else {
                    "N/A".to_string()
                }
            }
            // Inert placeholders: grayed on every build (see is_dlss_placeholder).
            GraphicsField::DlssRrPreset
            | GraphicsField::DlssSrPreset
            | GraphicsField::DlssRrResponsivity
            | GraphicsField::DlssSharpness => "N/A".to_string(),
        }
    }

    pub fn cycle(&mut self, field: GraphicsField, delta: i32) {
        match field {
            GraphicsField::Preset => {
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
                let vsync = self.vsync;
                let fps_cap = self.fps_cap;
                // Presets never own DLSS (kuluu decision, 2026-09): no preset
                // turns it on, and picking a preset doesn't turn it off. The
                // preset's own anti_aliasing applies only when DLSS wasn't the
                // active mode going in — same carry-over class as VSync.
                let was_dlss = matches!(self.anti_aliasing, AaMode::Dlss);
                let dlss_quality = self.dlss_quality;
                let dlss_supported = self.dlss_supported;
                // Retail+ gates are user choices, not preset-owned.
                let (dlss_menu_enabled, job_display, mob_hp_under) =
                    (self.dlss_menu_enabled, self.job_display, self.mob_hp_under);
                // NR is DLSS-family: presets never own it either.
                let nr = (
                    self.neural_uplift,
                    self.nr_intensity,
                    self.nr_local_tone_strength,
                    self.nr_structure_strength,
                );
                let next =
                    cycle_slot(self.preset, PRESET_CYCLE, delta).unwrap_or(QualityPreset::High);
                *self = Self::for_preset(next);
                self.dynamic_lights = lights;
                self.light_threshold = lt;
                self.light_intensity = li;
                self.light_range = lr;
                self.light_flicker = lf;
                self.realistic_character_lighting = realistic;
                self.faithful_shadow_receive = receive;
                self.zone_line_display = zld;
                self.vsync = vsync;
                self.fps_cap = fps_cap;
                self.dlss_quality = dlss_quality;
                self.dlss_supported = dlss_supported;
                self.dlss_menu_enabled = dlss_menu_enabled;
                self.job_display = job_display;
                self.mob_hp_under = mob_hp_under;
                (
                    self.neural_uplift,
                    self.nr_intensity,
                    self.nr_local_tone_strength,
                    self.nr_structure_strength,
                ) = nr;
                if was_dlss {
                    self.anti_aliasing = AaMode::Dlss;
                }
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
                // DLSS is NOT a cycler slot (user decision 2026-10): the
                // explicit DLSS on/off row owns that transition. When a json
                // lands us on Dlss, the current value isn't in AA_SLOTS;
                // cycle_slot treats an unknown current as slot 0, so one click
                // lands on a real mode — cycling away always works.
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
            }
            GraphicsField::Fullscreen => {
                // A quality preset shouldn't be reset just because the user
                // toggled fullscreen — this is a display preference, not a
                // preset-influencing knob. Same treatment as VSync.
                self.fullscreen = !self.fullscreen;
            }
            GraphicsField::Windowed => {
                // Chooses borderless (windowed) fullscreen vs exclusive. Only
                // has a visible effect while Fullscreen is on; when windowed
                // the flag is stored but the window stays a normal window.
                self.windowed_fullscreen = !self.windowed_fullscreen;
            }
            GraphicsField::FrameRateCap => {
                self.fps_cap = cycle_slot_u32(self.fps_cap, FPS_CAP_SLOTS, delta);
            }
            GraphicsField::CameraSpring => {
                if delta != 0 {
                    self.camera_spring = !self.camera_spring;
                }
            }
            GraphicsField::MenuScale => {
                if delta != 0 {
                    self.menu_scale = !self.menu_scale;
                }
            }
            GraphicsField::UiScale => {
                self.ui_scale = cycle_slot_f32(
                    self.ui_scale,
                    &[0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0],
                    delta,
                );
            }
            GraphicsField::Fov => {
                self.fov_deg = cycle_slot_f32(self.fov_deg, FOV_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::DynamicLights => {
                self.dynamic_lights = cycle_slot(self.dynamic_lights, DYNAMIC_LIGHTS_CYCLE, delta)
                    .unwrap_or(DynamicLights::Vanilla);
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
            GraphicsField::ModelLightCount => {
                self.model_light_count =
                    cycle_slot_u32(self.model_light_count, MODEL_LIGHT_COUNT_SLOTS, delta);
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
                // DLSS owns internal resolution while active; the row reads
                // "DLSS" (value_label) and refuses to move so the stored scale
                // can't silently drift under it.
                if self.dlss_active() {
                    return;
                }
                self.render_scale = cycle_slot_f32(self.render_scale, RENDER_SCALE_SLOTS, delta);
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::Dlss => {
                // On/Off mirror of anti_aliasing == Dlss. Refuses while not
                // selectable (the row reads "N/A" — runtime unsupported OR the
                // Retail+ gate closed), so both menu surfaces get the gray-out
                // from this one spot. Turning DLSS off lands on AA Off — the
                // user re-picks MSAA/TAA in the cycler if wanted.
                if !self.dlss_selectable() {
                    return;
                }
                self.anti_aliasing = if matches!(self.anti_aliasing, AaMode::Dlss) {
                    AaMode::Off
                } else {
                    AaMode::Dlss
                };
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::DlssQuality => {
                if !self.dlss_selectable() {
                    return;
                }
                self.dlss_quality = cycle_slot(self.dlss_quality, DLSS_QUALITY_SLOTS, delta)
                    .unwrap_or(DlssQuality::Auto);
                // A quality-tier change flows into the live camera via the AA
                // respawn key (apply_anti_aliasing_system); no preset reset —
                // like VSync, this is a display/perf preference, and presets
                // never own DLSS state.
            }
            // Live NR rows: refuse while not selectable (the row reads "N/A").
            GraphicsField::DlssNeuralUplift => {
                if !self.dlss_selectable() {
                    return;
                }
                self.neural_uplift = !self.neural_uplift;
                self.preset = QualityPreset::Custom;
            }
            GraphicsField::DlssNrIntensity => {
                if !self.dlss_selectable() {
                    return;
                }
                self.nr_intensity = cycle_slot_f32(self.nr_intensity, NR_INTENSITY_SLOTS, delta);
            }
            GraphicsField::DlssNrLocalTone => {
                if !self.dlss_selectable() {
                    return;
                }
                self.nr_local_tone_strength =
                    cycle_slot_f32(self.nr_local_tone_strength, NR_TONE_STRUCTURE_SLOTS, delta);
            }
            GraphicsField::DlssNrStructure => {
                if !self.dlss_selectable() {
                    return;
                }
                self.nr_structure_strength =
                    cycle_slot_f32(self.nr_structure_strength, NR_TONE_STRUCTURE_SLOTS, delta);
            }
            // Inert placeholders: nothing behind them yet, cycling is a no-op
            // on every build (the row reads "N/A").
            GraphicsField::DlssRrPreset
            | GraphicsField::DlssSrPreset
            | GraphicsField::DlssRrResponsivity
            | GraphicsField::DlssSharpness => {}
        }
    }

    pub fn reset_to_default(&mut self) {
        // Capability is runtime-detected, not a preference: a menu reset must
        // not un-detect DLSS support (the availability system only writes it
        // once at startup). The Retail+ gates are user choices too — a reset
        // returns quality knobs to High but keeps the menu/Job/Mob-HP decisions.
        let dlss_supported = self.dlss_supported;
        let dlss_menu_enabled = self.dlss_menu_enabled;
        let job_display = self.job_display;
        let mob_hp_under = self.mob_hp_under;
        *self = Self::for_preset(QualityPreset::High);
        self.dlss_supported = dlss_supported;
        self.dlss_menu_enabled = dlss_menu_enabled;
        self.job_display = job_display;
        self.mob_hp_under = mob_hp_under;
    }

    /// Reset only the DLSS Config surface: quality back to Auto, NR toggle off
    /// with its knobs back to defaults. The inert placeholders have no state
    /// to reset; SR on/off (the AA mode) is a main-list concern and stays put.
    pub fn reset_dlss_config(&mut self) {
        self.dlss_quality = DlssQuality::Auto;
        self.neural_uplift = false;
        self.nr_intensity = default_nr_intensity();
        self.nr_local_tone_strength = default_nr_local_tone();
        self.nr_structure_strength = default_nr_structure();
    }

    // Retail outputs colour directly with no filmic tonemap; a filmic curve desaturates
    // and flattens the authored palette.
    pub fn tonemapping(&self) -> Tonemapping {
        Tonemapping::None
    }

    fn lights_fine_is_default(&self) -> bool {
        (self.light_threshold - DEFAULT_LIGHT_THRESHOLD).abs() < 1e-3
            && (self.light_intensity - DEFAULT_LIGHT_INTENSITY).abs() < 1.0
            && (self.light_range - DEFAULT_LIGHT_RANGE).abs() < 1e-3
            && self.light_flicker == DEFAULT_LIGHT_FLICKER
    }

    pub fn msaa(&self) -> Msaa {
        match self.anti_aliasing {
            // Dlss: the DLSS pass is the anti-aliasing — multisampling under it
            // would burn fill for samples the upscaler ignores. (When Dlss is
            // set but unsupported this also means no AA, which the menu makes
            // visible as "DLSS (N/A)" so the user knows to cycle away.)
            AaMode::Off | AaMode::Taa | AaMode::Dlss => Msaa::Off,
            AaMode::Msaa2 => Msaa::Sample2,
            AaMode::Msaa4 => Msaa::Sample4,
            AaMode::Msaa8 => Msaa::Sample8,
        }
    }

    pub fn wants_taa(&self) -> bool {
        matches!(self.anti_aliasing, AaMode::Taa)
    }

    /// DLSS is usable in the menu: runtime-capable AND the Retail+ gate is
    /// open. Every user-facing DLSS surface keys off this — with the gate off
    /// (the default) the rows read N/A and refuse to cycle even on capable
    /// machines, which is what lets one dlss build serve both audiences.
    pub fn dlss_selectable(&self) -> bool {
        self.dlss_menu_enabled && self.dlss_supported
    }

    /// DLSS is chosen AND this build/machine can actually run it AND the
    /// Retail+ gate is open. The single gate every consumer keys off (camera
    /// respawn, render-scale composite, nameplate pass): intent without a
    /// working runtime — or with the menu gate closed — is always a no-op, so
    /// a dlss-build json loaded on a default build changes nothing.
    pub fn dlss_active(&self) -> bool {
        matches!(self.anti_aliasing, AaMode::Dlss) && self.dlss_selectable()
    }

    /// Neural Uplift (NR) is toggled AND DLSS itself is active. The gate for the
    /// NR pipeline (graphics/dlss_nr.rs): NR is a DLSS-family post effect, so it
    /// stands down entirely whenever the AA mode leaves Dlss — cycling to MSAA or
    /// TAA must stop evaluating, not just change what SR feeds it. Still
    /// independent of the quality tier: any DlssQuality (incl. Dlaa) keeps NR on.
    pub fn nr_active(&self) -> bool {
        self.neural_uplift && self.dlss_active()
    }

    /// Clamped render-scale factor (3D-buffer resolution ÷ window resolution).
    pub fn render_scale(&self) -> f32 {
        self.render_scale.clamp(0.25, 2.0)
    }

    /// True when the 3D buffer should be rendered off-window and (up/down)scaled.
    /// At exactly 1.0 the camera renders straight to the window (no extra passes).
    /// Always false while DLSS is active: DLSS owns internal resolution and
    /// upscaling, so the manual composite path must stand down or the frame
    /// gets scaled twice. This is the single gate every render-scale system
    /// keys off, so returning false here tears the composite down and blocks
    /// the pointer remap in one place.
    pub fn wants_render_scale(&self) -> bool {
        !self.dlss_active() && (self.render_scale() - 1.0).abs() > 1e-3
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

// Grouped: display -> interface/camera -> quality -> lighting.
pub const GRAPHICS_FIELDS: &[GraphicsField] = &[
    GraphicsField::Preset,
    GraphicsField::Fullscreen,
    GraphicsField::Windowed,
    GraphicsField::VSync,
    GraphicsField::FrameRateCap,
    GraphicsField::RenderScale,
    GraphicsField::Fov,
    GraphicsField::UiScale,
    GraphicsField::MenuScale,
    GraphicsField::CameraSpring,
    GraphicsField::AntiAliasing,
    GraphicsField::Dlss,
    GraphicsField::TextureFiltering,
    GraphicsField::ShadowMapSize,
    GraphicsField::ShadowCascadeCount,
    GraphicsField::ShadowMaxDistance,
    GraphicsField::BloomIntensity,
    GraphicsField::VolumetricFog,
    GraphicsField::FogStepCount,
    GraphicsField::ViewDistance,
    GraphicsField::DepthOfField,
    GraphicsField::DofAperture,
    GraphicsField::ZoneLineDisplay,
    GraphicsField::DynamicLights,
    GraphicsField::LightThreshold,
    GraphicsField::LightIntensity,
    GraphicsField::LightRange,
    GraphicsField::LightFlicker,
    GraphicsField::ModelLightCount,
    GraphicsField::CharacterLighting,
    GraphicsField::CharacterShadowReceive,
    GraphicsField::CharacterShadowCast,
];

/// The DLSS Config surface, top to bottom: the live quality knob first, then
/// the inert RenoDX-parity placeholders (see `is_dlss_placeholder`). Rendered
/// as a pushed submenu in-game and a disclosure block in the launcher.
pub const DLSS_CONFIG_FIELDS: &[GraphicsField] = &[
    GraphicsField::DlssQuality,
    GraphicsField::DlssRrPreset,
    GraphicsField::DlssSrPreset,
    GraphicsField::DlssRrResponsivity,
    GraphicsField::DlssNeuralUplift,
    GraphicsField::DlssNrIntensity,
    GraphicsField::DlssNrLocalTone,
    GraphicsField::DlssNrStructure,
    GraphicsField::DlssSharpness,
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
    mut last_applied: Local<Option<(Msaa, bool, bool, bool, DlssQuality)>>,
) {
    let target_msaa = caps
        .map(|c| c.clamp(settings.msaa()))
        .unwrap_or_else(|| settings.msaa());
    let want_taa = settings.wants_taa();
    // Both DLSS knobs are respawn-keyed: toggling adds/removes the camera's
    // Dlss component, and a quality-tier change goes through a full respawn
    // too — a fresh view entity is guaranteed to re-create the DLSS context
    // at the new internal resolution, where mutating a live component leans
    // on bevy's prepare-side re-creation that we can't compile-verify here.
    // One extra respawn per menu click is cheap; document as a possible
    // in-place optimization once a dlss build is in hand (docs/DLSS.md).
    let want_dlss = settings.dlss_active();
    let dlss_quality = settings.dlss_quality;
    // volumetric_fog is part of the respawn key because bevy's
    // `extract_volumetric_fog` is insert-only into the persistent render-world
    // view entity (its cleanup path only runs when *no* `VolumetricLight`
    // exists, and our sun/moon always carry one — see the `TODO: needs better
    // way to handle clean up` in bevy_pbr::volumetric_fog::render). Removing
    // `VolumetricFog` from a live camera therefore leaves fog rendering
    // forever; despawning the camera and rebuilding it is the only reliable
    // way to toggle it at runtime.
    let next = (
        target_msaa,
        want_taa,
        settings.volumetric_fog,
        want_dlss,
        dlss_quality,
    );

    if *last_applied == Some(next) {
        return;
    }

    let Ok((entity, transform)) = q_cam.single() else {
        return;
    };

    commands.entity(entity).despawn();
    let mut settings_for_respawn = settings.clone();
    // With DLSS active, keep Dlss as the respawn AA mode (msaa() already
    // reported Off for it, so the reconstruction below would clobber it to
    // AaMode::Off and the camera would come back without its Dlss component).
    let aa = if want_dlss {
        AaMode::Dlss
    } else if want_taa {
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

/// Owns `VolumetricFog` presence + quality (step count) on the operator
/// camera: inserts when the toggle is on and the component is missing (e.g.
/// right after a camera respawn), removes it when toggled off, and keeps
/// `step_count` in sync. Toggle-*off* additionally relies on the camera
/// respawn in [`apply_anti_aliasing_system`] (which keys on
/// `settings.volumetric_fog`), because bevy's render-world extraction is
/// insert-only and a bare `remove` leaves stale render-world state. Ambient
/// color/intensity are owned by `crate::weather::apply_zone_weather`, which
/// derives them from the zone fog DAT and time of day.
pub fn apply_volumetric_fog_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    mut q_cam: Query<(Entity, Option<&mut VolumetricFog>), With<OperatorCamera>>,
) {
    for (entity, fog) in q_cam.iter_mut() {
        match (settings.volumetric_fog, fog) {
            (true, Some(mut fog)) => {
                // Only own the quality knob here; ambient_color/intensity are
                // zone/time/weather-derived in weather::apply_zone_weather and
                // must survive settings changes.
                if fog.step_count != settings.fog_step_count {
                    fog.step_count = settings.fog_step_count;
                }
            }
            (true, None) => {
                // Ambient values are placeholders; apply_zone_weather rewrites
                // them from the zone DAT + day/night curve every frame.
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
            p.far = crate::skybox::camera_far(settings.view_distance);
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

/// Reflects `GraphicsSettings::fullscreen` onto the primary window's mode.
/// The initial window mode is chosen at startup (see `view_native/mod.rs`)
/// from either `FFXI_FULLSCREEN` or the persisted `fullscreen` field; this
/// system is what makes an in-game toggle actually change the window without
/// restart.
pub fn apply_fullscreen_system(
    settings: Res<GraphicsSettings>,
    mut q_window: Query<&mut Window, With<PrimaryWindow>>,
) {
    use bevy::window::{MonitorSelection, VideoModeSelection, WindowMode};
    for mut window in q_window.iter_mut() {
        // Three states:
        //   !fullscreen                        -> Windowed (a normal window)
        //   fullscreen && !windowed_fullscreen -> exclusive/true Fullscreen
        //   fullscreen &&  windowed_fullscreen -> borderless windowed-fullscreen
        // The Windowed row toggles the last bit and does nothing visible while
        // we're in plain Windowed mode.
        let target = if !settings.fullscreen {
            WindowMode::Windowed
        } else if settings.windowed_fullscreen {
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
        } else {
            WindowMode::Fullscreen(MonitorSelection::Primary, VideoModeSelection::Current)
        };
        // Compare by variant so re-asserting the same mode doesn't trigger a
        // redundant swap-chain rebuild (the fullscreen variants carry selection
        // payloads that don't derive PartialEq cleanly).
        let same = matches!(
            (&window.mode, &target),
            (WindowMode::Windowed, WindowMode::Windowed)
                | (
                    WindowMode::BorderlessFullscreen(_),
                    WindowMode::BorderlessFullscreen(_)
                )
                | (WindowMode::Fullscreen(_, _), WindowMode::Fullscreen(_, _))
        );
        if !same {
            window.mode = target;
        }
    }
}

pub fn apply_tonemapping_system(
    settings: Res<GraphicsSettings>,
    mut q_cam: Query<&mut Tonemapping, With<OperatorCamera>>,
) {
    let want = settings.tonemapping();
    for mut tm in q_cam.iter_mut() {
        if *tm != want {
            *tm = want;
        }
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
/// depth: Depth of Field, and TAA (which also requires it via
/// `#[require(DepthPrepass, …)]`, so we never strip it while TAA is on). The
/// Vanilla sun flare occludes via a CPU BVH raycast (`lens_flare::SunOcclusion`)
/// and needs no prepass. Runs every frame (not just on settings change) so the
/// `match` self-heals across the AA camera respawn.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_camera_prepass_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    q_cam: Query<(Entity, Option<&DepthPrepass>), With<OperatorCamera>>,
) {
    let Ok((entity, depth)) = q_cam.single() else {
        return;
    };
    // NR needs the prepass depth too (when single-sampled), so it keeps the
    // DepthPrepass alive even with SR off and no other depth consumer.
    let keep_depth = settings.depth_of_field
        || settings.wants_taa()
        || settings.dlss_active()
        || settings.nr_active();
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

/// Resolution-relative HUD scaling: UiScale = (logical height / 1080) x the
/// user's UI Scale setting, so panels authored against a 1080p baseline grow
/// and shrink with the window. Reacts to WindowResized events the same tick
/// they arrive, and marks UiScale mutated even when the numeric value is
/// unchanged so bevy_ui reflows on layout-affecting changes.
pub fn apply_ui_scale_system(
    settings: Res<GraphicsSettings>,
    windows: bevy::ecs::system::Query<
        &bevy::window::Window,
        bevy::ecs::query::With<bevy::window::PrimaryWindow>,
    >,
    mut ui: ResMut<bevy::ui::UiScale>,
    mut prev_size: bevy::ecs::system::Local<bevy::math::UVec2>,
) {
    let Ok(w) = windows.single() else {
        return;
    };
    // Frame-over-frame check on BOTH physical axes. A drag on either edge
    // must trigger relayout: width-only drags feed percent-sized nodes and
    // pane offsets, height-only drags feed the auto-scale ratio. Physical
    // (not logical) size updates the same frame the drag lands.
    let ph = w.physical_size();
    let resize_fired = *prev_size != ph;
    *prev_size = ph;
    // WHOLE-NUMBER EFFECTIVE SCALE, driven by the SMALLER axis so panels
    // authored against a 1080p landscape frame don't overflow when the
    // window is wider-than-tall in a way that makes the height-only
    // multiplier too small (or vice versa when very tall/narrow). Compare
    // both axes against their 1080p/1920p baselines and take the min:
    // whichever axis is tightest sets the fit.
    let logical = bevy::math::Vec2::new(w.width(), w.height());
    let auto_h = (logical.y / 1080.0).max(0.1);
    let auto_w = (logical.x / 1920.0).max(0.1);
    let auto_raw = auto_h.min(auto_w);
    // SMOOTH fractional scale (standard reference-resolution model, per
    // Unity CanvasScaler et al). Vector UI with text re-rasterized at final
    // size handles fractional factors fine; the earlier quarter/integer
    // quantization was a pixel-art technique misapplied to vector UI and
    // caused the 50%/75% collapse. Floored so scale can never reach zero.
    let want = (auto_raw * settings.ui_scale).max(0.25);
    let changed = (ui.0 - want).abs() > 0.001;
    if changed {
        ui.0 = want;
    } else if resize_fired {
        // Same scale value at the new size: still touch UiScale so bevy_ui
        // re-samples the viewport extent for percent-sized nodes. NOTE:
        // deliberately NO blanket per-node dirtying here. The earlier
        // all-nodes set_changed() sledgehammer forced full menu rebuilds on
        // every settings change, which reset the menu cursor mid-input and
        // let stray presses land on rows the user never selected (the
        // accidental TAA flip). Retained-mode rule: value changes repaint
        // row text; they never rebuild the tree.
        ui.set_changed();
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
        assert!((s.shadow_max_distance - 700.0).abs() < 1e-6);
    }

    #[test]
    fn shadow_map_size_slots_are_power_of_two() {
        for &size in SHADOW_MAP_SIZE_SLOTS {
            assert!(
                size.is_power_of_two(),
                "shadow_map_size slot {size} is not a power of two; bevy_light rounds it up"
            );
        }
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
    fn default_fov_derives_from_retail_focal_length() {
        assert!(
            (DEFAULT_FOV_DEG - retail_default_fov_deg()).abs() < 1e-3,
            "DEFAULT_FOV_DEG {} != 2*atan({}/{}) = {}",
            DEFAULT_FOV_DEG,
            RETAIL_PROJECTION_HALF_HEIGHT,
            RETAIL_DEFAULT_FOCAL_LENGTH,
            retail_default_fov_deg()
        );
        assert!(FOV_SLOTS.iter().any(|x| (x - DEFAULT_FOV_DEG).abs() < 1e-3));
    }

    #[test]
    fn json_roundtrip_preserves_all_fields() {
        let s = GraphicsSettings::for_preset(QualityPreset::Ultra);
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicsSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn legacy_dynamic_light_count_key_still_loads() {
        let s = GraphicsSettings {
            model_light_count: 16,
            ..GraphicsSettings::default()
        };
        let legacy = serde_json::to_string(&s)
            .unwrap()
            .replace("\"model_light_count\"", "\"dynamic_light_count\"");
        assert!(
            legacy.contains("dynamic_light_count"),
            "the pre-rename key name must actually be present for this to test anything"
        );

        let back: GraphicsSettings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(
            back.model_light_count, 16,
            "a graphics.json written before the rename must keep the player's value"
        );
    }

    #[test]
    fn cycling_a_lever_marks_preset_custom() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.preset, QualityPreset::High);
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.preset, QualityPreset::Custom);

        let mut s = GraphicsSettings::default();
        assert!(!s.volumetric_fog, "no retail light shafts at any tier");
        s.cycle(GraphicsField::VolumetricFog, 1);
        assert_eq!(s.preset, QualityPreset::Custom);
        assert!(s.volumetric_fog, "toggled on");
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
    fn sky_and_water_have_no_style_fork() {
        // The Enhanced sky/water variants were removed: the retail-faithful path is
        // the only path, so no field may reintroduce a user-selectable fork.
        assert!(!GRAPHICS_FIELDS
            .iter()
            .any(|f| f.label().contains("Sky") || f.label().contains("Water")));
        assert_eq!(GraphicsSettings::default().tonemapping(), Tonemapping::None);
    }

    #[test]
    fn settings_saved_before_the_sky_water_removal_still_load() {
        // GraphicsSettings has no deny_unknown_fields, so a user's existing
        // graphics.json keeps loading after a field is dropped. Pin that.
        let current = serde_json::to_string(&GraphicsSettings::default()).unwrap();
        let legacy = current.replacen('{', r#"{"sky_style":"Enhanced","enhanced_water":true,"#, 1);
        let s: GraphicsSettings =
            serde_json::from_str(&legacy).expect("legacy graphics.json loads");
        assert_eq!(s, GraphicsSettings::default());
    }

    #[test]
    fn tuning_a_light_knob_marks_custom_only_in_enhanced() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Vanilla");
        s.cycle(GraphicsField::LightIntensity, 1);
        assert_eq!(
            s.value_label(GraphicsField::DynamicLights),
            "Vanilla",
            "emitter knobs are inert in Vanilla, so the mode label must not read Custom"
        );
        assert_eq!(s.preset, QualityPreset::High, "light knob ⟂ quality tier");

        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Enhanced);
        assert!(
            s.lights_fine_is_default(),
            "mode cycle reset the fine knobs"
        );
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Enhanced");

        s.cycle(GraphicsField::LightIntensity, 1);
        assert_eq!(s.value_label(GraphicsField::DynamicLights), "Custom");
        assert_eq!(
            s.dynamic_lights,
            DynamicLights::Enhanced,
            "mode unchanged by knob tuning"
        );
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
    fn fps_cap_cycles_slots_and_stays_preset_orthogonal() {
        let mut s = GraphicsSettings::default();
        let tier = s.preset;
        assert_eq!(s.value_label(GraphicsField::FrameRateCap), "Off");
        s.cycle(GraphicsField::FrameRateCap, 1);
        assert_eq!(s.fps_cap, 30);
        assert_eq!(s.value_label(GraphicsField::FrameRateCap), "30 fps");
        assert_eq!(s.preset, tier, "fps cap is preset-orthogonal");
        s.cycle(GraphicsField::FrameRateCap, -1);
        assert_eq!(s.fps_cap, 0, "cycle wraps back to Off");
        s.fps_cap = 60;
        s.cycle(GraphicsField::Preset, 1);
        assert_eq!(s.fps_cap, 60, "preset cycle kept the cap");
    }

    #[test]
    fn preset_cycle_preserves_vsync_and_vsync_keeps_preset() {
        let mut s = GraphicsSettings::default();
        let tier = s.preset;
        s.cycle(GraphicsField::VSync, 1);
        assert!(!s.vsync);
        assert_eq!(s.preset, tier, "vsync is preset-orthogonal");
        s.cycle(GraphicsField::Preset, 1);
        assert!(!s.vsync, "preset cycle kept vsync off");
    }

    #[test]
    fn dynamic_lights_cycles_off_vanilla_enhanced() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.dynamic_lights, DynamicLights::Vanilla);
        assert!(s.dynamic_lights.faithful_enabled());
        assert!(!s.dynamic_lights.emitters_enabled());

        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Enhanced);
        assert_eq!(s.preset, QualityPreset::High, "lights must not flip preset");
        assert!(s.dynamic_lights.faithful_enabled());
        assert!(s.dynamic_lights.emitters_enabled());

        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Off, "wrapped");
        assert!(!s.dynamic_lights.faithful_enabled());
        assert!(!s.dynamic_lights.emitters_enabled());

        s.cycle(GraphicsField::DynamicLights, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Vanilla, "full cycle");
    }

    #[test]
    fn preset_cycle_preserves_dynamic_lights() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DynamicLights, -1);
        assert_eq!(s.dynamic_lights, DynamicLights::Off);
        s.cycle(GraphicsField::Preset, 1);
        assert_eq!(s.dynamic_lights, DynamicLights::Off, "preset cycle kept it");
    }

    #[test]
    fn presets_pin_dynamic_lights_vanilla() {
        for &preset in PRESET_CYCLE {
            assert_eq!(
                GraphicsSettings::for_preset(preset).dynamic_lights,
                DynamicLights::Vanilla,
                "preset {preset:?} must pin the faithful-only light mode"
            );
        }
    }

    #[test]
    fn legacy_dynamic_lights_strings_load_as_vanilla() {
        for legacy in ["\"Few\"", "\"Many\""] {
            let v: DynamicLights = serde_json::from_str(legacy).unwrap();
            assert_eq!(v, DynamicLights::Vanilla, "legacy {legacy} maps to Vanilla");
        }
        for (json, want) in [
            ("\"Off\"", DynamicLights::Off),
            ("\"Vanilla\"", DynamicLights::Vanilla),
            ("\"Enhanced\"", DynamicLights::Enhanced),
        ] {
            let v: DynamicLights = serde_json::from_str(json).unwrap();
            assert_eq!(v, want);
            assert_eq!(serde_json::to_string(&v).unwrap(), json, "roundtrip");
        }
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        let mut s = GraphicsSettings::default();

        // Default (High) sits on the top slot (4096), so +1 wraps to the bottom
        // and -1 wraps back to the top.
        s.cycle(GraphicsField::ShadowMapSize, 1);
        assert_eq!(s.shadow_map_size, 1024, "wrapped past 4096");
        s.cycle(GraphicsField::ShadowMapSize, -1);
        assert_eq!(s.shadow_map_size, 4096, "wrapped back");
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
            ui_scale: 1.0,
            camera_spring: false,
            menu_scale: true,
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
    fn model_shadow_casting_follows_preset_tier() {
        assert!(!GraphicsSettings::for_preset(QualityPreset::Low).character_shadow_cast);
        assert!(!GraphicsSettings::for_preset(QualityPreset::Medium).character_shadow_cast);
        assert!(GraphicsSettings::for_preset(QualityPreset::High).character_shadow_cast);
        assert!(GraphicsSettings::for_preset(QualityPreset::Ultra).character_shadow_cast);
        assert!(GraphicsSettings::default().character_shadow_cast);
    }

    #[test]
    fn model_shadow_casting_is_quality_lever_tied_to_tier() {
        let mut s = GraphicsSettings::default(); // High -> casting on
        assert_eq!(s.value_label(GraphicsField::CharacterShadowCast), "On");
        s.cycle(GraphicsField::CharacterShadowCast, 1);
        assert!(!s.character_shadow_cast);
        assert_eq!(s.value_label(GraphicsField::CharacterShadowCast), "Off");
        assert_eq!(s.preset, QualityPreset::Custom, "casting is a quality knob");

        // Unlike shadow receipt (orthogonal/sticky), casting tracks the tier: a
        // preset change resets it to that tier's default, not the toggled value.
        s.cycle(GraphicsField::Preset, -1); // Custom -> Ultra (tier default On)
        assert_eq!(s.preset, QualityPreset::Ultra);
        assert!(
            s.character_shadow_cast,
            "preset cycle reset casting to the Ultra tier default, not the toggled-off value"
        );
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
        // Depth of Field and TAA are the only prepass forcers (the Vanilla sun
        // flare occludes via CPU raycast); no preset turns either on, so
        // steady-state presets pay zero prepass.
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
    fn depth_of_field_flips_the_quality_tier() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DepthOfField, 1);
        assert!(s.depth_of_field);
        assert_eq!(s.preset, QualityPreset::Custom, "DoF is a quality knob");
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
    fn preset_cycle_resets_dof_to_the_tier_default() {
        let mut s = GraphicsSettings::default();
        s.cycle(GraphicsField::DepthOfField, 1); // on -> Custom

        s.cycle(GraphicsField::Preset, 1); // Custom -> Medium
        assert_eq!(s.preset, QualityPreset::Medium);
        assert!(
            !s.depth_of_field,
            "preset cycle reset DoF to the tier default"
        );
    }

    #[test]
    fn advanced_fields_are_exactly_the_indented_knobs() {
        let advanced: Vec<_> = GRAPHICS_FIELDS
            .iter()
            .copied()
            .filter(|f| f.is_advanced())
            .collect();
        // The 5 dynamic-light tuning knobs (threshold/intensity/range/flicker/count).
        assert_eq!(advanced.len(), 5, "advanced set drifted: {advanced:?}");
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
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicsSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn dlss_rows_refuse_while_unsupported() {
        let mut s = GraphicsSettings::default();
        assert!(!s.dlss_supported, "capability starts undetected");
        assert_eq!(s.value_label(GraphicsField::Dlss), "N/A");
        assert_eq!(s.value_label(GraphicsField::DlssQuality), "N/A");

        // Toggling and quality-cycling are no-ops while unsupported, and the
        // no-op must not dirty the preset either.
        s.cycle(GraphicsField::Dlss, 1);
        assert!(!matches!(s.anti_aliasing, AaMode::Dlss));
        s.cycle(GraphicsField::DlssQuality, 1);
        assert_eq!(s.dlss_quality, DlssQuality::Auto);
        assert_eq!(s.preset, QualityPreset::High);

        // The AA cycler never reaches Dlss without support: a full loop from
        // Off visits only the plain slots.
        s.anti_aliasing = AaMode::Off;
        for _ in 0..AA_SLOTS.len() {
            s.cycle(GraphicsField::AntiAliasing, 1);
            assert!(!matches!(s.anti_aliasing, AaMode::Dlss));
        }
    }

    #[test]
    fn dlss_gate_off_by_default_keeps_everything_na() {
        // The Retail+ gate is off by default: even on a capable machine DLSS
        // stays N/A and inert until explicitly enabled in the Debug menu —
        // this is what lets one dlss build serve both audiences.
        let mut s = GraphicsSettings {
            dlss_supported: true,
            ..Default::default()
        };
        assert!(!s.dlss_selectable(), "gate defaults to off");
        assert_eq!(s.value_label(GraphicsField::Dlss), "N/A");
        assert_eq!(s.value_label(GraphicsField::DlssQuality), "N/A");

        s.cycle(GraphicsField::Dlss, 1);
        assert!(!matches!(s.anti_aliasing, AaMode::Dlss));
        assert_eq!(
            s.preset,
            QualityPreset::High,
            "refused cycle must not dirty preset"
        );

        // A persisted Dlss mode + capability is still inert with the gate
        // closed: dlss_active drives camera/render-scale/nameplate, so it
        // must be false or the frame would upscale while the menu says N/A.
        s.anti_aliasing = AaMode::Dlss;
        assert!(!s.dlss_active());
        assert_eq!(s.value_label(GraphicsField::AntiAliasing), "DLSS (N/A)");

        // Opening the gate makes everything live again without touching AA mode.
        s.dlss_menu_enabled = true;
        assert!(s.dlss_selectable());
        assert!(s.dlss_active(), "gate + capability + Dlss mode => active");
        assert_eq!(s.value_label(GraphicsField::Dlss), "On");

        // Closing the gate again stands DLSS down (rendering falls back to no AA).
        s.dlss_menu_enabled = false;
        assert!(!s.dlss_active());
    }

    #[test]
    fn retail_gates_survive_preset_cycle_and_reset() {
        let mut s = GraphicsSettings {
            dlss_menu_enabled: true,
            job_display: true,
            mob_hp_under: true,
            ..Default::default()
        };
        s.cycle(GraphicsField::Preset, 1);
        assert!(
            s.dlss_menu_enabled && s.job_display && s.mob_hp_under,
            "preset cycle kept the gates"
        );
        s.reset_to_default();
        assert!(
            s.dlss_menu_enabled && s.job_display && s.mob_hp_under,
            "menu reset kept the gates"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dlss_toggle_and_cycler_when_supported() {
        let mut s = GraphicsSettings {
            dlss_supported: true,
            dlss_menu_enabled: true,
            ..Default::default()
        };
        assert_eq!(s.value_label(GraphicsField::Dlss), "Off");

        s.cycle(GraphicsField::Dlss, 1);
        assert!(matches!(s.anti_aliasing, AaMode::Dlss));
        assert!(s.dlss_active());
        assert_eq!(s.value_label(GraphicsField::Dlss), "On");
        assert_eq!(s.value_label(GraphicsField::AntiAliasing), "DLSS");
        assert_eq!(s.msaa(), Msaa::Off, "DLSS implies multisampling off");
        assert!(!s.wants_taa(), "DLSS implies TAA off");

        // Render scale is DLSS-owned while active: reads "DLSS", refuses to move.
        let scale_before = s.render_scale;
        s.cycle(GraphicsField::RenderScale, 1);
        assert_eq!(s.render_scale, scale_before);
        assert_eq!(s.value_label(GraphicsField::RenderScale), "DLSS");

        // Off lands on AA Off (user re-picks MSAA/TAA in the cycler).
        s.cycle(GraphicsField::Dlss, 1);
        assert!(matches!(s.anti_aliasing, AaMode::Off));

        // The AA cycler does NOT include Dlss (user decision): cycling from
        // Taa wraps to Off; the explicit DLSS row owns that transition.
        s.anti_aliasing = AaMode::Taa;
        s.cycle(GraphicsField::AntiAliasing, 1);
        assert!(matches!(s.anti_aliasing, AaMode::Off));
    }

    #[test]
    fn dlss_quality_cycles_and_survives_preset_and_reset() {
        let mut s = GraphicsSettings {
            dlss_supported: true,
            dlss_menu_enabled: true,
            ..Default::default()
        };
        s.cycle(GraphicsField::Dlss, 1);
        s.cycle(GraphicsField::DlssQuality, 1);
        assert_eq!(s.dlss_quality, DlssQuality::Dlaa); // Auto -> Dlaa is the first step

        // Presets never own DLSS: cycling a preset keeps on-state, tier, and
        // capability.
        s.cycle(GraphicsField::Preset, 1);
        assert!(s.dlss_active(), "preset cycle kept DLSS on");
        assert_eq!(s.dlss_quality, DlssQuality::Dlaa);
        assert!(s.dlss_supported);

        // DLSS Config reset touches only the tier.
        s.reset_dlss_config();
        assert_eq!(s.dlss_quality, DlssQuality::Auto);
        assert!(s.dlss_active(), "config reset left on/off alone");

        // Full menu reset returns to High (DLSS off, Msaa4) but must not
        // un-detect the runtime capability or close the Retail+ gate.
        s.reset_to_default();
        assert!(!matches!(s.anti_aliasing, AaMode::Dlss));
        assert!(s.dlss_supported, "reset preserved capability");
        assert!(s.dlss_menu_enabled, "reset preserved the Retail+ gate");
    }

    #[test]
    fn dlss_placeholders_stay_inert() {
        let mut s = GraphicsSettings {
            dlss_supported: true,
            ..Default::default()
        };
        for &f in DLSS_CONFIG_FIELDS {
            if f.is_dlss_placeholder() {
                assert_eq!(s.value_label(f), "N/A", "{f:?}");
                let before = s.clone();
                s.cycle(f, 1);
                assert_eq!(s, before, "{f:?} cycled state");
            }
        }
    }

    #[test]
    fn neural_uplift_rows_are_live_when_supported() {
        let mut s = GraphicsSettings::default();
        assert_eq!(s.value_label(GraphicsField::DlssNeuralUplift), "N/A");
        // Refuses to cycle while unsupported.
        s.cycle(GraphicsField::DlssNeuralUplift, 1);
        assert!(!s.neural_uplift);

        s.dlss_supported = true;
        s.dlss_menu_enabled = true;
        assert_eq!(s.value_label(GraphicsField::DlssNeuralUplift), "Off");
        assert!(!s.nr_active());

        // NR is a DLSS-family effect: the toggle alone, in any other AA mode,
        // must not activate it.
        s.cycle(GraphicsField::DlssNeuralUplift, 1);
        assert!(s.neural_uplift);
        assert!(!s.nr_active(), "toggle without Dlss mode stays off");

        s.anti_aliasing = AaMode::Dlss;
        assert!(s.nr_active(), "toggle + support + Dlss mode => nr_active");
        assert_eq!(s.value_label(GraphicsField::DlssNeuralUplift), "On");

        // The reported bug: leaving DLSS mode must stop NR even with the toggle
        // still on — otherwise it keeps evaluating under MSAA/TAA.
        s.anti_aliasing = AaMode::Msaa4;
        assert!(
            !s.nr_active(),
            "leaving Dlss mode turns NR off regardless of the toggle"
        );

        // Knobs cycle through their slots (default intensity is the addon's 1.01).
        assert!((s.nr_intensity - 1.01).abs() < 1e-6);
        s.cycle(GraphicsField::DlssNrIntensity, 1);
        assert!((s.nr_intensity - 1.25).abs() < 1e-6);

        // Presets never own NR state (toggle + knobs) NOR the DLSS on/off
        // mirror: from outside Dlss mode the preset's own AA applies and NR
        // stays off; from inside it, the cycle carries both over.
        s.cycle(GraphicsField::Preset, 1); // still in Msaa4 here
        assert!(s.neural_uplift && (s.nr_intensity - 1.25).abs() < 1e-6);
        assert!(!matches!(s.anti_aliasing, AaMode::Dlss));
        assert!(!s.nr_active());

        s.anti_aliasing = AaMode::Dlss;
        s.cycle(GraphicsField::Preset, 1);
        assert!(
            matches!(s.anti_aliasing, AaMode::Dlss),
            "presets carry the Dlss mirror over"
        );
        assert!(
            s.nr_active(),
            "NR stays active across a preset cycle inside DLSS mode"
        );

        // DLSS Config reset turns NR off and restores knob defaults; SR on/off stays put.
        s.anti_aliasing = AaMode::Msaa4; // non-Dlss so the toggle below lands ON
        s.cycle(GraphicsField::Dlss, 1); // SR on
        s.reset_dlss_config();
        assert!(!s.neural_uplift);
        assert!((s.nr_intensity - 1.01).abs() < 1e-6);
        assert!(
            matches!(s.anti_aliasing, AaMode::Dlss),
            "reset left SR on/off alone"
        );

        // Toggled in a json but unsupported => nr_active stays false (no-op).
        let s2 = GraphicsSettings {
            neural_uplift: true,
            ..Default::default()
        };
        assert!(!s2.nr_active());
    }

    #[test]
    fn dlss_mode_in_json_is_inert_on_default_builds() {
        // A graphics.json written by a dlss build round-trips: the mode
        // deserializes, but with capability undetected everything reads N/A
        // and dlss_active is false — pure no-op.
        let s = GraphicsSettings {
            anti_aliasing: AaMode::Dlss,
            dlss_quality: DlssQuality::Performance,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: GraphicsSettings = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.anti_aliasing, AaMode::Dlss));
        assert_eq!(back.dlss_quality, DlssQuality::Performance);
        assert!(!back.dlss_supported, "serde(skip) field never persists");
        assert!(!back.dlss_active());
        assert_eq!(back.value_label(GraphicsField::AntiAliasing), "DLSS (N/A)");
    }
}
