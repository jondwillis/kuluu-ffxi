//! Retail nameplate name colours: the `menu    ncol    ` table out of the
//! retail menu DAT, plus the per-actor index selection retail runs every idle
//! tick.
//!
//! research/XIClient/.../ActorTelemetry.cpp — `InitializeNameColors` (table
//! load) and `NameColorSet` (selection).

use bevy::prelude::*;
use ffxi_dat::ui_element::find_ui_element_group;
use kuluu_snapshot::{CharFlags, Entity, EntityKind, PartyMember};

use crate::ui_element_atlas::UiElementDatRoot;

const NCOL_GROUP: &str = "menu    ncol    ";

// research/XIClient/.../ActorTelemetry.h `NAME_COLOR_COUNT` — retail
// reads only the first 23 quads of the group even though the DAT ships more.
pub const NAME_COLOR_COUNT: usize = 23;

/// Semantic names for the `ncol` indices `NameColorSet` selects. Only the ones
/// reachable without a live event/ballista context are named here; the
/// GM-level and allegiance rows are addressed by table lookup instead.
pub mod ncol {
    /// A player who is not in your party — plain white.
    pub const PC: usize = 0;
    /// A party or alliance member, and a player's pet.
    pub const PARTY: usize = 1;
    /// Seeking party / seeking master / auto-invite.
    pub const SEEKING: usize = 2;
    pub const ANONYMOUS: usize = 3;
    /// NPCs, doors, lifts and scenery models.
    pub const NPC: usize = 4;
    /// A monster nobody has claimed.
    pub const MOB: usize = 5;
    /// A monster claimed by you or your own party.
    pub const CLAIMED_BY_PARTY: usize = 6;
    /// A monster claimed by anyone else.
    pub const CLAIMED_BY_OTHER: usize = 7;
    pub const YELL: usize = 8;
    pub const DEAD: usize = 9;
}

// research/XIClient/.../ActorTelemetry.cpp `NameColorIndicesByState` —
// GmLevel indexes this from 3 upward; levels 0..2 fall through to the normal
// selection instead of taking a GM colour.
const GM_COLOR_INDICES: [usize; 8] = [10, 10, 11, 12, 13, 14, 15, 16];
const MIN_GM_LEVEL: u8 = 3;

/// ALLEGIANCE_TYPE values that pick a nation/team colour, and the `ncol` row
/// each one takes. vendor/server/src/map/entities/baseentity.h `ALLEGIANCE_TYPE`
/// names the allegiances; `NameColorSet` maps them to rows.
const ALLEGIANCE_COLOR_INDICES: [(u8, usize); 5] = [(2, 18), (3, 19), (4, 20), (5, 21), (6, 22)];

// `NameColorSet` gates the whole allegiance block on this range.
const ALLEGIANCE_COLORED_MIN: u8 = 2;
const ALLEGIANCE_COLORED_MAX: u8 = 99;

/// The belligerence bit LSB ORs into the allegiance byte while a monstrosity is
/// outside the Ferretory (`Flags3.BallistaTeam |= 0x08`, char_update.cpp;
/// `Flags2.BallistaFlg |= 0x08`, char_status.cpp), on top of the base
/// ALLEGIANCE_TYPE. A player's base is always PLAYER (charentity.cpp:127), so a
/// belligerent one reads BELLIGERENT_PLAYER_ALLEGIANCE on the wire; the MOB
/// result only reaches it through an unvalidated `setAllegiance` script call.
/// The colour logic deliberately does not branch on these — retail maps 8/9 to
/// the PC row without returning, and every later check overwrites or matches
/// that white, so falling through is equivalent; they exist to name the wire
/// values in the tests pinning that behaviour.
#[allow(dead_code)] // test fixture + wire documentation; see doc comment
const BELLIGERENT_MOB_ALLEGIANCE: u8 = 0b1_000;
#[allow(dead_code)] // ditto
const BELLIGERENT_PLAYER_ALLEGIANCE: u8 = 0b1_001;

/// The nameplate's diffuse colour is drawn through `D3DTOP_MODULATE2X`
/// (`CXiActorNameDraw::OnMove`), so the 0x7F-based table
/// values reach the screen doubled.
const MODULATE_2X: f32 = 2.0;

#[derive(Resource, Default)]
pub struct NameColorTable {
    colors: Vec<Color>,
    loaded: bool,
    /// Bumped every time the table content actually changes. Consumers fold it
    /// into their change keys so a late load (the DAT root can land after the
    /// first HUD draw) forces exactly one rebuild instead of waiting for an
    /// unrelated key field to move; idempotent reloads do not bump.
    generation: u64,
}

