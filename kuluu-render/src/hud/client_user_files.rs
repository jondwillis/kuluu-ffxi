//! The player's own per-user macro and equipment-set files, read straight out of the install.
//!
//! Macros never ride the wire (LSB ships no macro packet id in
//! `vendor/server/src/map/enums/packet_c2s.h` or `packet_s2c.h`), so everything these windows show
//! comes from the client's own files under `<install>/USER/<id>/`:
//!
//! * `mcr.ttl` / `mcr_2.ttl` - the forty book names, twenty per file (books 1-20 then 21-40).
//! * `mcr.dat`, `mcr<N>.dat` - one book of twenty entries each. The name is built from parts in
//!   FFXiMain.dll's string pool (`%s%s%d%s` at file offset 0x51E64, then `mcr` at 0x51E70 and `.dat`
//!   at 0x51E74): the base name carries no number, every other book is addressed by its own.
//! * `es0.dat`..`es9.dat` - the two hundred equipment sets, twenty per page. A set's own index picks its
//!   file: FFXiMain.dll's page picker (retail-2026-10 RVA 0x1A84E0) divides the index by twenty and
//!   refuses everything from the tenth page up, then builds the name out of `es`, that number and `.dat`
//!   with the `%s%d%s` template at file offset 0x51F28. So a profile owning `es0/1/2/8/9` has sets
//!   1-60 and 161-200, exactly as the names on those files read.
//!
//! Byte layout of each file (measured against the three user directories of the reference install;
//! the arithmetic checks in [`tests`] re-derive it from the same shapes):
//!
//! * All three families open with 24 bytes: `u32` version (1), one further `u32` whose meaning is
//!   unknown and which retail leaves zero on plenty of files, then **the MD5 of everything from byte 24
//!   to the end of the file**. That digest was checked over every user file of all three profiles
//!   (macros, book titles, equip-set names, and most of the rest of the directory) and it holds; so a
//!   writer that changes records must recompute it, or the client no longer recognises the file and
//!   what the player typed is gone on the next load. See [`sign_user_file`].
//! * A title file follows with twenty 16-byte names, zero padded.
//! * A macro file follows with twenty entries of exactly 380 bytes: an unused `u32`, then SIX separate
//!   command fields of 61 bytes each (NUL-terminated, one command per field), then the display name as
//!   ten bytes, so a nine-character name fills that field. The commands are six independent slots and
//!   not one newline-separated block: no macro file on that install contains a single `0x0A` byte, while
//!   its shipped multi-command keytops (`JoinPT`, `SoloPlay`, `Entice`) fill slot 0, then 1, then 2,
//!   each ending at its own terminator. Windower's POLUtils reads and writes them the same way
//!   (`vendor/POLUtils/PlayOnline.FFXI.Utils.MacroManager/Macro.cs::WriteToDATFile`), which is the tool
//!   players have used on live retail for years.
//! * A set file follows with twenty records of exactly 80 bytes: the display name as sixteen bytes, then
//!   sixty-four further bytes that hold what the set wears. Both halves come from FFXiMain.dll's own set
//!   record code (retail-2026-10): the record getter at RVA 0x1A896D walks a page as `base + idx*0x50`,
//!   and the whole-record copy at RVA 0x1A85D0 first runs the name through the setter at RVA 0x1A73F0
//!   (which rejects anything fifteen characters or longer before it clears sixteen bytes) and then moves
//!   the remaining sixteen dwords from record offset 0x10. So a name is sixteen bytes with a terminator,
//!   `Set200` included. The sixty-four-byte half holds the set's contents: sixteen four-byte entries, one per
//!   equipment position, in the shape its wire entries have too (`equipsetrequestitem_t`, read and written
//!   through [`ffxi_proto::equipset::EquipSetEntry`] as established by
//!   `.agents/skills/retail-observe/references/2026-10-25-equip-set-apply.md`). A whole-record copy needs
//!   nothing of that shape, so [`copy_equipment_set`] still moves all eighty bytes; [`equip_set_contents`] and
//!   [`set_equip_set_contents`] are the entry views.
//!
//! A book only owns a file once something is in it, which is why profile `985` of the reference install
//! ships `mcr.dat` and nothing else while `722` also owns `mcr10/20/30/40.dat`. POLUtils follows the same
//! rule in reverse (`MacroSet::Save` opens with `OpenOrCreate`, writes a fresh version word and digest,
//! and deletes the file outright when its set comes back empty), so creating a book's file on the first
//! real edit is ordinary: see [`write_macro_book_file`].
//!
//! The four `mcrselop` book arms ([`set_book_title`], [`copy_macro_book`], [`paste_macro_book`],
//! [`delete_macro_book`]) write back into these same files, and so do the equipment-set arms
//! ([`set_set_name`], [`copy_equipment_set`], [`paste_equipment_set`], [`delete_equipment_set`]). Each keeps
//! every byte of framing it does not mean to change and re-signs the result; the shapes above are what a
//! rewritten file has to come out as.

use ffxi_proto::equipset::{EquipSetEntry, EQUIPSET_ENTRIES, EQUIPSET_ENTRY_BYTES};
use md5::Digest as _;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// The bytes every one of these files spends on version, spare word and tag before its records.
pub const FILE_HEADER_BYTES: usize = 24;
/// Books named by one title file (`mcr.ttl` holds 1-20, `mcr_2.ttl` holds 21-40).
pub const TITLES_PER_TITLE_FILE: usize = 20;
/// Bytes per book name inside a title file.
const TITLE_NAME_BYTES: usize = 16;
/// The longest book name one of those slots can hold. The field is NUL-terminated, so kuluu never
/// writes its last byte and never cuts a code point in half to fill it (see [`set_book_title`]).
pub const BOOK_TITLE_MAX_BYTES: usize = TITLE_NAME_BYTES - 1;
/// Keytops in one macro book: the palette's two rows of ten (see `hud::menu::MACRO_PALLET_HOTKEYS`).
pub const MACRO_BOOK_SLOTS: usize = 20;
/// Command slots one keytop holds, and the bytes each slot carries including its terminator: retail
/// keeps a command per field, six of them (module header).
pub const ENTRY_COMMAND_FIELDS: usize = 6;
const ENTRY_COMMAND_FIELD_BYTES: usize = 61;
/// Where an entry's first command field starts, after its spare `u32`.
const ENTRY_COMMANDS_OFFSET: usize = 4;
/// Where an entry's display name starts, and how wide the field is. The width is a tile law too: a
/// storable name is at most `ENTRY_NAME_BYTES - 1` characters, which is what sizes a palette block in
/// `hud::macro_pallet_view`.
const ENTRY_NAME_OFFSET: usize =
    ENTRY_COMMANDS_OFFSET + ENTRY_COMMAND_FIELDS * ENTRY_COMMAND_FIELD_BYTES;
pub const ENTRY_NAME_BYTES: usize = 10;
/// One macro entry: spare word, command fields, display name. Held to the record width every macro file
/// of the install measures (`24 + 20*380 = 7624`).
const MACRO_ENTRY_BYTES: usize = ENTRY_NAME_OFFSET + ENTRY_NAME_BYTES;
const _: () = assert!(MACRO_ENTRY_BYTES == 380);
/// The longest display name a keytop field holds (its last byte stays zero), and the longest command one
/// of its slots holds for the same reason. Both are what [`keytop_name_fits`] and [`command_fits`] stop
/// typing at, so nothing he types is cut on the way to disk.
pub const ENTRY_NAME_MAX_BYTES: usize = ENTRY_NAME_BYTES - 1;
pub const ENTRY_COMMAND_MAX_BYTES: usize = ENTRY_COMMAND_FIELD_BYTES - 1;
/// Where a user file keeps its digest of everything after the header, and how wide that digest is
/// (module header).
const DIGEST_OFFSET: usize = 8;
const DIGEST_BYTES: usize = 16;
/// Sets held by one page file (`es0.dat`..`es9.dat`).
pub const SETS_PER_SET_PAGE: usize = 20;
/// The whole width of one set record, name and contents together (module header).
const SET_RECORD_BYTES: usize = 80;
/// Bytes a set's display name gets, terminator included. Every page file of the reference install
/// measures exactly `24 + 20*80`, which is what pins this stride (module header).
const SET_NAME_BYTES: usize = 16;
/// Where a record's contents half starts: [`EQUIPSET_ENTRIES`] entries of [`EQUIPSET_ENTRY_BYTES`] bytes
/// each, one per equipment position, running to the end of the record (module header).
const SET_CONTENTS_OFFSET: usize = SET_NAME_BYTES;
const _: () =
    assert!(SET_CONTENTS_OFFSET + EQUIPSET_ENTRIES * EQUIPSET_ENTRY_BYTES == SET_RECORD_BYTES);
