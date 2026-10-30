//! GetText tables by table number: the d_msg catalogs and XISTRING tables the client's menu
//! windows, client messages and prompts read their strings from
//! (.agents/skills/retail-observe/references/2026-10-15-gettext-tables.md). Text is read from the
//! install, never transcribed.

use std::collections::BTreeMap;

use ffxi_dat::gettext::{GetTextFile, GetTextFormat, LanguageRow};
use ffxi_dat::xistring::{
    ItemNameRules, TemplateArgs, TemplateContext, UntracedItemNames, XiStringTable,
    PROMPT_BUFFER_BYTES,
};

/// GetText table 0, the sub-templates the table 0x0A prompts can pull in.
pub const GETTEXT_TABLE_SUB_TEMPLATES: u8 = 0;
/// GetText table 7, the client-message catalog.
pub const GETTEXT_TABLE_CLIENT_MESSAGES: u8 = 7;
/// GetText table 9, the chat-message catalog.
pub const GETTEXT_TABLE_9: u8 = 9;
/// GetText table 0x0A, the `merityn` prompt templates.
pub const GETTEXT_TABLE_10: u8 = 10;
/// GetText table 12.
pub const GETTEXT_TABLE_12: u8 = 12;
/// GetText table 0x0D, the short caption catalog.
pub const GETTEXT_TABLE_SHORT_CAPTIONS: u8 = 0x0D;
/// GetText table 0x0E, the caption catalog.
pub const GETTEXT_TABLE_CAPTIONS: u8 = 0x0E;

/// The language row kuluu reads every table from. The record does not say where the PlayOnline
/// host takes the language and its flag from, so kuluu stays on the English row.
pub const KULUU_LANGUAGE_ROW: LanguageRow = LanguageRow::English;

/// The file a GetText table reads on [`KULUU_LANGUAGE_ROW`].
pub fn gettext_table_file(table: u8) -> Option<GetTextFile> {
    ffxi_dat::gettext::gettext_file(table, KULUU_LANGUAGE_ROW)
}

/// Reads one string out of a GetText table.
pub trait GetText {
    fn get_text(&self, table: u8, id: u16) -> Option<&str>;
}

/// GetText tables loaded from an install: every table's entries as text, and each XISTRING
/// table's raw entries for the template formatter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GetTextTables {
    tables: BTreeMap<u8, Vec<String>>,
    templates: BTreeMap<u8, XiStringTable>,
}

/// Every string of a d_msg catalog, by entry index.
pub fn dmsg_strings(table: &ffxi_dat::dmsg::DmsgTable) -> Vec<String> {
    (0..table.len())
        .map(|i| table.text(i).unwrap_or_default().to_string())
        .collect()
}

/// Catalog bytes as kuluu shows them, one char per byte as [`ffxi_dat::dmsg::DmsgTable`] reads
/// its strings; [`catalog_bytes`] turns such text back into its bytes.
fn catalog_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn catalog_bytes(text: &str) -> Vec<u8> {
    text.chars().filter_map(|c| u8::try_from(c).ok()).collect()
}

impl GetTextTables {
    /// Loads every table in `tables` the install resolves, each by its own format; a missing or
    /// unreadable file leaves that table empty.
    pub fn load(dat: &ffxi_dat::archive::DatRoot, tables: &[u8]) -> Self {
        let mut loaded = Self::default();
        for &table in tables {
            let Some(file) = gettext_table_file(table) else {
                continue;
            };
            let Some(bytes) = dat
                .resolve(file.file_id)
                .ok()
                .and_then(|loc| std::fs::read(loc.path_under(dat)).ok())
            else {
                continue;
            };
            match file.format {
                GetTextFormat::DMsg => {
                    if let Ok(catalog) = ffxi_dat::dmsg::DmsgTable::parse(&bytes) {
                        loaded.tables.insert(table, dmsg_strings(&catalog));
                    }
                }
                GetTextFormat::XiString => {
                    if let Ok(xistring) = XiStringTable::parse(&bytes) {
                        loaded.insert_xistring(table, xistring);
                    }
                }
            }
        }
        loaded
    }