impl NameColorTable {
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Content version: 0 until the first successful load, +1 per content change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// The drawn colour for an `ncol` row, already doubled for MODULATE2X.
    /// Until the retail table has been read, the built-in [`DEFAULT_ROW`] stands
    /// in: a plate's colour is a fact about the entity (kind/status from the
    /// live table), not about whether a DAT read has finished — it must be
    /// drawable on the first frame. The real table overrides these on load; any
    /// plate whose row moved re-rasters via its key comparison.
    pub fn color(&self, index: usize) -> Color {
        self.colors
            .get(index)
            .copied()
            .unwrap_or_else(|| DEFAULT_ROW.get(index).copied().unwrap_or(Color::WHITE))
    }

    /// Retail's alliance-but-other-party claim colour: the average of the two
    /// claim rows. `NameColorSet` halves and adds the packed D3D
    /// colours, which is a per-channel mean.
    pub fn blend(&self, a: usize, b: usize) -> Color {
        let (a, b) = (self.color(a).to_srgba(), self.color(b).to_srgba());
        Color::srgba(
            (a.red + b.red) * 0.5,
            (a.green + b.green) * 0.5,
            (a.blue + b.blue) * 0.5,
            (a.alpha + b.alpha) * 0.5,
        )
    }

    pub fn load_from_dat(&mut self, dat_bytes: &[u8]) -> bool {
        let Some(group) = find_ui_element_group(dat_bytes, NCOL_GROUP) else {
            return false;
        };
        let colors = group
            .elements
            .iter()
            .take(NAME_COLOR_COUNT)
            .map(|el| {
                el.components
                    .first()
                    .map(|c| quad_color(c.colors[0]))
                    .unwrap_or(Color::WHITE)
            })
            .collect();
        if colors != self.colors {
            self.colors = colors;
            self.loaded = !self.colors.is_empty();
            self.generation += 1;
        }
        self.loaded
    }
}

/// One menu-shape quad vertex colour → the drawn nameplate colour.
/// research/XIClient/.../UIShapeQuad.cpp `ParseFromResource` nudges every
/// non-saturated RGB channel up by one and rescales partial alpha by 1.5 on
/// load; `InitializeNameColors` then reads those adjusted values.
fn quad_color(raw: [u8; 4]) -> Color {
    let channel = |v: u8| if v == u8::MAX { v } else { v + 1 };
    let alpha = match raw[3] {
        0 | u8::MAX => raw[3],
        v => ((f32::from(v + 1) * ALPHA_LEGACY_SCALE).min(f32::from(u8::MAX))) as u8,
    };
    Color::srgba(
        (f32::from(channel(raw[0])) / f32::from(u8::MAX) * MODULATE_2X).min(1.0),
        (f32::from(channel(raw[1])) / f32::from(u8::MAX) * MODULATE_2X).min(1.0),
        (f32::from(channel(raw[2])) / f32::from(u8::MAX) * MODULATE_2X).min(1.0),
        f32::from(alpha) / f32::from(u8::MAX),
    )
}

// research/XIClient/.../UIShapeQuad.cpp `ParseFromResource`
const ALPHA_LEGACY_SCALE: f32 = 1.5;

/// Everything the colour rule needs about the viewer's own situation.
#[derive(Debug, Clone, Copy)]
pub struct SelfContext<'a> {
    pub self_id: Option<u32>,
    pub party: &'a [PartyMember],
}

impl SelfContext<'_> {
    fn member(&self, id: u32) -> Option<&PartyMember> {
        self.party.iter().find(|m| m.id == id)
    }

    /// The party number every claim is compared against: the local player's.
    /// Retail reads it off the head of its own group list, which is always the
    /// local player (`NameColorSet`); our list arrives in server
    /// order, so look the player up by id and only fall back to the head.
    fn own_party_no(&self) -> Option<u8> {
        self.self_id
            .and_then(|id| self.member(id))
            .or_else(|| self.party.first())
            .map(|m| m.party_no)
    }
}

/// Which `ncol` row this actor's name draws in. `Blend(a, b)` is retail's
/// alliance-but-other-party claim colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameColorChoice {
    Row(usize),
    Blend(usize, usize),
}

impl NameColorChoice {
    /// The drawn colour: the retail row once the install's table has loaded,
    /// the built-in stand-in until then (see [`NameColorTable::color`]).
    pub fn resolve(self, table: &NameColorTable) -> Color {
        match self {
            Self::Row(i) => table.color(i),
            Self::Blend(a, b) => table.blend(a, b),
        }
    }
}

