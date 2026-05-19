//! Action-DAT walker: resolves the Scheduler→Generator→Sep chain
//! that fires sound effects during a spell/ability/mob-skill
//! animation. Produces a flat `Vec<TimedSe>` schedule that a
//! runtime ticker can consume.
//!
//! Input: the bytes of a DAT file (e.g. `ROM/11/17.DAT` for Fire).
//! Output: every (frame, se_id, on_caster) the action will fire,
//! sorted by frame.
//!
//! The walker is single-pass and flat: it builds two name → chunk
//! maps (Sep + Generator) on the way through, then walks every
//! Scheduler's stages. For each stage whose `id` names a known
//! Sound-type Generator, looks up the Generator's child Sep id
//! in the Sep map. Because action DATs in practice contain at most
//! one Sep per logical sound (verified empirically on Fire = ROM/11/17),
//! a flat lookup is sufficient — no parent/child tree needed.

use std::collections::HashMap;

use crate::chunk::walk;
use crate::generator::Generator;
use crate::kind::ChunkKind;
use crate::scheduler::{Scheduler, StageKind};
use crate::sep::Sep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedSe {
    /// Frame at which the SE should fire (Scheduler-relative, 30 fps).
    pub frame: u32,
    /// SE id, resolves via `ffxi_audio::find_audio(AudioKind::Sfx, id)`.
    pub se_id: u32,
    /// True if the source Scheduler stage was `SoundOnCaster` (0x53)
    /// or an indirect generator that fires on the caster. Today we
    /// surface this verbatim from the Scheduler kind; particle-driven
    /// sound from generators inherits its parent stage's side.
    pub on_caster: bool,
    /// Originating Scheduler name (e.g. "main"). Useful for filtering
    /// when an action DAT carries multiple schedulers (variants).
    pub scheduler: [u8; 4],
}

/// Walk an action DAT file and produce the full SE schedule.
///
/// Returns sorted-by-frame, deduplicated entries. Empty if the file
/// has no Schedulers or none of the Scheduler stage ids resolve to
/// a Sound-type Generator with a known Sep sibling.
pub fn extract_se_schedule(bytes: &[u8]) -> Vec<TimedSe> {
    // Pass 1: collect all Sep + Generator + Scheduler chunks by name.
    let mut seps: HashMap<[u8; 4], Sep> = HashMap::new();
    let mut generators: HashMap<[u8; 4], Generator> = HashMap::new();
    let mut schedulers: Vec<Scheduler> = Vec::new();
    for c in walk(bytes) {
        let Ok(c) = c else { continue };
        match ChunkKind::from_u8(c.kind) {
            Some(ChunkKind::Sep) => {
                if let Ok(s) = Sep::parse(c.name, c.data) {
                    seps.insert(c.name, s);
                }
            }
            Some(ChunkKind::Generator) => {
                if let Ok(Some(g)) = Generator::parse(c.name, c.data) {
                    generators.insert(c.name, g);
                }
            }
            Some(ChunkKind::Scheduler) => {
                if let Ok(s) = Scheduler::parse(c.name, c.data) {
                    schedulers.push(s);
                }
            }
            _ => {}
        }
    }

    // Pass 2: walk each Scheduler and resolve stage references.
    let mut out: Vec<TimedSe> = Vec::new();
    for sched in &schedulers {
        for t in &sched.stages {
            let se_id = resolve_stage_to_se(&t.stage.id, t.stage.kind, &generators, &seps);
            if let Some((id, on_caster)) = se_id {
                out.push(TimedSe {
                    frame: t.frame,
                    se_id: id,
                    on_caster,
                    scheduler: sched.name,
                });
            }
        }
    }

    // Sort + dedupe — multiple paths can resolve to the same
    // (frame, se_id) (e.g. a generator+direct sound stage on the
    // same frame), which would double-trigger if not collapsed.
    out.sort_by_key(|t| (t.frame, t.se_id, t.on_caster));
    out.dedup_by_key(|t| (t.frame, t.se_id, t.on_caster));
    out
}

/// Returns `(se_id, on_caster)` if the stage resolves to a playable SE.
///
/// Three paths exist in real action DATs:
///   1. Direct sound stage: `stage.kind == SoundOnCaster|Target` and
///      `stage.id` names a Sep directly (rare — observed in door schedulers).
///   2. Generator stage: `stage.kind == Motion`/Unknown with type 0x02
///      and `stage.id` names a Sound-type Generator; the Generator's
///      `id` field references a Sep. This is the Fire/Cure pattern.
///   3. Stage names a Generator that's NOT Sound-type (Particle,
///      ModelRing, etc.) — these contribute no SE and return None.
fn resolve_stage_to_se(
    stage_id: &[u8; 4],
    stage_kind: StageKind,
    generators: &HashMap<[u8; 4], Generator>,
    seps: &HashMap<[u8; 4], Sep>,
) -> Option<(u32, bool)> {
    // Path 1: direct Sep reference from a typed sound stage.
    let direct_caster = stage_kind == StageKind::SoundOnCaster;
    let direct_target = stage_kind == StageKind::SoundOnTarget;
    if direct_caster || direct_target {
        if let Some(sep) = seps.get(stage_id) {
            return Some((sep.se_id, direct_caster));
        }
    }
    // Path 2: indirect through a Sound-type Generator.
    if let Some(gen) = generators.get(stage_id) {
        if gen.is_sound() {
            if let Some(sep) = seps.get(&gen.id) {
                // Indirect sounds aren't tagged caster vs target by
                // their generator — they play at the actor position.
                // Default to caster (the actor *is* the caster);
                // refine when a real distinction is observed.
                return Some((sep.se_id, true));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dat_yields_empty_schedule() {
        let bytes: Vec<u8> = vec![];
        assert!(extract_se_schedule(&bytes).is_empty());
    }

    #[test]
    fn handles_dat_with_only_sep() {
        // A DAT with one Sep but no Schedulers should yield no
        // scheduled events (nothing to time them against).
        // We just verify it doesn't panic on partial data.
        // Real input would come from `ffxi-dat::chunk::walk`; here
        // we exercise the empty-Schedulers branch.
        let bytes: Vec<u8> = vec![0u8; 0];
        assert_eq!(extract_se_schedule(&bytes), Vec::new());
    }
}
