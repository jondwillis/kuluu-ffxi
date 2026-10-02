//! Client-side engage pre-checks: the rejection the server would send for a
//! doomed /attack, evaluated locally so the command never leaves the client.
//! The server stays authoritative — its own 0x029 lines (MsgBasic 78/12)
//! still reach the main log; these only save the round trip.
//! vendor/server/src/map/ai/controllers/player_controller.cpp
//! `CPlayerController::Engage`, entities/charentity.cpp `CanAttack`/`IsMobOwner`.

use kuluu_snapshot::{Entity, PartyMember, Vec3};

/// The server's engage range: Engage takes the target only while
/// `distance < 30`, else MsgBasic::TooFarAway.
pub const ENGAGE_RANGE: f32 = 30.0;

/// The server's engage-range test: `distance < 30`, else the engage is
/// refused (MsgBasic::TooFarAway).
pub fn in_engage_range(target_pos: Vec3, self_pos: Vec3) -> bool {
    let dx = target_pos.x - self_pos.x;
    let dy = target_pos.y - self_pos.y;
    let dz = target_pos.z - self_pos.z;
    (dx * dx + dy * dy + dz * dz).sqrt() < ENGAGE_RANGE
}

/// The server's rejection line for an engage it would refuse, or None when
/// the client sees no reason to withhold the command. Range is checked first,
/// mirroring the server's Engage order (the claim lands on the first swing).
/// GCD is not checked: the client has no authoritative swing-time state, and
/// the server's MsgBasic::WaitLonger (94) already routes to the main log.
pub fn rejection_line(
    target: &Entity,
    self_pos: Vec3,
    self_char_id: Option<u32>,
    party: &[PartyMember],
) -> Option<String> {
    if !in_engage_range(target.pos, self_pos) {
        return Some(format!(
            "{} is too far away.",
            target.name.as_deref().unwrap_or("your target")
        ));
    }
    if claimed_by_other(target, self_char_id, party) {
        return Some("Cannot attack. Your target is already claimed.".into());
    }
    None
}

/// The server's IsMobOwner rule mirrored locally: unclaimed, self, or an
/// owner in our party/alliance is ours to attack; anything else is claimed.
/// The claim id only rides CHAR_NPC (mobs), so PCs/NPCs pass by construction.
pub fn claimed_by_other(target: &Entity, self_char_id: Option<u32>, party: &[PartyMember]) -> bool {
    target.claim_id != 0
        && self_char_id.is_none_or(|self_id| target.claim_id != self_id)
        && !party.iter().any(|m| m.id == target.claim_id)
}

/// The claim is held by self or the party/alliance: ours to fight over.
pub fn claimed_by_party(target: &Entity, self_char_id: Option<u32>, party: &[PartyMember]) -> bool {
    target.claim_id != 0 && !claimed_by_other(target, self_char_id, party)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mob(id: u32, x: f32, y: f32, z: f32, claim_id: u32) -> Entity {
        Entity {
            id,
            act_index: 0,
            kind: kuluu_snapshot::EntityKind::Mob,
            name: Some(format!("Mob {id}")),
            pos: Vec3 { x, y, z },
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            claim_id,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: kuluu_snapshot::CharFlags::default(),
            monstrosity: false,
            name_vis: None,
        }
    }

    fn member(id: u32, party_no: u8) -> PartyMember {
        PartyMember {
            id,
            act_index: 0,
            name: None,
            hp: 0,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 0,
            main_job: 0,
            main_job_lv: 0,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            party_no,
            in_mog_house: false,
        }
    }

    const SELF: u32 = 0x0100_0001;
    const OTHER: u32 = 0x0100_0002;
    const PARTY: u32 = 0x0100_0003;
    const ALLIANCE: u32 = 0x0100_0004;
    const HERE: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[test]
    fn near_unclaimed_mob_passes() {
        let t = mob(1, 10.0, 0.0, 0.0, 0);
        assert_eq!(rejection_line(&t, HERE, Some(SELF), &[]), None);
    }

    /// The server takes the target only while distance < 30; just inside
    /// (29.9) passes.
    #[test]
    fn at_or_beyond_the_engage_range_is_too_far() {
        for (x, z) in [(30.0, 0.0), (35.0, 0.0), (0.0, -30.0), (20.0, 22.4)] {
            let t = mob(1, x, 0.0, z, 0);
            assert_eq!(
                rejection_line(&t, HERE, Some(SELF), &[]),
                Some("Mob 1 is too far away.".into()),
                "dist {x}/{z}"
            );
        }
        let t = mob(1, 29.9, 0.0, 0.0, 0);
        assert_eq!(rejection_line(&t, HERE, Some(SELF), &[]), None);
    }

    #[test]
    fn claimed_by_a_stranger_is_rejected() {
        let t = mob(1, 10.0, 0.0, 0.0, OTHER);
        assert_eq!(
            rejection_line(&t, HERE, Some(SELF), &[]),
            Some("Cannot attack. Your target is already claimed.".into())
        );
    }

    #[test]
    fn claimed_by_self_or_party_or_alliance_passes() {
        for (claim, party) in [
            (SELF, vec![]),
            (PARTY, vec![member(PARTY, 0)]),
            (ALLIANCE, vec![member(PARTY, 0), member(ALLIANCE, 1)]),
        ] {
            let t = mob(1, 10.0, 0.0, 0.0, claim);
            assert_eq!(
                rejection_line(&t, HERE, Some(SELF), &party),
                None,
                "claim {claim:#x}"
            );
        }
    }

    #[test]
    fn range_beats_claim_like_the_server() {
        let t = mob(1, 40.0, 0.0, 0.0, OTHER);
        assert_eq!(
            rejection_line(&t, HERE, Some(SELF), &[]),
            Some("Mob 1 is too far away.".into())
        );
    }

    #[test]
    fn unknown_self_id_treats_any_claim_as_foreign() {
        let t = mob(1, 10.0, 0.0, 0.0, OTHER);
        assert_eq!(
            rejection_line(&t, HERE, None, &[]),
            Some("Cannot attack. Your target is already claimed.".into())
        );
    }
}
