//! Format-level helpers shared by the workspace build scripts that scrape LSB
//! (vendor/server) SQL dumps, lua enums, and C++ headers into compile-time
//! Rust tables. Source-file-specific orchestration stays in each crate's
//! build.rs; only generic-signature walkers and table writers live here.

use std::fs;

use anyhow::{bail, Context, Result};

pub mod cpp_layout;
pub mod yaml;
pub use cpp_layout::{BitSpan, Field, Layouts, StructLayout};
pub use yaml::{
    parse_u32_lit, parse_yaml, parse_yaml_enum_values, parse_yaml_npcs, zone_data_files,
    zone_files, Yaml,
};

pub fn parse_int_lit(s: &str) -> Option<u16> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

pub fn parse_cpp_enum_class(src: &str, enum_name: &str) -> Result<Vec<(u32, String)>> {
    parse_cpp_enum_body(src, &format!("enum class {enum_name}"))
}

pub fn parse_cpp_plain_enum(src: &str, enum_name: &str) -> Result<Vec<(u32, String)>> {
    parse_cpp_enum_body(src, &format!("enum {enum_name}"))
}

fn parse_cpp_enum_body(src: &str, needle: &str) -> Result<Vec<(u32, String)>> {
    let header = src
        .find(needle)
        .with_context(|| format!("could not locate `{needle}` in source"))?;
    let body_start = src[header..]
        .find('{')
        .with_context(|| format!("no opening `{{` after `{needle}`"))?
        + header
        + 1;
    let body_end = src[body_start..]
        .find('}')
        .with_context(|| format!("no closing `}}` after `{needle}`"))?
        + body_start;

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in src[body_start..body_end].lines() {
        let line = line.trim();
        let Some(eq) = line.find('=') else { continue };
        let ident = line[..eq].trim();
        if ident.is_empty() || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let value = line[eq + 1..].split([',', '/']).next().unwrap_or("").trim();
        let Some(id) = parse_int_lit(value) else {
            continue;
        };
        if seen.insert(id) {
            out.push((id as u32, ident.to_string()));
        }
    }
    if out.is_empty() {
        bail!("parsed zero entries for `{needle}` — header format may have changed");
    }
    out.sort_by_key(|(id, _)| *id);
    Ok(out)
}

pub fn parse_packet_enum(src: &str, prefix: &str) -> Result<Vec<(u32, String)>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        let Some(eq) = rest.find('=') else { continue };
        let name = rest[..eq].trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let num_str = rest[eq + 1..].trim().trim_end_matches(',').trim();
        let Some(id) = parse_int_lit(num_str) else {
            continue;
        };
        if seen.insert(id) {
            out.push((id as u32, name.to_string()));
        }
    }
    if out.is_empty() {
        bail!("parsed zero `{prefix}*` packet ids — packet enum format may have changed");
    }
    Ok(out)
}

pub fn parse_xi_ident_table(src: &str, needle_prefix: &str) -> Result<Vec<(u32, String)>> {
    let needle = format!("{needle_prefix} =");
    let header = src
        .find(&needle)
        .with_context(|| format!("could not locate `{needle}` in source"))?;
    let body_start = src[header..]
        .find('{')
        .with_context(|| format!("no opening `{{` after `{needle}`"))?
        + header
        + 1;

    let body_end = src[body_start..]
        .find('}')
        .with_context(|| format!("no closing `}}` after `{needle}`"))?
        + body_start;
    let body = &src[body_start..body_end];

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in body.lines() {
        let line = line.trim();
        let Some(eq) = line.find('=') else { continue };
        let ident = line[..eq].trim();
        if ident.is_empty()
            || !ident
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            continue;
        }
        if !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let tail = line[eq + 1..].trim_start();
        let num_str: String = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        let Ok(id) = num_str.parse::<u32>() else {
            continue;
        };
        let pretty = prettify_snake_case(ident);
        if seen.insert(id) {
            out.push((id, pretty));
        }
    }
    if out.is_empty() {
        bail!("parsed zero entries for `{needle_prefix}` — source format may have changed");
    }
    Ok(out)
}