/// The longest set name one of those slots can hold; kuluu never writes the terminator byte or cuts a
/// code point in half to fill it, and neither does the client, whose own setter refuses a longer word
/// outright. That is what [`set_name_fits`] types against.
pub const SET_NAME_MAX_BYTES: usize = SET_NAME_BYTES - 1;
/// The ten page files of one profile's two-hundred sets, in the order the picker names them.
const SET_PAGE_FILES: [&str; SET_PAGE_COUNT] = [
    "es0.dat", "es1.dat", "es2.dat", "es3.dat", "es4.dat", "es5.dat", "es6.dat", "es7.dat",
    "es8.dat", "es9.dat",
];
const SET_PAGE_COUNT: usize = 10;
const _: () = assert!(
    FILE_HEADER_BYTES + SETS_PER_SET_PAGE * SET_RECORD_BYTES == 1624,
    "a set page file is 1624 bytes on every install kuluu has measured"
);
const _: () = assert!(SET_PAGE_COUNT * SETS_PER_SET_PAGE == crate::hud::menu::EQUIP_SET_COUNT);

/// One macro keytop as the client stores it: what a tile prints ([`MacroEntry::name`) and the commands
/// that fire when it is used. Those come out of the file's six command slots and ride in here as
/// newline-separated lines, which is the shape every surface wants; the slot layout stays at the writer
/// (module header).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroEntry {
    pub name: String,
    pub body: String,
}

impl MacroEntry {
    /// The entry's commands, one per line, in the order they fire.
    pub fn command_lines(&self) -> impl Iterator<Item = &str> {
        self.body.lines().filter(|line| !line.trim().is_empty())
    }
}

/// Everything kuluu needs from one user directory, already split into the shapes the menus page in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientUserFiles {
    /// Forty book names; `None` where the title files never named that book.
    pub book_titles: Vec<Option<String>>,
    /// Forty books, each [`MACRO_BOOK_SLOTS`] entries in keytop order (0..=9 fire on Ctrl+1..0,
    /// 10..=19 on Alt+1..0). Missing files leave empty entries.
    pub macro_books: Vec<Vec<MacroEntry>>,
    /// Two hundred set names; `None` where no page file named that set.
    pub set_names: Vec<Option<String>>,
}

impl ClientUserFiles {
    /// A store with nothing in it, which is also what every surface falls back to when the install
    /// holds no user files at all (a fresh profile, or a headless host with no install).
    pub fn empty() -> Self {
        Self::default()
    }

    /// The name book `book` (0-based) answers to; `None` leaves it un-named for the caller to fill
    /// with whatever an unnamed book reads as.
    pub fn book_title(&self, book: usize) -> Option<&str> {
        self.book_titles
            .get(book)?
            .as_deref()
            .filter(|t| !t.is_empty())
    }

    /// One keytop of one book, or the empty entry for a book the client has no file for.
    pub fn macro_entry(&self, book: usize, slot: usize) -> Option<&MacroEntry> {
        self.macro_books.get(book)?.get(slot)
    }

    /// The name set `set` (0-based) answers to.
    pub fn set_name(&self, set: usize) -> Option<&str> {
        self.set_names
            .get(set)?
            .as_deref()
            .filter(|n| !n.is_empty())
    }
}

/// One user file, read for parsing. Absent files are ordinary and answer `None`; a file whose own
/// digest no longer matches its bytes is named in the log (the client will not trust it either, so this
/// is the one sign that something else - an earlier kuluu build included - wrote it without signing).
fn read_user_file(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if !digest_matches(&bytes) {
        tracing::warn!(
            file = %path.display(),
            "client user file carries the digest of other bytes: the game will not load it"
        );
    }
    Some(bytes)
}

/// The MD5 a file of this family owes its own records (module header), written into its header.
fn sign_user_file(bytes: &mut [u8]) {
    if bytes.len() < FILE_HEADER_BYTES {
        return;
    }
    let digest = md5::Md5::digest(&bytes[FILE_HEADER_BYTES..]);
    bytes[DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_BYTES].copy_from_slice(digest.as_slice());
}

/// Whether a file's stored digest still describes its records. Anything too short to hold a header is
/// left for the parsers to refuse.
fn digest_matches(bytes: &[u8]) -> bool {
    if bytes.len() < FILE_HEADER_BYTES {
        return false;
    }
    let digest = md5::Md5::digest(&bytes[FILE_HEADER_BYTES..]);
    bytes[DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_BYTES] == *digest.as_slice()
}

/// A fresh file of this family: version 1, the unknown word left zero (which is how retail's own
/// untouched defaults ship), and no records yet - so nothing to sign until one is written.
fn new_user_file() -> Vec<u8> {
    let mut bytes = vec![0u8; FILE_HEADER_BYTES];
    bytes[..4].copy_from_slice(&1u32.to_le_bytes());
    bytes
}

