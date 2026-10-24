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
//! * `es<N>.dat` - twenty equipment-set names per page (`Set001`..`Set200`).
//!
//! Byte layout of each file (measured against the three user directories of the reference install;
//! the arithmetic checks in [`tests`] re-derive it from the same shapes):
//!
//! * All three families open with 24 bytes: `u32` version (1), one further `u32`, then a 16-byte tag.
//! * A title file follows with twenty 16-byte names, zero padded.
//! * A macro file follows with twenty entries of exactly 380 bytes: an unused `u32`, the command
//!   body as 366 bytes (several commands in one entry are separated by `\n`), and the display name
//!   as ten bytes, so a nine-character name fills the field. 24 + 20*380 = 7624, which is every
//!   macro file's length on disk.
//! * A set-name file follows with twenty 80-byte names: 24 + 20*80 = 1624.
//!
//! The four `mcrselop` book arms ([`set_book_title`], [`copy_macro_book`], [`paste_macro_book`],
//! [`delete_macro_book`]) write back into these same files, keeping every byte of framing they do not
//! mean to change; the shapes above are what a rewritten file has to come out as.

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
/// One macro entry: spare word + body + display name.
const MACRO_ENTRY_BYTES: usize = 380;
/// Where an entry's command text starts, after its spare `u32`.
const ENTRY_BODY_OFFSET: usize = 4;
/// Where an entry's display name starts, and how wide the field is. The width is a tile law too: a
/// storable name is at most `ENTRY_NAME_BYTES - 1` characters, which is what sizes a palette block in
/// `hud::macro_pallet_view`.
const ENTRY_NAME_OFFSET: usize = 370;
pub const ENTRY_NAME_BYTES: usize = 10;
/// Sets named by one page file, and the bytes each name gets.
pub const SETS_PER_SET_PAGE: usize = 20;
const SET_NAME_BYTES: usize = 80;

/// One macro keytop as the client stores it: what a tile prints ([`MacroEntry::name`) and the
/// command lines that fire when it is used (`body`, `n`-separated exactly as stored).
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
            let name_start = start + ENTRY_NAME_OFFSET;
            MacroEntry {
                body: field(&bytes[start + ENTRY_BODY_OFFSET..name_start]),
                name: field(&bytes[name_start..name_start + ENTRY_NAME_BYTES]),
            }
        })
        .collect()
}

