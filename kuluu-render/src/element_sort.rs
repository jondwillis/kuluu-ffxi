//! Retail's translucent-element draw order as a Bevy transparent-phase sort bias.
//! CMoElem.cpp CMoElem::OnDraw computes an ordering-table key per element and
//! RenderManager.cpp OtCallback walks the table from its far end, so the largest key draws
//! first. Bevy's transparent key is the view-space z (negative ahead of the camera) plus
//! `Material::depth_bias`, drawn smallest first — so every retail offset lands here negated.

use ffxi_dat::particle_gen::{DrawPriority, ParticleGeneratorDef};

// CMoElem.cpp CMoElem::OnDraw — a low-priority element's key is its depth key plus 400.
const LOW_PRIORITY_DISTANCE: f32 = 400.0;
// RenderManager.cpp RenderManager::RenderManager `OT->Init(512, 0.0, 512.0)` — keys clamp to
// 0..512, so a pinned element's constant key (`field_128`, normally 0) sits at the near end and
// draws after every depth-sorted element. Nothing ahead of the camera has a positive view z,
// so a band above zero reproduces that.
const PINNED_DISTANCE: f32 = 512.0;
// CMoOT.cpp CMoOT::Insert — equal keys draw in insertion order, the DAT's chunk order for zone
// generators. One f32 key has to carry that too: the chunk offset scaled so adjacent chunks
// (16 bytes apart at the least) still separate at f32 resolution near -512, while a 16 MiB DAT
// spreads under 256 units and stays inside the pinned band.
const DAT_ORDER_STEP: f32 = 1.0 / 65536.0;

/// `dat_offset` is the generator chunk's byte offset in its DAT; pass 0 when there is no
/// chunk order to keep (effect DATs resolve their elements one routine at a time).
pub fn transparent_sort_bias(def: &ParticleGeneratorDef, dat_offset: usize) -> f32 {
    match def.draw_priority {
        DrawPriority::Depth => -def.sort_offset,
        DrawPriority::Low => -(def.sort_offset + LOW_PRIORITY_DISTANCE),
        DrawPriority::Pinned => {
            PINNED_DISTANCE - def.sort_offset + dat_offset as f32 * DAT_ORDER_STEP
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(priority: DrawPriority, sort_offset: f32) -> ParticleGeneratorDef {
        ParticleGeneratorDef {
            draw_priority: priority,
            sort_offset,
            ..Default::default()
        }
    }

    /// DAT 345's sea group: `down` (low priority) first, then everything
    /// depth-sorted, then `col1`, `sea2`, `sea1` (all pinned) in chunk order —
    /// the base plane under the sheets.
    #[test]
    fn lower_jeuno_sea_group_draws_bottom_up() {
        let down = transparent_sort_bias(&def(DrawPriority::Low, 0.0), 0x1000);
        let col1 = transparent_sort_bias(&def(DrawPriority::Pinned, 0.0), 0x1100);
        let sea2 = transparent_sort_bias(&def(DrawPriority::Pinned, 0.0), 0x1200);
        let sea1 = transparent_sort_bias(&def(DrawPriority::Pinned, 0.0), 0x1300);
        let other = transparent_sort_bias(&def(DrawPriority::Depth, 0.0), 0x1400);
        assert!(down < other, "{down} {other}");
        assert!(other < col1, "{other} {col1}");
        assert!(col1 < sea2, "{col1} {sea2}");
        assert!(sea2 < sea1, "{sea2} {sea1}");
    }

    #[test]
    fn sort_offset_moves_every_priority_earlier() {
        for p in [DrawPriority::Depth, DrawPriority::Low, DrawPriority::Pinned] {
            let base = transparent_sort_bias(&def(p, 0.0), 64);
            let shifted = transparent_sort_bias(&def(p, 10.0), 64);
            assert_eq!(base - shifted, 10.0);
        }
    }

    #[test]
    fn adjacent_pinned_chunks_at_the_end_of_a_large_dat_still_order() {
        let far = 16 * 1024 * 1024;
        let a = transparent_sort_bias(&def(DrawPriority::Pinned, 0.0), far);
        let b = transparent_sort_bias(&def(DrawPriority::Pinned, 0.0), far + 16);
        assert!(a < b, "{a} {b}");
        assert!(a > 0.0 && b < 1024.0, "{b} stays in the pinned band");
    }
}
