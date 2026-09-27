//! Keys, and Vim's notation for them ("d2w", "ciwX<Esc>", "<C-r>").

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Ctrl with a letter or symbol, lower case: `Ctrl('r')`.
    Ctrl(char),
}

/// Parse keys in Vim's notation. Unknown `<...>` names are taken literally.
pub fn parse(notation: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut rest = notation;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && let Some(key) = named(&rest[1..end])
        {
            keys.push(key);
            rest = &rest[end + 1..];
            continue;
        }
        keys.push(Key::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    keys
}

fn named(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "esc" => Key::Esc,
        "cr" | "enter" | "return" => Key::Enter,
        "bs" => Key::Backspace,
        "del" => Key::Delete,
        "tab" => Key::Tab,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "lt" => Key::Char('<'),
        "space" => Key::Char(' '),
        _ => {
            let c = lower.strip_prefix("c-")?;
            let mut chars = c.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Ctrl(ch)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{Key, parse};

    #[test]
    fn reads_vim_notation() {
        assert_eq!(
            parse("ciwX<Esc><C-r><lt>é"),
            vec![
                Key::Char('c'),
                Key::Char('i'),
                Key::Char('w'),
                Key::Char('X'),
                Key::Esc,
                Key::Ctrl('r'),
                Key::Char('<'),
                Key::Char('é')
            ]
        );
        assert_eq!(
            parse("<nope>"),
            "<nope>".chars().map(Key::Char).collect::<Vec<_>>()
        );
    }
}
