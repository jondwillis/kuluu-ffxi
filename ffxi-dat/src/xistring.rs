//! XISTRING string tables (GetText tables 0, 8 and 0x0A) and the template formatter the client
//! runs over their entries
//! (.agents/skills/retail-observe/references/2026-10-15-gettext-tables.md). The client reads only
//! the version, the entry count and the index; the other header fields, which [`xistring_bytes`]
//! writes for fixtures, follow the vendored POLUtils reader
//! (vendor/POLUtils/PlayOnline.FFXI/FileTypes/XIStringTable.cs, Apache 2.0).

use std::ops::RangeInclusive;

use crate::gettext::LanguageRow;

const MAGIC: &[u8] = b"XISTRING";
/// The magic padded with NUL, right before the version.
const MAGIC_FIELD_LEN: usize = 10;
const VERSION_AT: usize = 0x0A;
const _: () = assert!(MAGIC.len() <= MAGIC_FIELD_LEN && MAGIC_FIELD_LEN == VERSION_AT);
const FILE_SIZE_AT: usize = 0x20;
const ENTRY_COUNT_AT: usize = 0x24;
const INDEX_BYTES_AT: usize = 0x28;
const TEXT_BYTES_AT: usize = 0x2C;
/// The index starts here and the text area follows it.
const INDEX_AT: usize = 0x38;

/// The first version whose index entries are [`INDEX_ENTRY_LEN`] bytes.
pub const WIDE_INDEX_VERSION: u16 = 2;
pub const INDEX_ENTRY_LEN: usize = 12;
const ENTRY_TEXT_OFFSET_AT: usize = 0;
/// The text's size, which only POLUtils reads.
const ENTRY_SIZE_AT: usize = 4;
const ENTRY_PARAM_COUNT_AT: usize = 6;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum XiStringError {
    #[error("XISTRING table too short for its header ({0} bytes)")]
    TooShort(usize),
    /// Below [`WIDE_INDEX_VERSION`] the client reads 4-byte index entries, whose layout the
    /// record leaves open; no such file is known.
    #[error("XISTRING version {version} uses the 4-byte index, which kuluu does not read")]
    NarrowIndex { version: u16 },
    #[error("XISTRING index of {count} entries runs past the file")]
    IndexPastEnd { count: u32 },
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_i16(bytes: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// One XISTRING file: entry `id` is index entry `id`, its text at the entry's offset into the
/// text area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XiStringTable {
    bytes: Vec<u8>,
    count: usize,
    text_base: usize,
}

/// One entry of an [`XiStringTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XiStringEntry<'a> {
    /// The file from the entry's text to its end: escape operands and jumps read on past the NUL
    /// that ends the plain text.
    tail: &'a [u8],
    /// Index byte 6: how many numeric parameters the formatted entry binds. A negative count
    /// binds none.
    pub param_count: i16,
}

impl<'a> XiStringEntry<'a> {
    /// The NUL-terminated text, escapes included: what a plain lookup returns.
    pub fn text(&self) -> &'a [u8] {
        let end = self
            .tail
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.tail.len());
        &self.tail[..end]
    }
}

impl XiStringTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, XiStringError> {
        if bytes.len() < INDEX_AT {
            return Err(XiStringError::TooShort(bytes.len()));
        }
        let version = read_u16(bytes, VERSION_AT).ok_or(XiStringError::TooShort(bytes.len()))?;
        if version < WIDE_INDEX_VERSION {
            return Err(XiStringError::NarrowIndex { version });
        }
        let count = read_u32(bytes, ENTRY_COUNT_AT).ok_or(XiStringError::TooShort(bytes.len()))?;
        let entries = usize::try_from(count).map_err(|_| XiStringError::IndexPastEnd { count })?;
        let text_base = entries
            .checked_mul(INDEX_ENTRY_LEN)
            .and_then(|index| index.checked_add(INDEX_AT))
            .filter(|&end| end <= bytes.len())
            .ok_or(XiStringError::IndexPastEnd { count })?;
        Ok(Self {
            bytes: bytes.to_vec(),
            count: entries,
            text_base,
        })
    }

    /// The entry count; ids at or past it have no string.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Entry `id`, `None` past the entry count or when its text offset points outside the file.
    pub fn entry(&self, id: usize) -> Option<XiStringEntry<'_>> {
        if id >= self.count {
            return None;
        }
        let at = INDEX_AT + id * INDEX_ENTRY_LEN;
        let offset = read_u32(&self.bytes, at + ENTRY_TEXT_OFFSET_AT)?;
        let param_count = read_i16(&self.bytes, at + ENTRY_PARAM_COUNT_AT)?;
        let start = self.text_base.checked_add(usize::try_from(offset).ok()?)?;
        Some(XiStringEntry {
            tail: self.bytes.get(start..)?,
            param_count,
        })
    }
}

