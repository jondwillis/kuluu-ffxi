use std::collections::HashMap;

use crate::chunk::walk;
use crate::generator::Generator;
use crate::kind::ChunkKind;
use crate::particle_gen::{AttachType, SoundGeneratorDef};
use crate::scheduler::{Scheduler, StageKind};
use crate::sep::Sep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedSe {
    pub frame: u32,

    pub se_id: u32,

    pub on_caster: bool,

    pub scheduler: [u8; 4],
}

pub fn extract_se_schedule(bytes: &[u8]) -> Vec<TimedSe> {
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

    let mut sound_defs: HashMap<[u8; 4], SoundGeneratorDef> = HashMap::new();
    for c in walk(bytes) {
        let Ok(c) = c else { continue };
        if ChunkKind::from_u8(c.kind) == Some(ChunkKind::Generator) {
            if let Ok(Some(d)) = SoundGeneratorDef::parse(c.data) {
                sound_defs.insert(c.name, d);
            }
        }
    }

    let mut out: Vec<TimedSe> = Vec::new();
    for sched in &schedulers {
        for t in &sched.stages {
            let resolved = resolve_stage_to_se(
                &t.stage.id,
                t.stage.kind,
                &generators,
                &sound_defs,
                &seps,
                t.stage.sound_range,
            );
            if let Some(res) = resolved {
                out.push(TimedSe {
                    frame: t.frame,
                    se_id: res.se_id,
                    on_caster: res.on_caster,
                    scheduler: sched.name,
                });
            }
        }
    }

    out.sort_by_key(|t| (t.frame, t.se_id, t.on_caster));
    out.dedup_by_key(|t| (t.frame, t.se_id, t.on_caster));
    out
}

/// A resolved routine-sound stage: the SE to play and how retail mixes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedSe {
    pub se_id: u32,

    pub on_caster: bool,

    /// The emitter's authored AudioRangeSetup `(far, near)`; 0.0 = not authored, Calc3D
    /// substitutes its class defaults (CYySepRes.cpp CYySepRes::Calc3D).
    pub far: f32,
    pub near: f32,

    /// Attached to an actor (Calc3D weights the vertical delta 3x) vs zone-static (1x);
    /// CYyGenerator.cpp CYyGenerator::ElemGenerate sets that flag from the attachment code.
    pub attached: bool,
}

pub fn resolve_stage_to_se(
    stage_id: &[u8; 4],
    stage_kind: StageKind,
    generators: &HashMap<[u8; 4], Generator>,
    sound_defs: &HashMap<[u8; 4], SoundGeneratorDef>,
    seps: &HashMap<[u8; 4], Sep>,
    stage_range: Option<(f32, f32)>,
) -> Option<ResolvedSe> {
    let direct_caster = stage_kind == StageKind::SoundOnCaster;
    let direct_target = stage_kind == StageKind::SoundOnTarget;
    // A non-positional stage carries the same sep payload; `on_caster` is moot
    // because the dispatcher mixes it dry, so report it as caster-sited.
    let direct_dry = stage_kind == StageKind::SoundNonPositional;
    if direct_caster || direct_target || direct_dry {
        if let Some(sep) = seps.get(stage_id) {
            // The emitter's own payload floats (research/xim EffectRoutineParser.kt
            // parseSoundEffectEmitter); a short stage ships none, and Calc3D substitutes
            // the class defaults for a 0.0.
            let (far, near) = stage_range.unwrap_or((0.0, 0.0));
            return Some(ResolvedSe {
                se_id: sep.se_id,
                on_caster: direct_caster || direct_dry,
                far,
                near,
                attached: true,
            });
        }
    }

    if let Some(gen) = generators.get(stage_id) {
        if gen.is_sound() {
            if let Some(sep) = seps.get(&gen.id) {
                // The generator's sec2 0x4C AudioRangeSetup; its attachment code selects
                // Calc3D's vertical weight (CYyGenerator.cpp CYyGenerator::ElemGenerate).
                let def = sound_defs.get(stage_id);
                return Some(ResolvedSe {
                    se_id: sep.se_id,
                    on_caster: true,
                    far: def.map(|d| d.far).unwrap_or(0.0),
                    near: def.map(|d| d.near).unwrap_or(0.0),
                    attached: def.is_some_and(|d| d.attach_type != AttachType::None),
                });
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
        let bytes: Vec<u8> = vec![0u8; 0];
        assert_eq!(extract_se_schedule(&bytes), Vec::new());
    }
}