    /// Sets one entry, growing its table as needed.
    pub fn insert(&mut self, table: u8, id: u16, text: impl Into<String>) {
        let entries = self.tables.entry(table).or_default();
        let at = usize::from(id);
        if entries.len() <= at {
            entries.resize(at + 1, String::new());
        }
        entries[at] = text.into();
    }

    /// Sets a whole XISTRING table: its entries' plain text, escapes included, and the raw
    /// entries [`Self::format_entry`] formats.
    pub fn insert_xistring(&mut self, table: u8, xistring: XiStringTable) {
        let plain = (0..xistring.len())
            .map(|id| {
                xistring
                    .entry(id)
                    .map(|entry| catalog_text(entry.text()))
                    .unwrap_or_default()
            })
            .collect();
        self.tables.insert(table, plain);
        self.templates.insert(table, xistring);
    }

    /// Entry `id` of XISTRING table `table` formatted with the template escapes into a
    /// `buffer_bytes` buffer: `strings` are string arguments 0, 1, ..., `numbers` numeric
    /// parameters 1, 2, .... `None` when the table or the entry does not resolve.
    pub fn format_entry(
        &self,
        table: u8,
        id: u16,
        strings: &[&str],
        numbers: &[i32],
        buffer_bytes: usize,
    ) -> Option<String> {
        let entry = self.templates.get(&table)?.entry(usize::from(id))?;
        let string_bytes: Vec<Vec<u8>> = strings.iter().map(|s| catalog_bytes(s)).collect();
        let strings: Vec<&[u8]> = string_bytes.iter().map(Vec::as_slice).collect();
        let cx = TemplateContext {
            sub_templates: self.templates.get(&GETTEXT_TABLE_SUB_TEMPLATES),
            item_names: &UntracedItemNames,
            item_rules: ItemNameRules::of(KULUU_LANGUAGE_ROW),
        };
        let args = TemplateArgs {
            strings: &strings,
            numbers,
        };
        Some(catalog_text(&ffxi_dat::xistring::format_template(
            entry,
            args,
            &cx,
            buffer_bytes,
        )))
    }
}

impl GetText for GetTextTables {
    fn get_text(&self, table: u8, id: u16) -> Option<&str> {
        self.tables
            .get(&table)?
            .get(usize::from(id))
            .map(String::as_str)
    }
}

impl GetText for BTreeMap<(u8, u16), String> {
    fn get_text(&self, table: u8, id: u16) -> Option<&str> {
        self.get(&(table, id)).map(String::as_str)
    }
}

/// One argument a [`GETTEXT_TABLE_10`] prompt template takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptArg {
    /// A string argument: a DAT name.
    Name(String),
    /// A numeric parameter: a point cost.
    Points(u16),
}

/// A `merityn` prompt: the [`GETTEXT_TABLE_10`] entry holding its template and the arguments the
/// template takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    pub entry: u16,
    pub args: Vec<PromptArg>,
}

impl ConfirmPrompt {
    /// The entry formatted into the prompts' buffer with the names as string arguments and the
    /// costs as numeric parameters, each in order. An entry the install lacks shows its id.
    pub fn text(&self, texts: &GetTextTables) -> String {
        let strings: Vec<&str> = self
            .args
            .iter()
            .filter_map(|arg| match arg {
                PromptArg::Name(name) => Some(name.as_str()),
                PromptArg::Points(_) => None,
            })
            .collect();
        let numbers: Vec<i32> = self
            .args
            .iter()
            .filter_map(|arg| match arg {
                PromptArg::Points(points) => Some(i32::from(*points)),
                PromptArg::Name(_) => None,
            })
            .collect();
        texts
            .format_entry(
                GETTEXT_TABLE_10,
                self.entry,
                &strings,
                &numbers,
                PROMPT_BUFFER_BYTES,
            )
            .unwrap_or_else(|| {
                crate::hud::unity_fp::client_message_fallback(GETTEXT_TABLE_10, self.entry)
            })
    }
}