// `field` selects which quoted string in each `[id] = { 'ABBR', 'Full Name' }`
// row to keep: 1 = abbreviation, 3 = display name (split-by-`'` part index).
pub fn parse_lua_indexed_pair_table(
    src: &str,
    needle_prefix: &str,
    field: usize,
) -> Result<Vec<(u32, String)>> {
    let needle = format!("{needle_prefix} =");
    let header = src
        .find(&needle)
        .with_context(|| format!("could not locate `{needle}` in source"))?;
    let body_start = src[header..]
        .find('{')
        .with_context(|| format!("no opening `{{` after `{needle}`"))?
        + header
        + 1;
    let body = &src[body_start..];

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in body.lines() {
        let line = line.trim();

        let Some(open) = line.find('[') else { continue };
        let Some(close) = line[open + 1..].find(']') else {
            continue;
        };
        let id_str = line[open + 1..open + 1 + close].trim();
        let Ok(id) = id_str.parse::<u32>() else {
            continue;
        };

        let rest = &line[open + 1 + close..];
        let parts: Vec<&str> = rest.split('\'').collect();

        if parts.len() <= field {
            continue;
        }
        let display = parts[field].trim();
        if display.is_empty() {
            continue;
        }
        if seen.insert(id) {
            out.push((id, display.to_string()));
        }
    }
    if out.is_empty() {
        bail!("parsed zero entries for `{needle_prefix}` — source format may have changed");
    }
    Ok(out)
}