/// Built-in stand-ins for every `ncol` row, drawn until (and unless) the
/// retail table loads from the install's UI DAT. Kind and status come from the
/// live entity table, so a plate's colour is known on its first frame — it must
/// never hold a white placeholder waiting on a DAT read. The rows that matter at
/// a glance follow the per-kind stand-ins the minimap already uses (PC white /
/// NPC green / MOB yellow); the claim and dead rows match the real table's
/// pinned values (`real_dat_table_loads_with_retail_claim_colors`).
const DEFAULT_ROW: [Color; NAME_COLOR_COUNT] = [
    Color::srgb(1.00, 1.00, 1.00), // 0 PC — plain white
    Color::srgb(0.45, 0.75, 1.00), // 1 PARTY — pale blue (minimap party stand-in)
    Color::srgb(1.00, 0.85, 0.35), // 2 SEEKING — gold
    Color::srgb(0.70, 0.70, 0.70), // 3 ANONYMOUS — grey
    Color::srgb(0.35, 0.95, 0.45), // 4 NPC — green (minimap npc stand-in)
    Color::srgb(1.00, 0.95, 0.35), // 5 MOB — yellow (minimap mob stand-in)
    Color::srgb(1.00, 0.51, 0.51), // 6 CLAIMED_BY_PARTY — retail's #FF8282 claim red
    Color::srgb(0.75, 0.55, 1.00), // 7 CLAIMED_BY_OTHER — retail's purple
    Color::srgb(1.00, 0.35, 0.35), // 8 YELL — red
    Color::srgb(0.55, 0.55, 0.55), // 9 DEAD — neutral grey (the real row is a neutral grey)
    Color::WHITE,                  // 10 GM level 3-4
    Color::WHITE,                  // 11 GM level 4
    Color::WHITE,                  // 12 GM level 5
    Color::WHITE,                  // 13 GM level 6
    Color::WHITE,                  // 14 GM level 7
    Color::WHITE,                  // 15 GM level 8
    Color::WHITE,                  // 16 GM level 9-10
    Color::srgb(0.45, 0.65, 1.00), // 17 (unused row — keep the table full)
    Color::srgb(0.45, 0.65, 1.00), // 18 SAN_DORIA allegiance
    Color::srgb(1.00, 0.60, 0.35), // 19 BASTOK allegiance
    Color::srgb(0.45, 0.95, 0.55), // 20 WINDURST allegiance
    Color::srgb(0.40, 0.85, 0.85), // 21 WYVERNS allegiance
    Color::srgb(0.75, 0.55, 0.95), // 22 GRIFFONS allegiance
];

/// Port of `ActorTelemetry::NameColorSet`
/// (research/XIClient/.../ActorTelemetry.cpp `NameColorSet`),
/// keeping retail's precedence. The branches with no live LSB wire source are
/// skipped, not guessed:
/// - the forced `AUDIT_1D8` colour index — an event/cutscene override; no s2c
///   field carries it in vendor/server;
/// - the `AUDIT_1D0 & 4` PvP team override — retail derives bit 4 from its party
///   walk and gates on MovementFlags.PvPFlag, which LSB never writes (char_update.cpp
///   hardcodes `Flags2.PvPFlag = 0`);
/// - the linked-actor inheritance (`AUDIT_1D0 & 0x100`) — the bit is derived
///   client-side from CharmFlag; our `charm && !pet → PARTY` below is the
///   documented adaptation, and copying a partner's computed colour needs retail
///   internals that are not verifiable against LSB;
/// - the `AUDIT_294` sub-allegiance switch — BallistaInfo byte 0x2E; declared in
///   char_update.cpp but never written by any vendored path;
/// - team categories 32–39 (even → row 18, odd → row 19) and the 40–43 pairing —
///   nothing in vendored LSB writes them: `/setallegiance` caps at 6 and only
///   reaches players, so an unvalidated `setAllegiance` script call is the sole
///   source.
///
/// Belligerence (8/9) does have a live wire source — char_update.cpp /
/// char_status.cpp OR in 0x08 outside the Ferretory — but retail's block maps it
/// to the PC row *without returning*, so every check that runs after it here
/// (GM, claim, party, yell) overwrites or matches that white anyway; falling
/// through is the faithful port. Self receives its own byte via 0x037
/// `Flags2.BallistaFlg` (the server skips its own 0x0D), decoded into this same
/// field.
pub fn name_color_choice(entity: &Entity, ctx: SelfContext<'_>) -> NameColorChoice {
    use NameColorChoice::Row;
    let flags = &entity.char_flags;
    // Every flag below except `allegiance`, `charm`, `trust` and `pet` is only
    // real on CHAR_PC. See `pc_flags_are_real`.
    let is_pc = pc_flags_are_real(entity.kind);

    if entity.is_door()
        || matches!(
            entity.look,
            Some(kuluu_snapshot::EntityLook::Transport { .. })
        )
    {
        return Row(ncol::NPC);
    }

    if entity.is_dead() {
        return Row(ncol::DEAD);
    }

    // 8/9 (belligerence) deliberately fall through: retail maps them to the PC
    // row without returning, and every check below overwrites or matches that
    // white — see `BELLIGERENT_PLAYER_ALLEGIANCE`.
    if (ALLEGIANCE_COLORED_MIN..=ALLEGIANCE_COLORED_MAX).contains(&flags.allegiance) {
        if let Some(&(_, row)) = ALLEGIANCE_COLOR_INDICES
            .iter()
            .find(|(a, _)| *a == flags.allegiance)
        {
            return Row(row);
        }
    }

    if is_pc && flags.gm_level >= MIN_GM_LEVEL {
        if let Some(&row) = GM_COLOR_INDICES.get(usize::from(flags.gm_level)) {
            return Row(row);
        }
    }

    if let Some(choice) = claim_color(entity, ctx) {
        return choice;
    }

    let is_self = ctx.self_id == Some(entity.id);
    if !is_self && ctx.member(entity.id).is_some() {
        return Row(ncol::PARTY);
    }

    if is_pc {
        if flags.yell {
            return Row(ncol::YELL);
        }
        if flags.lfg_master || flags.lfg || flags.auto_party {
            return Row(ncol::SEEKING);
        }
        if flags.anonymous {
            return Row(ncol::ANONYMOUS);
        }
        return Row(ncol::PC);
    }

    // A mob whose master is a PC — LSB sets CharmFlag for both a summoned pet
    // and a charmed monster (entity_update.cpp `CEntityUpdatePacket::updateWith`)
    // — draws in the party colour. `NameColorSet`.
    if flags.charm && !flags.pet {
        return Row(ncol::PARTY);
    }

    // Retail reads Flags1.MonsterFlag here, but LSB writes STATUS_TYPE over that
    // byte, so the bit is 0 for a live mob (see `pc_flags_are_real`). Our own
    // classification of the spawn is the trustworthy signal.
    if matches!(entity.kind, EntityKind::Mob) {
        Row(ncol::MOB)
    } else {
        Row(ncol::NPC)
    }
}

