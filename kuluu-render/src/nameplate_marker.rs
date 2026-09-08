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
    /// GM name markers, retail `GetPrimaryActorNameMarker`'s `ActorNameMarkers`
    /// table. GmLevel 1..=3 share the first glyph.
    pub const GM_1: u8 = 0x93; // GmLevel 1..=3
    pub const GM_4: u8 = 0x95; // GmLevel 3
    pub const GM_5: u8 = 0x96; // GmLevel 4
    pub const GM_6: u8 = 0x97; // GmLevel 5
    pub const GM_7: u8 = 0x98; // GmLevel 6
    pub const GM_8: u8 = 0x99; // GmLevel 7
    pub const BAZAAR: u8 = 0x9C;
    /// New Adventurer marker — the red question mark. Retail returns it for
    /// `AUDIT_130.BIT_21` in `GetPrimaryActorNameMarker`, just above bazaar; on
    /// LSB that state is `Flags3.NewCharacterFlag` (bit 23), which
    /// `CCharUpdatePacket::updateWith` writes under SendFlg.General from
    /// `!playerConfig.NewAdventurerOffFlg` — bit 10 of the `chars.settings`
    /// u32 (vendor/server/src/map/packets/char_update.cpp:347, common/mmo.h).
    pub const NEW_PLAYER: u8 = 0xA1;
    /// AutoPartyFlag — accepting invites automatically.
    pub const AUTO_PARTY: u8 = 0x9D;
    /// Ballista/besieged nation markers, selected by `Flags3.BallistaTeam`
    /// (ALLEGIANCE_TYPE) when the allegiance is a nation value. Retail
    /// `GetPrimaryActorNameMarker`'s ballista block.
    pub const NATION_SAN_DORIA: u8 = 0x9E; // ALLEGIANCE SAN_DORIA (2)
    pub const NATION_BASTOK: u8 = 0x9F; // BASTOK (3)
    pub const NATION_WINDURST: u8 = 0xA0; // WINDURST (4)
    pub const NATION_BEAUFORT: u8 = 0xA3; // WYVERNS/Beaufort (5)
    pub const NATION_RABANASTRE: u8 = 0xA4; // GRIFFONS/Rabanastre (6)
    /// Besieged markers, returned immediately for the ballista team codes
    /// 0x20..=0x27 (even/odd).
    pub const BESIEGED_EVEN: u8 = 0xA6;
    pub const BESIEGED_ODD: u8 = 0xA7;
    /// Monstrosity marker — `MonstrosityFlags != 0` in the Model block.
    pub const MONSTROSITY: u8 = 0xAB;
    /// The job-master star, drawn as a pair with its tail. Retail keys it off
    /// `Flags3.LfgMasterFlag` (`AUDIT_140.BIT_3`, research/XIClient/.../s2c/
    /// 0x00D.cpp:159), which LSB hardcodes to 0 (char_update.cpp:339) — on this
    /// server the star comes from `Flags4.JobMasterFlag` instead.
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
const GM_MARKERS: [u8; 8] = [
    glyph::GM_1,
    glyph::GM_1,
    glyph::GM_1,
    glyph::GM_4,
    glyph::GM_5,
    glyph::GM_6,
    glyph::GM_7,
    glyph::GM_8,
];

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

    // Retail expands every star in the marker string into a half-scale pair at
    // draw time (research/XIClient/.../CXiActorNameDraw.cpp:301), so append the
    // tail after each occurrence — a ballista-allegiance job master carries two.
    let push_marker = |markers: &mut Vec<u8>, code: u8| {
        markers.push(code);
        if code == glyph::JOB_MASTER {
            markers.push(glyph::JOB_MASTER_TAIL);
        }
    };

    let Some(primary) = primary_marker(flags, entity.monstrosity) else {
        return markers;
    };
    push_marker(&mut markers, primary);
    if let Some(secondary) = secondary_marker(flags) {
        push_marker(&mut markers, secondary);
    }
    markers
}

