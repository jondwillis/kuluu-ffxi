use std::collections::HashMap;

use crate::chunk::walk;
use crate::generator::Generator;
use crate::kind::ChunkKind;
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

    out.sort_by_key(|t| (t.frame, t.se_id, t.on_caster));
    out.dedup_by_key(|t| (t.frame, t.se_id, t.on_caster));
    out
}

pub fn resolve_stage_to_se(
    stage_id: &[u8; 4],
    stage_kind: StageKind,
    generators: &HashMap<[u8; 4], Generator>,
    seps: &HashMap<[u8; 4], Sep>,
) -> Option<(u32, bool)> {
    let direct_caster = stage_kind == StageKind::SoundOnCaster;
    let direct_target = stage_kind == StageKind::SoundOnTarget;
    if direct_caster || direct_target {
        if let Some(sep) = seps.get(stage_id) {
            return Some((sep.se_id, direct_caster));
        }
    }

    if let Some(gen) = generators.get(stage_id) {
        if gen.is_sound() {
            if let Some(sep) = seps.get(&gen.id) {
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
        let bytes: Vec<u8> = vec![0u8; 0];
        assert_eq!(extract_se_schedule(&bytes), Vec::new());
    }
}
