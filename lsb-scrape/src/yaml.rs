//! A line-oriented walker for the block-style YAML LandSandBoat writes by
//! hand under vendor/server/data: two-space block indentation, `key: scalar`,
//! `key:` opening a nested block, `- item` sequences of scalars or maps,
//! one-line `[a, b]` flow sequences and `#` comments. Anything outside that
//! subset (flow maps, anchors, block scalars, quoted keys) is an error, never
//! a guess, so a format drift fails the build instead of thinning a table.

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Yaml {
    Scalar(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

impl Yaml {
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Scalar(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Yaml]> {
        match self {
            Yaml::Seq(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(String, Yaml)]> {
        match self {
            Yaml::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// The scalar under `key`, parsed; `None` when the key is absent, an
    /// error when it is present but not a parseable scalar.
    pub fn field<T: std::str::FromStr>(&self, key: &str) -> Result<Option<T>>
    where
        T::Err: std::fmt::Display,
    {
        let Some(node) = self.get(key) else {
            return Ok(None);
        };
        let text = node
            .as_str()
            .with_context(|| format!("`{key}` is not a scalar"))?;
        text.parse::<T>()
            .map(Some)
            .map_err(|e| anyhow::anyhow!("`{key}`: cannot parse {text:?}: {e}"))
    }

    /// The `[a, b, c]` sequence under `key` as parsed scalars.
    pub fn list<T: std::str::FromStr>(&self, key: &str) -> Result<Option<Vec<T>>>
    where
        T::Err: std::fmt::Display,
    {
        let Some(node) = self.get(key) else {
            return Ok(None);
        };
        let items = node
            .as_seq()
            .with_context(|| format!("`{key}` is not a sequence"))?;
        items
            .iter()
            .map(|item| {
                let text = item
                    .as_str()
                    .with_context(|| format!("`{key}` holds a non-scalar item"))?;
                text.parse::<T>()
                    .map_err(|e| anyhow::anyhow!("`{key}`: cannot parse {text:?}: {e}"))
            })
            .collect::<Result<Vec<T>>>()
            .map(Some)
    }
}

struct Line {
    indent: usize,
    text: String,
}

pub fn parse_yaml(src: &str) -> Result<Yaml> {
    let mut lines = Vec::new();
    for (number, raw) in src.lines().enumerate() {
        let text = strip_comment(raw).trim_end();
        if text.trim().is_empty() {
            continue;
        }
        let stripped = text.trim_start_matches(' ');
        if stripped.starts_with('\t') {
            bail!("line {}: tab indentation", number + 1);
        }
        lines.push(Line {
            indent: text.len() - stripped.len(),
            text: stripped.to_string(),
        });
    }
    if lines.is_empty() {
        return Ok(Yaml::Map(Vec::new()));
    }
    let mut cursor = 0;
    let root_indent = lines[0].indent;
    let root = parse_block(&mut lines, &mut cursor, root_indent)?;
    if cursor < lines.len() {
        bail!(
            "unexpected line at indent {}: {:?}",
            lines[cursor].indent,
            lines[cursor].text
        );
    }
    Ok(root)
}

/// Cuts a trailing `#` comment; a `#` inside a quoted scalar is text
/// (`display_name: 'Home Point #1'`).
fn strip_comment(raw: &str) -> &str {
    if raw.trim_start().starts_with('#') {
        return "";
    }
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (at, c) in raw.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '#' && previous.is_whitespace() => return &raw[..at],
            None => {}
        }
        previous = c;
    }
    raw
}

fn is_seq_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

fn parse_block(lines: &mut Vec<Line>, cursor: &mut usize, indent: usize) -> Result<Yaml> {
    if is_seq_item(&lines[*cursor].text) {
        parse_seq(lines, cursor, indent)
    } else {
        parse_map(lines, cursor, indent)
    }
}

/// Parse a block sequence at `indent`. A map item carries its first key on
/// the dash line and the rest two columns deeper; re-homing the dash line at
/// that depth makes the whole item one contiguous map.
fn parse_seq(lines: &mut Vec<Line>, cursor: &mut usize, indent: usize) -> Result<Yaml> {
    let mut items = Vec::new();
    while *cursor < lines.len()
        && lines[*cursor].indent == indent
        && is_seq_item(&lines[*cursor].text)
    {
        let rest = lines[*cursor].text[1..].trim_start().to_string();
        if rest.is_empty() {
            *cursor += 1;
            if *cursor >= lines.len() || lines[*cursor].indent <= indent {
                bail!("`-` with nothing under it");
            }
            let child_indent = lines[*cursor].indent;
            items.push(parse_block(lines, cursor, child_indent)?);
        } else if split_key(&rest).is_some() {
            lines[*cursor].indent = indent + 2;
            lines[*cursor].text = rest;
            items.push(parse_map(lines, cursor, indent + 2)?);
        } else {
            items.push(parse_value(&rest)?);
            *cursor += 1;
        }
    }
    Ok(Yaml::Seq(items))
}

fn parse_map(lines: &mut Vec<Line>, cursor: &mut usize, indent: usize) -> Result<Yaml> {
    let mut entries = Vec::new();
    while *cursor < lines.len() && lines[*cursor].indent == indent {
        let text = lines[*cursor].text.clone();
        if is_seq_item(&text) {
            break;
        }
        let Some((key, value)) = split_key(&text) else {
            bail!("expected `key:` or `key: value`, found {text:?}");
        };
        *cursor += 1;
        let node = if value.is_empty() {
            if *cursor < lines.len() && lines[*cursor].indent > indent {
                let child_indent = lines[*cursor].indent;
                parse_block(lines, cursor, child_indent)?
            } else if *cursor < lines.len()
                && lines[*cursor].indent == indent
                && is_seq_item(&lines[*cursor].text)
            {
                parse_seq(lines, cursor, indent)?
            } else {
                Yaml::Map(Vec::new())
            }
        } else {
            parse_value(value)?
        };
        entries.push((key, node));
    }
    Ok(Yaml::Map(entries))
}

fn split_key(text: &str) -> Option<(String, &str)> {
    let colon = text.find(':')?;
    let key = &text[..colon];
    let rest = &text[colon + 1..];
    if key.is_empty()
        || key.starts_with(['[', '{', '"', '\'', '&', '*'])
        || key.contains(char::is_whitespace)
        || !(rest.is_empty() || rest.starts_with(' '))
    {
        return None;
    }
    Some((key.to_string(), rest.trim()))
}

fn parse_value(value: &str) -> Result<Yaml> {
    if let Some(inner) = value.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .with_context(|| format!("unterminated flow sequence {value:?}"))?;
        let items = inner
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| Ok(Yaml::Scalar(unquote(item)?.into_owned())))
            .collect::<Result<Vec<Yaml>>>()?;
        return Ok(Yaml::Seq(items));
    }
    if value.starts_with(['{', '&', '*', '|', '>']) {
        bail!("unsupported YAML syntax in value {value:?}");
    }
    Ok(Yaml::Scalar(unquote(value)?.into_owned()))
}

/// Strips one layer of quotes; a doubled quote inside a single-quoted scalar
/// is the quote itself.
fn unquote(value: &str) -> Result<std::borrow::Cow<'_, str>> {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote) {
            let inner = inner
                .strip_suffix(quote)
                .with_context(|| format!("unterminated quoted scalar {value:?}"))?;
            return Ok(if quote == '\'' && inner.contains("''") {
                std::borrow::Cow::Owned(inner.replace("''", "'"))
            } else {
                std::borrow::Cow::Borrowed(inner)
            });
        }
    }
    Ok(std::borrow::Cow::Borrowed(value))
}