/// `GetPrimaryActorNameMarker`, restricted to the states the snapshot carries.
/// The pet/trust-link markers keyed off `AUDIT_130` come from s2c 0x067
/// ActorSupplement, which LSB does not implement (no "Supplement" in
/// vendor/server/src/map), so they are omitted rather than guessed. The away
/// sub-case for a specific server type (`ServerID & 0xFF000000 == 0x01000000`)
/// is likewise skipped: it is not the retail NA/LSB path.
///
/// Deliberately NOT implemented, with reasons (do not re-derive):
/// - Glyphs 0x94 (`AUDIT_1DA`), 0xA8 (`AUDIT_130.BIT_23`) and the second 0x93
///   (`AUDIT_130.BIT_22`) ride only s2c 0x067 RecvActorSupplement, which LSB does
///   not implement — dead on this wire.
/// - The 0xA2/0xB1 bazaar check reads `AUDIT_130.BIT_20`, which the client sets
///   only in the 0x00E NPC path (research/XIClient/.../s2c/0x00E.cpp:301/:305);
///   the PC bazaar is `AUDIT_128.BIT_9` → glyph 0x9C, which we do draw.
/// - The campaign special markers (glyphs 0xC8-0xCB/0xCD via
///   `GetSpecialActorNameMarker`, keyed off Flags4 bits 2-5) are not decodable:
///   LSB never sets those bits and the glyphs do not exist in the NA `fontshp`
///   shape group, which ends at 0xB1.
fn primary_marker(flags: &CharFlags, monstrosity: bool) -> Option<u8> {
    // The seed: an actor in a linkshell starts with the pearl in the primary
    // slot, and every check below may overwrite it (`BuildTelemetryActorName`).
    let seed = flags.linkshell.then_some(glyph::LINKSHELL);

    if flags.play_online {
        return Some(glyph::PLAY_ONLINE);
    }
    if flags.linkdead {
        return Some(glyph::LINKDEAD);
    }
    // `AUDIT_210 != 0` — the Model block's MonstrosityFlags. Above away/GM.
    if monstrosity {
        return Some(glyph::MONSTROSITY);
    }

    // The ballista/besieged block (`status = AUDIT_1FF` = Flags3.BallistaTeam).
    // The besieged team codes 0x20..=0x27 return immediately, above away/GM/etc.
    let status = flags.allegiance;
    if (0x20u8..=0x27).contains(&status) {
        return Some(if status & 1 == 0 {
            glyph::BESIEGED_EVEN
        } else {
            glyph::BESIEGED_ODD
        });
    }

    if flags.away {
        return Some(glyph::AWAY);
    }
    if !flags.gm_icon && flags.gm_level != 0 {
        if let Some(&marker) = GM_MARKERS.get(usize::from(flags.gm_level)) {
            return Some(marker);
        }
    }
    // Retail's star check is `Flags3.LfgMasterFlag` alone; LSB hardcodes that
    // flag to 0 and carries the state in Flags4.JobMasterFlag instead, so both
    // light the same marker.
    if flags.lfg_master || flags.job_master_display {
        return Some(glyph::JOB_MASTER);
    }
    if flags.auto_party {
        return Some(glyph::AUTO_PARTY);
    }
    if flags.lfg {
        return Some(glyph::SEEKING);
    }
    // New Adventurer "?" — retail checks it above bazaar (AUDIT_130.BIT_21).
    if flags.new_character {
        return Some(glyph::NEW_PLAYER);
    }
    if flags.bazaar {
        return Some(glyph::BAZAAR);
    }

    // Nothing higher matched: the pending ballista nation marker, else the pearl.
    ballista_nation_marker(status).or(seed)
}

