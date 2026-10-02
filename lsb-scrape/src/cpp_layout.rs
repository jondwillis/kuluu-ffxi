//! `offsetof` for the structs LSB's packet headers declare, so a decoder's body
//! offsets can be pinned to upstream instead of to each other.
//!
//! The layout rule is the one the headers assume, read off their own sources:
//! the s2c headers walked here carry no `#pragma pack` (the ones that need it
//! say so at the struct, e.g. `0x113_currencies_1.h`), so members sit at their
//! natural alignment and the struct is padded out to its strictest member's.
//! Bit-fields are packed into a storage unit of their declared type and a field
//! that does not fit opens a new unit; a field that would *straddle* a unit is
//! an error rather than a guess, because MSVC and the Itanium ABI lay that case
//! out differently and a wire struct depending on it has no single answer.

use std::collections::{BTreeSet, HashMap};

use anyhow::{bail, Context, Result};

/// Sizes the C++ fixed-width types guarantee by definition, plus the plain
/// types at the sizes both LSB's LP64 build and the LLP64 Windows client agree
/// on. `long` is deliberately absent: those two disagree about it, so a header
/// using it has no single layout and must fail rather than resolve. Alignment
/// equals size for every one of these under the x86-64 SysV and Windows ABIs.
const FIXED_WIDTH_TYPES: &[(&str, usize)] = &[
    ("bool", 1),
    ("char", 1),
    ("signed char", 1),
    ("unsigned char", 1),
    ("int8_t", 1),
    ("uint8_t", 1),
    ("int8", 1),
    ("uint8", 1),
    ("short", 2),
    ("short int", 2),
    ("unsigned short", 2),
    ("unsigned short int", 2),
    ("int16_t", 2),
    ("uint16_t", 2),
    ("int16", 2),
    ("uint16", 2),
    ("int", 4),
    ("signed int", 4),
    ("unsigned", 4),
    ("unsigned int", 4),
    ("int32_t", 4),
    ("uint32_t", 4),
    ("int32", 4),
    ("uint32", 4),
    ("float", 4),
    ("long long", 8),
    ("unsigned long long", 8),
    ("int64_t", 8),
    ("uint64_t", 8),
    ("int64", 8),
    ("uint64", 8),
    ("double", 8),
];

/// An unscoped `enum`'s underlying type is implementation-defined but is `int`
/// on every compiler LSB and the client are built with.
const DEFAULT_ENUM_UNDERLYING: &str = "int";

const BITS_PER_BYTE: u32 = 8;

/// Stands in for the declarator of a member whose declaration the walker could
/// not read, so laying the struct out fails on its unresolvable type instead of
/// treating it as unnamed padding.
const UNPARSED_MEMBER: &str = "<unparsed>";

/// Where a bit-field sits inside the storage unit at its [`Field::offset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitSpan {
    pub shift: u32,
    pub width: u32,
}

/// One member of a laid-out struct, flattened: `path` names the member chain
/// from the outermost struct down, so a nested struct contributes both its own
/// entry and one per field it contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub path: Vec<String>,
    pub offset: usize,
    /// One element's size for an array member, the whole member's otherwise.
    pub stride: usize,
    pub count: Option<usize>,
    pub bits: Option<BitSpan>,
}