/// `0x` hex or decimal, the two spellings LSB's enum files use.
pub fn parse_u32_lit(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse::<u32>().ok(),
    }
}

/// One `<dir>/<key>/<file_name>` per subdirectory of vendor/server/data/zones
/// that has one, keyed by the directory name (the zone enum key), in key
/// order.
pub fn zone_files(
    dir: impl AsRef<std::path::Path>,
    file_name: &str,
) -> Result<Vec<(String, std::path::PathBuf)>> {
    let dir = dir.as_ref();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path().join(file_name);
        if !path.is_file() {
            continue;
        }
        let key = entry
            .file_name()
            .into_string()
            .map_err(|name| anyhow::anyhow!("non-UTF-8 zone directory {name:?}"))?;
        out.push((key, path));
    }
    if out.is_empty() {
        bail!("no <zone>/{file_name} under {}", dir.display());
    }
    out.sort();
    Ok(out)
}

/// The `<dir>/<key>/zone.yaml` files, see [`zone_files`].
pub fn zone_data_files(dir: &str) -> Result<Vec<(String, std::path::PathBuf)>> {
    zone_files(dir, "zone.yaml")
}

/// The `npcs:` map of a vendor/server/data/zones/<zone>/npcs.yaml file: each
/// NPC's entity id and its node (`display_name`, `render.look`, ...), in
/// file order.
pub fn parse_yaml_npcs(src: &str) -> Result<Vec<(u32, Yaml)>> {
    let root = parse_yaml(src)?;
    let npcs = root
        .get("npcs")
        .and_then(Yaml::as_map)
        .context("no `npcs:` map")?;
    npcs.iter()
        .map(|(id, node)| {
            let id = id
                .parse::<u32>()
                .with_context(|| format!("npc key `{id}` is not an entity id"))?;
            Ok((id, node.clone()))
        })
        .collect()
}