/// The value of the first `KEY = value,` line in a lua settings table, quotes
/// stripped and any trailing `--` comment dropped.
pub fn parse_lua_scalar_field(src: &str, key: &str) -> Result<String> {
    let line = src
        .lines()
        .map(str::trim)
        .find(|line| {
            line.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .with_context(|| format!("could not locate `{key} =` in source"))?;
    let rhs = line[key.len()..].trim_start()[1..].trim();
    let value = rhs.split(',').next().unwrap_or("");
    let value = value.split("--").next().unwrap_or("").trim();
    let value = value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .or_else(|| value.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
        .unwrap_or(value);
    if value.is_empty() {
        bail!("`{key}` has an empty value — settings format may have changed");
    }
    Ok(value.to_string())
}

/// Every `{ <int>, "<c-string>" }` pair inside the brace-initialised map whose
/// declaration line starts with `needle`, C escapes decoded, in source order.
/// std::map's initializer_list constructor keeps the first of two equal keys,
/// so a repeated key is dropped rather than overwritten.
pub fn parse_cpp_u32_str_map(src: &str, needle: &str) -> Result<Vec<(u32, String)>> {
    let header = line_starting_with(src, needle)
        .with_context(|| format!("could not locate a line starting with `{needle}` in source"))?;
    let body_start = src[header..]
        .find('{')
        .with_context(|| format!("no opening `{{` after `{needle}`"))?
        + header
        + 1;
    let mut cursor = CppCursor {
        chars: src[body_start..].chars().peekable(),
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    loop {
        cursor.skip_whitespace_and_commas();
        match cursor.chars.next() {
            Some('}') => break,
            Some('{') => {
                cursor.skip_whitespace();
                let key = cursor.read_int_lit()?;
                cursor.skip_whitespace();
                cursor.expect(',')?;
                cursor.skip_whitespace();
                cursor.expect('"')?;
                let value = cursor.read_c_string_body()?;
                cursor.skip_whitespace();
                cursor.expect('}')?;
                if seen.insert(key) {
                    out.push((key, value));
                }
            }
            Some(other) => bail!("unexpected `{other}` in `{needle}` body; expected `{{` or `}}`"),
            None => bail!("`{needle}` body has no closing `}}`"),
        }
    }
    if out.is_empty() {
        bail!("parsed zero entries for `{needle}` — source format may have changed");
    }
    Ok(out)
}

/// Byte offset of the first line whose leading whitespace is followed by
/// `needle`; a quoted copy of the declaration inside a comment does not match.
fn line_starting_with(src: &str, needle: &str) -> Option<usize> {
    let mut offset = 0;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with(needle) {
            return Some(offset + (line.len() - trimmed.len()));
        }
        offset += line.len();
    }
    None
}

struct CppCursor<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl CppCursor<'_> {
    fn skip_whitespace(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn skip_whitespace_and_commas(&mut self) {
        while self
            .chars
            .peek()
            .is_some_and(|c| c.is_whitespace() || *c == ',')
        {
            self.chars.next();
        }
    }

    fn expect(&mut self, want: char) -> Result<()> {
        match self.chars.next() {
            Some(c) if c == want => Ok(()),
            Some(c) => bail!("expected `{want}`, found `{c}`"),
            None => bail!("expected `{want}`, found end of source"),
        }
    }

    fn read_int_lit(&mut self) -> Result<u32> {
        let mut lit = String::new();
        while self.chars.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
            lit.push(self.chars.next().unwrap());
        }
        let parsed = match lit.strip_prefix("0x").or_else(|| lit.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => lit.parse::<u32>(),
        };
        parsed.with_context(|| format!("`{lit}` is not a u32 literal"))
    }

    /// The body of a C string literal after its opening quote, consuming the
    /// closing quote.
    fn read_c_string_body(&mut self) -> Result<String> {
        let mut out = String::new();
        loop {
            match self.chars.next() {
                None => bail!("unterminated string literal"),
                Some('"') => return Ok(out),
                Some('\\') => out.push(self.read_c_escape()?),
                Some(c) => out.push(c),
            }
        }
    }

    fn read_c_escape(&mut self) -> Result<char> {
        let Some(c) = self.chars.next() else {
            bail!("dangling `\\` at end of source");
        };
        Ok(match c {
            '"' => '"',
            '\'' => '\'',
            '\\' => '\\',
            '?' => '?',
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0'..='7' => {
                let mut code = c.to_digit(8).unwrap();
                for _ in 0..2 {
                    match self.chars.peek().and_then(|d| d.to_digit(8)) {
                        Some(d) => {
                            code = code * 8 + d;
                            self.chars.next();
                        }
                        None => break,
                    }
                }
                char::from_u32(code)
                    .with_context(|| format!("octal escape {code:#o} out of range"))?
            }
            'x' => {
                let mut code = 0u32;
                let mut digits = 0;
                while let Some(d) = self.chars.peek().and_then(|d| d.to_digit(16)) {
                    code = code * 16 + d;
                    digits += 1;
                    self.chars.next();
                }
                if digits == 0 {
                    bail!("`\\x` escape with no hex digits");
                }
                char::from_u32(code)
                    .with_context(|| format!("hex escape {code:#x} out of range"))?
            }
            other => bail!("unsupported C escape `\\{other}`"),
        })
    }
}

pub fn parse_sql_insert_rows(
    src: &str,
    table: &str,
    id_field: usize,
    name_field: usize,
) -> Result<Vec<(u32, String)>> {
    let needle = format!("INSERT INTO `{table}` VALUES ");
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle.as_str()) else {
            continue;
        };

        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let id_str = fields.get(id_field).map(|s| s.trim()).unwrap_or("");
            let name_raw = fields.get(name_field).map(|s| s.trim()).unwrap_or("");
            let Ok(id) = id_str.parse::<u32>() else {
                continue;
            };
            let Some(name) = strip_sql_string(name_raw) else {
                continue;
            };

            if name.is_empty()
                || name.chars().all(|c| c == '_')
                || !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            {
                continue;
            }
            out.push((id, prettify_snake_case(&name)));
        }
    }
    if out.is_empty() {
        bail!("parsed zero rows from `INSERT INTO {table}` — SQL format may have changed");
    }
    Ok(out)
}

pub fn parse_u16_pair_rows(src: &str, table: &str, value_field: usize) -> Result<Vec<(u16, u16)>> {
    let needle = format!("INSERT INTO `{table}` VALUES ");
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle.as_str()) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(Ok(id)) = fields.first().map(|s| s.trim().parse::<u16>()) else {
                continue;
            };
            let value = fields
                .get(value_field)
                .and_then(|s| s.trim().parse::<u16>().ok())
                .unwrap_or(0);
            out.push((id, value));
        }
    }
    if out.is_empty() {
        bail!("parsed zero rows from `INSERT INTO {table}` — SQL format may have changed");
    }
    Ok(out)
}

