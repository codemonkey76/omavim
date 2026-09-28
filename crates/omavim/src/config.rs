//! Settings, from `~/.config/omavim/config.toml`: the leader key and the
//! keys after it. Anything left out keeps its default.
//!
//! ```toml
//! leader = "space"   # or one character, such as ","
//!
//! [keys]             # the key after the leader, for each action
//! save = "w"
//! bold = "b"
//! ```

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// What a leader key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Save,
    SaveAs,
    Open,
    NewWindow,
    Print,
    Fullscreen,
    Bold,
    Italic,
    Link,
    Help,
    Close,
}

impl Action {
    pub const ALL: [Action; 11] = [
        Action::Save,
        Action::SaveAs,
        Action::Open,
        Action::NewWindow,
        Action::Print,
        Action::Fullscreen,
        Action::Bold,
        Action::Italic,
        Action::Link,
        Action::Help,
        Action::Close,
    ];

    fn default_key(self) -> char {
        match self {
            Action::Save => 'w',
            Action::SaveAs => 'W',
            Action::Open => 'o',
            Action::NewWindow => 'n',
            Action::Print => 'p',
            Action::Fullscreen => 'f',
            Action::Bold => 'b',
            Action::Italic => 'i',
            Action::Link => 'l',
            Action::Help => '?',
            Action::Close => 'q',
        }
    }

    /// What it does, for the key reference.
    pub fn describe(self) -> &'static str {
        match self {
            Action::Save => "save (asks where, the first time)",
            Action::SaveAs => "save as",
            Action::Open => "open",
            Action::NewWindow => "new window",
            Action::Print => "print",
            Action::Fullscreen => "fullscreen, or back",
            Action::Bold => "bold: the word, or the selection",
            Action::Italic => "italic: the word, or the selection",
            Action::Link => "link: the word, or the selection",
            Action::Help => "this key reference",
            Action::Close => "close (asks, if there are unsaved changes)",
        }
    }

    /// The same thing as a command, if there's one.
    pub fn command(self) -> Option<&'static str> {
        Some(match self {
            Action::Save => ":w",
            Action::SaveAs => ":saveas",
            Action::Open => ":e",
            Action::NewWindow => ":new",
            Action::Print => ":hardcopy",
            Action::Fullscreen => ":fullscreen",
            Action::Help => ":help",
            Action::Close => ":q",
            Action::Bold | Action::Italic | Action::Link => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub leader: char,
    keys: HashMap<char, Action>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            leader: ' ',
            keys: Action::ALL.iter().map(|&a| (a.default_key(), a)).collect(),
        }
    }
}

/// The file as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    leader: Option<String>,
    #[serde(default)]
    keys: HashMap<Action, String>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("omavim/config.toml"))
    }

    /// The settings, and what was wrong with the file if anything was (the
    /// defaults stand in for it then).
    pub fn load() -> (Config, Option<String>) {
        let Some(path) = Self::path() else {
            return (Config::default(), None);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(_) => (Config::default(), None),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let file: File = toml::from_str(text).map_err(|e| e.message().to_string())?;
        let mut config = Config::default();
        if let Some(leader) = file.leader {
            config.leader = key(&leader).ok_or(format!("leader: {leader:?} isn't one key"))?;
        }
        for (action, k) in file.keys {
            let k = key(&k).ok_or(format!("keys.{action:?}: {k:?} isn't one key"))?;
            config.keys.retain(|_, a| *a != action);
            config.keys.insert(k, action);
        }
        Ok(config)
    }

    /// What a key after the leader does.
    pub fn action(&self, key: char) -> Option<Action> {
        self.keys.get(&key).copied()
    }

    /// The key for an action.
    pub fn key_for(&self, action: Action) -> Option<char> {
        self.keys
            .iter()
            .find(|(_, a)| **a == action)
            .map(|(k, _)| *k)
    }

    /// The leader as it's written: "Space", or the key.
    pub fn leader_name(&self) -> String {
        match self.leader {
            ' ' => "Space".into(),
            c => c.to_string(),
        }
    }
}

/// One key: a character, or "space".
fn key(s: &str) -> Option<char> {
    if s.eq_ignore_ascii_case("space") {
        return Some(' ');
    }
    let mut chars = s.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_as_the_plan_has_them() {
        let c = Config::default();
        assert_eq!(c.leader, ' ');
        assert_eq!(c.action('w'), Some(Action::Save));
        assert_eq!(c.action('W'), Some(Action::SaveAs));
        assert_eq!(c.action('?'), Some(Action::Help));
        assert_eq!(c.action('x'), None);
    }

    #[test]
    fn the_file_changes_the_leader_and_keys() {
        let c = Config::parse("leader = \",\"\n[keys]\nsave = \"s\"\nbold = \"B\"\n").unwrap();
        assert_eq!(c.leader, ',');
        assert_eq!(c.action('s'), Some(Action::Save));
        assert_eq!(c.action('w'), None, "save moved off w");
        assert_eq!(c.action('B'), Some(Action::Bold));
        assert_eq!(c.action('o'), Some(Action::Open), "the rest stay");
        assert_eq!(Config::parse("leader = \"space\"").unwrap().leader, ' ');
    }

    #[test]
    fn a_mistake_in_the_file_says_what() {
        assert!(
            Config::parse("leader = \"ab\"")
                .unwrap_err()
                .contains("leader")
        );
        assert!(Config::parse("[keys]\nnope = \"x\"").is_err());
        assert!(Config::parse("colour = 1").is_err());
    }
}
