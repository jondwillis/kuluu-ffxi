//! Retail nameplate status icons: which glyphs prefix an actor's name.
//!
//! Retail builds the plate as one string — icon markers, then a space, then the
//! name — and the icon glyphs live in the same `font    fontshp ` shape group
//! as the letters, at codes 0x8E..0xB1, cropped off the `menu    ustatshd`
//! sheet. research/XIClient/.../ActorTelemetry.cpp
//! `BuildTelemetryActorName` assembles the prefix; :204
//! `GetPrimaryActorNameMarker` and :296 `GetSecondaryActorNameMarker` choose it.

use kuluu_snapshot::{CharFlags, Entity, EntityKind};

/// Glyph codes retail uses for the nameplate icons, indexed into the shape
/// group as `code - FIRST_GLYPH_CODE`
/// (`GetActorNameGlyphData`).
pub mod glyph {
    /// PlayOnelineFlag — the PlayOnline icon.
    pub const PLAY_ONLINE: u8 = 0x8E;
    pub const LINKDEAD: u8 = 0x8F;
    pub const AWAY: u8 = 0x90;
    /// LfgFlag — seeking party.
    pub const SEEKING: u8 = 0x91;
    /// The linkshell pearl. The one icon retail tints, using the actor's
    /// linkshell colour (`DrawActorNameText`).
    pub const LINKSHELL: u8 = 0x92;
    pub const BAZAAR: u8 = 0x9C;
    /// AutoPartyFlag — accepting invites automatically.
    pub const AUTO_PARTY: u8 = 0x9D;
    /// LfgMasterFlag — the job-master star, drawn as a pair with its tail.
    pub const JOB_MASTER: u8 = 0xAC;
    /// The half-scale companion glyph retail appends after JOB_MASTER
    /// (`DrawActorNameText`).
    pub const JOB_MASTER_TAIL: u8 = 0xAD;
}

/// `GetActorNameGlyphData` — the shape group starts at
/// the space character.
pub const FIRST_GLYPH_CODE: u8 = 0x20;

/// `GetPrimaryActorNameMarker` — retail's GmLevel marker
/// table, verbatim. Index 0 is unreachable: the lookup is guarded on a non-zero
/// level, so a level-0 actor keeps whatever marker it already had.
const GM_MARKERS: [u8; 8] = [0x93, 0x93, 0x93, 0x95, 0x96, 0x97, 0x98, 0x99];

/// The secondary marker slot only fills for an actor in the allegiance range
/// retail treats as a ballista/besieged combatant.
/// `GetSecondaryActorNameMarker`.
const SECONDARY_ALLEGIANCE_MIN: u8 = 2;
const SECONDARY_ALLEGIANCE_MAX: u8 = 0x63;
const SECONDARY_ALLEGIANCE_GAP: std::ops::RangeInclusive<u8> = 0x28..=0x2B;

/// The icon prefix for an actor's nameplate, in draw order (leftmost first).
///
/// Retail's rule is a strict priority list, not a set: the primary marker is
/// the *first* matching condition, and a secondary marker may follow it.
/// `BuildTelemetryActorName` seeds the primary slot with the linkshell pearl,
/// so the pearl shows unless a higher-priority state replaces it.
///
/// Only players carry icons. On `CHAR_NPC` (0x0E) retail clears the flags every
/// one of these markers reads — LFG, auto-party, anonymous, PlayOnline,
/// linkshell and linkdead are all forced to 0
/// (research/XIClient/.../0x00E.cpp `RecvCharNpc`) — so NPCs, mobs, pets and trusts
/// draw a bare name.
pub fn nameplate_markers(entity: &Entity) -> Vec<u8> {
    let mut markers = Vec::new();
    if !matches!(entity.kind, EntityKind::Pc) {
        return markers;
    }
    let flags = &entity.char_flags;

    let Some(primary) = primary_marker(flags) else {
        return markers;
    };
    markers.push(primary);
    if primary == glyph::JOB_MASTER {
        markers.push(glyph::JOB_MASTER_TAIL);
    }
    if let Some(secondary) = secondary_marker(flags) {
        markers.push(secondary);
    }
    markers
}