pub fn parse_u32_pair_rows(src: &str, table: &str, value_field: usize) -> Result<Vec<(u16, u32)>> {
    let needle = format!("INSERT INTO `{table}` VALUES ");
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(needle.as_str()) else {
            continue;
        };
        let mut cursor = rest;
        while let Some(open) = cursor.find('(') {
            cursor = &cursor[open + 1..];
            let Some((tuple, after)) = split_sql_tuple(cursor) else {
                break;
            };
            cursor = after;
            let fields = split_sql_fields(tuple);
            let Some(Ok(id)) = fields.first().map(|s| s.trim().parse::<u16>()) else {
                continue;
            };
            let value = fields
                .get(value_field)
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0);
            out.push((id, value));
        }
    }
    if out.is_empty() {
        bail!("parsed zero rows from `INSERT INTO {table}` — SQL format may have changed");
    }
    Ok(out)
}

pub fn split_sql_tuple(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_string = false;
            }
            i += 1;
        } else {
            if c == b'\'' {
                in_string = true;
                i += 1;
                continue;
            }
            if c == b')' {
                return Some((&s[..i], &s[i + 1..]));
            }
            i += 1;
        }
    }
    None
}

pub fn split_sql_fields(tuple: &str) -> Vec<&str> {
    let bytes = tuple.as_bytes();
    let mut fields = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_string = false;
            }
            i += 1;
        } else if c == b'\'' {
            in_string = true;
            i += 1;
        } else if c == b',' {
            fields.push(&tuple[start..i]);
            start = i + 1;
            i += 1;
        } else {
            i += 1;
        }
    }
    fields.push(&tuple[start..]);
    fields
}

pub fn strip_sql_string(field: &str) -> Option<String> {
    let f = field.trim();
    let stripped = f.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))?;
    Some(stripped.replace("''", "'"))
}

pub fn prettify_snake_case(s: &str) -> String {
    const ROMAN: &[&str] = &[
        "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii", "xiv",
        "xv",
    ];
    const CONNECTORS: &[&str] = &["of", "the", "a", "an", "in", "on", "and", "to"];
    let words: Vec<&str> = s.split('_').filter(|w| !w.is_empty()).collect();
    let mut out = String::with_capacity(s.len() + words.len());
    for (idx, word) in words.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let lower = word.to_ascii_lowercase();
        if ROMAN.iter().any(|r| *r == lower) {
            out.push_str(&lower.to_ascii_uppercase());
        } else if idx > 0 && CONNECTORS.iter().any(|c| *c == lower) {
            out.push_str(&lower);
        } else {
            let mut chars = lower.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out
}

pub fn rust_string_literal(s: &str) -> String {
    if !s.contains('"') && !s.contains('\\') {
        format!("\"{s}\"")
    } else {
        let mut esc = String::with_capacity(s.len() + 2);
        esc.push('"');
        for c in s.chars() {
            match c {
                '"' => esc.push_str("\\\""),
                '\\' => esc.push_str("\\\\"),
                _ => esc.push(c),
            }
        }
        esc.push('"');
        esc
    }
}

/// `CARGO_PKG_NAME` at build-script runtime is the package whose build.rs is
/// running, so generated headers and progress lines name the actual scraper.
fn scraping_package() -> String {
    std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "unknown".into())
}

fn generated_header(source_path: &str) -> String {
    format!(
        "// AUTO-GENERATED by {}/build.rs from {source_path}.\n",
        scraping_package()
    )
}

/// The floor for a scrape whose source yields `pinned_count` rows in the vendor
/// tree we pin today: half of it, so LSB adding or retiring rows never trips a
/// floor while a format drift the walker silently absorbs does. Passing the
/// observed count keeps the floor auditable without a build (kuluu-m4yk).
///
/// A table small enough that half of it is a single row needs its full count
/// spelled out instead -- half of two rows still passes when the walker matched
/// only one.
pub const fn scrape_floor(pinned_count: usize) -> usize {
    pinned_count / 2
}