/// Whether an entity's `CharFlags` carry their documented `CHAR_PC` meaning.
///
/// They only do on 0x0D. `CHAR_NPC` (0x0E) reuses the same bytes for unrelated
/// data — `ref<uint8>(0x20) = status` puts STATUS_TYPE over `Flags1`'s low byte
/// (so `MonsterFlag` is really `status & 1`, and 0 for a live mob), and
/// `ref<uint32>(0x21) = m_flags` covers the rest of `Flags1`, landing `m_flags`
/// bit 3 exactly on `LfgFlag` — which is why a plain NPC would otherwise draw in
/// the seeking-party colour. vendor/server/src/map/packets/entity_update.cpp
/// `CEntityUpdatePacket::updateWith`.
///
/// Retail reaches the same place from the other side: its 0x0E handler *zeroes*
/// LfgFlag, AutoPartyFlag, AnonymousFlag, PlayOnelineFlag, LinkShellFlag and
/// LinkDeadFlag rather than reading them off the packet
/// (research/XIClient/.../0x00E.cpp `RecvCharNpc`).
///
/// `allegiance` (0x29), `charm` (0x27 bit 3), `trust` and `pet` (0x28) survive:
/// LSB writes those explicitly on 0x0E.
fn pc_flags_are_real(kind: EntityKind) -> bool {
    matches!(kind, EntityKind::Pc)
}

/// The claimed-monster block of `NameColorSet`. A claim only
/// colours something retail treats as a monster.
fn claim_color(entity: &Entity, ctx: SelfContext<'_>) -> Option<NameColorChoice> {
    // Retail gates this on Flags1.MonsterFlag; that bit is unusable against LSB
    // (see `pc_flags_are_real`), so gate on the classified kind instead.
    if !matches!(entity.kind, EntityKind::Mob) || entity.claim_id == 0 {
        return None;
    }
    let claimed_by_self = ctx.self_id == Some(entity.claim_id);
    let Some(member) = ctx.member(entity.claim_id) else {
        return Some(NameColorChoice::Row(if claimed_by_self {
            ncol::CLAIMED_BY_PARTY
        } else {
            ncol::CLAIMED_BY_OTHER
        }));
    };
    let same_party = ctx.own_party_no() == Some(member.party_no);
    Some(if same_party {
        NameColorChoice::Row(ncol::CLAIMED_BY_PARTY)
    } else {
        NameColorChoice::Blend(ncol::CLAIMED_BY_PARTY, ncol::CLAIMED_BY_OTHER)
    })
}

/// The icon glyphs draw with a neutral diffuse so the sprite's own colours come
/// through MODULATE2X unchanged; only the linkshell pearl is tinted.
/// research/XIClient/.../CXiActorNameDraw.cpp `DrawActorNameText`.
pub const ICON_NEUTRAL_DIFFUSE: u8 = 0x80;

/// The pearl tint for an actor's linkshell icon.
/// `BuildTelemetryActorName` stores the packet's r/g/b as the alternate
/// colour; `DrawActorNameText` draws glyph 0x92 with it.
pub fn linkshell_tint(flags: &CharFlags) -> Color {
    let [r, g, b] = flags.linkshell_color;
    Color::srgb(
        (f32::from(r) / f32::from(u8::MAX) * MODULATE_2X).min(1.0),
        (f32::from(g) / f32::from(u8::MAX) * MODULATE_2X).min(1.0),
        (f32::from(b) / f32::from(u8::MAX) * MODULATE_2X).min(1.0),
    )
}

