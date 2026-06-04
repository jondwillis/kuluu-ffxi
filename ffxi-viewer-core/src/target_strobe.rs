//! One-shot white **strobe** on the model that just became the selected
//! target — three quick pulses, then back to normal.
//!
//! Classic FFXI flashes the selected entity white a few times the instant
//! you target it, on top of the persistent target cursor. We replicate
//! that here: when `Target::id` changes to a new entity, every mesh in
//! that entity's hierarchy gets its `emissive` pushed toward white in a
//! 3-hump sine envelope over [`STROBE_DURATION`], then restored.
//!
//! # Why clone-and-restore instead of poking the live material
//!
//! Placeholder capsules (mob cuboids, pre-skin-load actors) share the
//! cached `EntityMaterials` handles, so mutating those handles' emissive
//! would bleed the flash onto every other entity of the same kind. Baked
//! actor submeshes have unique materials and would be safe to poke
//! directly, but to keep one uniform code path we *clone* each mesh's
//! current material into a throwaway handle, swap the mesh to the clone
//! for the duration of the strobe, animate the clone, then swap the
//! original back. The clone is dropped (and its asset freed) when the
//! [`TargetStrobe`] component is removed.
//!
//! Known edge: while a *placeholder* (parent-mesh) entity is strobing,
//! `sync_aggro_system` may overwrite the parent's `MeshMaterial3d` if it
//! transitions aggro that frame, stomping the clone swap. This only
//! affects the brief pre-baked-model window; baked actors carry their
//! visible material on child meshes that the sync systems never touch, so
//! the common case is unaffected.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::scene::{Target, TrackedEntities};

/// Total length of the three-pulse flash, in seconds. Short and punchy —
/// long enough to register three distinct humps, short enough to feel
/// like an "acquired" blip rather than an ongoing glow.
const STROBE_DURATION: f32 = 0.6;

/// Number of white humps over [`STROBE_DURATION`]. Three is the classic
/// count.
const STROBE_PULSES: f32 = 3.0;

/// Peak emissive added at the top of each hump. "Slightly white" per the
/// reference: a visible flash that doesn't blow the model out to a solid
/// white silhouette.
const STROBE_PEAK: f32 = 0.6;

/// Per-entity strobe state. Holds the material swaps so the flash can be
/// reverted cleanly when it finishes (or when selection changes).
#[derive(Component)]
pub struct TargetStrobe {
    /// Seconds since the strobe began.
    pub elapsed: f32,
    /// `(mesh entity, original material handle, animated clone handle)`
    /// for every mesh in the strobing entity's hierarchy. The mesh wears
    /// the clone while strobing; restore puts the original back.
    pub swaps: Vec<(Entity, Handle<StandardMaterial>, Handle<StandardMaterial>)>,
}

/// Pure decision: white-flash intensity (0..=`STROBE_PEAK`) at `elapsed`
/// seconds into the strobe. `|sin(π · pulses · t)|` gives `pulses`
/// hump-shaped flashes over the normalized duration; clamped so callers
/// can pass an `elapsed` past the end without going negative.
pub fn strobe_intensity(elapsed: f32) -> f32 {
    let t = (elapsed / STROBE_DURATION).clamp(0.0, 1.0);
    (t * STROBE_PULSES * PI).sin().abs() * STROBE_PEAK
}

/// Drive the selection strobe: start one when the target changes, animate
/// the in-flight one, and tear it down when it completes.
///
/// Schedule expectation: `Update`, after `sync_entities_system` /
/// `sync_aggro_system` (so material handles for the frame are settled and
/// `TrackedEntities` is current).
#[allow(clippy::too_many_arguments)]
pub fn target_strobe_system(
    time: Res<Time>,
    target: Res<Target>,
    tracked: Res<TrackedEntities>,
    children_q: Query<&Children>,
    mut mat_q: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut strobe_q: Query<(Entity, &mut TargetStrobe)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut last_target: Local<Option<u32>>,
) {
    let cur = target.id;
    if *last_target != cur {
        *last_target = cur;

        // Stop every in-flight strobe: restore originals (the clones drop
        // with the component) and clear the marker.
        for (e, strobe) in &strobe_q {
            restore_swaps(&strobe.swaps, &mut mat_q);
            commands.entity(e).remove::<TargetStrobe>();
        }

        // Begin a fresh strobe on the newly-selected entity, if it has a
        // tracked Bevy entity with at least one mesh material.
        if let Some(parent) = cur.and_then(|id| tracked.by_id.get(&id).copied()) {
            let swaps = begin_strobe(parent, &children_q, &mut mat_q, &mut materials);
            if !swaps.is_empty() {
                commands.entity(parent).insert(TargetStrobe {
                    elapsed: 0.0,
                    swaps,
                });
            }
        }

        // The just-inserted strobe isn't visible to `strobe_q` until the
        // next command flush, and the ones we stopped are queued for
        // removal — nothing left to animate this frame.
        return;
    }

    let dt = time.delta_secs();
    for (e, mut strobe) in &mut strobe_q {
        strobe.elapsed += dt;
        if strobe.elapsed >= STROBE_DURATION {
            restore_swaps(&strobe.swaps, &mut mat_q);
            commands.entity(e).remove::<TargetStrobe>();
            continue;
        }
        let intensity = strobe_intensity(strobe.elapsed);
        let glow = LinearRgba::rgb(intensity, intensity, intensity);
        for (_, _, clone) in &strobe.swaps {
            if let Some(m) = materials.get_mut(clone) {
                m.emissive = glow;
            }
        }
    }
}