impl Field {
    /// Bytes the member occupies, array length included.
    pub fn len(&self) -> usize {
        self.stride * self.count.unwrap_or(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `.`-joined member chain, for error messages.
    pub fn dotted(&self) -> String {
        self.path.join(".")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    pub name: String,
    pub size: usize,
    pub align: usize,
    pub fields: Vec<Field>,
}

impl StructLayout {
    pub fn field(&self, dotted_path: &str) -> Result<&Field> {
        self.fields
            .iter()
            .find(|f| f.dotted() == dotted_path)
            .with_context(|| format!("`{}` has no member `{dotted_path}`", self.name))
    }

    pub fn offset_of(&self, dotted_path: &str) -> Result<usize> {
        Ok(self.field(dotted_path)?.offset)
    }
}

#[derive(Debug, Clone)]
struct Decl {
    ty: String,
    name: String,
    dims: Vec<usize>,
    bit_width: Option<u32>,
}

#[derive(Debug, Clone)]
struct RawStruct {
    name: String,
    members: Vec<Decl>,
}

#[derive(Debug, Clone, Copy)]
enum Resolved {
    Scalar { size: usize, align: usize },
    Struct,
}

/// The type table a header scan builds: fixed-width scalars, the enums and
/// structs the scanned sources declare, and any scalar alias registered for a
/// type whose definition is generated and so absent from the tree.
#[derive(Debug)]
pub struct Layouts {
    scalars: HashMap<String, (usize, usize)>,
    structs: HashMap<String, RawStruct>,
    ambiguous: BTreeSet<String>,
}

impl Default for Layouts {
    fn default() -> Self {
        Self::new()
    }
}

impl Layouts {
    pub fn new() -> Self {
        let scalars = FIXED_WIDTH_TYPES
            .iter()
            .map(|(name, size)| ((*name).to_string(), (*size, *size)))
            .collect();
        Self {
            scalars,
            structs: HashMap::new(),
            ambiguous: BTreeSet::new(),
        }
    }

    /// Give `name` the layout of `underlying`. LSB generates `xi::Job` and
    /// friends from `data/enums/*.yaml` at its own build time, so their headers
    /// are not in the tree and the underlying type has to come from the yaml.
    pub fn register_scalar(&mut self, name: &str, underlying: &str) -> Result<()> {
        let (size, align) = *self
            .scalars
            .get(underlying)
            .with_context(|| format!("`{name}` has unknown underlying type `{underlying}`"))?;
        self.scalars.insert(name.to_string(), (size, align));
        Ok(())
    }

    /// Read every `struct`/`class`/`enum` a C++ source declares into the table.
    pub fn scan(&mut self, src: &str) -> Result<()> {
        let toks = tokenize(&strip_noncode(src));
        let mut cursor = 0;
        self.scan_scope(&toks, &mut cursor, toks.len(), "")
    }

    /// Flattened layout of a struct, by qualified (`Outer::Inner`) or simple name.
    pub fn layout(&self, name: &str) -> Result<StructLayout> {
        let raw = self.lookup_struct(name)?;
        let mut fields = Vec::new();
        let mut visiting = Vec::new();
        self.flatten(raw, 0, &mut Vec::new(), &mut fields, &mut visiting)?;
        let (size, align) = self.size_align_of_struct(raw, &mut Vec::new())?;
        Ok(StructLayout {
            name: raw.name.clone(),
            size,
            align,
            fields,
        })
    }

    fn lookup_struct(&self, name: &str) -> Result<&RawStruct> {
        if let Some(raw) = self.structs.get(name) {
            return Ok(raw);
        }
        if self.ambiguous.contains(name) {
            bail!("`{name}` names more than one struct in the scanned sources — qualify it");
        }
        bail!("no struct `{name}` in the scanned sources")
    }

    fn resolve(&self, ty: &str) -> Result<Resolved> {
        if let Some((size, align)) = self.scalars.get(ty) {
            return Ok(Resolved::Scalar {
                size: *size,
                align: *align,
            });
        }
        if self.structs.contains_key(ty) {
            return Ok(Resolved::Struct);
        }
        if let Some((_, tail)) = ty.rsplit_once("::") {
            if self.scalars.contains_key(tail) || self.structs.contains_key(tail) {
                return self.resolve(tail);
            }
        }
        bail!("unknown type `{ty}`")
    }

    fn struct_for(&self, ty: &str) -> Option<&RawStruct> {
        if let Some(raw) = self.structs.get(ty) {
            return Some(raw);
        }
        ty.rsplit_once("::")
            .and_then(|(_, tail)| self.structs.get(tail))
    }

    fn size_align_of_struct(
        &self,
        raw: &RawStruct,
        visiting: &mut Vec<String>,
    ) -> Result<(usize, usize)> {
        let mut placed = Vec::new();
        self.place_members(raw, visiting, &mut placed)
    }

    /// Runs the allocator over one struct's members, returning its size and
    /// alignment and appending `(member index, byte offset, bit span)` per member.
    /// An unnamed zero-width bit-field exists only to close the current storage
    /// unit, so the next one starts fresh.
    fn place_members(
        &self,
        raw: &RawStruct,
        visiting: &mut Vec<String>,
        placed: &mut Vec<(usize, usize, Option<BitSpan>)>,
    ) -> Result<(usize, usize)> {
        if visiting.iter().any(|n| n == &raw.name) {
            bail!("`{}` contains itself", raw.name);
        }
        visiting.push(raw.name.clone());

        let mut offset = 0usize;
        let mut struct_align = 1usize;
        let mut unit: Option<(usize, usize, u32, String)> = None;

        for (index, decl) in raw.members.iter().enumerate() {
            let (size, align) = self
                .size_align_of(&decl.ty, visiting)
                .with_context(|| format!("`{}::{}`", raw.name, decl.name))?;
            struct_align = struct_align.max(align);

            let Some(width) = decl.bit_width else {
                unit = None;
                offset = align_up(offset, align);
                placed.push((index, offset, None));
                let count: usize = decl.dims.iter().product::<usize>().max(1);
                offset += size * count;
                continue;
            };

            if !decl.dims.is_empty() {
                bail!("`{}::{}` is an array of bit-fields", raw.name, decl.name);
            }
            if width == 0 {
                if !decl.name.is_empty() {
                    bail!(
                        "`{}::{}` is a named zero-width bit-field",
                        raw.name,
                        decl.name
                    );
                }
                unit = None;
                continue;
            }
            let unit_bits = size as u32 * BITS_PER_BYTE;
            if width > unit_bits {
                bail!(
                    "`{}::{}` is {width} bits wide but its type holds {unit_bits}",
                    raw.name,
                    decl.name
                );
            }
            match &mut unit {
                Some((unit_offset, unit_size, used, unit_ty))
                    if *unit_size == size && *unit_ty == decl.ty && *used + width <= unit_bits =>
                {
                    placed.push((
                        index,
                        *unit_offset,
                        Some(BitSpan {
                            shift: *used,
                            width,
                        }),
                    ));
                    *used += width;
                }
                Some((_, unit_size, used, unit_ty))
                    if *unit_size == size && *unit_ty == decl.ty && *used < unit_bits =>
                {
                    bail!(
                        "`{}::{}` would straddle its {unit_bits}-bit storage unit — MSVC and the \
                         Itanium ABI place that differently, so the header has no single layout",
                        raw.name,
                        decl.name
                    );
                }
                _ => {
                    offset = align_up(offset, align);
                    placed.push((index, offset, Some(BitSpan { shift: 0, width })));
                    unit = Some((offset, size, width, decl.ty.clone()));
                    offset += size;
                }
            }
        }

        visiting.pop();
        Ok((align_up(offset, struct_align), struct_align))
    }

    fn size_align_of(&self, ty: &str, visiting: &mut Vec<String>) -> Result<(usize, usize)> {
        match self.resolve(ty)? {
            Resolved::Scalar { size, align } => Ok((size, align)),
            Resolved::Struct => {
                let raw = self
                    .struct_for(ty)
                    .with_context(|| format!("struct `{ty}` vanished from the table"))?;
                self.size_align_of_struct(raw, visiting)
            }
        }
    }

    fn flatten(
        &self,
        raw: &RawStruct,
        base: usize,
        path: &mut Vec<String>,
        out: &mut Vec<Field>,
        visiting: &mut Vec<String>,
    ) -> Result<()> {
        let mut placed = Vec::new();
        self.place_members(raw, visiting, &mut placed)?;
        for (index, offset, bits) in placed {
            let decl = &raw.members[index];
            if decl.name.is_empty() {
                continue;
            }
            let (size, _) = self.size_align_of(&decl.ty, visiting)?;
            let count = if decl.dims.is_empty() {
                None
            } else {
                Some(decl.dims.iter().product::<usize>())
            };
            path.push(decl.name.clone());
            out.push(Field {
                path: path.clone(),
                offset: base + offset,
                stride: size,
                count,
                bits,
            });
            if let (Resolved::Struct, Some(nested)) =
                (self.resolve(&decl.ty)?, self.struct_for(&decl.ty))
            {
                self.flatten(nested, base + offset, path, out, visiting)?;
            }
            path.pop();
        }
        Ok(())
    }

    fn register_struct(&mut self, qualified: String, members: Vec<Decl>) {
        let simple = qualified
            .rsplit_once("::")
            .map(|(_, tail)| tail.to_string())
            .unwrap_or_else(|| qualified.clone());
        let raw = RawStruct {
            name: qualified.clone(),
            members,
        };
        if simple != qualified {
            if self.structs.contains_key(&simple) {
                self.ambiguous.insert(simple.clone());
                self.structs.remove(&simple);
            } else if !self.ambiguous.contains(&simple) {
                self.structs.insert(simple, raw.clone());
            }
        }
        self.structs.insert(qualified, raw);
    }

    fn scan_scope(&mut self, toks: &[Tok], i: &mut usize, end: usize, scope: &str) -> Result<()> {
        while *i < end {
            let Tok::Word(word) = &toks[*i] else {
                *i += 1;
                continue;
            };
            match word.as_str() {
                "namespace" => {
                    let name = match &toks.get(*i + 1) {
                        Some(Tok::Word(n)) => n.clone(),
                        _ => String::new(),
                    };
                    let Some(open) = find_tok(toks, *i, end, Tok::Punct('{')) else {
                        *i += 1;
                        continue;
                    };
                    let close = matching_brace(toks, open, end)?;
                    let inner_scope = if name.is_empty() {
                        scope.to_string()
                    } else {
                        format!("{scope}{name}::")
                    };
                    let mut inner = open + 1;
                    self.scan_scope(toks, &mut inner, close, &inner_scope)?;
                    *i = close + 1;
                }
                "enum" => {
                    *i = self.scan_enum(toks, *i, end, scope)?;
                }
                "struct" | "class" | "union" => {
                    *i = self.scan_record(toks, *i, end, scope)?;
                }
                _ => *i += 1,
            }
        }
        Ok(())
    }

    fn scan_enum(&mut self, toks: &[Tok], start: usize, end: usize, scope: &str) -> Result<usize> {
        let mut i = start + 1;
        if matches!(&toks.get(i), Some(Tok::Word(w)) if w == "class" || w == "struct") {
            i += 1;
        }
        let Some(Tok::Word(name)) = toks.get(i) else {
            return Ok(start + 1);
        };
        let name = name.clone();
        i += 1;
        let mut underlying = DEFAULT_ENUM_UNDERLYING.to_string();
        if matches!(toks.get(i), Some(Tok::Punct(':'))) {
            let mut words = Vec::new();
            i += 1;
            while let Some(Tok::Word(w)) = toks.get(i) {
                words.push(w.clone());
                i += 1;
            }
            if !words.is_empty() {
                underlying = words.join(" ");
            }
        }
        if !matches!(toks.get(i), Some(Tok::Punct('{'))) {
            return Ok(start + 1);
        }
        let close = matching_brace(toks, i, end)?;
        self.register_scalar(&format!("{scope}{name}"), &underlying)
            .with_context(|| format!("enum `{scope}{name}`"))?;
        self.register_scalar(&name, &underlying)?;
        Ok(close + 1)
    }

    fn scan_record(
        &mut self,
        toks: &[Tok],
        start: usize,
        end: usize,
        scope: &str,
    ) -> Result<usize> {
        let Some(Tok::Word(name)) = toks.get(start + 1) else {
            return Ok(start + 1);
        };
        let name = name.clone();
        let mut i = start + 2;
        while i < end && !matches!(toks[i], Tok::Punct('{') | Tok::Punct(';')) {
            i += 1;
        }
        if i >= end || toks[i] == Tok::Punct(';') {
            return Ok(i.min(end.saturating_sub(1)) + 1);
        }
        let close = matching_brace(toks, i, end)?;
        let qualified = format!("{scope}{name}");
        let members = self.parse_members(toks, i + 1, close, &format!("{qualified}::"))?;
        self.register_struct(qualified, members);
        Ok(close + 1)
    }

    fn parse_members(
        &mut self,
        toks: &[Tok],
        start: usize,
        end: usize,
        scope: &str,
    ) -> Result<Vec<Decl>> {
        const SKIP_TO_END_OF_STATEMENT: &[&str] = &[
            "using",
            "typedef",
            "static",
            "friend",
            "template",
            "constexpr",
            "inline",
            "virtual",
            "explicit",
            "auto",
            "operator",
            "~",
        ];
        let mut members = Vec::new();
        let mut decl: Vec<Tok> = Vec::new();
        let mut i = start;
        while i < end {
            match &toks[i] {
                Tok::Punct(';') => {
                    if !decl.is_empty() {
                        members.push(parse_decl(&decl));
                    }
                    decl.clear();
                    i += 1;
                }
                Tok::Punct('(') => {
                    decl.clear();
                    i = skip_statement(toks, i, end)?;
                }
                Tok::Punct('{') => {
                    decl.clear();
                    i = matching_brace(toks, i, end)? + 1;
                }
                Tok::Word(w) if w == "public" || w == "private" || w == "protected" => {
                    decl.clear();
                    i += 1;
                    if matches!(toks.get(i), Some(Tok::Punct(':'))) {
                        i += 1;
                    }
                }
                Tok::Word(w)
                    if decl.is_empty() && SKIP_TO_END_OF_STATEMENT.contains(&w.as_str()) =>
                {
                    i = skip_statement(toks, i, end)?;
                }
                Tok::Word(w)
                    if decl.is_empty() && (w == "struct" || w == "class" || w == "union") =>
                {
                    i = self.scan_record(toks, i, end, scope)?;
                }
                Tok::Word(w) if decl.is_empty() && w == "enum" => {
                    i = self.scan_enum(toks, i, end, scope)?;
                }
                tok => {
                    decl.push(tok.clone());
                    i += 1;
                }
            }
        }
        Ok(members)
    }
}

/// A member declaration that did not fit `Type name[dims] : bits` is kept with
/// its raw tokens as the type, so it only fails the build if the struct holding
/// it is actually laid out.
fn parse_decl(toks: &[Tok]) -> Decl {
    let unparsed = |toks: &[Tok]| Decl {
        ty: render_type(toks),
        name: UNPARSED_MEMBER.to_string(),
        dims: Vec::new(),
        bit_width: None,
    };

    let mut toks = toks;
    while matches!(toks.first(), Some(Tok::Word(w)) if w == "const" || w == "volatile" || w == "mutable")
    {
        toks = &toks[1..];
    }

    let mut bit_width = None;
    if toks.len() >= 3 && toks[toks.len() - 2] == Tok::Punct(':') {
        let Some(Tok::Word(w)) = toks.last() else {
            return unparsed(toks);
        };
        let Some(width) = parse_usize(w) else {
            return unparsed(toks);
        };
        bit_width = Some(width as u32);
        toks = &toks[..toks.len() - 2];
    }

    let mut dims = Vec::new();
    while toks.len() >= 3 && toks[toks.len() - 1] == Tok::Punct(']') {
        let Some(Tok::Word(w)) = toks.get(toks.len() - 2) else {
            return unparsed(toks);
        };
        if toks[toks.len() - 3] != Tok::Punct('[') {
            return unparsed(toks);
        }
        let Some(dim) = parse_usize(w) else {
            return unparsed(toks);
        };
        dims.insert(0, dim);
        toks = &toks[..toks.len() - 3];
    }

    if bit_width.is_some() && dims.is_empty() && toks.len() == 1 {
        return Decl {
            ty: render_type(toks),
            name: String::new(),
            dims,
            bit_width,
        };
    }
    let Some(Tok::Word(name)) = toks.last() else {
        return unparsed(toks);
    };
    if toks.len() < 2 || parse_usize(name).is_some() {
        return unparsed(toks);
    }
    Decl {
        ty: render_type(&toks[..toks.len() - 1]),
        name: name.clone(),
        dims,
        bit_width,
    }
}

fn render_type(toks: &[Tok]) -> String {
    let mut out = String::new();
    let mut prev_word = false;
    for tok in toks {
        match tok {
            Tok::Word(w) => {
                if prev_word {
                    out.push(' ');
                }
                out.push_str(w);
                prev_word = true;
            }
            Tok::Punct(c) => {
                out.push(*c);
                prev_word = false;
            }
        }
    }
    out
}

fn parse_usize(word: &str) -> Option<usize> {
    match word.strip_prefix("0x").or_else(|| word.strip_prefix("0X")) {
        Some(hex) => usize::from_str_radix(hex, 16).ok(),
        None => word.parse().ok(),
    }
}

fn align_up(offset: usize, align: usize) -> usize {
    offset.div_ceil(align.max(1)) * align.max(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Punct(char),
}

fn find_tok(toks: &[Tok], from: usize, end: usize, want: Tok) -> Option<usize> {
    (from..end).find(|i| toks[*i] == want)
}

fn matching_brace(toks: &[Tok], open: usize, end: usize) -> Result<usize> {
    let mut depth = 0usize;
    for (i, tok) in toks.iter().enumerate().take(end).skip(open) {
        match tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    bail!("unbalanced `{{` in source")
}

/// Past the end of a declaration we do not model: to the `;` that closes it, or
/// past the braced body it carries instead.
fn skip_statement(toks: &[Tok], from: usize, end: usize) -> Result<usize> {
    let mut i = from;
    let mut paren = 0usize;
    while i < end {
        match &toks[i] {
            Tok::Punct('(') => paren += 1,
            Tok::Punct(')') => paren = paren.saturating_sub(1),
            Tok::Punct(';') if paren == 0 => return Ok(i + 1),
            Tok::Punct('{') if paren == 0 => {
                let close = matching_brace(toks, i, end)?;
                let mut next = close + 1;
                if matches!(toks.get(next), Some(Tok::Punct(';'))) {
                    next += 1;
                }
                return Ok(next);
            }
            _ => {}
        }
        i += 1;
    }
    Ok(end)
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphanumeric() || c == '_' {
            let mut word = String::from(c);
            while let Some(n) = chars.peek() {
                if n.is_ascii_alphanumeric() || *n == '_' {
                    word.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            out.push(Tok::Word(word));
        } else if !c.is_whitespace() {
            out.push(Tok::Punct(c));
        }
    }
    out
}

/// Comments, preprocessor lines and literals out, so the token stream is
/// declarations only.
fn strip_noncode(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut at_line_start = true;
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
                out.push('\n');
                at_line_start = true;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                out.push(' ');
                at_line_start = false;
            }
            '#' if at_line_start => {
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if n == '\n' && prev != '\\' {
                        break;
                    }
                    prev = n;
                }
                out.push('\n');
                at_line_start = true;
            }
            '"' | '\'' => {
                let quote = c;
                while let Some(n) = chars.next() {
                    if n == '\\' {
                        chars.next();
                    } else if n == quote {
                        break;
                    }
                }
                out.push(' ');
                at_line_start = false;
            }
            '\n' => {
                out.push('\n');
                at_line_start = true;
            }
            c => {
                if !c.is_whitespace() {
                    at_line_start = false;
                }
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r#"
        #pragma once
        #include "base.h"

        // A synthetic LSB-style header: nested struct, array, bit-field run,
        // explicit u8 padding.
        struct Inner
        {
            uint16_t a;      /* 0x00 */
            uint8_t  b[3];   /* 0x02 */
            uint8_t  pad05;  /* 0x05 */
            uint32_t c;      /* 0x08 */
        };

        enum class Mode : uint16_t
        {
            None = 0,
            Some = 1,
        };

        struct Attr
        {
            uint32_t low : 2;
            uint32_t mid : 6;
            uint32_t high : 24;
        };

        class Outer final : public Base<Mode, Outer>
        {
        public:
            struct PacketData
            {
                uint8_t  lead;       /* 0x00 */
                Inner    inner;      /* 0x04 */
                Attr     attr;       /* 0x10 */
                Mode     mode;       /* 0x14 */
                uint8_t  pad16;      /* 0x16 */
                Inner    table[2];   /* 0x18 */
                float    tail;       /* 0x30 */
            };

            Outer(const char* name, int count);
            auto copy() const -> int override;
        };
    "#;

    fn layouts() -> Layouts {
        let mut l = Layouts::new();
        l.scan(HEADER).unwrap();
        l
    }

    #[test]
    fn nested_struct_array_and_padding_offsets() {
        let l = layouts();
        let inner = l.layout("Inner").unwrap();
        assert_eq!(inner.size, 12);
        assert_eq!(inner.align, 4);
        assert_eq!(inner.offset_of("a").unwrap(), 0);
        assert_eq!(inner.offset_of("b").unwrap(), 2);
        assert_eq!(inner.field("b").unwrap().count, Some(3));
        assert_eq!(inner.offset_of("pad05").unwrap(), 5);
        assert_eq!(inner.offset_of("c").unwrap(), 8);

        let data = l.layout("Outer::PacketData").unwrap();
        assert_eq!(data.offset_of("lead").unwrap(), 0x00);
        assert_eq!(data.offset_of("inner").unwrap(), 0x04);
        assert_eq!(data.offset_of("inner.c").unwrap(), 0x0C);
        assert_eq!(data.offset_of("attr").unwrap(), 0x10);
        assert_eq!(data.offset_of("mode").unwrap(), 0x14);
        assert_eq!(data.offset_of("pad16").unwrap(), 0x16);
        assert_eq!(data.offset_of("table").unwrap(), 0x18);
        assert_eq!(data.offset_of("tail").unwrap(), 0x30);
        assert_eq!(data.size, 0x34);
    }

    #[test]
    fn array_of_structs_reports_stride_and_element_fields() {
        let data = layouts().layout("Outer::PacketData").unwrap();
        let table = data.field("table").unwrap();
        assert_eq!(table.count, Some(2));
        assert_eq!(table.stride, 12);
        assert_eq!(table.len(), 24);
        assert_eq!(data.offset_of("table.b").unwrap(), 0x1A);
    }

    #[test]
    fn bitfields_share_a_storage_unit() {
        let l = layouts();
        let attr = l.layout("Attr").unwrap();
        assert_eq!(attr.size, 4);
        for (name, shift, width) in [("low", 0, 2), ("mid", 2, 6), ("high", 8, 24)] {
            let field = attr.field(name).unwrap();
            assert_eq!(field.offset, 0);
            assert_eq!(field.bits, Some(BitSpan { shift, width }));
        }
        let data = l.layout("Outer::PacketData").unwrap();
        assert_eq!(data.offset_of("attr.mid").unwrap(), 0x10);
        assert_eq!(data.field("attr.mid").unwrap().bits.unwrap().shift, 2);
    }

    #[test]
    fn enum_class_takes_its_underlying_width() {
        let data = layouts().layout("Outer::PacketData").unwrap();
        assert_eq!(data.field("mode").unwrap().stride, 2);
    }

    #[test]
    fn a_second_bitfield_unit_opens_when_the_first_is_full() {
        let mut l = Layouts::new();
        l.scan("struct S { uint8_t lead; uint16_t a : 12; uint16_t b : 4; uint16_t c : 1; };")
            .unwrap();
        let s = l.layout("S").unwrap();
        assert_eq!(s.offset_of("a").unwrap(), 2);
        assert_eq!(s.offset_of("b").unwrap(), 2);
        assert_eq!(s.field("b").unwrap().bits.unwrap().shift, 12);
        assert_eq!(s.offset_of("c").unwrap(), 4);
        assert_eq!(s.field("c").unwrap().bits.unwrap().shift, 0);
        assert_eq!(s.size, 6);
    }

    #[test]
    fn an_unnamed_zero_width_bitfield_closes_the_unit() {
        let mut l = Layouts::new();
        l.scan("struct S { uint16_t a : 4; uint16_t : 0; uint16_t b : 4; uint8_t tail; };")
            .unwrap();
        let s = l.layout("S").unwrap();
        assert_eq!(s.offset_of("a").unwrap(), 0);
        assert_eq!(s.offset_of("b").unwrap(), 2);
        assert_eq!(s.field("b").unwrap().bits.unwrap().shift, 0);
        assert_eq!(s.offset_of("tail").unwrap(), 4);
        assert!(s
            .fields
            .iter()
            .all(|f| !f.path.iter().any(String::is_empty)));
    }

    #[test]
    fn a_straddling_bitfield_is_an_error_not_a_guess() {
        let mut l = Layouts::new();
        l.scan("struct S { uint16_t a : 12; uint16_t b : 8; };")
            .unwrap();
        let err = l.layout("S").unwrap_err().to_string();
        assert!(err.contains("straddle"), "{err}");
        assert!(err.contains("::b"), "{err}");
    }

    #[test]
    fn an_unknown_member_type_names_the_member() {
        let mut l = Layouts::new();
        l.scan("struct S { uint32_t a; SomeThing b; };").unwrap();
        let err = format!("{:#}", l.layout("S").unwrap_err());
        assert!(err.contains("S::b"), "{err}");
        assert!(err.contains("SomeThing"), "{err}");
    }

    #[test]
    fn a_registered_scalar_stands_in_for_a_generated_enum() {
        let mut l = Layouts::new();
        l.register_scalar("xi::Job", "uint8_t").unwrap();
        l.scan("struct S { uint32_t a; xi::Job job; uint8_t lv; uint16_t z; };")
            .unwrap();
        let s = l.layout("S").unwrap();
        assert_eq!(s.offset_of("job").unwrap(), 4);
        assert_eq!(s.offset_of("lv").unwrap(), 5);
        assert_eq!(s.offset_of("z").unwrap(), 6);
        assert_eq!(s.size, 8);
    }

    #[test]
    fn a_missing_struct_is_an_error() {
        let err = Layouts::new().layout("Nope").unwrap_err().to_string();
        assert!(err.contains("Nope"), "{err}");
    }
}