/// Fills a GetText string used as a printf template: each `%d`/`%u`/`%s`-style conversion takes
/// the next argument in order, `%%` prints `%`, and a template with no conversion is returned
/// unchanged.
pub fn format_template(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        let mut spec = String::from('%');
        let mut converted = false;
        while let Some(&n) = chars.peek() {
            chars.next();
            spec.push(n);
            if n.is_ascii_alphabetic() {
                converted = true;
                break;
            }
        }
        match (converted, args.next()) {
            (true, Some(arg)) => out.push_str(arg),
            _ => out.push_str(&spec),
        }
    }
    out
}

/// A synthetic table 0x0A for the prompt tests: each prompt entry reads `<entry>:<name>`, then
/// `/<cost>` where the record gives the prompt a cost.
#[cfg(test)]
pub(crate) mod test_support {
    use ffxi_dat::xistring::{code, operand, xistring_bytes, XiStringTable, ESCAPE_LEAD};

    use crate::hud::merit_windows::{LOWER_PROMPT_ENTRY, RAISE_PROMPT_ENTRY, SPEND_PROMPT_ENTRY};
    use crate::hud::unity_fp::ALTER_EGO_PROMPT_ENTRY;

    /// Any valid second byte for an escape's introducing pair.
    const PAIR_TRAIL: u8 = 0x40;

    fn escape(escape_code: u8, operand: [u8; 2]) -> Vec<u8> {
        [vec![ESCAPE_LEAD, PAIR_TRAIL, escape_code], operand.to_vec()].concat()
    }

    /// The entries and whether each takes a cost.
    pub const PROMPTS: [(u16, bool); 4] = [
        (RAISE_PROMPT_ENTRY, true),
        (LOWER_PROMPT_ENTRY, false),
        (SPEND_PROMPT_ENTRY, true),
        (ALTER_EGO_PROMPT_ENTRY, true),
    ];

    pub fn prompt_templates() -> XiStringTable {
        let name = escape(code::STRING_ARGUMENT, [operand::VALUE, 0]);
        let cost = escape(code::DECIMAL, [operand::PARAMETER, 1]);
        let last = PROMPTS.iter().map(|&(entry, _)| entry).max().unwrap_or(0);
        let mut entries = vec![(Vec::new(), 0i16); usize::from(last) + 1];
        for (entry, with_cost) in PROMPTS {
            let mut template = [format!("{entry}:").into_bytes(), name.clone()].concat();
            if with_cost {
                template.push(b'/');
                template.extend_from_slice(&cost);
            }
            entries[usize::from(entry)] = (template, i16::from(with_cost));
        }
        let entries: Vec<(&[u8], i16)> = entries
            .iter()
            .map(|(template, count)| (template.as_slice(), *count))
            .collect();
        XiStringTable::parse(&xistring_bytes(&entries)).expect("synthetic table 0x0A")
    }