/// Clone every material in `parent`'s mesh hierarchy, swap each mesh onto
/// its clone, and return the `(mesh, original, clone)` triples. The clones
/// start at the original's emissive; the per-frame animation overrides it.
fn begin_strobe(
    parent: Entity,
    children_q: &Query<&Children>,
    mat_q: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
) -> Vec<(Entity, Handle<StandardMaterial>, Handle<StandardMaterial>)> {
    let mut swaps = Vec::new();
    for mesh_e in collect_mesh_entities(parent, children_q, mat_q) {
        let Ok(handle) = mat_q.get(mesh_e).map(|h| h.0.clone()) else {
            continue;
        };
        let Some(base) = materials.get(&handle).cloned() else {
            continue;
        };
        let clone = materials.add(base);
        if let Ok(mut slot) = mat_q.get_mut(mesh_e) {
            slot.0 = clone.clone();
            swaps.push((mesh_e, handle, clone));
        }
    }
    swaps
}

/// Put every swapped mesh back on its original material handle. Missing
/// meshes (despawned mid-strobe) are skipped; the orphaned clone handle is
/// freed when the caller drops the swap list.
fn restore_swaps(
    swaps: &[(Entity, Handle<StandardMaterial>, Handle<StandardMaterial>)],
    mat_q: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    for (mesh_e, original, _clone) in swaps {
        if let Ok(mut slot) = mat_q.get_mut(*mesh_e) {
            slot.0 = original.clone();
        }
    }
}

/// Depth-first collect of `root` plus every descendant that carries a
/// `MeshMaterial3d<StandardMaterial>`. Baked actors hang their visible
/// submeshes as children of the `WorldEntity` parent, so the strobe has to
/// reach into the hierarchy rather than just the parent.
fn collect_mesh_entities(
    root: Entity,
    children_q: &Query<&Children>,
    mat_q: &Query<&mut MeshMaterial3d<StandardMaterial>>,
) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if mat_q.contains(e) {
            out.push(e);
        }
        if let Ok(children) = children_q.get(e) {
            for child in children.iter() {
                stack.push(child);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The envelope starts and ends dark so there's no pop-in or residual
    /// glow left on the model after the flash completes.
    #[test]
    fn intensity_zero_at_ends() {
        assert!(strobe_intensity(0.0).abs() < 1e-6);
        assert!(strobe_intensity(STROBE_DURATION).abs() < 1e-6);
    }

    /// Past the end clamps to zero rather than continuing the sine (which
    /// would re-brighten) — guards the one-frame window where `elapsed`
    /// overshoots `STROBE_DURATION` before the system tears the strobe
    /// down.
    #[test]
    fn intensity_clamps_after_end() {
        assert!(strobe_intensity(STROBE_DURATION * 2.0).abs() < 1e-6);
    }

    /// Three pulses ⇒ three interior peaks at the odd sixths of the
    /// duration, each reaching the configured peak.
    #[test]
    fn three_peaks_reach_configured_peak() {
        for k in [1.0_f32, 3.0, 5.0] {
            let t = k / (2.0 * STROBE_PULSES) * STROBE_DURATION;
            assert!(
                (strobe_intensity(t) - STROBE_PEAK).abs() < 1e-4,
                "peak at t={t} should reach STROBE_PEAK",
            );
        }
    }

    /// Stays within `[0, STROBE_PEAK]` across the whole envelope — never
    /// negative (which would darken the model) and never overshoots the
    /// "slightly white" ceiling.
    #[test]
    fn intensity_bounded() {
        for i in 0..=120 {
            let t = i as f32 / 120.0 * STROBE_DURATION;
            let v = strobe_intensity(t);
            assert!((0.0..=STROBE_PEAK + 1e-6).contains(&v), "t={t} v={v}");
        }
    }
}