/// `GetPrimaryActorNameMarker`, restricted to the
/// states the snapshot carries. The ballista/besieged markers (0x9E..0xA7), the
/// Monstrosity marker and the pet/trust-link markers keyed off `AUDIT_130` need
/// a live allegiance, Monstrosity or pet-packet context we do not decode, so
/// they are omitted rather than guessed.
fn primary_marker(flags: &CharFlags) -> Option<u8> {
    // The seed: an actor in a linkshell starts with the pearl in the primary
    // slot, and every check below may overwrite it (`BuildTelemetryActorName`).
    let seed = flags.linkshell.then_some(glyph::LINKSHELL);

    if flags.play_online {
        return Some(glyph::PLAY_ONLINE);
    }
    if flags.linkdead {
        return Some(glyph::LINKDEAD);
    }
    if flags.away {
        return Some(glyph::AWAY);
    }
    if !flags.gm_icon && flags.gm_level != 0 {
        if let Some(&marker) = GM_MARKERS.get(usize::from(flags.gm_level)) {
            return Some(marker);
        }
    }
    if flags.lfg_master {
        return Some(glyph::JOB_MASTER);
    }
    if flags.auto_party {
        return Some(glyph::AUTO_PARTY);
    }
    if flags.lfg {
        return Some(glyph::SEEKING);
    }
    if flags.bazaar {
        return Some(glyph::BAZAAR);
    }
    seed
}