/// Prints a scrape's row count on plain build-script stdout, failing the build
/// when it lands under `floor`.
///
/// A vendor format drift is otherwise absorbed silently -- the walker matches a
/// subset of the rows, or none, and the thin table reaches the game as missing
/// names instead of as a build error. `floor` is the scrape's smallest
/// plausible successful count, the row-count analogue of the
/// MIN/MAX_PLAUSIBLE_YALMS band in ffxi-proto/build.rs. `label` is the whole
/// noun phrase: `check_scrape_count("spell validTarget entries", ..)`.
pub fn check_scrape_count(
    label: &str,
    source_path: &str,
    count: usize,
    floor: usize,
) -> Result<()> {
    if floor == 0 {
        bail!("scrape floor for {label} is 0, which cannot detect drift -- give it a real floor");
    }
    if count < floor {
        bail!(
            "scraped {count} {label} from {source_path}, expected at least {floor} -- \
             the source format drifted and the walker matched only part of it"
        );
    }
    println!("{}: scraped {count} {label}", scraping_package());
    Ok(())
}

pub fn write_u16_table(
    out_path: &std::path::Path,
    const_name: &str,
    source_path: &str,
    entries: &[(u32, String)],
) -> Result<()> {
    let mut entries: Vec<(u32, String)> = entries.to_vec();
    entries.sort_by_key(|(id, _)| *id);
    entries.dedup_by_key(|(id, _)| *id);

    for (id, name) in &entries {
        if *id > u16::MAX as u32 {
            bail!("entry id {id} ({name:?}) overflows u16 — table needs widening");
        }
    }
    let mut out = String::new();
    out.push_str(&generated_header(source_path));
    out.push_str("// Do not edit by hand.\n");
    out.push_str(&format!("pub const {const_name}: &[(u16, &str)] = &[\n"));
    for (id, text) in &entries {
        out.push_str(&format!("    ({id}, {}),\n", rust_string_literal(text)));
    }
    out.push_str("];\n");
    fs::write(out_path, &out)?;
    Ok(())
}