/// An XISTRING file holding `entries` (text, numeric parameter count) in id order, each text
/// NUL-terminated, with every header field POLUtils validates. For fixtures.
pub fn xistring_bytes(entries: &[(&[u8], i16)]) -> Vec<u8> {
    let mut index = Vec::with_capacity(entries.len() * INDEX_ENTRY_LEN);
    let mut text = Vec::new();
    for &(entry, param_count) in entries {
        let mut slot = [0u8; INDEX_ENTRY_LEN];
        let offset = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let size = i16::try_from(entry.len() + 1).unwrap_or(i16::MAX);
        slot[ENTRY_TEXT_OFFSET_AT..ENTRY_TEXT_OFFSET_AT + 4].copy_from_slice(&offset.to_le_bytes());
        slot[ENTRY_SIZE_AT..ENTRY_SIZE_AT + 2].copy_from_slice(&size.to_le_bytes());
        slot[ENTRY_PARAM_COUNT_AT..ENTRY_PARAM_COUNT_AT + 2]
            .copy_from_slice(&param_count.to_le_bytes());
        index.extend_from_slice(&slot);
        text.extend_from_slice(entry);
        text.push(0);
    }
    let word = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes();
    let mut out = vec![0u8; INDEX_AT];
    out[..MAGIC.len()].copy_from_slice(MAGIC);
    out[VERSION_AT..VERSION_AT + 2].copy_from_slice(&WIDE_INDEX_VERSION.to_le_bytes());
    out[FILE_SIZE_AT..FILE_SIZE_AT + 4].copy_from_slice(&word(INDEX_AT + index.len() + text.len()));
    out[ENTRY_COUNT_AT..ENTRY_COUNT_AT + 4].copy_from_slice(&word(entries.len()));
    out[INDEX_BYTES_AT..INDEX_BYTES_AT + 4].copy_from_slice(&word(index.len()));
    out[TEXT_BYTES_AT..TEXT_BYTES_AT + 4].copy_from_slice(&word(text.len()));
    out.extend_from_slice(&index);
    out.extend_from_slice(&text);
    out
}

/// The lead byte of the Shift-JIS pair that introduces an escape; the byte after the pair is the
/// escape code. The pair's second byte carries nothing.
pub const ESCAPE_LEAD: u8 = 0xFA;
const PAIR_LEN: usize = 2;
const SJIS_TRAIL: [RangeInclusive<u8>; 2] = [0x40..=0x7E, 0x80..=0xFC];

fn is_pair(lead: u8, trail: u8) -> bool {
    crate::dmsg::is_sjis_lead(lead) && SJIS_TRAIL.iter().any(|range| range.contains(&trail))
}

/// Escape codes, the byte after the introducing pair. Any other code appends nothing and takes
/// no operands.
pub mod code {
    /// A byte naming a table 0 entry (low 7 bits), one ignored byte, then as many operands as
    /// that entry's parameter count: appends the entry formatted with those values as its
    /// numeric parameters and no string arguments.
    pub const SUB_TEMPLATE: u8 = 0x81;
    /// Operand `s`, u14 `n`, u14 end, then `n` u14 case offsets; the end and the offsets count
    /// from the byte after the code. Goes to case `s` when `0 <= s < n`, else to the end, which
    /// [`SWITCH_END`] reads.
    pub const SWITCH: u8 = 0x82;
    /// Operands `a`, `b`: sets the equal, less (`a < b`) and greater (`a > b`) flags.
    pub const COMPARE: u8 = 0x83;
    /// A signed jump length `L`: skips the `L` bytes after it.
    pub const JUMP: u8 = 0x84;
    /// [`JUMP`] when the flags say so, otherwise the scan goes on after `L`.
    pub const SKIP_IF_EQUAL: u8 = 0x85;
    pub const SKIP_IF_NOT_EQUAL: u8 = 0x86;
    pub const SKIP_IF_GREATER: u8 = 0x87;
    pub const SKIP_IF_LESS: u8 = 0x88;
    pub const SKIP_IF_GREATER_OR_EQUAL: u8 = 0x89;
    pub const SKIP_IF_LESS_OR_EQUAL: u8 = 0x8A;
    /// Operand `v`: appends it in decimal, most significant digit first, unpadded, ungrouped.
    pub const DECIMAL: u8 = 0x8B;
    /// Operand `k`: appends string argument `k`.
    pub const STRING_ARGUMENT: u8 = 0x8C;
    /// Goes to the last [`SWITCH`]'s end when the scan is still before it.
    pub const SWITCH_END: u8 = 0x8D;
    /// Operands item, form, article: appends the item's name ([`super::ItemNameRules`]).
    pub const ITEM_NAME: u8 = 0x8E;
}

/// Operand type bytes.
pub mod operand {
    /// `01 n`: numeric parameter `n & 0x7F`, counted from 1.
    pub const PARAMETER: u8 = 0x01;
    /// `02 v`: the value `v & 0x7F`.
    pub const VALUE: u8 = 0x02;
    /// `03 lo hi`: `(lo & 0x7F) + 128 (hi & 0x7F)`, the u14 form.
    pub const WIDE_VALUE: u8 = 0x03;
}

const OPERAND_VALUE_MASK: u8 = 0x7F;
const WIDE_HIGH_WEIGHT: u16 = 128;
const WIDE_LEN: usize = 2;

/// Kuluu's value for an operand the record leaves undefined: a numeric parameter past the ones
/// the entry binds (the client reads past its arguments there) or an operand type other than
/// [`operand::PARAMETER`], [`operand::VALUE`] and [`operand::WIDE_VALUE`], which kuluu takes as
/// one byte.
pub const UNDEFINED_OPERAND_VALUE: i32 = 0;
/// Kuluu's bound on the scan steps of one formatting, nested sub-templates included, so a jump
/// that loops back cannot hang the caller. The record does not bound the scan.
pub const SCAN_STEP_LIMIT: usize = 1 << 16;
/// Kuluu's bound on nested [`code::SUB_TEMPLATE`]s, past which one appends nothing. The record
/// does not say whether table 0 entries nest.
pub const SUB_TEMPLATE_DEPTH_LIMIT: usize = 8;

/// The buffer the merit, job point and alter ego point prompts format into: 255 bytes of text
/// and the NUL.
pub const PROMPT_BUFFER_BYTES: usize = 256;