/// `GetSecondaryActorNameMarker`. Retail bails out of
/// the whole slot unless the actor is in the ballista/besieged allegiance range
/// (or is a pet, which rides a packet we do not decode), so ordinary play draws
/// one icon, not two.
fn secondary_marker(flags: &CharFlags) -> Option<u8> {
    let allegiance = flags.allegiance;
    let in_range = (SECONDARY_ALLEGIANCE_MIN..=SECONDARY_ALLEGIANCE_MAX).contains(&allegiance)
        && !SECONDARY_ALLEGIANCE_GAP.contains(&allegiance);
    if !in_range {
        return None;
    }
    if flags.lfg_master {
        return Some(glyph::JOB_MASTER);
    }
    if flags.auto_party {
        return Some(glyph::AUTO_PARTY);
    }
    if flags.lfg {
        return Some(glyph::SEEKING);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::Vec3;

    fn pc() -> Entity {
        Entity {
            id: 0x0100_0001,
            act_index: 1,
            kind: EntityKind::Pc,
            name: Some("Test".into()),
            pos: Vec3::default(),
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 25,
            speed_base: 25,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: CharFlags::default(),
        }
    }

    #[test]
    fn a_plain_player_gets_no_icons() {
        assert!(nameplate_markers(&pc()).is_empty());
    }

    #[test]
    fn a_linkshell_member_gets_the_pearl() {
        let mut e = pc();
        e.char_flags.linkshell = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::LINKSHELL]);
    }

    #[test]
    fn non_players_never_draw_icons() {
        for kind in [
            EntityKind::Npc,
            EntityKind::Mob,
            EntityKind::Pet,
            EntityKind::Other,
        ] {
            let mut e = pc();
            e.kind = kind;
            e.char_flags.linkshell = true;
            e.char_flags.away = true;
            assert!(
                nameplate_markers(&e).is_empty(),
                "0x0E clears every marker flag ({kind:?})"
            );
        }
    }

    #[test]
    fn away_outranks_the_pearl_in_the_primary_slot() {
        let mut e = pc();
        e.char_flags.linkshell = true;
        e.char_flags.away = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::AWAY]);
    }

    #[test]
    fn linkdead_outranks_away_and_playonline_outranks_both() {
        let mut e = pc();
        e.char_flags.away = true;
        e.char_flags.linkdead = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::LINKDEAD]);

        e.char_flags.play_online = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::PLAY_ONLINE]);
    }

    #[test]
    fn gm_levels_from_one_upward_take_retails_gm_marker() {
        let mut e = pc();
        e.char_flags.linkshell = true;
        for level in 1..=7u8 {
            e.char_flags.gm_level = level;
            assert_eq!(
                nameplate_markers(&e),
                vec![GM_MARKERS[usize::from(level)]],
                "gm level {level}"
            );
        }
    }

    #[test]
    fn gm_level_zero_leaves_the_pearl_alone() {
        let mut e = pc();
        e.char_flags.linkshell = true;
        e.char_flags.gm_level = 0;
        assert_eq!(nameplate_markers(&e), vec![glyph::LINKSHELL]);
    }

    /// `Flags2.GmIconFlag` is retail's "let the other icons show alongside the
    /// GM name colour" switch (char_update.cpp `CCharUpdatePacket::updateWith`),
    /// so it suppresses the GM glyph itself (`GetPrimaryActorNameMarker`).
    #[test]
    fn the_gm_icon_flag_suppresses_the_gm_marker() {
        let mut e = pc();
        e.char_flags.linkshell = true;
        e.char_flags.gm_level = 3;
        e.char_flags.gm_icon = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::LINKSHELL]);
    }

    #[test]
    fn the_bazaar_icon_is_the_lowest_priority_primary() {
        let mut e = pc();
        e.char_flags.bazaar = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::BAZAAR]);

        e.char_flags.lfg = true;
        assert_eq!(
            nameplate_markers(&e),
            vec![glyph::SEEKING],
            "seeking outranks bazaar for the primary slot"
        );
    }

    #[test]
    fn the_bazaar_icon_still_beats_the_bare_pearl() {
        let mut e = pc();
        e.char_flags.linkshell = true;
        e.char_flags.bazaar = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::BAZAAR]);
    }

    #[test]
    fn the_job_master_star_draws_with_its_tail_glyph() {
        let mut e = pc();
        e.char_flags.lfg_master = true;
        assert_eq!(
            nameplate_markers(&e),
            vec![glyph::JOB_MASTER, glyph::JOB_MASTER_TAIL]
        );
    }

    #[test]
    fn ordinary_play_draws_a_single_icon() {
        let mut e = pc();
        e.char_flags.away = true;
        e.char_flags.lfg = true;
        assert_eq!(
            nameplate_markers(&e),
            vec![glyph::AWAY],
            "the secondary slot stays shut outside the ballista allegiance range"
        );
    }

    #[test]
    fn the_secondary_slot_opens_inside_the_ballista_allegiance_range() {
        let mut e = pc();
        e.char_flags.away = true;
        e.char_flags.lfg = true;
        e.char_flags.allegiance = SECONDARY_ALLEGIANCE_MIN;
        assert_eq!(nameplate_markers(&e), vec![glyph::AWAY, glyph::SEEKING]);

        e.char_flags.allegiance = *SECONDARY_ALLEGIANCE_GAP.start();
        assert_eq!(
            nameplate_markers(&e),
            vec![glyph::AWAY],
            "the 0x28..0x2B categories are excluded"
        );
    }

    #[test]
    fn every_marker_is_inside_the_shape_group_icon_range() {
        let mut e = pc();
        e.char_flags = CharFlags {
            play_online: true,
            linkdead: true,
            away: true,
            lfg: true,
            linkshell: true,
            bazaar: true,
            auto_party: true,
            lfg_master: true,
            gm_level: 7,
            allegiance: SECONDARY_ALLEGIANCE_MIN,
            ..CharFlags::default()
        };
        for marker in nameplate_markers(&e) {
            assert!(
                (glyph::PLAY_ONLINE..=glyph::JOB_MASTER_TAIL).contains(&marker),
                "marker 0x{marker:02X} outside the icon glyph range"
            );
        }
    }
}
