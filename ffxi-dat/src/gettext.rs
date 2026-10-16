//! GetText tables by table number and language row: the file each table reads and whether it is
//! a d_msg catalog or an XISTRING table
//! (.agents/skills/retail-observe/references/2026-10-15-gettext-tables.md). Every id resolves
//! through the install's VTABLE/FTABLE like any other file id.

use crate::menu_windows::{
    CAPTION_CATALOG_TABLE_14_FILE_ID, CAPTION_SHORT_CATALOG_FILE_ID,
    GETTEXT_TABLE_12_CATALOG_FILE_ID, GETTEXT_TABLE_7_CATALOG_FILE_ID,
    GETTEXT_TABLE_9_CATALOG_FILE_ID,
};

/// The client's language value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Language {
    Japanese = 0,
    English = 1,
    German = 2,
    French = 3,
}

/// The language row a table's file is read from; it also picks the item-name grammar
/// ([`crate::xistring::ItemNameRules`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageRow {
    Japanese,
    English,
    German,
    French,
}

impl LanguageRow {
    /// Japanese and English read their own rows whatever the flag says; German and French read
    /// theirs with `english_flag` clear and the English row with it set.
    pub fn of(language: Language, english_flag: bool) -> Self {
        match language {
            Language::Japanese => Self::Japanese,
            Language::English => Self::English,
            Language::German | Language::French if english_flag => Self::English,
            Language::German => Self::German,
            Language::French => Self::French,
        }
    }

    fn column(self) -> usize {
        match self {
            Self::Japanese => 0,
            Self::English => 1,
            Self::German => 2,
            Self::French => 3,
        }
    }
}

/// How a GetText table's file is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetTextFormat {
    /// A d_msg catalog, [`crate::dmsg::DmsgTable`].
    DMsg,
    /// An XISTRING table, [`crate::xistring::XiStringTable`].
    XiString,
}

/// The file one GetText table reads on one language row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetTextFile {
    pub file_id: u32,
    pub format: GetTextFormat,
}

/// The tables whose files are XISTRING tables; every other table with a file is a d_msg catalog.
pub const XISTRING_TABLES: [u8; 3] = [0, 8, 0x0A];

const LANGUAGE_ROWS: usize = 4;

/// Tables 0 through 0x1B. Table 0x1C, the last the client defines, is never loaded.
const TABLES_WITH_FILES: usize = 0x1C;

const fn row(
    japanese: u32,
    english: u32,
    german: u32,
    french: u32,
) -> [Option<u32>; LANGUAGE_ROWS] {
    [Some(japanese), Some(english), Some(german), Some(french)]
}

const fn same(file_id: u32) -> [Option<u32>; LANGUAGE_ROWS] {
    row(file_id, file_id, file_id, file_id)
}

/// File ids by table, in the column order of [`LanguageRow`]; `None` where the table is not
/// loaded on that row.
const TABLE_FILES: [[Option<u32>; LANGUAGE_ROWS]; TABLES_WITH_FILES] = [
    [Some(45), Some(63), None, None],
    row(55470, 55470, 55782, 56202),
    same(55468),
    row(55467, 55467, 55776, 56196),
    same(55466),
    same(55465),
    row(55469, 55469, 55783, 56203),
    row(55526, GETTEXT_TABLE_7_CATALOG_FILE_ID, 55766, 56186),
    row(30, 48, 55767, 56187),
    row(55528, GETTEXT_TABLE_9_CATALOG_FILE_ID, 55768, 56188),
    row(32, 50, 55769, 56189),
    row(55530, 55650, 55770, 56190),
    row(55533, GETTEXT_TABLE_12_CATALOG_FILE_ID, 55773, 56193),
    row(55532, CAPTION_SHORT_CATALOG_FILE_ID, 55772, 56192),
    row(55531, CAPTION_CATALOG_TABLE_14_FILE_ID, 55771, 56191),
    row(55539, 55659, 55779, 56199),
    row(55538, 55658, 55778, 56198),
    row(55540, 55660, 55780, 56200),
    row(55536, 55467, 55776, 56196),
    row(55534, 55654, 55774, 56194),
    row(55535, 55465, 55775, 56195),
    row(55535, 55661, 55781, 56201),
    row(55560, 55680, 55800, 56220),
    row(55546, 55666, 55786, 56206),
    same(55471),
    same(55472),
    row(55559, 55679, 55799, 56219),
    row(55525, 55645, 55765, 56185),
];

