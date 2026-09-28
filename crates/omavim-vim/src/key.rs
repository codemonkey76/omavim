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

/// Vim's codes for the special keys in a register (after its K_SPECIAL
/// byte, kept here as U+0080).
const SPECIAL: [(Key, &str); 10] = [
    (Key::Backspace, "kb"),
    (Key::Delete, "kD"),
    (Key::Up, "ku"),
    (Key::Down, "kd"),
    (Key::Left, "kl"),
    (Key::Right, "kr"),
    (Key::Home, "kh"),
    (Key::End, "@7"),
    (Key::PageUp, "kP"),
    (Key::PageDown, "kN"),
];

/// Keys as Vim keeps them in a register (a recorded macro): chars as they
/// are, Esc, Enter, Tab and Ctrl keys as their control chars, and the others
/// as Vim's special key codes (`"\u{80}kb"` for Backspace).
pub fn to_register(keys: &[Key]) -> String {
    let mut s = String::new();
    for &k in keys {
        match k {
            Key::Char(c) => s.push(c),
            Key::Esc => s.push('\x1b'),
            Key::Enter => s.push('\r'),
            Key::Tab => s.push('\t'),
            Key::Ctrl(c) => match c {
                'a'..='z' => s.push((c as u8 - b'a' + 1) as char),
                '@' | '[' | '\\' | ']' | '^' | '_' => s.push((c as u8 - b'@') as char),
                _ => s.push(c),
            },
            k => {
                if let Some((_, code)) = SPECIAL.iter().find(|(s, _)| *s == k) {
                    s.push('\u{80}');
                    s.push_str(code);
                }
            }
        }
    }
    s
}

/// Back: a register's text as the keys it types (`@a`).
pub fn from_register(text: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        keys.push(match c {
            '\x1b' => Key::Esc,
            '\r' => Key::Enter,
            '\t' => Key::Tab,
            '\n' => Key::Ctrl('j'),
            '\u{80}' => {
                let code: String = chars.by_ref().take(2).collect();
                match SPECIAL.iter().find(|(_, s)| *s == code) {
                    Some((k, _)) => *k,
                    None => continue,
                }
            }
            c if (c as u32) < 0x20 => {
                let b = c as u8;
                Key::Ctrl(if (1..=26).contains(&b) {
                    (b - 1 + b'a') as char
                } else {
                    (b + b'@') as char
                })
            }
            c => Key::Char(c),
        });
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::{Key, from_register, parse, to_register};

    #[test]
    fn keys_go_into_registers_as_vim_keeps_them() {
        let keys = parse("ix<BS>y<Esc>:s/a/b/<CR><C-a><C-[>");
        let text = to_register(&keys);
        assert_eq!(text, "ix\u{80}kby\x1b:s/a/b/\r\x01\x1b");
        let mut back = keys.clone();
        *back.last_mut().unwrap() = Key::Esc;
        assert_eq!(from_register(&text), back);
    }

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