/// [`code::ITEM_NAME`]'s form operand for the singular; any other form gives the plural.
pub const SINGULAR_FORM: i32 = 1;
/// [`code::ITEM_NAME`]'s article operand that puts an indefinite article before the singular.
pub const WITH_ARTICLE: i32 = 0;
const ARTICLE_A: &[u8] = b"a ";
const ARTICLE_AN: &[u8] = b"an ";
/// A singular starting with one of these, in either case, takes [`ARTICLE_AN`].
const ARTICLE_AN_INITIALS: &[u8] = b"aiueo";

/// The name of an item [`code::ITEM_NAME`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemNameForm {
    Singular,
    Plural,
    /// The Japanese rules' only form.
    Plain,
}

/// Item names for [`code::ITEM_NAME`]; `None` is an unknown item, which appends nothing.
pub trait ItemNameSource {
    fn item_name(&self, item: i32, form: ItemNameForm) -> Option<&[u8]>;
}

/// The record does not say which item DAT fields hold the singular, plural and plain names, so
/// this source knows no item and [`code::ITEM_NAME`] appends nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct UntracedItemNames;

impl ItemNameSource for UntracedItemNames {
    fn item_name(&self, _item: i32, _form: ItemNameForm) -> Option<&[u8]> {
        None
    }
}

/// The grammar [`code::ITEM_NAME`] follows, chosen by the language row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemNameRules {
    /// [`SINGULAR_FORM`] gives the singular, after "a " or "an " with [`WITH_ARTICLE`]; any
    /// other form the plural.
    English,
    /// The plain name; form and article are ignored.
    Japanese,
    /// The German and French rows' own rules, which were not traced: nothing is appended.
    NotTraced,
}

impl ItemNameRules {
    pub fn of(row: LanguageRow) -> Self {
        match row {
            LanguageRow::Japanese => Self::Japanese,
            LanguageRow::English => Self::English,
            LanguageRow::German | LanguageRow::French => Self::NotTraced,
        }
    }
}

/// What a formatted entry binds: string arguments 0, 1, ... and numeric parameters 1, 2, ....
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateArgs<'a> {
    /// Copied verbatim; escapes inside them are not expanded.
    pub strings: &'a [&'a [u8]],
    /// The entry binds the first [`XiStringEntry::param_count`] of these.
    pub numbers: &'a [i32],
}

/// What the formatter reads besides the entry and its arguments.
#[derive(Clone, Copy)]
pub struct TemplateContext<'a> {
    /// GetText table 0, which [`code::SUB_TEMPLATE`] formats from; `None` on a row that loads
    /// none. A sub-template the table lacks appends nothing and, its parameter count unknown,
    /// reads no operands: the record does not say how many the client skips there.
    pub sub_templates: Option<&'a XiStringTable>,
    pub item_names: &'a dyn ItemNameSource,
    pub item_rules: ItemNameRules,
}

/// `entry` formatted with the template escapes into a `buffer_bytes` buffer, without its NUL.
/// Text past the buffer is dropped byte by byte; the record does not say whether a Shift-JIS
/// pair cut at the edge keeps its first byte.
pub fn format_template(
    entry: XiStringEntry<'_>,
    args: TemplateArgs<'_>,
    cx: &TemplateContext<'_>,
    buffer_bytes: usize,
) -> Vec<u8> {
    let mut run = Run {
        cx,
        out: Vec::new(),
        limit: buffer_bytes.saturating_sub(1),
        steps_left: SCAN_STEP_LIMIT,
    };
    run.format_entry(entry, args, 0);
    run.out
}

#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    equal: bool,
    less: bool,
    greater: bool,
}

struct Scan<'a> {
    bytes: &'a [u8],
    pos: usize,
    flags: Flags,
    switch_end: Option<usize>,
}

impl Scan<'_> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    fn wide(&mut self) -> Option<u16> {
        let lo = self.byte()?;
        let hi = self.byte()?;
        Some(
            u16::from(lo & OPERAND_VALUE_MASK)
                + WIDE_HIGH_WEIGHT * u16::from(hi & OPERAND_VALUE_MASK),
        )
    }

    /// Reads a jump length and returns where the scan goes on: past the length, or that many
    /// bytes further when `taken`. `None` when that lands before the file.
    fn jump(&mut self, taken: bool) -> Option<usize> {
        let length = i8::from_le_bytes([self.byte()?]);
        if taken {
            self.pos.checked_add_signed(isize::from(length))
        } else {
            Some(self.pos)
        }
    }

    fn operand(&mut self, params: &[i32]) -> Option<i32> {
        Some(match self.byte()? {
            operand::PARAMETER => {
                let number = usize::from(self.byte()? & OPERAND_VALUE_MASK);
                number
                    .checked_sub(1)
                    .and_then(|index| params.get(index))
                    .copied()
                    .unwrap_or(UNDEFINED_OPERAND_VALUE)
            }
            operand::VALUE => i32::from(self.byte()? & OPERAND_VALUE_MASK),
            operand::WIDE_VALUE => i32::from(self.wide()?),
            _ => UNDEFINED_OPERAND_VALUE,
        })
    }
}

struct Run<'c> {
    cx: &'c TemplateContext<'c>,
    out: Vec<u8>,
    limit: usize,
    steps_left: usize,
}

fn bound_parameters(param_count: i16) -> usize {
    usize::try_from(param_count).unwrap_or(0)
}

