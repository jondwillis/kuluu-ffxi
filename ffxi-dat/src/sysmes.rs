//! The client's locally composed message tables — the strings retail builds
//! itself rather than receiving as text. Two share this grammar and this
//! composer: the **system-message table** ([`SysMesDat`], every treasure-pool
//! line) and the **basic-message table** ([`MesBasicDat`], the battle log the
//! `MesNo` of a battle-message packet indexes).
//!
//! Same container format as [`crate::dmsg::StringDat`]; what differs is the
//! control-code grammar, which carries substitution slots the zone dialog
//! tables do not use. [`SYS_MES_FILE_ID`] and [`MES_BASIC_FILE_ID`] are
//! empirical — found by scanning for known wording, like the emote table next
//! to them — so each `open` validates the shape rather than trusting the id.
//!
//! Composition returns spans, not a flat string, because retail colours the
//! item-name substitution differently from the text around it — "You find a
//! [pair of bounding boots] on Leaping Lizzy." renders the bracketed item green
//! against white (`.agents/skills/retail-observe/references/treasure-pool-chat.md`,
//! "What the screenshots show").

use crate::dmsg::{
    self, parse_inline_tag, split_alternative, StringDat, ALT_OPEN, CC_AUTO, CC_INLINE_TAG,
    CC_NEWLINE, CC_NUM, INLINE_KIND_STATUS, MARKER_ITEM, MARKER_KEY_ITEM, PRINTABLE,
};

/// The system-message table's file id; `ROM/27/76.DAT` on the horizonxi-2023
/// and retail-2026-09 [`crate::client_profile::KNOWN_CLIENTS`] rows.
pub const SYS_MES_FILE_ID: u32 = 7031;

/// The basic-message table's file id; `ROM/27/72.DAT` on the horizonxi-2023 and
/// retail-2026-09 [`crate::client_profile::KNOWN_CLIENTS`] rows, both resolved
/// through the install's FTABLE.
pub const MES_BASIC_FILE_ID: u32 = 7027;

/// Entry whose NA text is the untranslated placeholder `sysmes262`, used to
/// tell a real system-message table from any other DialogTable that happens to
/// parse.
const SHAPE_PROBE_INDEX: usize = 262;
const SHAPE_PROBE_TEXT: &str = "sysmes";

/// Entry 0 of the basic-message table is the unused `dummy` slot msg number 0
/// maps to, on both measured rows; it tells a real table from any other
/// DialogTable that happens to parse.
const MES_BASIC_PROBE_INDEX: usize = 0;
const MES_BASIC_PROBE_TEXT: &str = "dummy";

/// Leading `0x1F <mode>`: the retail chat-log message type, which selects the
/// line's colour from the player's Config → Font Colors. Distinct from `0x1E`
/// (`CC_SET_COLOR`), which sets an inline colour and appears nowhere in this
/// table.
const CC_LOG_MODE: u8 = 0x1f;
/// `0x1C <n>`: text parameter `n`. POLUtils names this code `ChocoboName`; in
/// the system-message table it is plainly a generic string slot (player names,
/// and the lot value in entry 17).
const CC_TEXT_PARAM: u8 = 0x1c;
/// `0x12 <n>`: numeric parameter `n`, a second family alongside [`CC_NUM`].
const CC_NUM2: u8 = 0x12;

/// `0x01 0x01 <slot>` — a bare substitution slot, sharing its prefix with the
/// emote table's caster/target slots (0x10/0x11).
const SLOT_PREFIX: [u8; 2] = [0x01, 0x01];
/// The a/an article chosen for the item named by the next inline item tag.
const SLOT_ARTICLE: u8 = 0x01;
/// The entity the message is about — the mob or object that dropped the item,
/// or a battle line's target.
const SLOT_TARGET_NAME: u8 = 0x11;
/// The entity the message is by — a battle line's actor.
const SLOT_CASTER_NAME: u8 = 0x10;

/// Resource-name codes: `<code> <n>` names the resource whose id the caller put
/// in message parameter `n`. Which table each resolves against is the caller's
/// business, so composition takes the finished name from
/// [`SysMesParams::names`]; [`MesBasicDat::resource_refs`] reports the pairing.
/// Combat-skill name ("Dagger", "Evasion").
const CC_COMBAT_SKILL: u8 = 0x05;
/// Spell name.
const CC_SPELL: u8 = 0x10;
/// Weapon-skill / mob-skill name.
const CC_WEAPON_SKILL: u8 = 0x16;

