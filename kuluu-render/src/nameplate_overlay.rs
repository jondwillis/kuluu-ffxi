use bevy::camera::visibility::RenderLayers;

/// The render layer nameplate billboards live on. core_3d skips them — the
/// operator camera's layers are `[0, gizmo]` only (see `build_operator_camera`)
/// — and `nameplate_final_pass` is now the sole thing that draws them: a final
/// in-view pass after all post effects, before upscaling. That pass replaced
/// the second-camera design this file used to host (`NameplateOverlayCamera`,
/// removed 2026-08): same retail rationale — names go to the backbuffer AFTER
/// the scene and its effects (research/XIClient/.../CXiActorNameDraw.cpp), no
/// bloom, no fog, no tonemap — but without a second camera riding the shared
/// A/B main-texture flip of one view target.
pub const NAMEPLATE_RENDER_LAYER: usize = 4;

/// The order slot immediately after the operator camera (0). The render-scale
/// composite camera takes this +1 so it stays behind whatever writes the scene
/// into the window, in every mode.
pub const NAMEPLATE_OVERLAY_CAMERA_ORDER: isize = 1;

pub fn nameplate_render_layers() -> RenderLayers {
    RenderLayers::layer(NAMEPLATE_RENDER_LAYER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::WORLD_GIZMO_LAYER;
    use crate::minimap::topdown::MINIMAP_BAKE_LAYER;

    #[test]
    fn nameplate_layer_collides_with_no_other_view() {
        assert_ne!(NAMEPLATE_RENDER_LAYER, 0);
        assert_ne!(NAMEPLATE_RENDER_LAYER, WORLD_GIZMO_LAYER);
        assert_ne!(NAMEPLATE_RENDER_LAYER, MINIMAP_BAKE_LAYER);
    }
}