/// The pending ballista/besieged *nation* marker for an allegiance in the
/// combatant range. Retail's switch on `markerType`, with BallistaInfo (AUDIT_294)
/// provably 0 from LSB so `markerType == status - 1`: only the five nation values
/// (ALLEGIANCE_TYPE 2..=6) select a glyph; every other allegiance leaves the
/// pending marker at its fallback.
fn ballista_nation_marker(status: u8) -> Option<u8> {
    match status {
        2 => Some(glyph::NATION_SAN_DORIA),
        3 => Some(glyph::NATION_BASTOK),
        4 => Some(glyph::NATION_WINDURST),
        5 => Some(glyph::NATION_BEAUFORT),
        6 => Some(glyph::NATION_RABANASTRE),
        _ => None,
    }
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
    // Same dual source as the primary slot — see `primary_marker`.
    if flags.lfg_master || flags.job_master_display {
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
            monstrosity: false,
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
    fn a_new_adventurer_gets_the_question_mark() {
        let mut e = pc();
        e.char_flags.new_character = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::NEW_PLAYER]);

        // Retail checks the "?" above bazaar and the pearl/nation fallback.
        e.char_flags.bazaar = true;
        e.char_flags.linkshell = true;
        e.char_flags.allegiance = 2;
        assert_eq!(nameplate_markers(&e), vec![glyph::NEW_PLAYER]);

        // ...but below seeking-party, which retail checks first. (Allegiance back
        // to 0 so the secondary slot stays shut and only the primary is asserted.)
        e.char_flags.allegiance = 0;
        e.char_flags.lfg = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::SEEKING]);
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

    /// LSB's actual job-master state — `Flags4.JobMasterFlag` (the /mastery-
    /// display toggle) — lights the same star as retail's LfgMasterFlag.
    #[test]
    fn the_job_master_display_flag_draws_the_star_too() {
        let mut e = pc();
        e.char_flags.job_master_display = true;
        assert_eq!(
            nameplate_markers(&e),
            vec![glyph::JOB_MASTER, glyph::JOB_MASTER_TAIL]
        );

        // ...and it sits at retail's priority: below away/GM, above auto-party,
        // seeking and bazaar.
        e.char_flags.away = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::AWAY]);
        e.char_flags.away = false;
        e.char_flags.auto_party = true;
        e.char_flags.lfg = true;
        e.char_flags.bazaar = true;
        assert_eq!(
            nameplate_markers(&e),
            vec![glyph::JOB_MASTER, glyph::JOB_MASTER_TAIL]
        );
    }

    /// The secondary slot opens for a ballista-allegiance actor on the LSB flag
    /// as well — retail carries two stars there, each with its half-scale tail.
    #[test]
    fn the_job_master_display_flag_fills_the_secondary_slot() {
        let mut e = pc();
        e.char_flags.job_master_display = true;
        e.char_flags.allegiance = SECONDARY_ALLEGIANCE_MIN;
        assert_eq!(
            nameplate_markers(&e),
            vec![
                glyph::JOB_MASTER,
                glyph::JOB_MASTER_TAIL,
                glyph::JOB_MASTER,
                glyph::JOB_MASTER_TAIL
            ]
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
    fn monstrosity_outranks_away_and_gm_but_not_linkdead() {
        let mut e = pc();
        e.monstrosity = true;
        e.char_flags.away = true;
        e.char_flags.gm_level = 5;
        assert_eq!(nameplate_markers(&e), vec![glyph::MONSTROSITY]);

        // LinkDead is above the Monstrosity marker in retail's order.
        e.char_flags.linkdead = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::LINKDEAD]);
    }

    #[test]
    fn a_monstrosity_in_a_linkshell_still_shows_the_marker_not_the_pearl() {
        let mut e = pc();
        e.monstrosity = true;
        e.char_flags.linkshell = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::MONSTROSITY]);
    }

    #[test]
    fn ballista_nation_allegiance_selects_the_nation_marker() {
        // ALLEGIANCE_TYPE 2..=6 are the five nations; with no higher-priority state
        // the pending nation marker is what shows (it beats the pearl fallback).
        let cases = [
            (2u8, glyph::NATION_SAN_DORIA),
            (3, glyph::NATION_BASTOK),
            (4, glyph::NATION_WINDURST),
            (5, glyph::NATION_BEAUFORT),
            (6, glyph::NATION_RABANASTRE),
        ];
        for (allegiance, want) in cases {
            let mut e = pc();
            e.char_flags.allegiance = allegiance;
            assert_eq!(nameplate_markers(&e), vec![want], "allegiance {allegiance}");
        }
    }

    #[test]
    fn a_nation_marker_is_pending_below_away() {
        // Away is checked after the ballista block's immediate return but before the
        // pending marker is returned, so it wins over a nation allegiance.
        let mut e = pc();
        e.char_flags.allegiance = 2;
        e.char_flags.away = true;
        assert_eq!(nameplate_markers(&e), vec![glyph::AWAY]);
    }

    #[test]
    fn besieged_team_codes_return_immediately_above_away_and_gm() {
        // 0x20..=0x27 are the ballista team codes: even -> 0xA6, odd -> 0xA7,
        // returned before away/GM/etc are consulted.
        let mut e = pc();
        e.char_flags.allegiance = 0x20;
        e.char_flags.away = true;
        e.char_flags.gm_level = 4;
        assert_eq!(nameplate_markers(&e), vec![glyph::BESIEGED_EVEN]);

        e.char_flags.allegiance = 0x21;
        assert_eq!(nameplate_markers(&e), vec![glyph::BESIEGED_ODD]);
    }

    #[test]
    fn every_marker_is_inside_the_shape_group_icon_range() {
        // First-match-wins means one entity exercises one branch; sweep the new
        // branches (monstrosity, ballista allegiance) plus a fully-flagged player
        // and assert every emitted marker sits in retail's icon glyph span 0x8E..=0xB1.
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
        for monstrosity in [false, true] {
            e.monstrosity = monstrosity;
            for marker in nameplate_markers(&e) {
                assert!(
                    (0x8Eu8..=0xB1).contains(&marker),
                    "marker 0x{marker:02X} outside the icon glyph range"
                );
            }
        }
    }
}