// `0x7F <kind> [<param>]` sequences. The emote table's caster-emphasis pair
// (0xFC/0xFB), article alternative (0x88) and terminator (0x31) are shared;
// see [`crate::dmsg`] for those.
/// Capitalize the next substitution.
const AUTO_CAPITALIZE: u8 = 0x80;
/// `[singular/plural]` chosen by numeric parameter `n`.
const AUTO_PLURAL_WORD: u8 = 0x86;
/// `[/s]` — the bare plural suffix, chosen by numeric parameter `n`.
const AUTO_PLURAL_SUFFIX: u8 = 0x92;
/// A gil amount from numeric parameter `n`, rendered with its unit.
const AUTO_GIL: u8 = 0xb4;
/// Job-ability name from message parameter `n`.
const AUTO_JOB_ABILITY: u8 = 0x8f;
/// `[singular/plural]` chosen by the grammatical number of the entity the
/// following clause is about — retail's second branch is the second-person
/// form it prints when that entity is the local player.
const AUTO_SUBJECT_AGREEMENT: u8 = 0x87;
/// A numeric parameter carrying tenths, rendered with one decimal place: the
/// horizonxi-2023 table spells the same slot as a literal `0.` ahead of a bare
/// [`CC_NUM2`], which retail-2026-09 folded into this code.
const AUTO_TENTHS: u8 = 0x9b;
/// Introduces a battle line's result clause and carries no text of its own:
/// the NA table places it at the head of the clause naming what the action did,
/// the JP table (`ROM/27/71.DAT`) at the same point, right after that table's
/// double-arrow separator.
const AUTO_RESULT_CLAUSE: u8 = 0x84;
/// `[his/her]` for the actor and for the target. Two bytes, not three — the
/// gender comes from the entity, not from a parameter
/// ([`crate::dmsg::AUTO_EMOTE_GENDER`] is the actor's, shared with the emote
/// table).
const AUTO_GENDER_TARGET: u8 = 0x91;

/// Tenths per whole unit, for [`AUTO_TENTHS`].
const TENTHS_PER_UNIT: i64 = 10;

/// Retail writes gil amounts with thousands separators.
const GIL_GROUP_DIGITS: usize = 3;
const GIL_UNIT: &str = "gil";

/// Substitution slots addressable by a `<n>` parameter byte. The table's
/// highest observed reference is 4 (entry 219's chevron counts).
pub const PARAM_SLOTS: usize = 8;

/// Which retail colour a composed span takes. Only the item-name family is
/// coloured apart from the line's log-mode colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Text,
    Item,
    KeyItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub kind: SpanKind,
}

/// One composed system message. `lines` holds one entry per retail log line —
/// a `0x07` inside an entry starts a new one rather than wrapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysMesLine {
    pub log_mode: Option<u8>,
    pub lines: Vec<Vec<Span>>,
}