/// The `values:` map of a vendor/server/data/enums/*.yaml file, in file
/// order.
pub fn parse_yaml_enum_values(src: &str) -> Result<Vec<(String, u32)>> {
    let root = parse_yaml(src)?;
    let values = root
        .get("values")
        .and_then(Yaml::as_map)
        .context("no `values:` map")?;
    let mut out = Vec::with_capacity(values.len());
    for (name, node) in values {
        let text = node
            .as_str()
            .with_context(|| format!("enum value `{name}` is not a scalar"))?;
        let value = parse_u32_lit(text)
            .with_context(|| format!("enum value `{name}`: not an integer: {text:?}"))?;
        out.push((name.clone(), value));
    }
    if out.is_empty() {
        bail!("`values:` map is empty");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE_FILE: &str = "# yaml-language-server: $schema=../../schemas/zone.schema.json

type: [city]
misc: [mazurka, mogmenu]
music:
  day:          112

zonelines:

  z6w0:
    from:  [16.895, -19.333, 104.284]
    to:    valkurm_dunes
    at:    [60.989, -4.898, -151.001, 4.712389]
    scale: [1.000, 4.000]

transport:
  ship: 17793088
  runs:
    selbina_mhaura_boat:
      door: _6ww
      dock: [9.294, 0.000, -69.775, 0]
      voyage:
        - ship_bound_for_selbina
        - ship_bound_for_selbina_pirates
      every:     1152 # seconds
      phases:
        - state:     arriving
          animation: animation_18
          seconds:   41
          moves:
            - to:    dock
              after: 20
        - state:     docked
          seconds:   192
        - state:     departing
          hide:      38
          animation: animation_19
";

    #[test]
    fn walks_the_zone_file_shape() {
        let root = parse_yaml(ZONE_FILE).unwrap();
        assert_eq!(
            root.get("type").unwrap(),
            &Yaml::Seq(vec![Yaml::Scalar("city".into())])
        );
        assert_eq!(
            root.get("music").unwrap().field::<u16>("day").unwrap(),
            Some(112)
        );
        let line = root.get("zonelines").unwrap().get("z6w0").unwrap();
        assert_eq!(
            line.field::<String>("to").unwrap().as_deref(),
            Some("valkurm_dunes")
        );
        assert_eq!(
            line.list::<f32>("at").unwrap().unwrap(),
            vec![60.989, -4.898, -151.001, 4.712389]
        );
        let run = root
            .get("transport")
            .unwrap()
            .get("runs")
            .unwrap()
            .get("selbina_mhaura_boat")
            .unwrap();
        assert_eq!(run.field::<u32>("every").unwrap(), Some(1152));
        let voyage: Vec<&str> = run
            .get("voyage")
            .unwrap()
            .as_seq()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            voyage,
            ["ship_bound_for_selbina", "ship_bound_for_selbina_pirates"]
        );
        let phases = run.get("phases").unwrap().as_seq().unwrap();
        assert_eq!(phases.len(), 3);
        assert_eq!(
            phases[0].field::<String>("state").unwrap().as_deref(),
            Some("arriving")
        );
        assert_eq!(phases[0].field::<u32>("seconds").unwrap(), Some(41));
        let moves = phases[0].get("moves").unwrap().as_seq().unwrap();
        assert_eq!(moves[0].field::<u32>("after").unwrap(), Some(20));
        assert_eq!(phases[2].field::<u32>("seconds").unwrap(), None);
        assert_eq!(phases[2].field::<u32>("hide").unwrap(), Some(38));
    }

    #[test]
    fn empty_block_key_and_same_indent_sequence() {
        let root = parse_yaml("zonelines:\ntransport:\n  ship: 1\n").unwrap();
        assert_eq!(root.get("zonelines").unwrap(), &Yaml::Map(Vec::new()));
        assert_eq!(
            root.get("transport").unwrap().field::<u32>("ship").unwrap(),
            Some(1)
        );
        let root = parse_yaml("flags:\n- a\n- b\nid: 2\n").unwrap();
        assert_eq!(root.get("flags").unwrap().as_seq().unwrap().len(), 2);
        assert_eq!(root.field::<u32>("id").unwrap(), Some(2));
    }

    #[test]
    fn enum_values_take_hex_and_decimal() {
        let src = "meta:\n  flags: true\n\nvalues:\n  none:       0x00000000\n  dispelable: 0x00000001\n  ten:        10\n";
        assert_eq!(
            parse_yaml_enum_values(src).unwrap(),
            vec![
                ("none".to_string(), 0),
                ("dispelable".to_string(), 1),
                ("ten".to_string(), 10)
            ]
        );
    }

    #[test]
    fn npcs_map_yields_ids_and_nodes() {
        let src = "npcs:\n\n  17719636:\n    script:       Voidwatch_Purveyor\n    display_name: Voidwatch Purveyor\n    render:\n      look:\n        type:  equipped\n        race:  3\n  17719637:\n    script: blank\n    render:\n      look:\n        type:  standard\n        model: 50\n";
        let npcs = parse_yaml_npcs(src).unwrap();
        assert_eq!(npcs.len(), 2);
        assert_eq!(npcs[0].0, 17719636);
        assert_eq!(
            npcs[0]
                .1
                .field::<String>("display_name")
                .unwrap()
                .as_deref(),
            Some("Voidwatch Purveyor")
        );
        let look = npcs[1].1.get("render").unwrap().get("look").unwrap();
        assert_eq!(look.field::<u16>("model").unwrap(), Some(50));
        assert!(parse_yaml_npcs("npcs:\n  abc:\n    script: x\n").is_err());
    }

    #[test]
    fn rejects_what_it_does_not_understand() {
        assert!(parse_yaml("a: {b: 1}\n").is_err());
        assert!(parse_yaml("a: &anchor 1\n").is_err());
        assert!(parse_yaml("a: |\n  text\n").is_err());
        assert!(parse_yaml("a:\n\tb: 1\n").is_err());
        assert!(parse_yaml("a: [1, 2\n").is_err());
        assert!(parse_yaml("- a\nb: 1\n").is_err());
    }

    #[test]
    fn comments_and_quotes_are_stripped() {
        let root = parse_yaml("# leading\nevery: 3456 # trailing\nname: 'x y'\n").unwrap();
        assert_eq!(root.field::<u32>("every").unwrap(), Some(3456));
        assert_eq!(
            root.field::<String>("name").unwrap().as_deref(),
            Some("x y")
        );
    }
}
