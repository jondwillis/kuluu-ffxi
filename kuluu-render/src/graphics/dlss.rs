//! DLSS runtime plumbing: capability detection and the settings -> bevy
//! mapping. Compiled only with the `dlss` cargo feature; the settings-side
//! data model (AaMode::Dlss, DlssQuality, dlss_supported) lives unconditionally
//! in `settings.rs` so persisted configs round-trip on every build.
//!
//! Wiring lives in three places:
//! - the app inserts [`project_id`] before `DefaultPlugins`
//!   (kuluu/src/view_native/mod.rs) — Bevy adds `DlssInitPlugin` itself under
//!   the dlss feature, and it panics without that resource;
//! - [`update_dlss_availability_system`] (registered in kuluu-render's plugin,
//!   ordered before `apply_anti_aliasing_system`) copies the renderer's
//!   verdict into `GraphicsSettings::dlss_supported`;
//! - `camera::build_operator_camera` inserts the `Dlss` component via
//!   [`to_bevy_quality`] when `dlss_active()`.

use bevy::anti_alias::dlss::{DlssPerfQualityMode, DlssProjectId, DlssSuperResolutionSupported};
use bevy::prelude::*;

use super::settings::{DlssQuality, GraphicsSettings};

/// Stable project id for the NVIDIA DLSS SDK (it keys per-app driver
/// behavior/telemetry on this). Fixed at first release and must never change
/// for the lifetime of the project — regenerating it makes the driver treat
/// kuluu as a brand-new application.
pub const KULUU_DLSS_PROJECT_ID: u128 = 0xa7c3_f2e1_9d4b_4e8a_b6f5_2c8d_91e0_734a;

/// The resource `DlssInitPlugin` reads during app build.
pub fn project_id() -> DlssProjectId {
    DlssProjectId(uuid::Uuid::from_u128(KULUU_DLSS_PROJECT_ID))
}

/// Copies the renderer's capability verdict into the settings resource, once.
///
/// `DlssSuperResolutionSupported` is inserted by bevy's DlssPlugin during
/// renderer init when the GPU/backend/DLL checks all pass, and never appears
/// otherwise, so this converges on frame 1 and then never writes again — the
/// write is guarded because a per-frame ResMut deref-mut would re-trigger the
/// resource_changed-gated apply chain (and the graphics.json persist) every
/// frame.
pub fn update_dlss_availability_system(
    supported: Option<Res<DlssSuperResolutionSupported>>,
    mut settings: ResMut<GraphicsSettings>,
) {
    let is_supported = supported.is_some();
    if settings.dlss_supported != is_supported {
        settings.dlss_supported = is_supported;
    }
}

/// Settings tier -> dlss_wgpu tier. One-to-one; `DlssQuality` exists so the
/// menu/serde layer never has to name a feature-gated foreign type.
pub fn to_bevy_quality(q: DlssQuality) -> DlssPerfQualityMode {
    match q {
        DlssQuality::Auto => DlssPerfQualityMode::Auto,
        DlssQuality::Dlaa => DlssPerfQualityMode::Dlaa,
        DlssQuality::Quality => DlssPerfQualityMode::Quality,
        DlssQuality::Balanced => DlssPerfQualityMode::Balanced,
        DlssQuality::Performance => DlssPerfQualityMode::Performance,
        DlssQuality::UltraPerformance => DlssPerfQualityMode::UltraPerformance,
    }
}