impl SysMesLine {
    /// The whole message as plain text, log lines joined by `\n`. For tests and
    /// for consumers that have nowhere to put colour.
    pub fn to_plain(&self) -> String {
        self.lines
            .iter()
            .map(|spans| spans.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Values substituted into an entry's slots, indexed by the parameter byte the
/// entry references. `items` is pre-resolved by the caller because item-id →
/// name lives outside this crate.
#[derive(Debug, Default, Clone)]
pub struct SysMesParams<'a> {
    pub strings: [Option<&'a str>; PARAM_SLOTS],
    pub numbers: [i64; PARAM_SLOTS],
    pub items: [Option<&'a str>; PARAM_SLOTS],
    pub key_items: [Option<&'a str>; PARAM_SLOTS],
    /// Resource names for the entry's [`MesBasicDat::resource_refs`], indexed
    /// by the same parameter slot the id was read from.
    pub names: [Option<&'a str>; PARAM_SLOTS],
    /// Fills [`SLOT_TARGET_NAME`].
    pub target_name: Option<&'a str>,
    /// Keep the leading article of a `[the /]` alternative — cleared for a
    /// named entity, which retail refers to without "the".
    pub target_article: bool,
    /// Fills [`SLOT_CASTER_NAME`].
    pub caster_name: Option<&'a str>,
    /// [`Self::target_article`] for the actor.
    pub caster_article: bool,
}

pub struct SysMesDat {
    dat: StringDat,
}

impl SysMesDat {
    pub fn open(root: &crate::DatRoot) -> Option<Self> {
        let path = root.resolve(SYS_MES_FILE_ID).ok()?.path_under(root);
        let bytes = std::fs::read(path).ok()?;
        let dat = StringDat::parse(&bytes).ok()?;
        dat.text(SHAPE_PROBE_INDEX)?
            .contains(SHAPE_PROBE_TEXT)
            .then_some(Self { dat })
    }

    pub fn len(&self) -> usize {
        self.dat.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dat.is_empty()
    }

    pub fn message(&self, index: usize, params: &SysMesParams) -> Option<SysMesLine> {
        Some(compose(self.dat.raw(index)?, params).line)
    }
}

/// Which table a [`CC_COMBAT_SKILL`]-family resource code resolves its
/// parameter against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MesBasicResource {
    CombatSkill,
    Spell,
    WeaponSkill,
    JobAbility,
    StatusEffect,
}

/// One resource name a basic-message entry needs: the id sits in message
/// parameter `slot`, and its name belongs in [`SysMesParams::names`] at the
/// same index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MesBasicResourceRef {
    pub kind: MesBasicResource,
    pub slot: usize,
}

/// The battle log's message table, indexed by a battle-message packet's
/// `MesNo`. Entry count is stable across the measured rows but the wording is
/// not, which is why it is read from the install rather than pinned.
pub struct MesBasicDat {
    dat: StringDat,
}

impl MesBasicDat {
    pub fn open(root: &crate::DatRoot) -> Option<Self> {
        let path = root.resolve(MES_BASIC_FILE_ID).ok()?.path_under(root);
        let bytes = std::fs::read(path).ok()?;
        let dat = StringDat::parse(&bytes).ok()?;
        dat.text(MES_BASIC_PROBE_INDEX)?
            .contains(MES_BASIC_PROBE_TEXT)
            .then_some(Self { dat })
    }

    pub fn len(&self) -> usize {
        self.dat.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dat.is_empty()
    }

    pub fn resource_refs(&self, index: usize) -> Vec<MesBasicResourceRef> {
        self.dat
            .raw(index)
            .map(|entry| compose(entry, &SysMesParams::default()).resources)
            .unwrap_or_default()
    }

    /// The message-parameter slots whose item id the entry names inline; the
    /// caller resolves each id and supplies the name in
    /// [`SysMesParams::items`] at the same index.
    pub fn item_refs(&self, index: usize) -> Vec<usize> {
        self.dat
            .raw(index)
            .map(|entry| compose(entry, &SysMesParams::default()).item_slots)
            .unwrap_or_default()
    }

    /// `None` when the entry is absent, empty, or carries a control code this
    /// composer cannot render — a caller with a second wording source must use
    /// it rather than print a line with a hole in it.
    pub fn message(&self, index: usize, params: &SysMesParams) -> Option<SysMesLine> {
        let composed = compose(self.dat.raw(index)?, params);
        (composed.fully_rendered && !composed.line.lines.is_empty()).then_some(composed.line)
    }
}

/// Indices of the treasure-pool messages, read out of `ROM/27/76.DAT`; the
/// horizonxi-2023 and retail-2026-09 tables both hold 326 entries and differ
/// only at entry 317, so the indexes hold on either row.
/// The client picks between the pairs by packet flags: `IsContainer` selects
/// [`FIND_IN`] over [`FIND_ON`], and a zero `LootUniqueNo`/`EntryUniqueNo`
/// selects the first-person wording (research/XiPackets/world/server/0x00D3).
pub mod treasure {
    /// `<name> does not meet the necessary requirements to obtain the <item>.`
    /// plus a second line, `<item> lost.`
    pub const OTHER_INELIGIBLE: usize = 15;
    /// `You find a <item> on [the ]<mob>.`
    pub const FIND_ON: usize = 16;
    /// `<name>'s lot for the <item>: <n> points.`
    pub const LOT: usize = 17;
    /// `<name> obtains a <item>.`
    pub const OBTAINS_ITEM: usize = 18;
    /// `<name> obtains <n> gil.`
    pub const OBTAINS_GIL: usize = 19;
    /// `You do not meet the requirements to obtain the <item>.` plus `<item> lost.`
    pub const YOU_INELIGIBLE: usize = 31;
    /// `You cast lots for the <item>.`
    pub const YOU_CAST_LOTS: usize = 130;
    /// `You obtain a <item>.`
    pub const YOU_OBTAIN: usize = 131;
    /// `A <item> was lost.`
    pub const WAS_LOST: usize = 164;
    /// `You find a <item> in the <container>.`
    pub const FIND_IN: usize = 218;
}

/// Which `[a/b]` alternative the pending `0x7F` code selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alt {
    None,
    /// `[the /]` — keep the first branch for an unnamed entity.
    Article,
    /// `[singular/plural]` or `[/s]`, decided by a numeric parameter.
    Plural(usize),
    /// A branch retail picks from the entity rather than from a parameter:
    /// subject agreement and `[his/her]`. Composition names every entity in the
    /// third person singular, which is the first branch of both.
    Entity,
}

/// What [`compose`] produced, beyond the line itself.
struct Composed {
    line: SysMesLine,
    /// Every control code in the entry was rendered. False once a code this
    /// composer does not know was skipped, so the text has a hole in it.
    fully_rendered: bool,
    resources: Vec<MesBasicResourceRef>,
    /// Parameter slots the entry's inline item tags read, in entry order.
    item_slots: Vec<usize>,
}

struct Composer {
    lines: Vec<Vec<Span>>,
    pending: String,
    alt: Alt,
    capitalize: bool,
    fully_rendered: bool,
    resources: Vec<MesBasicResourceRef>,
    item_slots: Vec<usize>,
}

impl Composer {
    fn new() -> Self {
        Self {
            lines: vec![Vec::new()],
            pending: String::new(),
            alt: Alt::None,
            capitalize: false,
            fully_rendered: true,
            resources: Vec::new(),
            item_slots: Vec::new(),
        }
    }

    /// Record the resource the caller has to name, and substitute whatever it
    /// supplied for that slot.
    fn push_resource(&mut self, kind: MesBasicResource, slot: usize, params: &SysMesParams) {
        self.resources.push(MesBasicResourceRef { kind, slot });
        match params.names.get(slot).copied().flatten() {
            Some(name) => self.push_text(name),
            None => self.fully_rendered = false,
        }
    }

    fn flush_text(&mut self) {
        if !self.pending.is_empty() {
            let text = std::mem::take(&mut self.pending);
            self.current().push(Span {
                text,
                kind: SpanKind::Text,
            });
        }
    }

    fn current(&mut self) -> &mut Vec<Span> {
        self.lines.last_mut().expect("always one open line")
    }

    fn push_text(&mut self, s: &str) {
        let s = self.take_capitalization(s);
        self.pending.push_str(&s);
    }

    fn push_span(&mut self, text: &str, kind: SpanKind) {
        if text.is_empty() {
            return;
        }
        let text = self.take_capitalization(text);
        self.flush_text();
        self.current().push(Span { text, kind });
    }

    fn take_capitalization(&mut self, s: &str) -> String {
        if !self.capitalize {
            return s.to_string();
        }
        self.capitalize = false;
        let mut chars = s.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    fn newline(&mut self) {
        self.flush_text();
        self.lines.push(Vec::new());
    }

    fn finish(mut self) -> Vec<Vec<Span>> {
        self.flush_text();
        self.lines.retain(|l| !l.is_empty());
        self.lines
    }
}

fn compose(entry: &[u8], params: &SysMesParams) -> Composed {
    let mut log_mode = None;
    let mut c = Composer::new();
    let mut i = 0;
    while i < entry.len() {
        let b = entry[i];

        if b == CC_LOG_MODE {
            log_mode = entry.get(i + 1).copied();
            i += 2;
            continue;
        }

        if b == CC_AUTO {
            let Some(&kind) = entry.get(i + 1) else {
                break;
            };
            let param = entry.get(i + 2).copied().unwrap_or(0) as usize;
            match kind {
                dmsg::AUTO_EMOTE_END => break,
                AUTO_CAPITALIZE => c.capitalize = true,
                dmsg::AUTO_EMOTE_TARGET_ARTICLE => c.alt = Alt::Article,
                AUTO_PLURAL_WORD | AUTO_PLURAL_SUFFIX => c.alt = Alt::Plural(param),
                AUTO_GIL => {
                    let n = params.numbers.get(param).copied().unwrap_or(0);
                    c.push_text(&format_gil(n));
                }
                AUTO_TENTHS => {
                    let n = params.numbers.get(param).copied().unwrap_or(0);
                    c.push_text(&format_tenths(n));
                }
                AUTO_JOB_ABILITY => c.push_resource(MesBasicResource::JobAbility, param, params),
                AUTO_SUBJECT_AGREEMENT => c.alt = Alt::Entity,
                AUTO_RESULT_CLAUSE => {}
                // dmsg.rs: the caster-emphasis pair and the gender alternatives carry no
                // parameter of their own.
                dmsg::AUTO_EMOTE_CASTER_OPEN | dmsg::AUTO_EMOTE_CASTER_CLOSE => {
                    i += 2;
                    continue;
                }
                dmsg::AUTO_EMOTE_GENDER | AUTO_GENDER_TARGET => {
                    c.alt = Alt::Entity;
                    i += 2;
                    continue;
                }
                _ => c.fully_rendered = false,
            }
            i += 3;
            continue;
        }

        if entry[i..].starts_with(&SLOT_PREFIX) {
            match entry.get(i + 2) {
                Some(&SLOT_ARTICLE) => {
                    let article = article_for(next_item_name(entry, i, params).unwrap_or_default());
                    c.push_text(article);
                    i += 3;
                    continue;
                }
                Some(&SLOT_TARGET_NAME) => {
                    let name = params.target_name.unwrap_or_default().to_string();
                    c.push_text(&name);
                    i += 3;
                    continue;
                }
                Some(&SLOT_CASTER_NAME) => {
                    let name = params.caster_name.unwrap_or_default().to_string();
                    c.push_text(&name);
                    i += 3;
                    continue;
                }
                _ => {}
            }
        }

        if b == CC_INLINE_TAG {
            // A malformed tag drops only the 0x01, like other control bytes.
            if let Some(tag) = parse_inline_tag(entry, i) {
                let slot = tag.param as usize;
                match tag.marker {
                    Some(MARKER_ITEM) => {
                        c.item_slots.push(slot);
                        let name = params
                            .items
                            .get(slot)
                            .copied()
                            .flatten()
                            .unwrap_or_default();
                        c.push_span(name, SpanKind::Item);
                    }
                    Some(MARKER_KEY_ITEM) => {
                        let name = params
                            .key_items
                            .get(slot)
                            .copied()
                            .flatten()
                            .unwrap_or_default();
                        c.push_span(name, SpanKind::KeyItem);
                    }
                    Some(_) => {
                        let n = params.numbers.get(slot).copied().unwrap_or(0);
                        c.push_text(&n.to_string());
                    }
                    None if tag.kind == INLINE_KIND_STATUS => {
                        c.push_resource(MesBasicResource::StatusEffect, slot, params)
                    }
                    None => c.fully_rendered = false,
                }
                i += tag.len;
                continue;
            }
            i += 1;
            continue;
        }

        if matches!(b, CC_NUM | CC_NUM2) {
            let slot = entry.get(i + 1).copied().unwrap_or(0) as usize;
            let n = params.numbers.get(slot).copied().unwrap_or(0);
            c.push_text(&n.to_string());
            i += 2;
            continue;
        }

        if let Some(kind) = resource_code(b) {
            let slot = entry.get(i + 1).copied().unwrap_or(0) as usize;
            c.push_resource(kind, slot, params);
            i += 2;
            continue;
        }

        if b == CC_TEXT_PARAM {
            let slot = entry.get(i + 1).copied().unwrap_or(0) as usize;
            let s = params
                .strings
                .get(slot)
                .copied()
                .flatten()
                .unwrap_or_default()
                .to_string();
            c.push_text(&s);
            i += 2;
            continue;
        }

        if b == ALT_OPEN && c.alt != Alt::None {
            if let Some((first, second, after)) = split_alternative(&entry[i..]) {
                let keep_first = match c.alt {
                    Alt::Article => next_name_article(entry, i, params),
                    Alt::Plural(slot) => params.numbers.get(slot).copied().unwrap_or(0) == 1,
                    Alt::Entity | Alt::None => true,
                };
                let branch = if keep_first { first } else { second }.to_string();
                c.push_text(&branch);
                c.alt = Alt::None;
                i += after;
                continue;
            }
        }

        if b == CC_NEWLINE {
            c.newline();
        } else if PRINTABLE.contains(&b) {
            let ch = b as char;
            c.push_text(ch.encode_utf8(&mut [0u8; 4]));
        } else if dmsg::is_sjis_lead(b) {
            c.push_text("\u{FFFD}"); // cp932 double-byte run not yet mapped
            c.fully_rendered = false;
            i += 1;
        }
        i += 1;
    }

    let fully_rendered = c.fully_rendered;
    let resources = std::mem::take(&mut c.resources);
    let item_slots = std::mem::take(&mut c.item_slots);
    Composed {
        line: SysMesLine {
            log_mode,
            lines: c.finish(),
        },
        fully_rendered,
        resources,
        item_slots,
    }
}

fn resource_code(b: u8) -> Option<MesBasicResource> {
    match b {
        CC_COMBAT_SKILL => Some(MesBasicResource::CombatSkill),
        CC_SPELL => Some(MesBasicResource::Spell),
        CC_WEAPON_SKILL => Some(MesBasicResource::WeaponSkill),
        _ => None,
    }
}

/// Whether the entity the pending `[the /]` introduces keeps its article. The
/// alternative sits ahead of the name slot it qualifies, and an entry may
/// qualify either entity, so the branch follows the next slot rather than a
/// fixed one.
fn next_name_article(entry: &[u8], from: usize, params: &SysMesParams) -> bool {
    let mut i = from;
    while i + 2 < entry.len() {
        if entry[i..].starts_with(&SLOT_PREFIX) {
            match entry[i + 2] {
                SLOT_CASTER_NAME => return params.caster_article,
                SLOT_TARGET_NAME => return params.target_article,
                _ => {}
            }
        }
        i += 1;
    }
    params.target_article
}

/// The item name the next inline item tag will substitute — the article slot
/// sits ahead of its item, so choosing "a" or "an" means looking forward.
fn next_item_name<'a>(entry: &[u8], from: usize, params: &SysMesParams<'a>) -> Option<&'a str> {
    let mut i = from;
    while i < entry.len() {
        if entry[i] == CC_INLINE_TAG {
            if let Some(tag) = parse_inline_tag(entry, i) {
                if tag.marker == Some(MARKER_ITEM) {
                    return params.items.get(tag.param as usize).copied().flatten();
                }
                i += tag.len;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Retail writes "a lizard tail" and "a pair of bounding boots" — the article
/// follows the item name's leading sound, approximated by its leading letter.
/// An empty name yields "a", matching the entry's fallback spacing.
fn article_for(item_name: &str) -> &'static str {
    const VOWELS: [char; 5] = ['a', 'e', 'i', 'o', 'u'];
    match item_name.chars().next() {
        Some(c) if VOWELS.contains(&c.to_ascii_lowercase()) => "an",
        _ => "a",
    }
}

/// One decimal place, from a value the table carries in tenths.
fn format_tenths(tenths: i64) -> String {
    format!(
        "{}.{}",
        tenths / TENTHS_PER_UNIT,
        (tenths % TENTHS_PER_UNIT).abs()
    )
}

fn format_gil(amount: i64) -> String {
    let mut grouped = String::new();
    let digits = amount.abs().to_string();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(GIL_GROUP_DIGITS) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let sign = if amount < 0 { "-" } else { "" };
    format!("{sign}{grouped} {GIL_UNIT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<Span> {
        vec![Span {
            text: text.to_string(),
            kind: SpanKind::Text,
        }]
    }

    #[test]
    fn article_follows_the_leading_letter() {
        assert_eq!(article_for("lizard tail"), "a");
        assert_eq!(article_for("pair of bounding boots"), "a");
        assert_eq!(article_for("ingot"), "an");
        assert_eq!(article_for("Emperor's Ring"), "an");
        assert_eq!(article_for(""), "a");
    }

    #[test]
    fn gil_is_grouped_in_thousands() {
        assert_eq!(format_gil(0), "0 gil");
        assert_eq!(format_gil(42), "42 gil");
        assert_eq!(format_gil(1_200), "1,200 gil");
        assert_eq!(format_gil(1_234_567), "1,234,567 gil");
    }

    /// `1f 79` log mode, literal text, `1c 02` text param, `0a 00` number.
    #[test]
    fn log_mode_and_params_substitute() {
        let entry = b"\x1fy\x1c\x02 rolls \x0a\x00!";
        let mut p = SysMesParams::default();
        p.strings[2] = Some("Daisy");
        p.numbers[0] = 7;
        let line = compose(entry, &p).line;
        assert_eq!(line.log_mode, Some(0x79));
        assert_eq!(line.to_plain(), "Daisy rolls 7!");
    }

    #[test]
    fn an_item_tag_becomes_its_own_span() {
        // `01 05 27 82 80 80 80` — item name from parameter 0.
        let entry = b"\x1fyYou obtain \x01\x01\x01 \x01\x05'\x82\x80\x80\x80.";
        let mut p = SysMesParams::default();
        p.items[0] = Some("lizard tail");
        let composed = compose(entry, &p);
        assert_eq!(
            composed.item_slots,
            vec![0],
            "the entry reports which parameter slot its item tag reads"
        );
        let line = composed.line;
        assert_eq!(line.lines.len(), 1);
        assert_eq!(
            line.lines[0],
            vec![
                Span {
                    text: "You obtain a ".into(),
                    kind: SpanKind::Text
                },
                Span {
                    text: "lizard tail".into(),
                    kind: SpanKind::Item
                },
                Span {
                    text: ".".into(),
                    kind: SpanKind::Text
                },
            ],
            "the item name must be isolated so it can take retail's green"
        );
    }

    #[test]
    fn article_looks_ahead_to_the_item_it_introduces() {
        let entry = b"You find \x01\x01\x01 \x01\x05'\x82\x80\x80\x80.";
        let mut p = SysMesParams::default();
        p.items[0] = Some("ingot");
        assert_eq!(compose(entry, &p).line.to_plain(), "You find an ingot.");
    }

    #[test]
    fn the_article_alternative_drops_for_a_named_entity() {
        let entry = b"on \x7f\x88\x01[the /]\x01\x01\x11.";
        let mut p = SysMesParams {
            target_name: Some("Leaping Lizzy"),
            target_article: false,
            ..Default::default()
        };
        assert_eq!(compose(entry, &p).line.to_plain(), "on Leaping Lizzy.");
        p.target_article = true;
        p.target_name = Some("Rock Lizard");
        assert_eq!(compose(entry, &p).line.to_plain(), "on the Rock Lizard.");
    }

    #[test]
    fn plural_alternative_follows_its_numeric_parameter() {
        let entry = b"\x12\x00 \x7f\x86\x00[second/seconds] left";
        let mut p = SysMesParams::default();
        p.numbers[0] = 1;
        assert_eq!(compose(entry, &p).line.to_plain(), "1 second left");
        p.numbers[0] = 9;
        assert_eq!(compose(entry, &p).line.to_plain(), "9 seconds left");
    }

    #[test]
    fn a_newline_starts_a_second_log_line() {
        let entry = b"\x1f{first\x07second";
        let line = compose(entry, &SysMesParams::default()).line;
        assert_eq!(line.lines, vec![spans("first"), spans("second")]);
        assert_eq!(line.to_plain(), "first\nsecond");
    }

    #[test]
    fn capitalize_code_uppercases_the_next_substitution() {
        let entry = b"\x7f\x80\x01\x01\x05&\x82\x80\x80\x80 lost.";
        let mut p = SysMesParams::default();
        p.items[0] = Some("lizard tail");
        assert_eq!(compose(entry, &p).line.to_plain(), "Lizard tail lost.");
    }

    #[test]
    fn gil_slot_renders_with_its_unit() {
        let entry = b"\x1f\x7f\x1c\x00 obtains \x7f\xb4\x00.";
        let mut p = SysMesParams::default();
        p.strings[0] = Some("Daisy");
        p.numbers[0] = 1_200;
        let line = compose(entry, &p).line;
        assert_eq!(line.log_mode, Some(0x7f));
        assert_eq!(line.to_plain(), "Daisy obtains 1,200 gil.");
    }

    /// Entry 100's shape: `[The /]<caster> [uses/use] <ability>.`
    #[test]
    fn a_battle_entry_names_both_entities_and_its_resource() {
        let entry = b"\x7f\x88\x00[The /]\x01\x01\x10 \x7f\x87\x00[uses/use] \x7f\x8f\x00 on \x7f\x88\x01[the /]\x01\x01\x11.\x7f1\x00\x07";
        let mut names = [None; PARAM_SLOTS];
        names[0] = Some("Boost");
        let params = SysMesParams {
            names,
            caster_name: Some("Daisy"),
            caster_article: false,
            target_name: Some("Rock Lizard"),
            target_article: true,
            ..Default::default()
        };
        let composed = compose(entry, &params);
        assert!(composed.fully_rendered);
        assert_eq!(
            composed.line.to_plain(),
            "Daisy uses Boost on the Rock Lizard."
        );
        assert_eq!(
            composed.resources,
            vec![MesBasicResourceRef {
                kind: MesBasicResource::JobAbility,
                slot: 0,
            }]
        );
    }

    /// The `[the /]` alternative sits ahead of the slot it qualifies, and an
    /// entry may qualify either entity, so it follows the next name slot.
    #[test]
    fn the_article_alternative_follows_the_entity_it_introduces() {
        let entry = b"\x7f\x88\x01[The /]\x01\x01\x11 hits \x7f\x88\x00[the /]\x01\x01\x10.";
        let params = SysMesParams {
            caster_name: Some("Daisy"),
            caster_article: false,
            target_name: Some("Rock Lizard"),
            target_article: true,
            ..Default::default()
        };
        assert_eq!(
            compose(entry, &params).line.to_plain(),
            "The Rock Lizard hits Daisy."
        );
    }

    #[test]
    fn resource_codes_report_their_slot_and_table() {
        let cases: [(&[u8], MesBasicResource); 4] = [
            (b"\x05\x02", MesBasicResource::CombatSkill),
            (b"\x10\x02", MesBasicResource::Spell),
            (b"\x16\x02", MesBasicResource::WeaponSkill),
            (b"\x7f\x8f\x02", MesBasicResource::JobAbility),
        ];
        for (entry, kind) in cases {
            let mut names = [None; PARAM_SLOTS];
            names[2] = Some("Combo");
            let params = SysMesParams {
                names,
                ..Default::default()
            };
            let composed = compose(entry, &params);
            assert_eq!(composed.line.to_plain(), "Combo");
            assert_eq!(
                composed.resources,
                vec![MesBasicResourceRef { kind, slot: 2 }],
                "entry {entry:?}"
            );
        }
    }

    /// `01 05 13 82 81 80 80` — status effect from parameter 1.
    #[test]
    fn a_status_tag_asks_for_the_effect_name() {
        let entry = b"gains the effect of \x01\x05\x13\x82\x81\x80\x80.";
        let mut names = [None; PARAM_SLOTS];
        names[1] = Some("Protect");
        let params = SysMesParams {
            names,
            ..Default::default()
        };
        let composed = compose(entry, &params);
        assert_eq!(composed.line.to_plain(), "gains the effect of Protect.");
        assert_eq!(
            composed.resources,
            vec![MesBasicResourceRef {
                kind: MesBasicResource::StatusEffect,
                slot: 1,
            }]
        );
    }

    #[test]
    fn tenths_render_with_one_decimal_place() {
        let entry = b"rises \x7f\x9b\x01 points.";
        let mut params = SysMesParams::default();
        params.numbers[1] = 15;
        assert_eq!(compose(entry, &params).line.to_plain(), "rises 1.5 points.");
        params.numbers[1] = 3;
        assert_eq!(compose(entry, &params).line.to_plain(), "rises 0.3 points.");
    }

    /// A code the composer does not know leaves a hole, so the entry must say
    /// so rather than hand back text a caller would print: the merit-name code
    /// (no parameter array here) and an unresolved resource — AUTO_JOB_ABILITY
    /// with a missing parameter — are the same failure from the caller's side.
    #[test]
    fn an_unknown_code_clears_fully_rendered() {
        let known = b"plain text";
        assert!(compose(known, &SysMesParams::default()).fully_rendered);
        let unknown = b"Your \x7f\xb0\x00 rose.";
        assert!(!compose(unknown, &SysMesParams::default()).fully_rendered);
        let unresolved = b"\x7f\x8f\x00";
        assert!(!compose(unresolved, &SysMesParams::default()).fully_rendered);
    }

    #[test]
    fn the_terminator_stops_composition() {
        let entry = b"done.\x7f1\x00\x07trailing";
        assert_eq!(
            compose(entry, &SysMesParams::default()).line.to_plain(),
            "done."
        );
    }

    #[test]
    fn missing_parameters_render_empty_rather_than_panicking() {
        let entry = b"\x1fy\x1c\x07 finds \x01\x05'\x82\x87\x80\x80 at \x0a\x07.";
        let line = compose(entry, &SysMesParams::default()).line;
        assert_eq!(line.to_plain(), " finds  at 0.");
    }
}
