use bevy::prelude::Resource;

use ffxi_dat::main_dll::CommandTable;

/// Retail reserves every line beginning with this, so nothing a player could
/// legally say is lost to it.
pub const RETAIL_PREFIX: char = '/';
/// Kuluu's own commands. Every name in the client's table carries exactly one
/// leading slash, so a doubled one can't collide with a command Square
/// Enix adds later. Windower reached the same place for the same reason;
/// Ashita put addons on a single slash and its addons have had to dodge retail
/// names ever since (its `clock` addon answers to `/time` because `/clock` was
/// taken).
pub const EXTENSION_PREFIX: &str = "\u{002F}\u{002F}";
/// Separates an owner from a command in the fully qualified extension form,
/// `//kuluu:lights`. Bare `//lights` resolves while nothing else claims it.
pub const OWNER_SEPARATOR: char = ':';
/// The owner first-party commands register under.
pub const FIRST_PARTY_OWNER: &str = "kuluu";
/// The extension surface's own help. Exempt from [`EnabledSets`] because it is
/// how a player finds out which sets exist and which are off.
pub const EXTENSION_HELP_NAMES: &[&str] = &["?", "help"];

/// Which surface a command belongs to, and therefore how it is typed and
/// whether it can be switched off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandSet {
    /// A command the retail client itself accepts. Typed under
    /// [`RETAIL_PREFIX`] and not switchable.
    Retail,
    /// Gameplay Kuluu offers that retail has no command for.
    Core,
    /// Debugging and tooling.
    Dev,
    /// Driving and inspecting an agent session.
    Agent,
}

impl CommandSet {
    pub const ALL: [CommandSet; 4] = [
        CommandSet::Retail,
        CommandSet::Core,
        CommandSet::Dev,
        CommandSet::Agent,
    ];

    /// The word `//set` names this set by.
    pub const fn word(self) -> &'static str {
        match self {
            CommandSet::Retail => "retail",
            CommandSet::Core => "core",
            CommandSet::Dev => "dev",
            CommandSet::Agent => "agent",
        }
    }

    pub const fn is_retail(self) -> bool {
        matches!(self, CommandSet::Retail)
    }
}

/// Which sets answer. Retail is not switchable; the rest are all on until the
/// runtime toggle and the per-server policy land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnabledSets {
    core: bool,
    dev: bool,
    agent: bool,
}

impl Default for EnabledSets {
    fn default() -> Self {
        Self {
            core: true,
            dev: true,
            agent: true,
        }
    }
}

impl EnabledSets {
    pub fn is_enabled(&self, set: CommandSet) -> bool {
        match set {
            CommandSet::Retail => true,
            CommandSet::Core => self.core,
            CommandSet::Dev => self.dev,
            CommandSet::Agent => self.agent,
        }
    }

    pub fn set_enabled(&mut self, set: CommandSet, on: bool) {
        match set {
            CommandSet::Retail => {}
            CommandSet::Core => self.core = on,
            CommandSet::Dev => self.dev = on,
            CommandSet::Agent => self.agent = on,
        }
    }
}

/// Which slash commands exist, per the client the player installed, plus which
/// of Kuluu's own sets answer.
#[derive(Resource, Debug, Clone, Default)]
pub struct CommandSurface {
    table: CommandTable,
    pub enabled: EnabledSets,
}

impl CommandSurface {
    pub fn new(table: CommandTable) -> Self {
        Self {
            table,
            enabled: EnabledSets::default(),
        }
    }

    /// Read from the install beside `dat_root`. An install whose table could
    /// not be located yields a surface that still answers long forms.
    pub fn from_dat_root(dat_root: Option<&ffxi_dat::DatRoot>) -> Self {
        let table = dat_root
            .and_then(|root| kuluu_render::scheduler_runtime::main_dll_for_root(root.root()))
            .map(|dll| dll.commands())
            .unwrap_or_default();
        if table.is_empty() {
            tracing::warn!(
                "no slash-command table in the install; retail commands answer to their long \
                 form only"
            );
        }
        Self::new(table)
    }

    /// False when the table could not be read, which is the only state in
    /// which a retail alias goes unrecognised.
    pub fn table_loaded(&self) -> bool {
        !self.table.is_empty()
    }

    /// The command id `word` names, or `None` when this client has no such
    /// command.
    pub fn id_for(&self, word: &str) -> Option<u16> {
        self.table.id_for(word)
    }

    /// The long form of the command `word` names. Falls back to `word` itself,
    /// which is what lets an unreadable table still resolve `/attack`.
    pub fn canonical<'a>(&'a self, word: &'a str) -> &'a str {
        self.table
            .id_for(word)
            .and_then(|id| self.table.canonical(id))
            .unwrap_or(word)
    }

    /// Every name this client accepts for one command, long form first.
    pub fn names_for(&self, id: u16) -> impl Iterator<Item = &str> {
        self.table.names_for(id)
    }

