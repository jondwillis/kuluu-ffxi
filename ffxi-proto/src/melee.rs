// vendor/server/src/map/enums/action/category.h ActionCategory BasicAttack — `action.cmd_no`, 4 bits.
pub const CATEGORY_BASIC_ATTACK: u8 = 1;

// vendor/server/src/map/enums/action/info.h ActionInfo - the per-result `info` bits of a basic
// attack. Other categories overload the same bits (Dancer step levels, Rune Fencer runes), so
// only read them behind a CATEGORY_BASIC_ATTACK gate.
pub const INFO_DEFEATED: u8 = 1;
pub const INFO_CRITICAL_HIT: u8 = 2;

// The outcome bits that follow `animation` in every result block of
// vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack:
// info(5), hitDistortion(2), knockback(3). `hit_distortion` is the damage as a share of the
// target's max HP (vendor/server/src/map/action/action.cpp action_result_t::recordDamage), not
// the crit flag: a crit is `info & INFO_CRITICAL_HIT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ResultOutcome {
    pub info: u8,
    /// vendor/server/src/map/enums/action/hit_distortion.h HitDistortion, 0..=3.
    pub hit_distortion: u8,
    /// vendor/server/src/map/enums/action/knockback.h Knockback, 0..=7.
    pub knockback: u8,
}

impl ResultOutcome {
    pub const fn from_wire(info: u8, hit_distortion: u8, knockback: u8) -> Self {
        Self {
            info,
            hit_distortion,
            knockback,
        }
    }

    pub const fn to_wire(self) -> (u8, u8, u8) {
        (self.info, self.hit_distortion, self.knockback)
    }

    pub const fn is_critical(self) -> bool {
        self.info & INFO_CRITICAL_HIT != 0
    }

    pub const fn defeated(self) -> bool {
        self.info & INFO_DEFEATED != 0
    }
}

// vendor/server/src/map/enums/action/resolution.h — `result.resolution`, 3 bits in
// vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActionResolution {
    Hit,
    Miss,
    Guard,
    Parry,
    Block,
}

impl ActionResolution {
    pub fn from_wire(bits: u8) -> Option<Self> {
        Some(match bits {
            0 => Self::Hit,
            1 => Self::Miss,
            2 => Self::Guard,
            3 => Self::Parry,
            4 => Self::Block,
            _ => return None,
        })
    }

    pub fn to_wire(self) -> u8 {
        match self {
            Self::Hit => 0,
            Self::Miss => 1,
            Self::Guard => 2,
            Self::Parry => 3,
            Self::Block => 4,
        }
    }
}

// vendor/server/src/map/attack.h AttackAnimation. Set from `attack.GetAnimationID()` into
// `actionResult.animation` (vendor/server/src/map/entities/battleentity.cpp CBattleEntity::OnAttack) — for a basic
// attack this is the swing slot, not a skill id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttackAnimation {
    RightAttack,
    LeftAttack,
    RightKick,
    LeftKick,
    Throw,
}

impl AttackAnimation {
    pub fn from_wire(bits: u16) -> Option<Self> {
        Some(match bits {
            0 => Self::RightAttack,
            1 => Self::LeftAttack,
            2 => Self::RightKick,
            3 => Self::LeftKick,
            4 => Self::Throw,
            _ => return None,
        })
    }

    pub fn to_wire(self) -> u16 {
        match self {
            Self::RightAttack => 0,
            Self::LeftAttack => 1,
            Self::RightKick => 2,
            Self::LeftKick => 3,
            Self::Throw => 4,
        }
    }
}

// vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack - one
// result block's bits: resolution(3), kind(2), animation(12), info(5), hitDistortion(2),
// knockback(3). A body that carries no result block, or that ends mid-block, has none of them
// at all: `resolution == 0` is `Hit`, so absence must not be spelled as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeleeResult {
    pub resolution: ActionResolution,
    pub animation: AttackAnimation,
}

impl MeleeResult {
    pub fn from_wire(resolution: u8, animation: u16) -> Option<Self> {
        Some(Self {
            resolution: ActionResolution::from_wire(resolution)?,
            animation: AttackAnimation::from_wire(animation)?,
        })
    }

    pub fn to_wire(self) -> (u8, u16) {
        (self.resolution.to_wire(), self.animation.to_wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrips_through_melee_result() {
        for resolution in 0..=4u8 {
            for animation in 0..=4u16 {
                let r = MeleeResult::from_wire(resolution, animation).expect("in-range bits");
                assert_eq!(r.to_wire(), (resolution, animation));
            }
        }
        assert_eq!(MeleeResult::from_wire(5, 0), None);
        assert_eq!(MeleeResult::from_wire(0, 5), None);
    }

    // The outcome bits ride through unvalidated: the bit reader already bounds them to their
    // field widths (info 5, hitDistortion 2, knockback 3).
    #[test]
    fn outcome_bits_roundtrip_and_flags() {
        let o = ResultOutcome::from_wire(INFO_CRITICAL_HIT, 3, 2);
        assert_eq!(o.to_wire(), (INFO_CRITICAL_HIT, 3, 2));
        assert!(o.is_critical());
        assert!(!o.defeated());
        // recordDamage sets hitDistortion from the damage share alone: Heavy without the flag
        // is not a crit, and a crit can land Light.
        assert!(!ResultOutcome::from_wire(0, 3, 0).is_critical());
        assert!(ResultOutcome::from_wire(INFO_CRITICAL_HIT, 1, 0).is_critical());
        assert!(ResultOutcome::from_wire(INFO_DEFEATED, 0, 0).defeated());
    }

    #[test]
    fn wire_values_match_lsb_enums() {
        assert_eq!(ActionResolution::from_wire(0), Some(ActionResolution::Hit));
        assert_eq!(
            ActionResolution::from_wire(4),
            Some(ActionResolution::Block)
        );
        assert_eq!(ActionResolution::from_wire(5), None);
        assert_eq!(
            AttackAnimation::from_wire(0),
            Some(AttackAnimation::RightAttack)
        );
        assert_eq!(AttackAnimation::from_wire(4), Some(AttackAnimation::Throw));
        assert_eq!(AttackAnimation::from_wire(5), None);
    }
}