/// One zero-padded ASCII field, cut at its first NUL byte; bytes the client wrote in another code
/// page survive as their replacement characters rather than failing the read.
fn field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// The twenty book names in `mcr.ttl`/`mcr_2.ttl`, or nothing when the file is absent or mis-sized.
fn parse_titles(bytes: &[u8]) -> Vec<Option<String>> {
    if bytes.len() < FILE_HEADER_BYTES + TITLE_NAME_BYTES {
        return Vec::new();
    }
    let count = (bytes.len() - FILE_HEADER_BYTES) / TITLE_NAME_BYTES;
    (0..count.min(TITLES_PER_TITLE_FILE))
        .map(|slot| {
            let start = FILE_HEADER_BYTES + slot * TITLE_NAME_BYTES;
            let name = field(&bytes[start..start + TITLE_NAME_BYTES]);
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// The twenty entries of one book file, or nothing when it is short of the full record arithmetic.
fn parse_macro_entries(bytes: &[u8]) -> Vec<MacroEntry> {
    if bytes.len() < FILE_HEADER_BYTES + MACRO_ENTRY_BYTES {
        return Vec::new();
    }
    let count = (bytes.len() - FILE_HEADER_BYTES) / MACRO_ENTRY_BYTES;
    (0..count.min(MACRO_BOOK_SLOTS))
        .map(|slot| {
            let start = FILE_HEADER_BYTES + slot * MACRO_ENTRY_BYTES;
            let record = &bytes[start..start + MACRO_ENTRY_BYTES];
            MacroEntry {
                body: entry_body(record),
                name: field(&record[ENTRY_NAME_OFFSET..ENTRY_NAME_OFFSET + ENTRY_NAME_BYTES]),
            }
        })
        .collect()
}

/// One command slot of one record, cut at its own terminator (module header).
fn command_slot(record: &[u8], no: usize) -> String {
    let start = ENTRY_COMMANDS_OFFSET + no * ENTRY_COMMAND_FIELD_BYTES;
    field(&record[start..start + ENTRY_COMMAND_FIELD_BYTES])
}

/// The commands one record holds, in firing order, as the lines [`MacroEntry::body`] keeps them. Each of
/// the six slots stands on its own in the file and ends at its own terminator, so an empty slot is simply
/// a missing command rather than the end of the entry.
fn entry_body(record: &[u8]) -> String {
    (0..ENTRY_COMMAND_FIELDS)
        .map(|no| command_slot(record, no))
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The twenty set names of one page file, or nothing when it is short of the full record arithmetic. A
/// record's own contents are never looked at here (module header), so a worn set and an empty one read
/// the same way by name.
fn parse_set_names(bytes: &[u8]) -> Vec<Option<String>> {
    if bytes.len() < FILE_HEADER_BYTES + SET_NAME_BYTES {
        return Vec::new();
    }
    let count = (bytes.len() - FILE_HEADER_BYTES) / SET_RECORD_BYTES;
    (0..count.min(SETS_PER_SET_PAGE))
        .map(|slot| {
            let (start, end) = set_name_region(slot);
            let name = field(&bytes[start..end]);
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// Which page file holds set `set` (0-based), and which of that page's twenty records it is; `None` past
/// the two-hundred-store limit the client's own picker refuses (module header).
pub fn set_page_and_slot(set: usize) -> Option<(&'static str, usize)> {
    let page = set / SETS_PER_SET_PAGE;
    if page >= SET_PAGE_COUNT {
        return None;
    }
    Some((SET_PAGE_FILES[page], set % SETS_PER_SET_PAGE))
}

/// Where one record of a page file starts, and where inside it the name field is.
fn set_record_offset(slot: usize) -> usize {
    FILE_HEADER_BYTES + slot * SET_RECORD_BYTES
}

fn set_name_region(slot: usize) -> (usize, usize) {
    let start = set_record_offset(slot);
    (start, start + SET_NAME_BYTES)
}

/// The user directory whose macro files the client wrote most recently: with several profiles in an
/// install this is whichever one the player last edited macros in, which is the only one whose
/// macros can be shown to them. Every `mcr` file counts (`mcr.dat`, `mcr.sys` and the title files
/// are written together when a book changes), so the pick does not depend on which of them a given
/// edit touched. No profile holding any macro file yields `None`.
fn freshest_user_dir(install_root: &Path) -> Option<PathBuf> {
    let user_root = install_root.join("USER");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let Ok(dirs) = std::fs::read_dir(&user_root) else {
        return None;
    };
    for entry in dirs.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            if !file.file_name().to_string_lossy().starts_with("mcr") {
                continue;
            }
            let Ok(written) = file.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if best.as_ref().is_none_or(|(seen, _)| *seen < written) {
                best = Some((written, dir.clone()));
            }
        }
    }
    best.map(|(_, dir)| dir)
}

/// One book's file name: the base `mcr.dat` for the first book, then every other book addressed by
/// its own number (see this module's header). Books beyond the numbered files an install ships read
/// empty.
fn macro_file_name(book: usize) -> String {
    if book == 0 {
        "mcr.dat".to_string()
    } else {
        format!("mcr{}.dat", book + 1)
    }
}

/// Read every file of one user directory. Absent files are normal (a profile with macros in only
/// some books ships only those), and leave empty entries behind.
pub fn load_user_dir(user_dir: &Path) -> ClientUserFiles {
    let mut titles = Vec::new();
    for title_file in ["mcr.ttl", "mcr_2.ttl"] {
        match read_user_file(&user_dir.join(title_file)) {
            Some(bytes) => titles.extend(parse_titles(&bytes)),
            None => titles.extend(std::iter::repeat_n(None, TITLES_PER_TITLE_FILE)),
        }
    }

    let mut macro_books = Vec::new();
    for book in 0..crate::hud::menu::MACRO_BOOK_COUNT {
        match read_user_file(&user_dir.join(macro_file_name(book))) {
            Some(bytes) => macro_books.push(parse_macro_entries(&bytes)),
            None => macro_books.push(Vec::new()),
        }
    }

    let mut set_names = Vec::new();
    for page_file in SET_PAGE_FILES {
        match read_user_file(&user_dir.join(page_file)) {
            Some(bytes) => set_names.extend(parse_set_names(&bytes)),
            None => set_names.extend(std::iter::repeat_n(None, SETS_PER_SET_PAGE)),
        }
    }

    ClientUserFiles {
        book_titles: titles,
        macro_books,
        set_names,
    }
}

/// The cached store, plus the profile its files were read from so that an edit writes back to where
/// it read rather than picking a second profile out of the install.
struct UserFileStore {
    /// `None` when the install holds no user directory (or nothing resolves at all): every edit then
    /// stays in memory for this run, and kuluu never creates files the retail client has never owned.
    user_dir: Option<PathBuf>,
    files: RwLock<ClientUserFiles>,
}

impl UserFileStore {
    fn load() -> Self {
        let user_dir = install_root().and_then(|root| freshest_user_dir(&root));
        let files = user_dir
            .as_deref()
            .map(load_user_dir)
            .unwrap_or_else(ClientUserFiles::empty);
        // Which profile this run reads, and how much is actually in it: a band that shows nothing is
        // either an empty book or the wrong profile, and only this line can tell them apart afterwards.
        let filled_books = files
            .macro_books
            .iter()
            .filter(|book| {
                book.iter()
                    .any(|entry| !entry.name.is_empty() || !entry.body.is_empty())
            })
            .count();
        match &user_dir {
            Some(dir) => tracing::info!(
                user_dir = %dir.display(),
                books_read = files.macro_books.len(),
                books_with_any_keytop = filled_books,
                "client macro/user files read"
            ),
            None => tracing::warn!(
                "no client USER profile resolved: macros will not be readable or writable"
            ),
        }
        Self {
            user_dir,
            files: RwLock::new(files),
        }
    }
}

static USER_FILES: OnceLock<UserFileStore> = OnceLock::new();

fn store() -> &'static UserFileStore {
    USER_FILES.get_or_init(UserFileStore::load)
}

/// The four edits below are the only writers, and each assigns whole records it assembled in full
/// beforehand, so a panicked writer leaves nothing half-built that the HUD should refuse to draw.
fn files_read() -> RwLockReadGuard<'static, ClientUserFiles> {
    store()
        .files
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn files_write() -> RwLockWriteGuard<'static, ClientUserFiles> {
    store()
        .files
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The user files for the install this run resolves (see [`crate::hud`]'s DAT law: `FFXI_DAT_PATH`,
/// else the registry's default install); an empty store when nothing resolves or nothing is in it.
/// Read once per process, then changed only by the five book arms below ([`set_book_title`],
/// [`copy_macro_book`], [`paste_macro_book`], [`delete_macro_book`]), which write through to the same
/// files they were read from. Every reader takes this guard and drops it before the frame is done.
pub fn client_user_files() -> RwLockReadGuard<'static, ClientUserFiles> {
    files_read()
}

/// What a book arm could not do. kuluu reports these in the chat log rather than inventing a dialog:
/// `mcrselop` carries no error string of its own (module header).
#[derive(Debug)]
pub enum MacroEditError {
    /// The index is outside the forty-book library.
    NoSuchBook,
    /// `Copy` pressed on a book whose twenty keytops all hold nothing.
    NothingToCopy,
    /// `Paste` pressed while the paste buffer is empty, i.e. nothing copied yet this run.
    NothingCopied,
    /// The keytop index is outside a book's twenty.
    NoSuchKeytop,
    /// The set index is outside the two-hundred-store the client addresses (`es0.dat`..`es9.dat`).
    NoSuchSet,
    /// The install's own file refused to be rewritten; the message names it.
    NotWritten(std::io::Error),
}

impl std::fmt::Display for MacroEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchBook => write!(f, "no such macro book"),
            Self::NothingToCopy => write!(f, "that macro book holds no macros"),
            Self::NothingCopied => write!(f, "nothing copied yet"),
            Self::NoSuchKeytop => write!(f, "no such macro keytop"),
            Self::NoSuchSet => write!(f, "no such equipment set"),
            Self::NotWritten(e) => write!(f, "{e}"),
        }
    }
}

/// Which title file names book `book`, and which of its twenty slots that book owns: `mcr.ttl` holds
/// books 1-20 and `mcr_2.ttl` books 21-40 (module header).
pub fn title_slot(book: usize) -> Option<(&'static str, usize)> {
    if book < TITLES_PER_TITLE_FILE {
        Some(("mcr.ttl", book))
    } else if book < 2 * TITLES_PER_TITLE_FILE {
        Some(("mcr_2.ttl", book - TITLES_PER_TITLE_FILE))
    } else {
        None
    }
}

/// Grow the store to the whole library, so editing a book the client has no file for still shows: an
/// absent book reads as twenty empty keytops and forty unnamed books.
fn ensure_library(files: &mut ClientUserFiles) {
    let books = crate::hud::menu::MACRO_BOOK_COUNT;
    files.book_titles.resize_with(books, || None);
    files.macro_books.resize_with(books, Vec::new);
}

/// Whether a book name being typed can take `extra` more bytes and still fit its title slot.
pub fn book_title_fits(draft: &str, extra_bytes: usize) -> bool {
    draft.len() + extra_bytes <= BOOK_TITLE_MAX_BYTES
}

/// `text` cut to `max_bytes`, control characters dropped except the newline when `keep_newline` (how
/// several commands ride one entry), and never a code point in half.
fn truncate_text(text: &str, max_bytes: usize, keep_newline: bool) -> String {
    let mut kept = String::new();
    for ch in text.chars() {
        if ch.is_control() && !(keep_newline && ch == '\n') {
            continue;
        }
        if kept.len() + ch.len_utf8() > max_bytes {
            break;
        }
        kept.push(ch);
    }
    kept
}

/// A name cut to what one title slot holds (control characters dropped, never a partial code point).
pub fn truncate_to_book_title(name: &str) -> String {
    truncate_text(name, BOOK_TITLE_MAX_BYTES, false)
}

/// Whether the keytop display name being typed can take `extra` more bytes and still fit its field.
pub fn keytop_name_fits(draft: &str, extra_bytes: usize) -> bool {
    draft.len() + extra_bytes <= ENTRY_NAME_MAX_BYTES
}

/// A keytop display name cut to what its ten-byte field holds.
pub fn truncate_to_keytop_name(name: &str) -> String {
    truncate_text(name, ENTRY_NAME_MAX_BYTES, false)
}

/// Whether the command being typed into one slot can take `extra` more bytes and still fit that slot. A
/// keytop holds [`ENTRY_COMMAND_FIELDS`] commands and each rides in its own field (module header), so the
/// room is counted per command instead of across the entry.
pub fn command_fits(draft: &str, extra_bytes: usize) -> bool {
    draft.len() + extra_bytes <= ENTRY_COMMAND_MAX_BYTES
}

/// A keytop's commands cut to what one entry can carry: at most [`ENTRY_COMMAND_FIELDS`] of them, each at
/// most [`ENTRY_COMMAND_MAX_BYTES`] bytes and never a code point in half. Commands past the last slot go
/// rather than crowding the ones before them.
pub fn truncate_to_commands(body: &str) -> String {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .take(ENTRY_COMMAND_FIELDS)
        .map(|line| truncate_text(line, ENTRY_COMMAND_MAX_BYTES, false))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body a keytop editor's line slots store as: the slots that hold something, in order, joined by
/// the newline the format uses as its separator (module header). An empty slot is not a command, and a
/// file carries no count of them - which is why an emptied slot simply goes.
pub fn macro_body_from_slots(slots: &[String]) -> String {
    slots
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` into a file's fixed-width field: characters up to one byte less than the field (that last
/// byte stays zero so both kuluu and retail read a terminated string), then zeros. Control characters
/// are dropped except the newline in a command body, which is how several commands ride one keytop.
fn write_field(field: &mut [u8], text: &str, keep_newline: bool) {
    let mut used = 0usize;
    let mut scratch = [0u8; 4];
    for ch in text.chars() {
        if ch.is_control() && !(keep_newline && ch == '\n') {
            continue;
        }
        let encoded = ch.encode_utf8(&mut scratch);
        if used + encoded.len() >= field.len() {
            break;
        }
        field[used..used + encoded.len()].copy_from_slice(encoded.as_bytes());
        used += encoded.len();
    }
}

/// One keytop as its record in the file: unused word, then each command in its own slot (a seventh
/// command goes, and so does anything past a slot's own limit), then the display name (module header).
fn encode_macro_entry(entry: &MacroEntry) -> Vec<u8> {
    let mut record = vec![0u8; MACRO_ENTRY_BYTES];
    for (no, line) in entry.command_lines().take(ENTRY_COMMAND_FIELDS).enumerate() {
        let start = ENTRY_COMMANDS_OFFSET + no * ENTRY_COMMAND_FIELD_BYTES;
        write_field(
            &mut record[start..start + ENTRY_COMMAND_FIELD_BYTES],
            line,
            false,
        );
    }
    write_field(&mut record[ENTRY_NAME_OFFSET..], &entry.name, false);
    record
}

/// The error every file step reports with the name of the file it was working on.
fn file_error(path: &Path, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// Write-then-rename, the same discipline kuluu's own stores keep (see `kuluu-session`'s
/// `macro_store`): a crash mid-write must not eat the client's file.
fn rewrite_file(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let tmp = path.with_extension("dat.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| file_error(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| file_error(&tmp, e))
}

/// One book-name slot rewritten in its own file, every other byte of that file kept and the whole file
/// re-signed afterwards (module header).
fn patch_title_file(path: &Path, slot: usize, name: &str) -> Result<(), std::io::Error> {
    let mut bytes = std::fs::read(path).map_err(|e| file_error(path, e))?;
    let start = FILE_HEADER_BYTES + slot * TITLE_NAME_BYTES;
    let end = start + TITLE_NAME_BYTES;
    if bytes.len() < end {
        return Err(file_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "short of that title slot"),
        ));
    }
    write_field(&mut bytes[start..end], name, false);
    sign_user_file(&mut bytes);
    rewrite_file(path, &bytes)
}

/// One book's records rewritten in its own file. An existing file keeps every byte it does not touch
/// except its digest; a book with no file yet gets one, because that is how these files come to exist at
/// all (module header). A book that would come out completely empty and has never had a file is left
/// without one - nothing on this install owns a file full of blank keytops.
fn write_macro_book_file(path: &Path, entries: &[MacroEntry]) -> Result<(), std::io::Error> {
    let holds_something = entries
        .iter()
        .any(|entry| !entry.name.is_empty() || !entry.body.is_empty());
    let mut bytes = match std::fs::read(path) {
        Ok(existing) if existing.len() >= FILE_HEADER_BYTES => existing,
        Ok(_) | Err(_) if holds_something => new_user_file(),
        // Nothing to store and nothing on disk: leave the book without a file rather than making one.
        Ok(_) | Err(_) => return Ok(()),
    };
    let records = MACRO_BOOK_SLOTS * MACRO_ENTRY_BYTES;
    if bytes.len() < FILE_HEADER_BYTES + records {
        bytes.resize(FILE_HEADER_BYTES + records, 0);
    }
    for slot in 0..MACRO_BOOK_SLOTS {
        let record = encode_macro_entry(entries.get(slot).unwrap_or(&MacroEntry::default()));
        let start = FILE_HEADER_BYTES + slot * MACRO_ENTRY_BYTES;
        bytes[start..start + MACRO_ENTRY_BYTES].copy_from_slice(&record);
    }
    sign_user_file(&mut bytes);
    rewrite_file(path, &bytes)
}

/// The book file of the profile kuluu read, or `None` when there is no profile to write into.
fn macro_book_path(book: usize) -> Option<PathBuf> {
    store()
        .user_dir
        .as_deref()
        .map(|dir| dir.join(macro_file_name(book)))
}

/// `Change Name`: give book `book` that name. The memory copy changes first so both pagers read the
/// new word on the next frame, then the one 16-byte slot in its title file is rewritten; an empty name
/// clears the book back to retail's unnamed state, which leaves the pagers falling back to their own
/// `BookNN` pattern. A name longer than the field is cut by [`truncate_to_book_title`] - both here and
/// while typing, through [`book_title_fits`].
pub fn set_book_title(book: usize, name: &str) -> Result<(), MacroEditError> {
    let (file_name, slot) = title_slot(book).ok_or(MacroEditError::NoSuchBook)?;
    let name = truncate_to_book_title(name);
    {
        let mut files = files_write();
        ensure_library(&mut files);
        files.book_titles[book] = (!name.is_empty()).then_some(name.clone());
    }
    let Some(path) = store().user_dir.as_deref().map(|dir| dir.join(file_name)) else {
        return Ok(());
    };
    patch_title_file(&path, slot, &name).map_err(MacroEditError::NotWritten)
}

/// What `Copy` put in the paste buffer: the book's twenty keytops. A file pasted into a book that has
/// never owned one is built fresh and signed (see [`write_macro_book_file`]), so nothing of the source
/// file's header travels with it.
#[derive(Clone)]
struct CopiedMacroBook {
    entries: Vec<MacroEntry>,
}

/// The one book in the paste buffer. Never persisted: a kuluu that starts again starts with an empty
/// paste buffer, like reopening the client.
static COPIED_MACRO_BOOK: RwLock<Option<CopiedMacroBook>> = RwLock::new(None);

fn copied_macro_book() -> Option<CopiedMacroBook> {
    COPIED_MACRO_BOOK
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// `Copy`: take book `book`'s twenty keytops into the paste buffer. Only the buffer changes; nothing on
/// disk does.
pub fn copy_macro_book(book: usize) -> Result<(), MacroEditError> {
    if book >= crate::hud::menu::MACRO_BOOK_COUNT {
        return Err(MacroEditError::NoSuchBook);
    }
    let entries = files_read()
        .macro_books
        .get(book)
        .cloned()
        .unwrap_or_default();
    if entries
        .iter()
        .all(|entry| entry.name.is_empty() && entry.body.is_empty())
    {
        return Err(MacroEditError::NothingToCopy);
    }
    *COPIED_MACRO_BOOK
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CopiedMacroBook { entries });
    Ok(())
}

/// `Paste`: write what [`copy_macro_book`] took into book `book`, replacing all twenty of its keytops,
/// and put the same entries in memory so the band shows them on the next frame.
pub fn paste_macro_book(book: usize) -> Result<(), MacroEditError> {
    let Some(copied) = copied_macro_book() else {
        return Err(MacroEditError::NothingCopied);
    };
    if book >= crate::hud::menu::MACRO_BOOK_COUNT {
        return Err(MacroEditError::NoSuchBook);
    }
    let mut entries = copied.entries;
    entries.resize_with(MACRO_BOOK_SLOTS, MacroEntry::default);
    if let Some(path) = macro_book_path(book) {
        write_macro_book_file(&path, &entries).map_err(MacroEditError::NotWritten)?;
    }
    {
        let mut files = files_write();
        ensure_library(&mut files);
        files.macro_books[book] = entries;
    }
    Ok(())
}

/// `Delete`: wipe book `book`. Its twenty records are zeroed in its own file - the 24-byte header and
/// the record framing survive, so what is left is exactly the shape of a fresh empty book - and the
/// memory copy goes empty too. The row is called `Delete`, so kuluu asks nothing first.
pub fn delete_macro_book(book: usize) -> Result<(), MacroEditError> {
    if book >= crate::hud::menu::MACRO_BOOK_COUNT {
        return Err(MacroEditError::NoSuchBook);
    }
    if let Some(path) = macro_book_path(book) {
        write_macro_book_file(&path, &[]).map_err(MacroEditError::NotWritten)?;
    }
    {
        let mut files = files_write();
        ensure_library(&mut files);
        files.macro_books[book] = Vec::new();
    }
    Ok(())
}

/// One keytop of one book, as the file and the memory copy will hold it: display name cut to its field
/// and commands cut to the slots that carry them, so what the band prints is what comes off the disk.
fn stored_keytop(name: &str, body: &str) -> MacroEntry {
    MacroEntry {
        name: truncate_to_keytop_name(name),
        body: truncate_to_commands(body),
    }
}

/// `Enter` in a keytop editor: give slot `slot` of book `book` that display name and those command
/// lines (already capped per command by [`command_fits`] while typing, cut again here through
/// [`truncate_to_commands`] for a caller that skipped that check). Every other keytop of the book is
/// rewritten from the memory copy unchanged, so its file keeps its framing and its untouched records,
/// and comes back signed; the band reads the new word on the next frame.
pub fn set_macro_entry(
    book: usize,
    slot: usize,
    name: &str,
    body: &str,
) -> Result<(), MacroEditError> {
    if book >= crate::hud::menu::MACRO_BOOK_COUNT {
        return Err(MacroEditError::NoSuchBook);
    }
    if slot >= MACRO_BOOK_SLOTS {
        return Err(MacroEditError::NoSuchKeytop);
    }
    let entry = stored_keytop(name, body);
    let mut entries = files_read()
        .macro_books
        .get(book)
        .cloned()
        .unwrap_or_default();
    entries.resize_with(MACRO_BOOK_SLOTS, MacroEntry::default);
    entries[slot] = entry;
    if let Some(path) = macro_book_path(book) {
        write_macro_book_file(&path, &entries).map_err(MacroEditError::NotWritten)?;
    }
    {
        let mut files = files_write();
        ensure_library(&mut files);
        files.macro_books[book] = entries;
    }
    Ok(())
}

/// Whether the set name being typed can take `extra` more bytes and still fit its record's sixteen-byte
/// field. The client's own setter refuses a longer word outright rather than cutting it (module header),
/// so stopping the caret there is what both kuluu and retail show.
pub fn set_name_fits(draft: &str, extra_bytes: usize) -> bool {
    draft.len() + extra_bytes <= SET_NAME_MAX_BYTES
}

/// A set name cut to what its field holds (control characters dropped, never a partial code point).
pub fn truncate_to_set_name(name: &str) -> String {
    truncate_text(name, SET_NAME_MAX_BYTES, false)
}

/// Grow the store to the whole two-hundred-set library, so editing a set whose page file is missing still
/// shows in the pager.
fn ensure_sets(files: &mut ClientUserFiles) {
    files
        .set_names
        .resize_with(crate::hud::menu::EQUIP_SET_COUNT, || None);
}

/// A page file with nothing in it yet: the header and twenty zeroed records, which reads back as twenty
/// unnamed empty sets (module header). This is what a profile that never owned the page gets when one of
/// its sets is edited.
fn new_set_page() -> Vec<u8> {
    let mut bytes = new_user_file();
    bytes.resize(FILE_HEADER_BYTES + SETS_PER_SET_PAGE * SET_RECORD_BYTES, 0);
    bytes
}

/// One set page file read whole, ready to be rewritten. A profile that has never owned it comes back as
/// [`new_set_page`], and a file that exists but falls short of its twenty records is refused instead of
/// being re-framed around whatever kuluu guessed (see [`patch_title_file`] for the same rule on titles).
fn read_set_page(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let whole = FILE_HEADER_BYTES + SETS_PER_SET_PAGE * SET_RECORD_BYTES;
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= whole => Ok(bytes),
        Ok(_) => Err(file_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "short of its set records"),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(new_set_page()),
        Err(e) => Err(file_error(path, e)),
    }
}

/// The page file of set `set` in the profile kuluu read from, and which record of it that set is.
fn set_page_of(set: usize) -> Result<(PathBuf, usize), MacroEditError> {
    let (page_file, slot) = set_page_and_slot(set).ok_or(MacroEditError::NoSuchSet)?;
    let path = store()
        .user_dir
        .as_deref()
        .map(|dir| dir.join(page_file))
        .ok_or(MacroEditError::NotWritten(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no client USER profile to write into",
        )))?;
    Ok((path, slot))
}

/// `Change Name`: give set `set` that name. The memory copy changes first so the pager reads the new word
/// on the next frame; then only those sixteen bytes of that one record are rewritten, every other byte of
/// the page - including the sixty-four contents bytes kuluu has never read - kept as it was and the file
/// re-signed afterwards (module header). A name too long for the field is cut here and while typing
/// through [`set_name_fits`], exactly where the client stops too.
pub fn set_set_name(set: usize, name: &str) -> Result<(), MacroEditError> {
    let (path, slot) = set_page_of(set)?;
    let name = truncate_to_set_name(name);
    {
        let mut files = files_write();
        ensure_sets(&mut files);
        files.set_names[set] = (!name.is_empty()).then_some(name.clone());
    }
    let (start, end) = set_name_region(slot);
    let mut bytes = read_set_page(&path).map_err(MacroEditError::NotWritten)?;
    write_field(&mut bytes[start..end], &name, false);
    sign_user_file(&mut bytes);
    rewrite_file(&path, &bytes).map_err(MacroEditError::NotWritten)
}

/// What `Copy` took from one set: the whole record, contents and all. Unlike a book copy, which kuluu can
/// rebuild because it understands every byte of a macro entry, a set copy only ever travels as bytes.
#[derive(Clone)]
struct CopiedEquipmentSet {
    record: [u8; SET_RECORD_BYTES],
}

/// The one set in the paste buffer. Never persisted: a kuluu that starts again starts with an empty paste
/// buffer, like reopening the client.
static COPIED_EQUIPMENT_SET: RwLock<Option<CopiedEquipmentSet>> = RwLock::new(None);

fn copied_equipment_set() -> Option<CopiedEquipmentSet> {
    COPIED_EQUIPMENT_SET
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// One set's record as its page file holds it. `None` when the profile has no such page yet, which is a
/// set nothing was ever put into. Read straight from the file rather than from the memory copy because the
/// contents half only exists there (module header).
fn stored_set_record(set: usize) -> Result<Option<[u8; SET_RECORD_BYTES]>, MacroEditError> {
    let Some(user_dir) = store().user_dir.clone() else {
        return Ok(None);
    };
    let (page_file, slot) = set_page_and_slot(set).ok_or(MacroEditError::NoSuchSet)?;
    let path = user_dir.join(page_file);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_set_page(&path).map_err(MacroEditError::NotWritten)?;
    let start = set_record_offset(slot);
    let mut record = [0u8; SET_RECORD_BYTES];
    record.copy_from_slice(&bytes[start..start + SET_RECORD_BYTES]);
    Ok(Some(record))
}

/// The contents half of set `set`'s stored record: one entry per equipment position (module header).
/// `Ok(None)` when the profile owns no page for it, which is a set nothing was ever put into.
pub fn equip_set_contents(
    set: usize,
) -> Result<Option<[EquipSetEntry; EQUIPSET_ENTRIES]>, MacroEditError> {
    let Some(record) = stored_set_record(set)? else {
        return Ok(None);
    };
    let mut contents = [EquipSetEntry::EMPTY; EQUIPSET_ENTRIES];
    for (position, entry) in contents.iter_mut().enumerate() {
        let at = SET_CONTENTS_OFFSET + position * EQUIPSET_ENTRY_BYTES;
        let bytes: [u8; EQUIPSET_ENTRY_BYTES] = record[at..at + EQUIPSET_ENTRY_BYTES]
            .try_into()
            .expect("the region is one entry wide");
        *entry = EquipSetEntry::from_record_bytes(bytes);
    }
    Ok(Some(contents))
}

/// Write `contents` over set `set`'s record: the name field keeps its word, every other byte of the page
/// keeps itself, and only then is the file re-signed (module header law).
pub fn set_equip_set_contents(
    set: usize,
    contents: &[EquipSetEntry; EQUIPSET_ENTRIES],
) -> Result<(), MacroEditError> {
    let (path, slot) = set_page_of(set)?;
    let mut bytes = read_set_page(&path).map_err(MacroEditError::NotWritten)?;
    let base = set_record_offset(slot) + SET_CONTENTS_OFFSET;
    for (position, entry) in contents.iter().enumerate() {
        let at = base + position * EQUIPSET_ENTRY_BYTES;
        bytes[at..at + EQUIPSET_ENTRY_BYTES].copy_from_slice(&entry.to_record_bytes());
    }
    sign_user_file(&mut bytes);
    rewrite_file(&path, &bytes).map_err(MacroEditError::NotWritten)
}

/// The name a record carries, in the shape the store keeps it: empty answers `None` so the pager falls back
/// to retail's `SetNNN` pattern.
fn record_name(record: &[u8; SET_RECORD_BYTES]) -> Option<String> {
    let name = field(&record[..SET_NAME_BYTES]);
    (!name.is_empty()).then_some(name)
}

/// `Copy`: take set `set`'s whole record into the paste buffer. Only the buffer changes; nothing on disk
/// does.
pub fn copy_equipment_set(set: usize) -> Result<(), MacroEditError> {
    let Some(record) = stored_set_record(set)? else {
        return Err(MacroEditError::NothingToCopy);
    };
    if record.iter().all(|&byte| byte == 0) {
        return Err(MacroEditError::NothingToCopy);
    }
    *COPIED_EQUIPMENT_SET
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CopiedEquipmentSet { record });
    Ok(())
}

/// `Paste`: write what [`copy_equipment_set`] took over set `set`'s record, replacing its name and its
/// contents together. The pager shows the pasted word on the next frame.
pub fn paste_equipment_set(set: usize) -> Result<(), MacroEditError> {
    let Some(copied) = copied_equipment_set() else {
        return Err(MacroEditError::NothingCopied);
    };
    let (path, slot) = set_page_of(set)?;
    let name = record_name(&copied.record);
    let mut bytes = read_set_page(&path).map_err(MacroEditError::NotWritten)?;
    let start = set_record_offset(slot);
    bytes[start..start + SET_RECORD_BYTES].copy_from_slice(&copied.record);
    sign_user_file(&mut bytes);
    rewrite_file(&path, &bytes).map_err(MacroEditError::NotWritten)?;
    {
        let mut files = files_write();
        ensure_sets(&mut files);
        files.set_names[set] = name;
    }
    Ok(())
}

/// `Delete`: retail does not blank a record. It stamps the default name for that set number back over it
/// ([`crate::hud::menu::default_equip_set_name`] carries the fixed template) and clears all sixteen contents
/// entries, so what is left reads exactly as an unnamed empty set (module header law). The 24-byte header and
/// the other nineteen records survive untouched. The row is called `Delete`, so kuluu asks nothing first.
pub fn delete_equipment_set(set: usize) -> Result<(), MacroEditError> {
    let (path, slot) = set_page_of(set)?;
    let existed = path.exists();
    if existed {
        let default_name = crate::hud::menu::default_equip_set_name(set);
        let mut bytes = read_set_page(&path).map_err(MacroEditError::NotWritten)?;
        let (name_start, name_end) = set_name_region(slot);
        write_field(&mut bytes[name_start..name_end], &default_name, false);
        let record_start = set_record_offset(slot);
        bytes[record_start + SET_CONTENTS_OFFSET..record_start + SET_RECORD_BYTES].fill(0);
        sign_user_file(&mut bytes);
        rewrite_file(&path, &bytes).map_err(MacroEditError::NotWritten)?;
    }
    {
        let mut files = files_write();
        ensure_sets(&mut files);
        // The memory copy carries the stamped word only when a record existed to stamp it into.
        files.set_names[set] = existed.then(|| crate::hud::menu::default_equip_set_name(set));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn install_root() -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn install_root() -> Option<PathBuf> {
    ffxi_dat::install::resolve().ok().map(|r| r.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Vec<u8> {
        let mut bytes = vec![1, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend(std::iter::repeat_n(0xA5u8, 16));
        bytes
    }

    #[test]
    fn title_files_hold_twenty_fixed_width_names() {
        let mut bytes = header();
        for book in 0..TITLES_PER_TITLE_FILE {
            let name = format!("Book{:02}", book + 1);
            let mut slot = vec![0u8; TITLE_NAME_BYTES];
            slot[..name.len()].copy_from_slice(name.as_bytes());
            bytes.extend(slot);
        }
        assert_eq!(
            bytes.len(),
            FILE_HEADER_BYTES + TITLES_PER_TITLE_FILE * TITLE_NAME_BYTES
        );
        let titles = parse_titles(&bytes);
        assert_eq!(titles.len(), TITLES_PER_TITLE_FILE);
        assert_eq!(titles[0].as_deref(), Some("Book01"));
        assert_eq!(titles[TITLES_PER_TITLE_FILE - 1].as_deref(), Some("Book20"));
    }

    /// One record of a macro file, laid out the way retail lays it out: a spare word, up to six
    /// independent command slots, then the display name. The install's shipped defaults are the proof - a
    /// four-command `Entice` fills slots 0..=3 and a five-command `JoinPT` fills 0..=4, each ending at its
    /// own terminator (module header).
    #[test]
    fn a_macro_entry_is_six_command_slots_then_a_name() {
        let entice = ["/party Luring <t>!", "/magic \"Dia\" <t>", "/wait 4"];
        let mut bytes = header();
        for slot in 0..MACRO_BOOK_SLOTS {
            // Slot 4 stays as a player leaves it: no commands and no display name.
            let entry_start = bytes.len();
            let blank_keytop = slot == 4;
            let last_keytop = slot + 1 == MACRO_BOOK_SLOTS;
            let mut entry = vec![0u8; MACRO_ENTRY_BYTES];
            if !blank_keytop {
                put_command(&mut entry, 0, if last_keytop { entice[0] } else { "/yes" });
                if last_keytop {
                    for (no, command) in entice.iter().enumerate().skip(1) {
                        put_command(&mut entry, no, command);
                    }
                    put_command(&mut entry, 3, "/attack <t>");
                }
            }
            if !blank_keytop {
                let name = format!("N{slot}");
                write_field(
                    &mut entry[ENTRY_NAME_OFFSET..ENTRY_NAME_OFFSET + ENTRY_NAME_BYTES],
                    &name,
                    false,
                );
            }
            bytes.extend(entry);
            assert_eq!(bytes.len(), entry_start + MACRO_ENTRY_BYTES);
        }

        let entries = parse_macro_entries(&bytes);
        assert_eq!(entries.len(), MACRO_BOOK_SLOTS);
        assert_eq!(entries[0].name, "N0");
        assert_eq!(entries[0].body, "/yes");
        // A blank keytop reads as an empty entry rather than a name from next door.
        assert_eq!(entries[4], MacroEntry::default());
        // Several commands ride in one entry, each in its own slot, and survive verbatim in firing order.
        let last = &entries[MACRO_BOOK_SLOTS - 1];
        assert_eq!(last.name, "N19");
        assert_eq!(
            last.command_lines().collect::<Vec<_>>(),
            vec![entice[0], entice[1], entice[2], "/attack <t>"]
        );
        // And the file spends no byte on a separator: nothing in it is a newline.
        assert!(
            !bytes.contains(&b'\n'),
            "a command separator crept into a macro-shaped file"
        );
    }

    /// One page file as the client writes it: a name field and then what that set wears. The contents half is
    /// filled with recognisable junk here rather than zeros, so every writer test can show it kept them.
    fn set_page() -> Vec<u8> {
        let mut bytes = header();
        for slot in 0..SETS_PER_SET_PAGE {
            let mut record = vec![0u8; SET_RECORD_BYTES];
            write_field(
                &mut record[..SET_NAME_BYTES],
                &format!("Set{:03}", slot + 1),
                false,
            );
            for (offset, byte) in record
                .iter_mut()
                .enumerate()
                .take(SET_RECORD_BYTES)
                .skip(SET_NAME_BYTES)
            {
                *byte = (slot as u8 * 3 + offset as u8).wrapping_add(1);
            }
            bytes.extend(record);
        }
        bytes
    }

    fn record(bytes: &[u8], slot: usize) -> &[u8] {
        let start = set_record_offset(slot);
        &bytes[start..start + SET_RECORD_BYTES]
    }

    /// Twenty records of `name[16] + contents[64]`, which is what every page file of the reference install
    /// measures (`24 + 20*80 = 1624`). A truncated tail drops just that partial record instead of
    /// half-parsing it into an invented row.
    #[test]
    fn a_set_page_is_twenty_records_of_name_then_contents() {
        let bytes = set_page();
        assert_eq!(bytes.len(), 1624);
        let names = parse_set_names(&bytes);
        assert_eq!(names[0].as_deref(), Some("Set001"));
        assert_eq!(names[SETS_PER_SET_PAGE - 1].as_deref(), Some("Set020"));
        // The name stops at its own field: a set whose name left the field early reads as empty rather than
        // picking up the contents bytes behind it.
        let mut blanked = bytes.clone();
        blanked[set_record_offset(4)..set_record_offset(4) + SET_NAME_BYTES].fill(0);
        assert_eq!(parse_set_names(&blanked)[4], None);
        assert!(record(&blanked, 4)
            .iter()
            .skip(SET_NAME_BYTES)
            .any(|&b| b != 0));
        // A file short of even one whole record is refused outright.
        assert!(parse_set_names(&bytes[..FILE_HEADER_BYTES + SET_NAME_BYTES - 1]).is_empty());
        assert_eq!(
            parse_set_names(&bytes[..bytes.len() - 4]).len(),
            SETS_PER_SET_PAGE - 1
        );
    }

    /// The arithmetic the page picker in FFXiMain.dll does (module header): twenty sets per file, ten files,
    /// nothing past set two hundred.
    #[test]
    fn every_set_addresses_its_own_page_file() {
        assert_eq!(set_page_and_slot(0), Some(("es0.dat", 0)));
        assert_eq!(
            set_page_and_slot(SETS_PER_SET_PAGE - 1),
            Some(("es0.dat", 19))
        );
        assert_eq!(set_page_and_slot(SETS_PER_SET_PAGE), Some(("es1.dat", 0)));
        // Profile `985` of the reference install owns exactly `es0/1/2/8/9`, and those five files hold sets
        // 1-60 and 161-200. The first set of the last page is number 181, which is what `es9.dat`'s own
        // names read as.
        assert_eq!(
            set_page_and_slot(SET_PAGE_COUNT * SETS_PER_SET_PAGE - SETS_PER_SET_PAGE),
            Some(("es9.dat", 0))
        );
        assert_eq!(
            set_page_and_slot(crate::hud::menu::EQUIP_SET_COUNT - 1),
            Some((SET_PAGE_FILES[SET_PAGE_COUNT - 1], SETS_PER_SET_PAGE - 1))
        );
        assert_eq!(set_page_and_slot(crate::hud::menu::EQUIP_SET_COUNT), None);
    }

    /// A rename is the one edit kuluu makes inside a record it does not understand, so it stops dead at the
    /// name field: the contents half of that same record and all nineteen others survive byte for byte.
    #[test]
    fn renaming_a_set_rewrites_only_its_name_field() {
        let mut bytes = set_page();
        let before = bytes.clone();
        let (start, end) = set_name_region(3);
        write_field(&mut bytes[start..end], "Pulled", false);

        assert_eq!(parse_set_names(&bytes)[3].as_deref(), Some("Pulled"));
        assert_eq!(&bytes[..start], &before[..start]);
        // Everything from the terminator onwards, in this record and every other one, is untouched.
        assert_eq!(
            &record(&bytes, 3)[SET_NAME_BYTES..],
            &record(&before, 3)[SET_NAME_BYTES..]
        );
        for slot in [0, 2, 4, SETS_PER_SET_PAGE - 1] {
            assert_eq!(record(&bytes, slot), record(&before, slot));
        }
    }

    /// A set copy is the whole 80-byte record moved as-is (module header): paste lands both halves, and a
    /// delete zeroes exactly one record of its page.
    #[test]
    fn copying_a_set_moves_the_whole_record_and_deleting_one_zeroes_only_that_one() {
        let mut bytes = set_page();
        let before = bytes.clone();
        // The paste buffer is a separate copy of the record, exactly as `CopiedEquipmentSet` keeps it.
        let copied = record(&bytes, 5).to_vec();
        let dst = set_record_offset(7);
        bytes[dst..dst + SET_RECORD_BYTES].copy_from_slice(&copied);

        assert_eq!(parse_set_names(&bytes)[7].as_deref(), Some("Set006"));
        assert_eq!(record(&bytes, 7), record(&before, 5));

        let killed = set_record_offset(9);
        bytes[killed..killed + SET_RECORD_BYTES].fill(0);
        assert_eq!(parse_set_names(&bytes)[9], None);
        assert!(record(&bytes, 9).iter().all(|&byte| byte == 0));
        for slot in [8, 10] {
            assert_eq!(record(&bytes, slot), record(&before, slot));
        }
    }

    /// The client's own setter refuses a set name longer than fifteen characters instead of cutting it (module
    /// header), so typing stops at the same place kuluu writes.
    #[test]
    fn a_set_name_is_cut_to_its_field_and_never_across_a_code_point() {
        assert_eq!(SET_NAME_MAX_BYTES, SET_NAME_BYTES - 1);
        assert!(set_name_fits("abc", SET_NAME_MAX_BYTES - 3));
        assert!(!set_name_fits("abc", SET_NAME_MAX_BYTES - 2));
        let full = "a".repeat(SET_NAME_MAX_BYTES);
        assert_eq!(
            truncate_to_set_name(&format!("{full}overflow")).len(),
            full.len()
        );
        // Never half a code point: with one byte left in the field, a two-byte character goes whole rather
        // than as half of it.
        let filler = "o".repeat(SET_NAME_MAX_BYTES - 1);
        assert_eq!(truncate_to_set_name(&filler), filler);
        assert_eq!(truncate_to_set_name(&format!("{filler}éé")), filler);
    }

    #[test]
    fn only_the_first_book_carries_no_number_in_its_file_name() {
        assert_eq!(macro_file_name(0), "mcr.dat");
        assert_eq!(macro_file_name(1), "mcr2.dat");
        assert_eq!(
            macro_file_name(crate::hud::menu::MACRO_BOOK_COUNT - 1),
            "mcr40.dat"
        );
    }

    #[test]
    fn an_empty_store_leaves_naming_to_the_caller() {
        let files = ClientUserFiles::empty();
        assert_eq!(files.book_title(0), None);
        assert_eq!(files.set_name(0), None);
        // With no book file there is no keytop to read, so the caller keeps its own empty-tile law.
        assert!(files.macro_entry(0, 0).is_none());
    }

    #[test]
    fn forty_books_split_across_the_two_title_files_in_order() {
        assert_eq!(title_slot(0), Some(("mcr.ttl", 0)));
        assert_eq!(title_slot(TITLES_PER_TITLE_FILE - 1), Some(("mcr.ttl", 19)));
        assert_eq!(title_slot(TITLES_PER_TITLE_FILE), Some(("mcr_2.ttl", 0)));
        assert_eq!(
            title_slot(2 * TITLES_PER_TITLE_FILE - 1),
            Some(("mcr_2.ttl", 19))
        );
        // The library is forty books, so there is no third title file to write into.
        assert_eq!(title_slot(2 * TITLES_PER_TITLE_FILE), None);
    }

    #[test]
    fn a_book_name_is_cut_to_its_field_and_never_across_a_code_point() {
        // One byte short of the 16-byte slot, so what kuluu writes always leaves a terminator.
        assert_eq!(BOOK_TITLE_MAX_BYTES, TITLE_NAME_BYTES - 1);
        assert!(book_title_fits("abc", BOOK_TITLE_MAX_BYTES - 3));
        assert!(!book_title_fits("abc", BOOK_TITLE_MAX_BYTES - 2));
        let full = "a".repeat(BOOK_TITLE_MAX_BYTES);
        assert_eq!(truncate_to_book_title(&full), full);
        // Past the field a name stops at its last whole character, and control characters never enter
        // a title at all.
        assert_eq!(truncate_to_book_title(&(full.clone() + "a")), full);
        assert_eq!(truncate_to_book_title(&(full.clone() + "\u{e9}")), full);
        assert_eq!(truncate_to_book_title("a\nb"), "ab");
    }

    #[test]
    fn encoded_entries_read_back_as_what_was_written() {
        let written = [
            MacroEntry::default(),
            MacroEntry {
                name: "Ninechars".to_string(),
                body: "/ra <me>\n/heal <me>".to_string(),
            },
            // A control character in a display name is dropped; the body keeps its newlines, since
            // that is how several commands ride one keytop.
            MacroEntry {
                name: "tab\there".to_string(),
                body: "/wait 1".to_string(),
            },
        ];
        let mut bytes = header();
        for slot in 0..MACRO_BOOK_SLOTS {
            let blank = MacroEntry::default();
            let record = encode_macro_entry(written.get(slot).unwrap_or(&blank));
            assert_eq!(record.len(), MACRO_ENTRY_BYTES);
            bytes.extend(record);
        }
        assert_eq!(
            bytes.len(),
            FILE_HEADER_BYTES + MACRO_BOOK_SLOTS * MACRO_ENTRY_BYTES
        );
        let parsed = parse_macro_entries(&bytes);
        assert_eq!(parsed[0], written[0]);
        assert_eq!(parsed[1], written[1]);
        // Each command of that two-line keytop landed in its own slot, which is how retail finds them,
        // and the slots past it stayed empty instead of running into each other.
        let second = &bytes[FILE_HEADER_BYTES + MACRO_ENTRY_BYTES..][..MACRO_ENTRY_BYTES];
        assert_eq!(command_slot(second, 0), "/ra <me>");
        assert_eq!(command_slot(second, 1), "/heal <me>");
        assert_eq!(command_slot(second, 2), "");
        // Only the control character goes; everything else in that record came back as it went.
        assert_eq!(parsed[2].name, "tabhere");
        assert_eq!(parsed[2].body, written[2].body);
        // Beyond what was written the records stay blank keytops, not the previous book's words.
        assert_eq!(parsed[MACRO_BOOK_SLOTS - 1], MacroEntry::default());
    }

    #[test]
    fn more_commands_than_the_entry_has_slots_are_cut_to_the_slots() {
        let too_many = (0..9)
            .map(|no| format!("/cmd {no}"))
            .collect::<Vec<_>>()
            .join("\n");
        let entry = MacroEntry {
            name: String::new(),
            body: too_many,
        };
        assert_eq!(
            stored_keytop("", &entry.body)
                .command_lines()
                .collect::<Vec<_>>(),
            vec!["/cmd 0", "/cmd 1", "/cmd 2", "/cmd 3", "/cmd 4", "/cmd 5"]
        );
        // One command longer than its own slot holds is cut at that slot's limit, and never a code point
        // in half.
        let long = truncate_to_commands(&"u".repeat(ENTRY_COMMAND_MAX_BYTES + 8));
        assert_eq!(long.len(), ENTRY_COMMAND_MAX_BYTES);
    }

    /// The digest law of the whole user-file family (module header), pinned against a stock MD5 vector so
    /// it cannot quietly change underneath kuluu's writers.
    #[test]
    fn every_file_kuluu_writes_signs_its_own_bytes() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");

        let mut bytes = header();
        bytes.extend("/ra <me>".as_bytes());
        assert!(
            !digest_matches(&bytes),
            "the test header carries a made-up tag"
        );
        sign_user_file(&mut bytes);
        assert!(digest_matches(&bytes));
        // Change one record byte and the file is untrusted again until it is re-signed.
        let last = bytes.len() - 1;
        bytes[last] = b'x';
        assert!(!digest_matches(&bytes));
    }

    /// A book with no file yet gets one, signed; a book that would come out empty and never had a file is
    /// left without one (module header).
    #[test]
    fn writing_a_book_with_no_file_creates_it_signed() {
        let dir = std::env::temp_dir().join(format!("kuluu-macro-book-test-{}", file_id()));
        // Book 2, which is the case profile `985` of the reference install ships: no file at all.
        let path = dir.join(macro_file_name(1));
        std::fs::create_dir_all(&dir).expect("temp dir");

        write_macro_book_file(&path, &[]).expect("nothing to store, so nothing written");
        assert!(!path.exists(), "an empty book is not given a file");

        let mut entries: Vec<MacroEntry> = (0..MACRO_BOOK_SLOTS)
            .map(|_| MacroEntry {
                name: "Naima".to_string(),
                body: "/item \"Hysteroanima\" <t>\n/wait 10".to_string(),
            })
            .collect();
        entries[3] = MacroEntry {
            name: "Third".to_string(),
            body: "/ra <me>".to_string(),
        };
        write_macro_book_file(&path, &entries).expect("first real edit creates the book file");

        let written = std::fs::read(&path).expect("the book file exists now");
        assert_eq!(
            written.len(),
            FILE_HEADER_BYTES + MACRO_BOOK_SLOTS * MACRO_ENTRY_BYTES
        );
        assert!(
            digest_matches(&written),
            "a created file signs what it holds"
        );
        let back = parse_macro_entries(&written);
        assert_eq!(back[0], entries[0]);
        assert_eq!(
            back[3].command_lines().collect::<Vec<_>>(),
            vec!["/ra <me>"]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn put_command(record: &mut [u8], no: usize, command: &str) {
        let start = ENTRY_COMMANDS_OFFSET + no * ENTRY_COMMAND_FIELD_BYTES;
        write_field(
            &mut record[start..start + ENTRY_COMMAND_FIELD_BYTES],
            command,
            false,
        );
    }

    fn md5_hex(data: &[u8]) -> String {
        md5::Md5::digest(data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Names a temporary directory uniquely to this test process, so these file tests cannot collide with
    /// each other when they run in parallel.
    fn file_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed) << 32 | std::process::id() as u64
    }
}
