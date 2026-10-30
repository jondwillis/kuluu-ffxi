//! Quest-log name/briefing catalogs, read from the install — never
//! transcribed. Each of the eleven `vendor/server/src/map/enums/quest_log.h`
//! logs has one fixed-block `d_msg` file ([`crate::dmsg::DmsgTable::parse_mission_family`]
//! — same layout as the mission chains) whose blocks self-label with their quest
//! id word at +0x1C: lookups key off that word because block *position* is not
//! trustworthy here — ROM/176/62.DAT (Windurst) lists its first non-blank blocks
//! under words 5, 7, 64, 50…, and the gated test below opens it to prove keyed
//! lookup lands. Briefings are each block's second string slot: retail's quest
//! text windows (`quest01`) draw `Client: …`/`Summary:` notes from it. The
//! English tables' `<dir>/<no>` pairs were measured against the retail install
//! by matching LSB's scraped `scripts/globals/quests.lua` names to block
//! contents; DE/FR twins live under other numbers and kuluu stays on the
//! English set like every other GetText read.

use crate::archive::DatRoot;
use crate::dmsg::{DmsgError, DmsgTable};

/// The log's English catalog is not a fixed-block quest table on this install
/// (unknown log, or an install without the file).
#[derive(Debug, thiserror::Error)]
pub enum QuestCatalog {
    /// No measured client catalog serves this log.
    #[error("no measured quest-name catalog maps this log")]
    Unmapped,
    /// The install's tables do not carry this log's measured file at all.
    #[error("catalog file {dir}/{file}.DAT absent on this install")]
    Absent { dir: u16, file: u8 },
    /// A table parsed but two of its non-blank blocks claim the same id word
    /// with no placeholder between them — a keyed lookup could not pick
    /// between real quests, so the whole catalog is refused. (Placeholder
    /// collisions are allowed; see [`check_quest_id_words`] for the measured law.)
    #[error("catalog blocks disagree on id word {word}")]
    IdsDisagree {
        /// The first shared word found.
        word: u32,
    },
    /// The located file could not be read (permissions, sharing).
    #[error(transparent)]
    Read {
        #[from]
        source: std::io::Error,
    },
    /// The file parses as a fixed-block table but not this family's shape.
    #[error(transparent)]
    Parse(#[from] DmsgError),
}

/// The measured `<dir>/<no>` pair of the English catalog serving `log`
/// (`QuestLog`, `vendor/server/src/map/enums/quest_log.h`). Every log 0..=10
/// has one; the pin coverage is pinned by test below.
pub fn quest_catalog_file(log: u8) -> Option<(u16, u8)> {
    match log {
        0 => Some((176, 60)), // Sandoria (block 0 = "A Sentry's Peril")
        1 => Some((176, 61)), // Bastok
        2 => Some((176, 62)), // Windurst — blocks are scrambled, words key it
        3 => Some((176, 63)), // Jeuno
        4 => Some((176, 64)), // Other Areas
        5 => Some((176, 65)), // Outlands ("Firebloom"; block 0 is a stray "FR Quest 0" slot)
        6 => Some((176, 66)), // Aht Urhgan
        // The Crystal War-era table lives outside the dir-176 set with Wings of
        // the Goddess' mission chain (mission_dat pins ROM/196/7.DAT).
        7 => Some((196, 6)),
        8 => Some((242, 64)), // Abyssea
        // The Adoulin-era pair sits with the (unclaimed) SoA mission table in
        // dir 293; DE/FR twins are 293/73+76 and 293/74+77.
        9 => Some((293, 70)),  // Adoulin
        10 => Some((293, 71)), // Coalition — names carry a literal `+` prefix ("+Procure: …") the scraped lattice drops
        _ => None,
    }
}

/// Read one log's catalog through the install's own tables: locate the measured
/// file, parse it with the fixed-block law, and refuse anything whose blocks do
/// not key unambiguously (see [`check_quest_id_words`]).
pub fn open_quest_catalog(root: &DatRoot, log: u8) -> Result<DmsgTable, QuestCatalog> {
    let (dir, file) = quest_catalog_file(log).ok_or(QuestCatalog::Unmapped)?;
    let loc = root
        .locate(dir, file)
        .ok_or(QuestCatalog::Absent { dir, file })?;
    let bytes = std::fs::read(loc.path_under(root))?;
    let catalog = DmsgTable::parse_mission_family(&bytes)?;
    check_quest_id_words(&catalog).map_err(|word| QuestCatalog::IdsDisagree { word })?;
    Ok(catalog)
}

/// A quest table's real quests key uniquely; the only measured shared words
/// across all eleven tables are word 99 of ROM/176/61.DAT (Bastok) and
/// ROM/176/62.DAT (Windurst), each held only by unassigned placeholder slots
/// naming themselves with a trailing ASCII digit number — `Windurst Quest #NN`
/// or localized `…gNN` junk (the census test pins the shared-word set). So: a
/// repeated id word is refused unless every block claiming it carries that
/// numbering; real names occasionally end in digits too (“The All-New C-2000”)
/// but never share a word on any measured table. Lookups take the first hit
/// either way. Looser than [`crate::mission_dat`]'s chain law only for this.
fn check_quest_id_words(catalog: &DmsgTable) -> Result<(), u32> {
    let mut seen: Vec<(u32, bool)> = Vec::new();
    for i in 0..catalog.len() {
        let Some(strings) = catalog.entry_strings(i) else {
            continue;
        };
        if strings.is_empty() {
            continue;
        }
        let word = catalog.entry_id_word(i).ok_or(u32::MAX)?;
        let numbered = strings
            .first()
            .is_some_and(|name| name.ends_with(|c: char| c.is_ascii_digit()));
        match seen.iter().find(|(w, _)| *w == word) {
            Some((_, prior_numbered)) if !(numbered && *prior_numbered) => {
                return Err(word);
            }
            Some(_) => {}
            None => seen.push((word, numbered)),
        }
    }
    Ok(())
}

/// The name entry `id` carries — retail's own wording ("A Sentry's Peril" with
/// its apostrophe where the scraped lattice had to strip it) — or `None` when no
/// block labels itself `id` or its first string is blank.
pub fn name(catalog: &DmsgTable, id: u32) -> Option<&str> {
    catalog.name_for_id(id)
}

/// The briefing entry `id` carries (`Client: …\nSummary: …` notes), or `None`
/// when the block has none.
pub fn description(catalog: &DmsgTable, id: u32) -> Option<&str> {
    catalog.briefing_for_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pin map covers exactly the eleven `quest_log.h` logs.
    #[test]
    fn every_quest_log_is_pinned() {
        for log in 0u8..=10 {
            assert!(quest_catalog_file(log).is_some(), "log {log} unpinned");
        }
        assert_eq!(quest_catalog_file(11), None);
    }

    /// Gated on a retail install: every pinned file opens under the quest word
    /// law (Windurst only thanks to the placeholder allowance — its trio of
    /// unused blocks share one word), and keyed lookups land where position
    /// would miss. Priority proofs are strings the scraped lattice could not
    /// produce: apostrophes, scrambled positions, raw "+" prefixes.
    #[test]
    fn real_quest_catalogs_name_and_self_label() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let mut catalogs = Vec::new();
        for log in 0u8..=10 {
            match open_quest_catalog(&root, log) {
                Ok(table) => catalogs.push((log, table)),
                Err(e) => panic!("quest catalog {log} must open: {e}"),
            }
        }
        // The shared-word census behind `check_quest_id_words`'s allowance:
        // across all eleven tables the only repeated words are Bastok's and
        // Windurst's word 99 — both held by numbering-suffixed placeholders.
        let mut shared: Vec<(u8, u32)> = catalogs
            .iter()
            .flat_map(|(log, table)| {
                let mut words: Vec<u32> = (0..table.len())
                    .filter(|&i| table.entry_strings(i).is_some_and(|s| !s.is_empty()))
                    .filter_map(|i| table.entry_id_word(i))
                    .collect();
                words.sort_unstable();
                words
                    .windows(2)
                    .filter(|pair| pair[0] == pair[1])
                    .map(move |pair| (*log, pair[0]))
                    .collect::<Vec<_>>()
            })
            .collect();
        shared.dedup();
        assert_eq!(shared, vec![(1u8, 99u32), (2u8, 99u32)]);
        let sandoria = open_quest_catalog(&root, 0).expect("sandoria");
        // The vocab lattice spells this "A Sentrys Peril"; the DAT keeps the apostrophe.
        assert_eq!(name(&sandoria, 0), Some("A Sentry's Peril"));
        // Windurst's id-5 block is not block 5 — position-keying could not find it.
        let windurst = open_quest_catalog(&root, 2).expect("windurst");
        assert_eq!(name(&windurst, 5), Some("In a Pickle"));
        assert!(description(&windurst, 5).is_some_and(|d| d.starts_with("Client: Chamama")));
        let adoulin = open_quest_catalog(&root, 9).expect("adoulin");
        assert_eq!(name(&adoulin, 5), Some("Don't Clam Up on Me Now"));
        // Coalition names carry the DAT's literal "+" prefix (LSB re-types it away); kept raw.
        let coalition = open_quest_catalog(&root, 10).expect("coalition");
        assert_eq!(name(&coalition, 0), Some("+Procure: Ceizak Battlegrounds"));
    }
}