    /// The text [`prompt_templates`] gives `entry` with `name` and `cost`.
    pub fn prompt_text(entry: u16, name: &str, cost: Option<u16>) -> String {
        match cost {
            Some(cost) => format!("{entry}:{name}/{cost}"),
            None => format!("{entry}:{name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{prompt_templates, prompt_text, PROMPTS};
    use super::*;
    use crate::hud::unity_fp::client_message_fallback;

    fn prompt_tables() -> GetTextTables {
        let mut texts = GetTextTables::default();
        texts.insert_xistring(GETTEXT_TABLE_10, prompt_templates());
        texts
    }

    #[test]
    fn template_takes_arguments_in_order() {
        assert_eq!(format_template("No.%d", &["4"]), "No.4");
        assert_eq!(format_template("%s for %dP?", &["HP", "2"]), "HP for 2P?");
        assert_eq!(format_template("100%% %s", &["done"]), "100% done");
        assert_eq!(format_template("plain", &["unused"]), "plain");
        assert_eq!(
            format_template("%d and %d", &["1"]),
            "1 and %d",
            "a conversion without an argument stays as written"
        );
    }

    /// Raise, lower, job point spend and alter ego upgrade each format their own entry: the name
    /// as string argument 0 and, where there is one, the cost as numeric parameter 1.
    #[test]
    fn the_four_prompts_format_their_entries() {
        let texts = prompt_tables();
        const COST: u16 = 3;
        for (entry, with_cost) in PROMPTS {
            let mut args = vec![PromptArg::Name("Max HP".into())];
            if with_cost {
                args.push(PromptArg::Points(COST));
            }
            assert_eq!(
                ConfirmPrompt { entry, args }.text(&texts),
                prompt_text(entry, "Max HP", with_cost.then_some(COST)),
                "entry {entry}"
            );
        }
    }

    #[test]
    fn a_prompt_binds_its_names_before_its_costs() {
        let texts = prompt_tables();
        let (entry, _) = PROMPTS[0];
        let prompt = |args| ConfirmPrompt { entry, args }.text(&texts);
        assert_eq!(
            prompt(vec![PromptArg::Points(7), PromptArg::Name("n".into())]),
            prompt(vec![PromptArg::Name("n".into()), PromptArg::Points(7)])
        );
    }

    #[test]
    fn names_reach_the_formatter_byte_for_byte() {
        let texts = prompt_tables();
        let (entry, _) = PROMPTS[1];
        let name = "\u{e9}t\u{e9} \u{fa}\u{40}";
        let prompt = ConfirmPrompt {
            entry,
            args: vec![PromptArg::Name(name.into())],
        };
        assert_eq!(prompt.text(&texts), prompt_text(entry, name, None));
    }

    #[test]
    fn a_prompt_the_install_lacks_shows_its_id() {
        let (entry, _) = PROMPTS[0];
        let prompt = ConfirmPrompt {
            entry,
            args: vec![PromptArg::Name("n".into()), PromptArg::Points(1)],
        };
        assert_eq!(
            prompt.text(&GetTextTables::default()),
            client_message_fallback(GETTEXT_TABLE_10, entry)
        );
    }

    /// A plain lookup in an XISTRING table returns the entry's raw text, escapes included.
    #[test]
    fn an_xistring_table_answers_plain_lookups_raw() {
        let texts = prompt_tables();
        let (entry, _) = PROMPTS[1];
        let raw = texts.get_text(GETTEXT_TABLE_10, entry).unwrap();
        assert!(raw.starts_with(&format!("{entry}:")));
        assert!(raw.contains(char::from(ffxi_dat::xistring::ESCAPE_LEAD)));
        assert_eq!(texts.get_text(GETTEXT_TABLE_10, 0), Some(""));
    }

    #[test]
    fn every_table_reads_its_english_row_file() {
        for table in 0..=u8::MAX {
            assert_eq!(
                gettext_table_file(table),
                ffxi_dat::gettext::gettext_file(table, LanguageRow::English)
            );
        }
        for table in [GETTEXT_TABLE_SUB_TEMPLATES, GETTEXT_TABLE_10] {
            assert_eq!(
                gettext_table_file(table).map(|file| file.format),
                Some(GetTextFormat::XiString)
            );
        }
        assert_eq!(
            gettext_table_file(GETTEXT_TABLE_12).map(|file| file.file_id),
            Some(ffxi_dat::menu_windows::GETTEXT_TABLE_12_CATALOG_FILE_ID)
        );
        assert_eq!(
            gettext_table_file(GETTEXT_TABLE_CLIENT_MESSAGES).map(|file| file.file_id),
            Some(ffxi_dat::menu_windows::GETTEXT_TABLE_7_CATALOG_FILE_ID)
        );
    }

    #[test]
    fn inserted_entries_answer_and_gaps_read_empty() {
        let mut tables = GetTextTables::default();
        tables.insert(GETTEXT_TABLE_12, 3, "three");
        assert_eq!(tables.get_text(GETTEXT_TABLE_12, 3), Some("three"));
        assert_eq!(tables.get_text(GETTEXT_TABLE_12, 1), Some(""));
        assert_eq!(tables.get_text(GETTEXT_TABLE_12, 4), None);
        assert_eq!(tables.get_text(GETTEXT_TABLE_9, 3), None);
    }

    #[test]
    fn map_source_answers_by_table_and_id() {
        let mut texts = BTreeMap::new();
        texts.insert((GETTEXT_TABLE_12, 395), "title".to_string());
        assert_eq!(texts.get_text(GETTEXT_TABLE_12, 395), Some("title"));
        assert_eq!(texts.get_text(GETTEXT_TABLE_9, 395), None);
    }
}