/// The twenty set names of one page file, or nothing when it is short of the full record arithmetic.
fn parse_set_names(bytes: &[u8]) -> Vec<Option<String>> {
    if bytes.len() < FILE_HEADER_BYTES + SET_NAME_BYTES {
        return Vec::new();
    }
    let count = (bytes.len() - FILE_HEADER_BYTES) / SET_NAME_BYTES;
    (0..count.min(SETS_PER_SET_PAGE))
        .map(|slot| {
            let start = FILE_HEADER_BYTES + slot * SET_NAME_BYTES;
            let name = field(&bytes[start..start + SET_NAME_BYTES]);
            (!name.is_empty()).then_some(name)
        })
        .collect()
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
        match std::fs::read(user_dir.join(title_file)) {
            Ok(bytes) => titles.extend(parse_titles(&bytes)),
            Err(_) => titles.extend(std::iter::repeat_n(None, TITLES_PER_TITLE_FILE)),
        }
    }

    let mut macro_books = Vec::new();
    for book in 0..crate::hud::menu::MACRO_BOOK_COUNT {
        match std::fs::read(user_dir.join(macro_file_name(book))) {
            Ok(bytes) => macro_books.push(parse_macro_entries(&bytes)),
            Err(_) => macro_books.push(Vec::new()),
        }
    }

    let mut set_names = Vec::new();
    for page in 0..crate::hud::menu::EQUIP_SET_COUNT / SETS_PER_SET_PAGE {
        match std::fs::read(user_dir.join(format!("es{page}.dat"))) {
            Ok(bytes) => set_names.extend(parse_set_names(&bytes)),
            Err(_) => set_names.extend(std::iter::repeat_n(None, SETS_PER_SET_PAGE)),
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
    /// The install's own file refused to be rewritten; the message names it.
    NotWritten(std::io::Error),
}

impl std::fmt::Display for MacroEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchBook => write!(f, "no such macro book"),
            Self::NothingToCopy => write!(f, "that macro book holds no macros"),
            Self::NothingCopied => write!(f, "nothing copied yet"),
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

/// A name cut to what one title slot holds (control characters dropped, never a partial code point).
pub fn truncate_to_book_title(name: &str) -> String {
    let mut kept = String::new();
    for ch in name.chars() {
        if ch.is_control() {
            continue;
        }
        if !book_title_fits(&kept, ch.len_utf8()) {
            break;
        }
        kept.push(ch);
    }
    kept
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

/// One keytop as its record in the file: unused word, command body, display name (module header).
fn encode_macro_entry(entry: &MacroEntry) -> Vec<u8> {
    let mut record = vec![0u8; MACRO_ENTRY_BYTES];
    write_field(
        &mut record[ENTRY_BODY_OFFSET..ENTRY_NAME_OFFSET],
        &entry.body,
        true,
    );
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

/// One book-name slot rewritten in its own file, every other byte of that file kept.
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
    rewrite_file(path, &bytes)
}

/// One book's records rewritten in its own file. An existing file keeps its header and any bytes past
/// the last record; a book with no file yet is created from `header` verbatim when the caller brought
/// one (what [`copy_macro_book`] read off the source) and left unwritten otherwise, because kuluu will
/// not invent the version word or the 16-byte tag (module header).
fn write_macro_book_file(
    path: &Path,
    entries: &[MacroEntry],
    header: Option<&[u8]>,
) -> Result<(), std::io::Error> {
    let mut bytes = match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= FILE_HEADER_BYTES => bytes,
        Ok(_) | Err(_) => {
            let Some(header) = header.filter(|h| h.len() == FILE_HEADER_BYTES) else {
                return Ok(());
            };
            Vec::from(header)
        }
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

/// What `Copy` put in the paste buffer: the book's keytops plus the 24 header bytes of the file they
/// came from, which [`paste_macro_book`] reuses verbatim if it has to create a file.
#[derive(Clone)]
struct CopiedMacroBook {
    entries: Vec<MacroEntry>,
    header: Option<Vec<u8>>,
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

/// `Copy`: take book `book`'s twenty keytops into the paste buffer, with its file's header along. Only
/// the buffer changes; nothing on disk does.
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
    let header = macro_book_path(book)
        .and_then(|path| std::fs::read(&path).ok())
        .map(|bytes| bytes[..FILE_HEADER_BYTES.min(bytes.len())].to_vec());
    *COPIED_MACRO_BOOK
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(CopiedMacroBook { entries, header });
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
        write_macro_book_file(&path, &entries, copied.header.as_deref())
            .map_err(MacroEditError::NotWritten)?;
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
        write_macro_book_file(&path, &[], None).map_err(MacroEditError::NotWritten)?;
    }
    {
        let mut files = files_write();
        ensure_library(&mut files);
        files.macro_books[book] = Vec::new();
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

    #[test]
    fn a_macro_entry_is_a_spare_word_then_body_then_name() {
        let mut bytes = header();
        for slot in 0..MACRO_BOOK_SLOTS {
            // One record, spare word included: `ENTRY_BODY_OFFSET` bytes of nothing come first.
            let entry_start = bytes.len();
            // Slot 4 stays as a player leaves it: no commands and no display name.
            let blank_keytop = slot == 4;
            let body = if slot == 19 {
                "/party Luring <t>!\n/magic \"Dia\" <t>\n/attack <t>"
            } else if blank_keytop {
                ""
            } else {
                "/yes"
            };
            let mut entry = vec![0u8; MACRO_ENTRY_BYTES];
            entry[ENTRY_BODY_OFFSET..ENTRY_BODY_OFFSET + body.len()]
                .copy_from_slice(body.as_bytes());
            if blank_keytop {
                bytes.extend(entry);
                continue;
            }
            let name = format!("N{slot}");
            entry[ENTRY_NAME_OFFSET..ENTRY_NAME_OFFSET + name.len()]
                .copy_from_slice(name.as_bytes());
            bytes.extend(entry);
            assert_eq!(bytes.len(), entry_start + MACRO_ENTRY_BYTES);
        }

        let entries = parse_macro_entries(&bytes);
        assert_eq!(entries.len(), MACRO_BOOK_SLOTS);
        assert_eq!(entries[0].name, "N0");
        assert_eq!(entries[0].body, "/yes");
        // A blank keytop reads as an empty entry rather than a name from next door.
        assert_eq!(entries[4], MacroEntry::default());
        // Several commands ride in one entry, newline separated, and survive verbatim.
        let last = &entries[MACRO_BOOK_SLOTS - 1];
        assert_eq!(last.name, "N19");
        assert_eq!(
            last.command_lines().collect::<Vec<_>>(),
            vec!["/party Luring <t>!", "/magic \"Dia\" <t>", "/attack <t>"]
        );
    }

    #[test]
    fn set_pages_hold_twenty_names_and_short_files_are_refused() {
        let mut bytes = header();
        for slot in 0..SETS_PER_SET_PAGE {
            let name = format!("Set{:03}", slot + 1);
            let mut entry = vec![0u8; SET_NAME_BYTES];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            bytes.extend(entry);
        }
        assert_eq!(
            bytes.len(),
            FILE_HEADER_BYTES + SETS_PER_SET_PAGE * SET_NAME_BYTES
        );
        let names = parse_set_names(&bytes);
        assert_eq!(names[0].as_deref(), Some("Set001"));
        assert_eq!(names[SETS_PER_SET_PAGE - 1].as_deref(), Some("Set020"));
        // A file short of even one whole name is refused, and a truncated tail drops just that
        // partial record instead of half-parsing it into an invented row.
        assert!(parse_set_names(&bytes[..FILE_HEADER_BYTES + SET_NAME_BYTES - 1]).is_empty());
        assert_eq!(
            parse_set_names(&bytes[..bytes.len() - 4]).len(),
            SETS_PER_SET_PAGE - 1
        );
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
        // Only the control character goes; everything else in that record came back as it went.
        assert_eq!(parsed[2].name, "tabhere");
        assert_eq!(parsed[2].body, written[2].body);
        // Beyond what was written the records stay blank keytops, not the previous book's words.
        assert_eq!(parsed[MACRO_BOOK_SLOTS - 1], MacroEntry::default());
    }
}