impl Run<'_> {
    fn push(&mut self, bytes: &[u8]) {
        let room = self.limit.saturating_sub(self.out.len());
        self.out.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    fn format_entry(&mut self, entry: XiStringEntry<'_>, args: TemplateArgs<'_>, depth: usize) {
        let bound = bound_parameters(entry.param_count).min(args.numbers.len());
        let params = &args.numbers[..bound];
        let mut scan = Scan {
            bytes: entry.tail,
            pos: 0,
            flags: Flags::default(),
            switch_end: None,
        };
        while self.steps_left > 0 {
            self.steps_left -= 1;
            let Some(&byte) = scan.bytes.get(scan.pos) else {
                return;
            };
            if byte == 0 {
                return;
            }
            let pair = scan
                .bytes
                .get(scan.pos + 1)
                .is_some_and(|&trail| is_pair(byte, trail));
            if !pair {
                self.push(&[byte]);
                scan.pos += 1;
            } else if byte != ESCAPE_LEAD {
                self.push(&scan.bytes[scan.pos..scan.pos + PAIR_LEN]);
                scan.pos += PAIR_LEN;
            } else {
                scan.pos += PAIR_LEN;
                match self.escape(&mut scan, params, args.strings, depth) {
                    Some(next) => scan.pos = next,
                    None => return,
                }
            }
        }
    }

    /// Runs the escape whose code is next and returns where the scan goes on; `None` when the
    /// escape runs off the file or jumps before it.
    fn escape(
        &mut self,
        scan: &mut Scan<'_>,
        params: &[i32],
        strings: &[&[u8]],
        depth: usize,
    ) -> Option<usize> {
        let escape_code = scan.byte()?;
        let after_code = scan.pos;
        match escape_code {
            code::SUB_TEMPLATE => {
                let id = scan.byte()? & OPERAND_VALUE_MASK;
                scan.byte()?;
                let sub = self
                    .cx
                    .sub_templates
                    .and_then(|table| table.entry(usize::from(id)));
                let count = sub.map_or(0, |sub| bound_parameters(sub.param_count));
                let values = (0..count)
                    .map(|_| scan.operand(params))
                    .collect::<Option<Vec<_>>>()?;
                if let Some(sub) = sub.filter(|_| depth < SUB_TEMPLATE_DEPTH_LIMIT) {
                    let args = TemplateArgs {
                        strings: &[],
                        numbers: &values,
                    };
                    self.format_entry(sub, args, depth + 1);
                }
                Some(scan.pos)
            }
            code::SWITCH => {
                let selector = scan.operand(params)?;
                let cases = usize::from(scan.wide()?);
                let end = after_code + usize::from(scan.wide()?);
                scan.switch_end = Some(end);
                match usize::try_from(selector).ok().filter(|&case| case < cases) {
                    Some(case) => {
                        scan.pos += case * WIDE_LEN;
                        Some(after_code + usize::from(scan.wide()?))
                    }
                    None => Some(end),
                }
            }
            code::COMPARE => {
                let a = scan.operand(params)?;
                let b = scan.operand(params)?;
                scan.flags = Flags {
                    equal: a == b,
                    less: a < b,
                    greater: a > b,
                };
                Some(scan.pos)
            }
            code::JUMP => scan.jump(true),
            code::SKIP_IF_EQUAL => scan.jump(scan.flags.equal),
            code::SKIP_IF_NOT_EQUAL => scan.jump(!scan.flags.equal),
            code::SKIP_IF_GREATER => scan.jump(scan.flags.greater),
            code::SKIP_IF_LESS => scan.jump(scan.flags.less),
            code::SKIP_IF_GREATER_OR_EQUAL => scan.jump(scan.flags.greater || scan.flags.equal),
            code::SKIP_IF_LESS_OR_EQUAL => scan.jump(scan.flags.less || scan.flags.equal),
            code::DECIMAL => {
                let value = scan.operand(params)?;
                self.push(value.unsigned_abs().to_string().as_bytes());
                Some(scan.pos)
            }
            code::STRING_ARGUMENT => {
                let index = scan.operand(params)?;
                if let Some(text) = usize::try_from(index).ok().and_then(|i| strings.get(i)) {
                    self.push(text);
                }
                Some(scan.pos)
            }
            code::SWITCH_END => Some(
                scan.switch_end
                    .filter(|&end| scan.pos < end)
                    .unwrap_or(scan.pos),
            ),
            code::ITEM_NAME => {
                let item = scan.operand(params)?;
                let form = scan.operand(params)?;
                let article = scan.operand(params)?;
                self.item_name(item, form, article);
                Some(scan.pos)
            }
            _ => Some(scan.pos),
        }
    }

    fn item_name(&mut self, item: i32, form: i32, article: i32) {
        let names = self.cx.item_names;
        match self.cx.item_rules {
            ItemNameRules::English if form == SINGULAR_FORM => {
                let Some(name) = names.item_name(item, ItemNameForm::Singular) else {
                    return;
                };
                if article == WITH_ARTICLE {
                    let an = name.first().is_some_and(|initial| {
                        ARTICLE_AN_INITIALS.contains(&initial.to_ascii_lowercase())
                    });
                    self.push(if an { ARTICLE_AN } else { ARTICLE_A });
                }
                self.push(name);
            }
            ItemNameRules::English => {
                if let Some(name) = names.item_name(item, ItemNameForm::Plural) {
                    self.push(name);
                }
            }
            ItemNameRules::Japanese => {
                if let Some(name) = names.item_name(item, ItemNameForm::Plain) {
                    self.push(name);
                }
            }
            ItemNameRules::NotTraced => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gettext::Language;

    /// Any valid second byte for the introducing pair.
    const PAIR_TRAIL: u8 = 0x40;
    /// The bit operand bytes may carry above their 7-bit value.
    const HIGH_BIT: u8 = 0x80;
    const UNDEFINED_CODE: u8 = 0x90;

    fn esc(escape_code: u8) -> Vec<u8> {
        vec![ESCAPE_LEAD, PAIR_TRAIL, escape_code]
    }

    fn value(v: u8) -> Vec<u8> {
        vec![operand::VALUE, v]
    }

    fn param(n: u8) -> Vec<u8> {
        vec![operand::PARAMETER, n]
    }

    fn small(v: i32) -> u8 {
        u8::try_from(v).unwrap()
    }

    /// The u14 form with both high bits set, which the reader masks off.
    fn u14(v: u16) -> Vec<u8> {
        vec![
            u8::try_from(v % WIDE_HIGH_WEIGHT).unwrap() | HIGH_BIT,
            u8::try_from(v / WIDE_HIGH_WEIGHT).unwrap() | HIGH_BIT,
        ]
    }

    fn wide(v: u16) -> Vec<u8> {
        [vec![operand::WIDE_VALUE], u14(v)].concat()
    }

    fn jump_length(length: i8) -> Vec<u8> {
        length.to_le_bytes().to_vec()
    }

    fn context(item_rules: ItemNameRules) -> TemplateContext<'static> {
        TemplateContext {
            sub_templates: None,
            item_names: &UntracedItemNames,
            item_rules,
        }
    }

    fn format_with(
        template: &[u8],
        param_count: i16,
        strings: &[&[u8]],
        numbers: &[i32],
        cx: &TemplateContext,
    ) -> Vec<u8> {
        let table = XiStringTable::parse(&xistring_bytes(&[(template, param_count)])).unwrap();
        format_template(
            table.entry(0).unwrap(),
            TemplateArgs { strings, numbers },
            cx,
            PROMPT_BUFFER_BYTES,
        )
    }

    fn format(template: &[u8], param_count: i16, strings: &[&[u8]], numbers: &[i32]) -> Vec<u8> {
        format_with(
            template,
            param_count,
            strings,
            numbers,
            &context(ItemNameRules::English),
        )
    }

    /// A table laid out by hand from the record: only the version, the count and the index are
    /// read, offsets count from the text area, and the parameter count is index byte 6.
    #[test]
    fn hand_built_table_reads_by_the_record_layout() {
        const RECORD_PINNED_VERSION_AT: usize = 0x0A;
        const RECORD_PINNED_WIDE_VERSION: u16 = 2;
        const RECORD_PINNED_COUNT_AT: usize = 0x24;
        const RECORD_PINNED_INDEX_AT: usize = 0x38;
        const RECORD_PINNED_ENTRY_LEN: usize = 12;
        const RECORD_PINNED_PARAM_COUNT_AT: usize = 6;
        let mut bytes = vec![0u8; RECORD_PINNED_INDEX_AT + 2 * RECORD_PINNED_ENTRY_LEN];
        bytes[RECORD_PINNED_VERSION_AT..RECORD_PINNED_VERSION_AT + 2]
            .copy_from_slice(&RECORD_PINNED_WIDE_VERSION.to_le_bytes());
        bytes[RECORD_PINNED_COUNT_AT..RECORD_PINNED_COUNT_AT + 4]
            .copy_from_slice(&2u32.to_le_bytes());
        let text_area: &[u8] = b"second\0first\0";
        for (id, offset, param_count) in [(0usize, 7u32, 3i16), (1, 0, 1)] {
            let at = RECORD_PINNED_INDEX_AT + id * RECORD_PINNED_ENTRY_LEN;
            bytes[at..at + 4].copy_from_slice(&offset.to_le_bytes());
            bytes[at + RECORD_PINNED_PARAM_COUNT_AT..at + RECORD_PINNED_PARAM_COUNT_AT + 2]
                .copy_from_slice(&param_count.to_le_bytes());
        }
        bytes.extend_from_slice(text_area);

        let table = XiStringTable::parse(&bytes).expect("no magic or size check");
        assert_eq!(table.len(), 2);
        let first = table.entry(0).unwrap();
        assert_eq!((first.text(), first.param_count), (&b"first"[..], 3));
        let second = table.entry(1).unwrap();
        assert_eq!((second.text(), second.param_count), (&b"second"[..], 1));
        assert!(table.entry(2).is_none(), "ids at or past the count");
        assert_eq!(WIDE_INDEX_VERSION, RECORD_PINNED_WIDE_VERSION);
    }

    #[test]
    fn fixture_writer_fills_the_header_polutils_checks() {
        let bytes = xistring_bytes(&[(b"one", 0), (b"", 2), (b"three", -1)]);
        let word = |at: usize| read_u32(&bytes, at).unwrap() as usize;
        assert_eq!(&bytes[..MAGIC.len()], MAGIC);
        assert_eq!(read_u16(&bytes, VERSION_AT), Some(WIDE_INDEX_VERSION));
        assert_eq!(word(FILE_SIZE_AT), bytes.len());
        assert_eq!(word(ENTRY_COUNT_AT), 3);
        assert_eq!(word(INDEX_BYTES_AT), 3 * INDEX_ENTRY_LEN);
        assert_eq!(
            INDEX_AT + word(INDEX_BYTES_AT) + word(TEXT_BYTES_AT),
            bytes.len()
        );

        let table = XiStringTable::parse(&bytes).unwrap();
        assert_eq!(table.len(), 3);
        assert_eq!(table.entry(0).map(|e| e.text()), Some(&b"one"[..]));
        assert_eq!(
            table.entry(1).map(|e| (e.text(), e.param_count)),
            Some((&b""[..], 2))
        );
        assert_eq!(table.entry(2).map(|e| e.param_count), Some(-1));
    }

    #[test]
    fn a_version_below_two_is_the_untraced_narrow_index() {
        let narrow = WIDE_INDEX_VERSION - 1;
        let mut bytes = xistring_bytes(&[(b"x", 0)]);
        bytes[VERSION_AT..VERSION_AT + 2].copy_from_slice(&narrow.to_le_bytes());
        assert_eq!(
            XiStringTable::parse(&bytes),
            Err(XiStringError::NarrowIndex { version: narrow })
        );
    }

    #[test]
    fn short_files_counts_past_the_file_and_stray_offsets_read_nothing() {
        assert_eq!(
            XiStringTable::parse(&[0u8; INDEX_AT - 1]),
            Err(XiStringError::TooShort(INDEX_AT - 1))
        );
        let mut bytes = xistring_bytes(&[(b"x", 0)]);
        bytes[ENTRY_COUNT_AT..ENTRY_COUNT_AT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            XiStringTable::parse(&bytes),
            Err(XiStringError::IndexPastEnd { count: u32::MAX })
        );
        let mut bytes = xistring_bytes(&[(b"x", 0)]);
        bytes[INDEX_AT..INDEX_AT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(XiStringTable::parse(&bytes).unwrap().entry(0).is_none());
    }

    /// A Shift-JIS pair is copied whole, so an 0xFA that is a pair's second byte starts nothing.
    #[test]
    fn plain_bytes_and_shift_jis_pairs_copy_whole() {
        let template = [&b"ab"[..], &[0x82, 0xA0], &[0x81, ESCAPE_LEAD], b"c"].concat();
        assert_eq!(format(&template, 0, &[], &[]), template);
    }

    #[test]
    fn an_escape_lead_without_a_valid_second_byte_is_a_plain_byte() {
        let template = [ESCAPE_LEAD, b'%', ESCAPE_LEAD];
        assert_eq!(format(&template, 0, &[], &[]), template);
    }

    #[test]
    fn percent_signs_are_literal() {
        assert_eq!(format(b"100% %d %s %%", 1, &[b"x"], &[1]), b"100% %d %s %%");
    }

    #[test]
    fn operands_read_their_three_forms_masked_to_seven_bits() {
        let template = [
            esc(code::DECIMAL),
            value(5 | HIGH_BIT),
            b",".to_vec(),
            esc(code::DECIMAL),
            wide(300),
            b",".to_vec(),
            esc(code::DECIMAL),
            param(1 | HIGH_BIT),
        ]
        .concat();
        assert_eq!(format(&template, 1, &[], &[7]), b"5,300,7");
    }

    /// No padding, no grouping, no sign: kuluu prints a negative value's magnitude, which the
    /// record does not show.
    #[test]
    fn decimal_prints_bare_digits() {
        let template = [esc(code::DECIMAL), param(1)].concat();
        for (number, text) in [
            (0, &b"0"[..]),
            (16383, b"16383"),
            (1234567, b"1234567"),
            (-42, b"42"),
        ] {
            assert_eq!(format(&template, 1, &[], &[number]), text);
        }
    }

    #[test]
    fn string_arguments_count_from_zero_and_copy_verbatim() {
        let template = [
            esc(code::STRING_ARGUMENT),
            value(1),
            b"-".to_vec(),
            esc(code::STRING_ARGUMENT),
            value(0),
            esc(code::STRING_ARGUMENT),
            value(2),
        ]
        .concat();
        let holds_an_escape = [esc(code::DECIMAL), value(9)].concat();
        assert_eq!(
            format(&template, 0, &[b"zero", &holds_an_escape], &[]),
            [&holds_an_escape[..], b"-", b"zero"].concat(),
            "argument 2 was not given and appends nothing"
        );
    }

    /// The caller's string arguments come first and its numeric parameters after; each binds by
    /// its own count, whatever order the template reads them in.
    #[test]
    fn strings_and_numbers_bind_separately() {
        let template = [
            esc(code::DECIMAL),
            param(2),
            esc(code::STRING_ARGUMENT),
            value(0),
            esc(code::DECIMAL),
            param(1),
        ]
        .concat();
        assert_eq!(format(&template, 2, &[b"s"], &[10, 20]), b"20s10");
    }

    #[test]
    fn the_entry_binds_only_its_declared_parameter_count() {
        let template = [esc(code::DECIMAL), param(1)].concat();
        let undefined = UNDEFINED_OPERAND_VALUE.to_string().into_bytes();
        assert_eq!(format(&template, 1, &[], &[5]), b"5");
        assert_eq!(format(&template, 0, &[], &[5]), undefined, "count 0");
        assert_eq!(
            format(&template, -1, &[], &[5]),
            undefined,
            "negative count"
        );
        assert_eq!(format(&template, 1, &[], &[]), undefined, "none supplied");
        let parameter_zero = [esc(code::DECIMAL), param(0)].concat();
        assert_eq!(format(&parameter_zero, 1, &[], &[5]), undefined, "from 1");
    }

    /// `a ? b` then a skip of one byte over "X": the skip is taken when its flags hold.
    #[test]
    fn compare_sets_the_flags_each_skip_reads() {
        let skips = [
            (code::SKIP_IF_EQUAL, [false, true, false]),
            (code::SKIP_IF_NOT_EQUAL, [true, false, true]),
            (code::SKIP_IF_GREATER, [false, false, true]),
            (code::SKIP_IF_LESS, [true, false, false]),
            (code::SKIP_IF_GREATER_OR_EQUAL, [false, true, true]),
            (code::SKIP_IF_LESS_OR_EQUAL, [true, true, false]),
        ];
        for (skip, taken) in skips {
            for ((a, b), taken) in [(1, 2), (2, 2), (3, 2)].into_iter().zip(taken) {
                let template = [
                    esc(code::COMPARE),
                    value(a),
                    value(b),
                    esc(skip),
                    jump_length(1),
                    b"XY".to_vec(),
                ]
                .concat();
                let want: &[u8] = if taken { b"Y" } else { b"XY" };
                assert_eq!(format(&template, 0, &[], &[]), want, "{skip:#x} {a} {b}");
            }
        }
    }

    #[test]
    fn the_compare_is_signed() {
        let template = [
            esc(code::COMPARE),
            param(1),
            value(0),
            esc(code::SKIP_IF_LESS),
            jump_length(1),
            b"XY".to_vec(),
        ]
        .concat();
        assert_eq!(format(&template, 1, &[], &[-1]), b"Y");
    }

    #[test]
    fn every_flag_is_clear_when_an_entry_starts() {
        for (skip, taken) in [
            (code::SKIP_IF_EQUAL, false),
            (code::SKIP_IF_NOT_EQUAL, true),
            (code::SKIP_IF_GREATER, false),
            (code::SKIP_IF_LESS, false),
            (code::SKIP_IF_GREATER_OR_EQUAL, false),
            (code::SKIP_IF_LESS_OR_EQUAL, false),
        ] {
            let template = [esc(skip), jump_length(1), b"XY".to_vec()].concat();
            let want: &[u8] = if taken { b"Y" } else { b"XY" };
            assert_eq!(format(&template, 0, &[], &[]), want, "{skip:#x}");
        }
    }

    #[test]
    fn jumps_skip_forward_and_a_jump_before_the_entry_ends_it() {
        let forward = [esc(code::JUMP), jump_length(2), b"abc".to_vec()].concat();
        assert_eq!(format(&forward, 0, &[], &[]), b"c");
        let before = [
            b"x".to_vec(),
            esc(code::JUMP),
            jump_length(-10),
            b"y".to_vec(),
        ]
        .concat();
        assert_eq!(format(&before, 0, &[], &[]), b"x");
        let looping = [
            b"x".to_vec(),
            esc(code::JUMP),
            jump_length(-4),
            b"y".to_vec(),
        ]
        .concat();
        assert_eq!(
            format(&looping, 0, &[], &[]),
            b"x",
            "a jump back onto itself stops at the step bound"
        );
    }

    /// Cases "A" and "B" each end in a switch end; the end itself reads "E".
    #[test]
    fn switch_goes_to_its_case_or_its_end() {
        const CASE_A: u16 = 10;
        const CASE_B: u16 = 14;
        const END: u16 = 18;
        let switch = |selector: Vec<u8>| {
            [
                esc(code::SWITCH),
                selector,
                u14(2),
                u14(END),
                u14(CASE_A),
                u14(CASE_B),
                b"A".to_vec(),
                esc(code::SWITCH_END),
                b"B".to_vec(),
                esc(code::SWITCH_END),
                b"E".to_vec(),
            ]
            .concat()
        };
        assert_eq!(format(&switch(value(0)), 0, &[], &[]), b"AE");
        assert_eq!(format(&switch(value(1)), 0, &[], &[]), b"BE");
        assert_eq!(format(&switch(value(2)), 0, &[], &[]), b"E", "s = n");
        assert_eq!(format(&switch(param(1)), 1, &[], &[-1]), b"E", "s < 0");
        let no_switch = [b"a".to_vec(), esc(code::SWITCH_END), b"b".to_vec()].concat();
        assert_eq!(format(&no_switch, 0, &[], &[]), b"ab");
    }

    #[test]
    fn sub_templates_format_table_0_entries_with_the_operand_values() {
        let table_0 = XiStringTable::parse(&xistring_bytes(&[
            (b"zero", 0),
            (
                &[b"<".to_vec(), esc(code::DECIMAL), param(1), b">".to_vec()].concat(),
                1,
            ),
            (
                &[esc(code::STRING_ARGUMENT), value(0), b"|".to_vec()].concat(),
                0,
            ),
        ]))
        .unwrap();
        let cx = TemplateContext {
            sub_templates: Some(&table_0),
            ..context(ItemNameRules::English)
        };
        let ignored = 0x55;
        let template = [
            b"x".to_vec(),
            esc(code::SUB_TEMPLATE),
            vec![1 | HIGH_BIT, ignored],
            param(1),
            b"y".to_vec(),
            esc(code::SUB_TEMPLATE),
            vec![2, ignored],
            b"z".to_vec(),
        ]
        .concat();
        assert_eq!(
            format_with(&template, 1, &[b"s"], &[42], &cx),
            b"x<42>y|z",
            "the sub-template gets the operand values and no string arguments"
        );
        assert_eq!(
            format(&template[..template.len() - 1], 1, &[], &[42]),
            b"x\x01\x01y",
            "with no table 0 nothing is appended and no operand is read"
        );
    }

    #[test]
    fn nested_sub_templates_stop_at_the_depth_bound() {
        let nests = [b"n".to_vec(), esc(code::SUB_TEMPLATE), vec![0, 0]].concat();
        let table_0 = XiStringTable::parse(&xistring_bytes(&[(&nests, 0)])).unwrap();
        let cx = TemplateContext {
            sub_templates: Some(&table_0),
            ..context(ItemNameRules::English)
        };
        let template = [esc(code::SUB_TEMPLATE), vec![0, 0]].concat();
        assert_eq!(
            format_with(&template, 0, &[], &[], &cx),
            b"n".repeat(SUB_TEMPLATE_DEPTH_LIMIT)
        );
    }

    struct Names;

    impl ItemNameSource for Names {
        fn item_name(&self, item: i32, form: ItemNameForm) -> Option<&[u8]> {
            Some(match (item, form) {
                (1, ItemNameForm::Singular) => b"ingot",
                (1, ItemNameForm::Plural) => b"ingots",
                (1, ItemNameForm::Plain) => b"plain",
                (2, ItemNameForm::Singular) => b"Orb",
                (3, ItemNameForm::Singular) => b"bell",
                (3, ItemNameForm::Plural) => b"bells",
                _ => return None,
            })
        }
    }

    #[test]
    fn item_names_follow_the_rows_rules() {
        let item = |item: u8, form: i32, article: i32| {
            [
                esc(code::ITEM_NAME),
                value(item),
                value(small(form)),
                value(small(article)),
            ]
            .concat()
        };
        let other_form = SINGULAR_FORM + 1;
        let no_article = WITH_ARTICLE + 1;
        let with = |rules| TemplateContext {
            item_names: &Names,
            ..context(rules)
        };
        let english = with(ItemNameRules::English);
        for (template, want) in [
            (item(1, SINGULAR_FORM, WITH_ARTICLE), &b"an ingot"[..]),
            (item(2, SINGULAR_FORM, WITH_ARTICLE), b"an Orb"),
            (item(3, SINGULAR_FORM, WITH_ARTICLE), b"a bell"),
            (item(3, SINGULAR_FORM, no_article), b"bell"),
            (item(3, other_form, WITH_ARTICLE), b"bells"),
            (item(9, SINGULAR_FORM, WITH_ARTICLE), b""),
        ] {
            assert_eq!(format_with(&template, 0, &[], &[], &english), want);
        }
        let japanese = with(ItemNameRules::Japanese);
        assert_eq!(
            format_with(&item(1, other_form, WITH_ARTICLE), 0, &[], &[], &japanese),
            b"plain"
        );
        let not_traced = with(ItemNameRules::NotTraced);
        assert_eq!(
            format_with(
                &item(1, SINGULAR_FORM, WITH_ARTICLE),
                0,
                &[],
                &[],
                &not_traced
            ),
            b""
        );
        assert_eq!(
            format(&item(1, SINGULAR_FORM, WITH_ARTICLE), 0, &[], &[]),
            b"",
            "kuluu has no item names yet"
        );
    }

    #[test]
    fn the_language_row_picks_the_item_rules() {
        let rules = |language, flag| ItemNameRules::of(LanguageRow::of(language, flag));
        assert_eq!(rules(Language::English, false), ItemNameRules::English);
        assert_eq!(rules(Language::Japanese, true), ItemNameRules::Japanese);
        assert_eq!(rules(Language::German, false), ItemNameRules::NotTraced);
        assert_eq!(rules(Language::French, false), ItemNameRules::NotTraced);
        assert_eq!(rules(Language::German, true), ItemNameRules::English);
        assert_eq!(rules(Language::French, true), ItemNameRules::English);
    }

    #[test]
    fn undefined_codes_append_nothing_and_take_no_operands() {
        let template = [b"a".to_vec(), esc(UNDEFINED_CODE), b"b".to_vec()].concat();
        assert_eq!(format(&template, 0, &[], &[]), b"ab");
    }

    /// The code byte is read whatever it holds, even the NUL that ends the entry's text.
    #[test]
    fn escape_bytes_are_read_past_the_text_and_a_cut_escape_stops() {
        let table = XiStringTable::parse(&xistring_bytes(&[
            (&[ESCAPE_LEAD, PAIR_TRAIL][..], 0),
            (b"b", 0),
        ]))
        .unwrap();
        let entry = table.entry(0).unwrap();
        assert_eq!(entry.text(), [ESCAPE_LEAD, PAIR_TRAIL]);
        let cx = context(ItemNameRules::English);
        assert_eq!(
            format_template(entry, TemplateArgs::default(), &cx, PROMPT_BUFFER_BYTES),
            b"b"
        );

        let mut cut = xistring_bytes(&[(
            &[b"a".to_vec(), esc(code::DECIMAL), vec![operand::VALUE]].concat(),
            0,
        )]);
        cut.pop();
        let table = XiStringTable::parse(&cut).unwrap();
        assert_eq!(
            format_template(
                table.entry(0).unwrap(),
                TemplateArgs::default(),
                &cx,
                PROMPT_BUFFER_BYTES
            ),
            b"a"
        );
    }

    #[test]
    fn output_stops_at_the_buffer_less_its_nul() {
        let template = [esc(code::STRING_ARGUMENT), value(0), b"tail".to_vec()].concat();
        let long = vec![b'x'; PROMPT_BUFFER_BYTES + 1];
        let out = format(&template, 0, &[&long], &[]);
        assert_eq!(out.len(), PROMPT_BUFFER_BYTES - 1);
        assert!(out.iter().all(|&b| b == b'x'));
        let table = XiStringTable::parse(&xistring_bytes(&[(b"abc", 0)])).unwrap();
        let cx = context(ItemNameRules::English);
        assert_eq!(
            format_template(table.entry(0).unwrap(), TemplateArgs::default(), &cx, 0),
            b""
        );
    }
}