pub fn load_name_colors_system(
    mut table: ResMut<NameColorTable>,
    dat_root: Res<UiElementDatRoot>,
    mut scanned_without_group: Local<bool>,
) {
    if table.is_loaded() {
        return;
    }
    let Some(root) = dat_root.0.as_ref() else {
        // No retail install yet (headless/relay paths): the fallback colour
        // is the documented behaviour, nothing to warn about.
        return;
    };
    for (id, bytes) in crate::ui_element_atlas::read_ui_dats(root) {
        if table.load_from_dat(&bytes) {
            info!(
                entries = table.len(),
                dat = id,
                "loaded retail nameplate name-colour table"
            );
            return;
        }
    }
    // A retail install is present but none of its UI DATs carries the ncol
    // group (locale mismatch, partial install). Plates then draw the built-in
    // stand-ins for the whole session — surface it once instead of failing
    // silently.
    if !*scanned_without_group {
        *scanned_without_group = true;
        warn!(
            group = NCOL_GROUP,
            "retail install present but no UI DAT carries the nameplate colour table; plates use the built-in stand-in colours"
        );
    }
}

pub struct NameColorPlugin;

impl Plugin for NameColorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NameColorTable>()
            .add_systems(Update, load_name_colors_system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::{Entity, EntityKind, Vec3};

    const SELF_ID: u32 = 0x0100_0001;
    const MATE_ID: u32 = 0x0100_0002;
    const STRANGER_ID: u32 = 0x0100_0003;

    fn entity(kind: EntityKind, id: u32) -> Entity {
        Entity {
            id,
            act_index: 1,
            kind,
            name: Some("Test".into()),
            pos: Vec3::default(),
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            name_vis: None,
            face_target: 0,
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

    fn mob(claim_id: u32) -> Entity {
        let mut e = entity(EntityKind::Mob, 0x0200_0001);
        e.claim_id = claim_id;
        e.char_flags.monster = true;
        e
    }

    fn member(id: u32, party_no: u8) -> PartyMember {
        PartyMember {
            id,
            act_index: 0,
            name: Some("Mate".into()),
            hp: 1,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 0,
            main_job: 1,
            main_job_lv: 1,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            party_no,
            in_mog_house: false,
        }
    }

    fn solo(party: &[PartyMember]) -> SelfContext<'_> {
        SelfContext {
            self_id: Some(SELF_ID),
            party,
        }
    }

    #[test]
    fn unclaimed_monster_is_the_mob_row() {
        assert_eq!(
            name_color_choice(&mob(0), solo(&[])),
            NameColorChoice::Row(ncol::MOB)
        );
    }

    #[test]
    fn own_claim_is_the_party_claim_row() {
        assert_eq!(
            name_color_choice(&mob(SELF_ID), solo(&[])),
            NameColorChoice::Row(ncol::CLAIMED_BY_PARTY)
        );
    }

    #[test]
    fn a_strangers_claim_is_the_other_claim_row() {
        assert_eq!(
            name_color_choice(&mob(STRANGER_ID), solo(&[])),
            NameColorChoice::Row(ncol::CLAIMED_BY_OTHER)
        );
    }

    #[test]
    fn a_party_mates_claim_is_the_party_claim_row() {
        let party = [member(SELF_ID, 0), member(MATE_ID, 0)];
        assert_eq!(
            name_color_choice(&mob(MATE_ID), solo(&party)),
            NameColorChoice::Row(ncol::CLAIMED_BY_PARTY)
        );
    }

    #[test]
    fn an_alliance_mate_in_another_party_blends_the_two_claim_rows() {
        let party = [member(SELF_ID, 0), member(MATE_ID, 1)];
        assert_eq!(
            name_color_choice(&mob(MATE_ID), solo(&party)),
            NameColorChoice::Blend(ncol::CLAIMED_BY_PARTY, ncol::CLAIMED_BY_OTHER)
        );
    }

    /// The group list arrives in server order, not with the local player first,
    /// so the comparison must find the player by id.
    #[test]
    fn own_party_number_is_found_wherever_the_player_sits_in_the_list() {
        let party = [member(MATE_ID, 1), member(SELF_ID, 1)];
        assert_eq!(
            name_color_choice(&mob(MATE_ID), solo(&party)),
            NameColorChoice::Row(ncol::CLAIMED_BY_PARTY),
            "both are in party 1, so the claim is a party-mate's"
        );

        let party = [member(MATE_ID, 0), member(SELF_ID, 2)];
        assert_eq!(
            name_color_choice(&mob(MATE_ID), solo(&party)),
            NameColorChoice::Blend(ncol::CLAIMED_BY_PARTY, ncol::CLAIMED_BY_OTHER)
        );
    }

    #[test]
    fn a_dead_monster_is_grey_whoever_claimed_it() {
        let mut m = mob(STRANGER_ID);
        m.hp_pct = Some(0);
        assert_eq!(
            name_color_choice(&m, solo(&[])),
            NameColorChoice::Row(ncol::DEAD)
        );
    }

    #[test]
    fn a_plain_player_is_white_and_a_party_mate_is_the_party_row() {
        let stranger = entity(EntityKind::Pc, STRANGER_ID);
        assert_eq!(
            name_color_choice(&stranger, solo(&[])),
            NameColorChoice::Row(ncol::PC)
        );

        let party = [member(SELF_ID, 0), member(STRANGER_ID, 0)];
        assert_eq!(
            name_color_choice(&stranger, solo(&party)),
            NameColorChoice::Row(ncol::PARTY)
        );
    }

    #[test]
    fn own_plate_is_never_the_party_colour() {
        let me = entity(EntityKind::Pc, SELF_ID);
        let party = [member(SELF_ID, 0)];
        assert_eq!(
            name_color_choice(&me, solo(&party)),
            NameColorChoice::Row(ncol::PC),
            "retail's group walk skips the local player"
        );
    }

    #[test]
    fn a_players_pet_takes_the_party_colour() {
        let mut pet = entity(EntityKind::Pet, 0x0200_0009);
        pet.char_flags.charm = true;
        assert_eq!(
            name_color_choice(&pet, solo(&[])),
            NameColorChoice::Row(ncol::PARTY)
        );
    }

    #[test]
    fn a_charmed_monster_is_not_given_the_pet_colour() {
        // LSB only sets the triggerable PetFlag bit for TYPE_MOB
        // (entity_update.cpp `CEntityUpdatePacket::updateWith`), which is how
        // retail tells a charmed mob from a summoned pet.
        let mut charmed = entity(EntityKind::Mob, 0x0200_000A);
        charmed.char_flags.charm = true;
        charmed.char_flags.pet = true;
        charmed.char_flags.monster = true;
        assert_eq!(
            name_color_choice(&charmed, solo(&[])),
            NameColorChoice::Row(ncol::MOB)
        );
    }

    #[test]
    fn an_npc_is_the_npc_row() {
        assert_eq!(
            name_color_choice(&entity(EntityKind::Npc, 0x0300_0001), solo(&[])),
            NameColorChoice::Row(ncol::NPC)
        );
    }

    /// On 0x0E, `ref<uint32>(0x21) = m_flags` covers Flags1 bits 8..31, so an
    /// ordinary NPC's flag word lights LfgFlag, AnonymousFlag, YellFlag and a
    /// GmLevel at random. None of them may reach the colour: a friendly NPC is
    /// green whatever that word happens to hold.
    #[test]
    fn npc_flag_bytes_are_junk_and_never_recolour_it() {
        let mut npc = entity(EntityKind::Npc, 0x0300_0001);
        npc.char_flags = CharFlags {
            lfg: true,
            lfg_master: true,
            auto_party: true,
            anonymous: true,
            yell: true,
            gm_level: 7,
            linkshell: true,
            away: true,
            ..CharFlags::default()
        };
        assert_eq!(
            name_color_choice(&npc, solo(&[])),
            NameColorChoice::Row(ncol::NPC),
            "m_flags bleeding through Flags1 must not colour an NPC"
        );
    }

    /// The same junk on a mob must leave it the plain mob colour.
    #[test]
    fn mob_flag_bytes_are_junk_and_never_recolour_it() {
        let mut m = mob(0);
        m.char_flags.lfg = true;
        m.char_flags.anonymous = true;
        m.char_flags.gm_level = 5;
        assert_eq!(
            name_color_choice(&m, solo(&[])),
            NameColorChoice::Row(ncol::MOB)
        );
    }

    /// LSB writes STATUS_TYPE over the byte retail reads MonsterFlag from, so
    /// the bit is 0 for a live mob. Colouring must not depend on it.
    #[test]
    fn a_mob_colours_from_its_kind_not_the_monster_bit() {
        let mut unclaimed = mob(0);
        unclaimed.char_flags.monster = false;
        assert_eq!(
            name_color_choice(&unclaimed, solo(&[])),
            NameColorChoice::Row(ncol::MOB),
            "a live mob has MonsterFlag clear under LSB"
        );

        let mut claimed = mob(STRANGER_ID);
        claimed.char_flags.monster = false;
        assert_eq!(
            name_color_choice(&claimed, solo(&[])),
            NameColorChoice::Row(ncol::CLAIMED_BY_OTHER),
            "the claim colour must not need the monster bit"
        );
    }

    /// A GM colour is a PC-only outcome; the bits are junk on any other spawn.
    #[test]
    fn only_players_can_take_a_gm_colour() {
        for kind in [EntityKind::Npc, EntityKind::Mob, EntityKind::Pet] {
            let mut e = entity(kind, 0x0300_0002);
            e.char_flags.gm_level = 5;
            assert_ne!(
                name_color_choice(&e, solo(&[])),
                NameColorChoice::Row(GM_COLOR_INDICES[5]),
                "{kind:?} must not take a GM colour"
            );
        }
    }

    #[test]
    fn gm_levels_outrank_party_membership_but_low_levels_do_not() {
        let party = [member(SELF_ID, 0), member(STRANGER_ID, 0)];
        let mut gm = entity(EntityKind::Pc, STRANGER_ID);

        gm.char_flags.gm_level = MIN_GM_LEVEL - 1;
        assert_eq!(
            name_color_choice(&gm, solo(&party)),
            NameColorChoice::Row(ncol::PARTY),
            "GmLevel below 3 is not a GM colour"
        );

        for level in MIN_GM_LEVEL..=7 {
            gm.char_flags.gm_level = level;
            assert_eq!(
                name_color_choice(&gm, solo(&party)),
                NameColorChoice::Row(GM_COLOR_INDICES[usize::from(level)])
            );
        }
    }

    #[test]
    fn seeking_and_anonymous_rank_below_party_membership() {
        let mut pc = entity(EntityKind::Pc, STRANGER_ID);
        pc.char_flags.lfg = true;
        assert_eq!(
            name_color_choice(&pc, solo(&[])),
            NameColorChoice::Row(ncol::SEEKING)
        );

        let party = [member(SELF_ID, 0), member(STRANGER_ID, 0)];
        assert_eq!(
            name_color_choice(&pc, solo(&party)),
            NameColorChoice::Row(ncol::PARTY)
        );

        let mut anon = entity(EntityKind::Pc, STRANGER_ID);
        anon.char_flags.anonymous = true;
        assert_eq!(
            name_color_choice(&anon, solo(&[])),
            NameColorChoice::Row(ncol::ANONYMOUS)
        );
    }

    #[test]
    fn allegiance_nations_take_their_own_rows() {
        for (allegiance, row) in ALLEGIANCE_COLOR_INDICES {
            let mut pc = entity(EntityKind::Pc, STRANGER_ID);
            pc.char_flags.allegiance = allegiance;
            assert_eq!(
                name_color_choice(&pc, solo(&[])),
                NameColorChoice::Row(row),
                "allegiance {allegiance}"
            );
        }
    }

    #[test]
    fn player_and_mob_allegiances_do_not_take_a_nation_row() {
        for allegiance in [0u8, 1] {
            let mut pc = entity(EntityKind::Pc, STRANGER_ID);
            pc.char_flags.allegiance = allegiance;
            assert_eq!(
                name_color_choice(&pc, solo(&[])),
                NameColorChoice::Row(ncol::PC),
                "allegiance {allegiance}"
            );
        }
    }

    /// Belligerence (base | 0x08 outside the Ferretory) takes no nation row:
    /// retail maps 8/9 to the PC row without returning, so a belligerent player
    /// draws plain white and a belligerent mob keeps its kind colour.
    #[test]
    fn belligerent_allegiances_fall_through_to_the_kind_colour() {
        for allegiance in [BELLIGERENT_MOB_ALLEGIANCE, BELLIGERENT_PLAYER_ALLEGIANCE] {
            let mut pc = entity(EntityKind::Pc, STRANGER_ID);
            pc.char_flags.allegiance = allegiance;
            assert_eq!(
                name_color_choice(&pc, solo(&[])),
                NameColorChoice::Row(ncol::PC),
                "allegiance {allegiance}"
            );

            let mut m = mob(0);
            m.char_flags.allegiance = allegiance;
            assert_eq!(
                name_color_choice(&m, solo(&[])),
                NameColorChoice::Row(ncol::MOB),
                "allegiance {allegiance}"
            );
        }
    }

    /// Retail's 8/9 white is set without returning: a claimed belligerent mob
    /// still takes its claim colour, and a belligerent party mate still takes
    /// the party row.
    #[test]
    fn belligerence_does_not_outrank_claim_or_party() {
        let mut m = mob(MATE_ID);
        m.char_flags.allegiance = BELLIGERENT_MOB_ALLEGIANCE;
        assert_eq!(
            name_color_choice(&m, solo(&[])),
            NameColorChoice::Row(ncol::CLAIMED_BY_OTHER),
            "the claim colour overwrites retail's 8/9 white"
        );

        let party = [member(SELF_ID, 0), member(STRANGER_ID, 0)];
        let mut pc = entity(EntityKind::Pc, STRANGER_ID);
        pc.char_flags.allegiance = BELLIGERENT_PLAYER_ALLEGIANCE;
        assert_eq!(
            name_color_choice(&pc, solo(&party)),
            NameColorChoice::Row(ncol::PARTY),
            "the party walk overwrites retail's 8/9 white"
        );
    }

    /// Nation allegiance OR'd with belligerence (10–14) gets no allegiance
    /// colour at all in retail — the switch leaves it on the dead-row default,
    /// which resets to "no colour".
    #[test]
    fn nation_allegiance_with_belligerence_takes_no_colour() {
        for allegiance in 10..=14 {
            let mut pc = entity(EntityKind::Pc, STRANGER_ID);
            pc.char_flags.allegiance = allegiance;
            assert_eq!(
                name_color_choice(&pc, solo(&[])),
                NameColorChoice::Row(ncol::PC),
                "allegiance {allegiance}"
            );
        }
    }

    #[test]
    fn quad_color_applies_the_legacy_channel_nudge_and_doubles_for_modulate2x() {
        // The real table's white row: 0x7F -> 0x80 -> doubled to full white.
        let white = quad_color([0x7F, 0x7F, 0x7F, 0x7F]).to_srgba();
        assert!((white.red - 1.0).abs() < 1e-6);
        assert!((white.green - 1.0).abs() < 1e-6);
        assert!((white.blue - 1.0).abs() < 1e-6);

        // The claim row: 0x7F4040 -> #FF8282 after the nudge and doubling.
        let claim = quad_color([0x7F, 0x40, 0x40, 0x7F]).to_srgba();
        assert!((claim.red - 1.0).abs() < 1e-6);
        assert!((claim.green - claim.blue).abs() < 1e-6);
        assert!(claim.green > 0.5 && claim.green < 0.55);
    }

    #[test]
    fn saturated_channels_are_not_nudged_past_full() {
        let c = quad_color([u8::MAX, u8::MAX, u8::MAX, u8::MAX]).to_srgba();
        assert!((c.red - 1.0).abs() < 1e-6);
        assert!((c.alpha - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_unloaded_table_resolves_to_the_builtin_rows() {
        let table = NameColorTable::default();
        assert!(!table.is_loaded());
        // A plate's colour is a fact about the entity (kind/status from the
        // live table), not about whether a DAT read has finished: it must
        // resolve on frame one.
        assert_eq!(
            NameColorChoice::Row(ncol::PC).resolve(&table),
            DEFAULT_ROW[ncol::PC]
        );
        assert_eq!(
            NameColorChoice::Row(ncol::NPC).resolve(&table),
            DEFAULT_ROW[ncol::NPC]
        );
        assert_eq!(
            NameColorChoice::Row(ncol::MOB).resolve(&table),
            DEFAULT_ROW[ncol::MOB]
        );

        let blended = NameColorChoice::Blend(ncol::CLAIMED_BY_PARTY, ncol::CLAIMED_BY_OTHER)
            .resolve(&table)
            .to_srgba();
        let (a, b) = (
            DEFAULT_ROW[ncol::CLAIMED_BY_PARTY].to_srgba(),
            DEFAULT_ROW[ncol::CLAIMED_BY_OTHER].to_srgba(),
        );
        assert!((blended.red - (a.red + b.red) * 0.5).abs() < 1e-6);
        assert!((blended.green - (a.green + b.green) * 0.5).abs() < 1e-6);
        assert!((blended.blue - (a.blue + b.blue) * 0.5).abs() < 1e-6);
    }

    #[test]
    fn blend_is_the_per_channel_mean_of_the_two_rows() {
        let mut table = NameColorTable {
            colors: vec![Color::BLACK; NAME_COLOR_COUNT],
            ..Default::default()
        };
        table.colors[ncol::CLAIMED_BY_PARTY] = Color::srgba(1.0, 0.0, 0.0, 1.0);
        table.colors[ncol::CLAIMED_BY_OTHER] = Color::srgba(1.0, 0.0, 1.0, 1.0);
        table.loaded = true;

        let blended = table
            .blend(ncol::CLAIMED_BY_PARTY, ncol::CLAIMED_BY_OTHER)
            .to_srgba();
        assert!((blended.red - 1.0).abs() < 1e-6);
        assert!((blended.green - 0.0).abs() < 1e-6);
        assert!((blended.blue - 0.5).abs() < 1e-6);
    }

    /// Gated on a retail install (self-skips). Pins the real table's shape and
    /// the two rows the claim colours depend on.
    #[test]
    fn real_dat_table_loads_with_retail_claim_colors() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut table = NameColorTable::default();
        let loaded = crate::ui_element_atlas::read_ui_dats(&root)
            .into_iter()
            .any(|(_, bytes)| table.load_from_dat(&bytes));
        assert!(
            loaded,
            "the ncol group must resolve from the retail UI DATs"
        );
        assert_eq!(table.len(), NAME_COLOR_COUNT);

        let party_claim = table.color(ncol::CLAIMED_BY_PARTY).to_srgba();
        assert!(
            party_claim.red > party_claim.green && party_claim.red > party_claim.blue,
            "an own-party claim is retail's red"
        );

        let other_claim = table.color(ncol::CLAIMED_BY_OTHER).to_srgba();
        assert!(
            other_claim.red > other_claim.green && other_claim.blue > other_claim.green,
            "another party's claim is retail's purple"
        );

        let dead = table.color(ncol::DEAD).to_srgba();
        assert!(
            dead.red < 0.75 && dead.red == dead.green && dead.green == dead.blue,
            "the dead row is a neutral grey"
        );
    }
}