/// The file GetText table `table` reads on `row`, `None` when the table has no file there (table
/// 0 on the German and French rows, table 0x1C, and every id the client does not define).
pub fn gettext_file(table: u8, row: LanguageRow) -> Option<GetTextFile> {
    let file_id = TABLE_FILES.get(usize::from(table))?[row.column()]?;
    let format = if XISTRING_TABLES.contains(&table) {
        GetTextFormat::XiString
    } else {
        GetTextFormat::DMsg
    };
    Some(GetTextFile { file_id, format })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: [LanguageRow; LANGUAGE_ROWS] = [
        LanguageRow::Japanese,
        LanguageRow::English,
        LanguageRow::German,
        LanguageRow::French,
    ];

    fn file_id(table: u8, row: LanguageRow) -> Option<u32> {
        gettext_file(table, row).map(|file| file.file_id)
    }

    #[test]
    fn only_german_and_french_follow_the_english_flag() {
        for flag in [false, true] {
            assert_eq!(
                LanguageRow::of(Language::Japanese, flag),
                LanguageRow::Japanese
            );
            assert_eq!(
                LanguageRow::of(Language::English, flag),
                LanguageRow::English
            );
        }
        assert_eq!(
            LanguageRow::of(Language::German, false),
            LanguageRow::German
        );
        assert_eq!(
            LanguageRow::of(Language::French, false),
            LanguageRow::French
        );
        assert_eq!(
            LanguageRow::of(Language::German, true),
            LanguageRow::English
        );
        assert_eq!(
            LanguageRow::of(Language::French, true),
            LanguageRow::English
        );
    }

    #[test]
    fn language_values_pin() {
        assert_eq!(Language::Japanese as u8, 0);
        assert_eq!(Language::English as u8, 1);
        assert_eq!(Language::German as u8, 2);
        assert_eq!(Language::French as u8, 3);
    }

    /// Tables 0, 8 and 0x0A read XISTRING files on every row that loads them; table 0 has none
    /// on the German and French rows.
    #[test]
    fn xistring_tables_read_their_record_files() {
        const RECORD_PINNED_TABLE_0: [Option<u32>; 4] = [Some(45), Some(63), None, None];
        const RECORD_PINNED_TABLE_8: [Option<u32>; 4] =
            [Some(30), Some(48), Some(55767), Some(56187)];
        const RECORD_PINNED_TABLE_10: [Option<u32>; 4] =
            [Some(32), Some(50), Some(55769), Some(56189)];
        for (table, files) in [
            (0, RECORD_PINNED_TABLE_0),
            (8, RECORD_PINNED_TABLE_8),
            (0x0A, RECORD_PINNED_TABLE_10),
        ] {
            for (row, file) in ROWS.into_iter().zip(files) {
                assert_eq!(file_id(table, row), file, "table {table:#x} on {row:?}");
                if file.is_some() {
                    assert_eq!(
                        gettext_file(table, row).map(|f| f.format),
                        Some(GetTextFormat::XiString)
                    );
                }
            }
        }
    }

    #[test]
    fn every_other_table_up_to_0x1b_is_a_dmsg_catalog_on_every_row() {
        for table in 0..=0x1B_u8 {
            if XISTRING_TABLES.contains(&table) {
                continue;
            }
            for row in ROWS {
                assert_eq!(
                    gettext_file(table, row).map(|f| f.format),
                    Some(GetTextFormat::DMsg),
                    "table {table:#x} on {row:?}"
                );
            }
        }
    }

    #[test]
    fn the_tables_kuluu_already_reads_keep_their_english_files() {
        for (table, file) in [
            (7, GETTEXT_TABLE_7_CATALOG_FILE_ID),
            (9, GETTEXT_TABLE_9_CATALOG_FILE_ID),
            (0x0C, GETTEXT_TABLE_12_CATALOG_FILE_ID),
            (0x0D, CAPTION_SHORT_CATALOG_FILE_ID),
            (0x0E, CAPTION_CATALOG_TABLE_14_FILE_ID),
        ] {
            assert_eq!(file_id(table, LanguageRow::English), Some(file));
        }
    }

    #[test]
    fn some_rows_reuse_another_tables_file() {
        let english = LanguageRow::English;
        assert_eq!(file_id(0x12, english), file_id(3, english));
        assert_eq!(file_id(0x14, english), file_id(5, english));
        assert_eq!(
            file_id(0x14, LanguageRow::Japanese),
            file_id(0x15, LanguageRow::Japanese)
        );
        for table in [2, 4, 5, 0x18, 0x19] {
            let japanese = file_id(table, LanguageRow::Japanese);
            assert!(japanese.is_some());
            for row in ROWS {
                assert_eq!(
                    file_id(table, row),
                    japanese,
                    "table {table:#x} is one file"
                );
            }
        }
    }

    #[test]
    fn nothing_resolves_from_table_0x1c_on() {
        for table in 0x1C..=u8::MAX {
            for row in ROWS {
                assert_eq!(gettext_file(table, row), None, "table {table:#x}");
            }
        }
    }
}