    /// Every name for the command `word` belongs to, long form first; just
    /// `word` when the table does not name it.
    pub fn alias_group(&self, word: &str) -> Vec<&str> {
        match self.table.id_for(word) {
            Some(id) => self.names_for(id).collect(),
            None => Vec::new(),
        }
    }
}

/// Which surface a typed line is aimed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Retail,
    Extension,
}

/// A command line split into its surface, command word and argument tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed<'a> {
    pub surface: Surface,
    /// Present only on the extension surface, from the `//owner:name` form.
    pub owner: Option<&'a str>,
    /// Lowercased, so `/Say` and `/say` are one command as in retail.
    pub word: String,
    pub rest: &'a str,
}

/// Split a submitted line. `None` when it is ordinary chat text rather than a
/// command.
pub fn classify(line: &str) -> Option<Typed<'_>> {
    let trimmed = line.trim_start();
    let body = match trimmed.strip_prefix(EXTENSION_PREFIX) {
        Some(body) => body,
        None => return classify_retail(trimmed),
    };
    let (head, rest) = split_word(body);
    let (owner, word) = match head.split_once(OWNER_SEPARATOR) {
        Some((owner, name)) => (Some(owner), name),
        None => (None, head),
    };
    (!word.is_empty()).then(|| Typed {
        surface: Surface::Extension,
        owner,
        word: word.to_ascii_lowercase(),
        rest,
    })
}

fn classify_retail(trimmed: &str) -> Option<Typed<'_>> {
    let body = trimmed.strip_prefix(RETAIL_PREFIX)?;
    let (word, rest) = split_word(body);
    (!word.is_empty()).then(|| Typed {
        surface: Surface::Retail,
        owner: None,
        word: word.to_ascii_lowercase(),
        rest,
    })
}

fn split_word(body: &str) -> (&str, &str) {
    match body.find(char::is_whitespace) {
        Some(at) => (&body[..at], body[at..].trim()),
        None => (body, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word_of(line: &str) -> Option<(Surface, Option<String>, String, String)> {
        classify(line).map(|t| {
            (
                t.surface,
                t.owner.map(str::to_owned),
                t.word,
                t.rest.to_owned(),
            )
        })
    }

    #[test]
    fn a_single_slash_is_the_retail_surface() {
        assert_eq!(
            word_of("/attack on"),
            Some((Surface::Retail, None, "attack".into(), "on".into()))
        );
        assert_eq!(
            word_of("/Say hello world"),
            Some((Surface::Retail, None, "say".into(), "hello world".into())),
            "retail accepts /Say"
        );
    }

    #[test]
    fn a_doubled_slash_is_the_extension_surface() {
        assert_eq!(
            word_of("\u{002F}\u{002F}lights 8"),
            Some((Surface::Extension, None, "lights".into(), "8".into()))
        );
        assert_eq!(
            word_of("\u{002F}\u{002F}kuluu:lights 8"),
            Some((
                Surface::Extension,
                Some(FIRST_PARTY_OWNER.into()),
                "lights".into(),
                "8".into()
            ))
        );
    }

    #[test]
    fn ordinary_chat_is_not_a_command() {
        for line in ["hello", "!unstuck", "@here", "", "   ", "3/4 done"] {
            assert_eq!(word_of(line), None, "{line:?}");
        }
    }

    #[test]
    fn a_bare_prefix_is_not_a_command() {
        for line in [
            "/",
            "\u{002F}\u{002F}",
            "/ say",
            "\u{002F}\u{002F} lights",
            "\u{002F}\u{002F}:",
        ] {
            assert_eq!(word_of(line), None, "{line:?}");
        }
    }

    #[test]
    fn the_argument_tail_survives_its_own_slashes() {
        let typed = classify("/tell Bob see \u{002F}\u{002F}lights").expect("a command");
        assert_eq!(typed.rest, "Bob see \u{002F}\u{002F}lights");
    }

    #[test]
    fn canonical_falls_back_to_the_typed_word_without_a_table() {
        let surface = CommandSurface::default();
        assert!(!surface.table_loaded());
        assert_eq!(surface.canonical("attack"), "attack");
        assert_eq!(surface.canonical("a"), "a", "aliases need the table");
        assert!(surface.alias_group("attack").is_empty());
    }

    #[test]
    fn retail_is_always_enabled_and_cannot_be_switched_off() {
        let mut sets = EnabledSets::default();
        sets.set_enabled(CommandSet::Retail, false);
        assert!(sets.is_enabled(CommandSet::Retail));
        sets.set_enabled(CommandSet::Dev, false);
        assert!(!sets.is_enabled(CommandSet::Dev));
        assert!(sets.is_enabled(CommandSet::Core));
    }

    #[test]
    fn every_set_has_a_distinct_word() {
        let mut words: Vec<&str> = CommandSet::ALL.iter().map(|s| s.word()).collect();
        words.sort_unstable();
        let total = words.len();
        words.dedup();
        assert_eq!(words.len(), total);
    }
}