pub fn write_u16_u8_table(
    out_path: &std::path::Path,
    const_name: &str,
    source_path: &str,
    entries: &[(u16, u8)],
) -> Result<()> {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|(id, _)| *id);
    entries.dedup_by_key(|(id, _)| *id);

    let mut out = String::new();
    out.push_str(&generated_header(source_path));
    out.push_str("// Do not edit by hand.\n");
    out.push_str(&format!("pub const {const_name}: &[(u16, u8)] = &[\n"));
    for (id, skill) in &entries {
        out.push_str(&format!("    ({id}, {skill}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_path, &out)?;
    Ok(())
}

pub fn write_u16_u16_table(
    out_path: &std::path::Path,
    const_name: &str,
    source_path: &str,
    entries: &[(u16, u16)],
) -> Result<()> {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|(id, _)| *id);
    entries.dedup_by_key(|(id, _)| *id);

    let mut out = String::new();
    out.push_str(&generated_header(source_path));
    out.push_str("// Do not edit by hand.\n");
    out.push_str(&format!("pub const {const_name}: &[(u16, u16)] = &[\n"));
    for (id, value) in &entries {
        out.push_str(&format!("    ({id}, {value}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_path, &out)?;
    Ok(())
}

pub fn write_u16_u32_table(
    out_path: &std::path::Path,
    const_name: &str,
    source_path: &str,
    entries: &[(u16, u32)],
) -> Result<()> {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|(id, _)| *id);
    entries.dedup_by_key(|(id, _)| *id);

    let mut out = String::new();
    out.push_str(&generated_header(source_path));
    out.push_str("// Do not edit by hand.\n");
    out.push_str(&format!("pub const {const_name}: &[(u16, u32)] = &[\n"));
    for (id, value) in &entries {
        out.push_str(&format!("    ({id}, {value:#x}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_path, &out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettify_handles_roman_numerals_and_connectors() {
        assert_eq!(prettify_snake_case("cure"), "Cure");
        assert_eq!(prettify_snake_case("cure_iv"), "Cure IV");
        assert_eq!(prettify_snake_case("cure_vi"), "Cure VI");
        assert_eq!(
            prettify_snake_case("pile_of_chocobo_bedding"),
            "Pile of Chocobo Bedding"
        );
        assert_eq!(prettify_snake_case("BLAZE_SPIKES"), "Blaze Spikes");
        assert_eq!(prettify_snake_case("mighty_strikes"), "Mighty Strikes");
    }

    #[test]
    fn scrape_floor_is_half_the_pinned_count() {
        assert_eq!(scrape_floor(243), 121);
        assert_eq!(scrape_floor(8), 4);
        assert_eq!(scrape_floor(2), 1);
    }

    #[test]
    fn scrape_count_floor_rejects_a_thinned_out_scrape() {
        assert!(check_scrape_count("spell entries", "spell_list.sql", 890, 400).is_ok());
        assert!(check_scrape_count("spell entries", "spell_list.sql", 400, 400).is_ok());

        let err = check_scrape_count("spell entries", "spell_list.sql", 399, 400)
            .unwrap_err()
            .to_string();
        assert!(err.contains("scraped 399 spell entries"), "{err}");
        assert!(err.contains("spell_list.sql"), "{err}");
        assert!(err.contains("at least 400"), "{err}");

        let err = check_scrape_count("spell entries", "spell_list.sql", 0, 400)
            .unwrap_err()
            .to_string();
        assert!(err.contains("scraped 0 spell entries"), "{err}");

        let err = check_scrape_count("spell entries", "spell_list.sql", 0, 0)
            .unwrap_err()
            .to_string();
        assert!(err.contains("floor"), "{err}");
    }

    #[test]
    fn lua_scalar_field_takes_the_first_definition_and_strips_quotes() {
        let src = "xi.settings.login =\n{\n    -- only exact CLIENT_VER allowed\n    CLIENT_VER = '30260203_0',\n    VER_LOCK = 2, -- default\n    CLIENT_VER = 'shadowed',\n    NAME = \"dq\"\n}\n";
        assert_eq!(
            parse_lua_scalar_field(src, "CLIENT_VER").unwrap(),
            "30260203_0"
        );
        assert_eq!(parse_lua_scalar_field(src, "VER_LOCK").unwrap(), "2");
        assert_eq!(parse_lua_scalar_field(src, "NAME").unwrap(), "dq");
        assert!(parse_lua_scalar_field(src, "VER").is_err());
        assert!(parse_lua_scalar_field(src, "MAINT_MODE").is_err());
        assert!(parse_lua_scalar_field("EMPTY = '',\n", "EMPTY").is_err());
        assert!(parse_lua_scalar_field("EMPTY = ,\n", "EMPTY").is_err());
    }

    #[test]
    fn cpp_u32_str_map_decodes_c_escapes_and_keeps_the_first_duplicate() {
        let src = "/* f.write(\"const std::map<unsigned int, const char*> values =\\n{\\n\") */\nconst std::map<unsigned int, const char*> values =\n{\n    { 66050, \"Greetings\" },\n    { 0x01010202, \"Nice to meet you.\" },\n    { 3489989127, \"\\\" A \\\" Egg\" },\n    { 7, \"tab\\there\\\\\\x41\\101\" },\n    { 66050, \"shadowed\" },\n};\n";
        let rows = parse_cpp_u32_str_map(src, "const std::map<unsigned int, const char*> values =")
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (66050, "Greetings".to_string()),
                (0x0101_0202, "Nice to meet you.".to_string()),
                (3_489_989_127, "\" A \" Egg".to_string()),
                (7, "tab\there\\AA".to_string()),
            ]
        );
    }

    #[test]
    fn cpp_u32_str_map_bails_on_malformed_rows() {
        assert!(parse_cpp_u32_str_map("values = { { 1 \"x\" } };", "values =").is_err());
        assert!(parse_cpp_u32_str_map("values = { { 1, x } };", "values =").is_err());
        assert!(parse_cpp_u32_str_map("values = { { 1, \"x\" }", "values =").is_err());
        assert!(parse_cpp_u32_str_map("values = { { 1, \"\\q\" } };", "values =").is_err());
        assert!(parse_cpp_u32_str_map("values = { };", "values =").is_err());
        assert!(parse_cpp_u32_str_map("values = { { 1, \"x\" } };", "other =").is_err());
    }

    #[test]
    fn sql_tuple_handles_escaped_quotes() {
        let (body, after) = split_sql_tuple("1,'it''s',2);extra").unwrap();
        assert_eq!(body, "1,'it''s',2");
        assert_eq!(after, ";extra");
        let fields = split_sql_fields(body);
        assert_eq!(fields.len(), 3);
        assert_eq!(strip_sql_string(fields[1]).unwrap(), "it's");
    }
}
