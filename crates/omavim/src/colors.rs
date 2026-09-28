//! The colours: the current Omarchy theme's when there is one (and again
//! when it changes), else Omavim's own dark or light ones, following the
//! desktop. Highlight names map to them as in Omarchy's Helix theme, a
//! little quieter for prose: headings bold in the accent colour, emphasis
//! in italic and bold, the Markdown around them dimmed.

use crate::portal::Scheme;
use iced::Color;
use std::path::PathBuf;
use std::time::SystemTime;

/// How to draw a highlight: a colour (or the text's), and the font's style.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Style {
    pub color: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Colors {
    pub dark: bool,
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub selection: Color,
    pub muted: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    /// Where they came from and when it last changed, to notice a new theme.
    pub source: Option<(PathBuf, SystemTime)>,
}

fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_matches('"').trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some(Color::from_rgb8(v(0)?, v(2)?, v(4)?))
}

impl Colors {
    /// The current Omarchy theme's colours file.
    pub fn omarchy_file() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        let p = PathBuf::from(home).join(".local/state/omarchy/current/theme/colors.toml");
        p.exists().then_some(p)
    }

    /// When the Omarchy theme last changed (None without one).
    pub fn omarchy_stamp() -> Option<(PathBuf, SystemTime)> {
        let p = Self::omarchy_file()?;
        let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
        Some((p, t))
    }

    /// The colours to use: Omarchy's, else Omavim's for the desktop's scheme.
    pub fn current(scheme: Scheme) -> Colors {
        Self::omarchy().unwrap_or_else(|| Self::builtin(scheme))
    }

    /// The current Omarchy theme's colours.
    pub fn omarchy() -> Option<Colors> {
        let source = Self::omarchy_stamp()?;
        let text = std::fs::read_to_string(&source.0).ok()?;
        Self::from_toml(&text, Some(source))
    }

    /// An Omarchy theme's `colors.toml`.
    pub fn from_toml(text: &str, source: Option<(PathBuf, SystemTime)>) -> Option<Colors> {
        let mut keys = std::collections::HashMap::new();
        for line in text.lines() {
            // (`#` starts a comment only at a line's start: colours have one.)
            if line.trim_start().starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                keys.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
            }
        }
        // Semantic names, or the terminal palette's (older themes).
        let get = |names: &[&str]| names.iter().find_map(|n| keys.get(*n).and_then(|v| hex(v)));
        let background = get(&["background", "color0"])?;
        let foreground = get(&["foreground", "color7"])?;
        let dark = keys
            .get("mode")
            .map(|m| m != "light")
            .unwrap_or_else(|| luminance(background) < 0.5);
        let mix = |a: Color, b: Color, t: f32| Color {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: 1.0,
        };
        Some(Colors {
            dark,
            background,
            foreground,
            accent: get(&["accent", "blue", "color4"]).unwrap_or(foreground),
            selection: get(&["selection", "selection_background"])
                .unwrap_or(mix(background, foreground, 0.2)),
            muted: get(&["muted", "dark_foreground", "color8"])
                .unwrap_or(mix(background, foreground, 0.5)),
            red: get(&["red", "color1"]).unwrap_or(foreground),
            green: get(&["green", "color2"]).unwrap_or(foreground),
            yellow: get(&["yellow", "color3"]).unwrap_or(foreground),
            blue: get(&["blue", "color4"]).unwrap_or(foreground),
            magenta: get(&["magenta", "color5"]).unwrap_or(foreground),
            cyan: get(&["cyan", "color6"]).unwrap_or(foreground),
            source,
        })
    }

    /// Omavim's own: Tokyo Night, and Tokyo Night Day.
    pub fn builtin(scheme: Scheme) -> Colors {
        let c = |s| hex(s).unwrap();
        match scheme {
            Scheme::Dark => Colors {
                dark: true,
                background: c("1a1b26"),
                foreground: c("a9b1d6"),
                accent: c("7aa2f7"),
                selection: c("292e42"),
                muted: c("565f89"),
                red: c("f7768e"),
                green: c("9ece6a"),
                yellow: c("e0af68"),
                blue: c("7aa2f7"),
                magenta: c("ad8ee6"),
                cyan: c("449dab"),
                source: None,
            },
            Scheme::Light => Colors {
                dark: false,
                background: c("e1e2e7"),
                foreground: c("3760bf"),
                accent: c("2e7de9"),
                selection: c("b7c1e3"),
                muted: c("848cb5"),
                red: c("f52a65"),
                green: c("587539"),
                yellow: c("8c6c3e"),
                blue: c("2e7de9"),
                magenta: c("9854f1"),
                cyan: c("007197"),
                source: None,
            },
        }
    }

    /// The app's palette.
    pub fn palette(&self) -> iced::theme::Palette {
        iced::theme::Palette {
            background: self.background,
            text: self.foreground,
            primary: self.accent,
            success: self.green,
            warning: self.yellow,
            danger: self.red,
        }
    }

    /// How to draw a highlight name: the most specific one there's a style
    /// for (`function.method`, then `function`).
    pub fn style(&self, name: &str) -> Style {
        let mut name = name;
        loop {
            if let Some(s) = self.exact(name) {
                return s;
            }
            match name.rfind('.') {
                Some(i) => name = &name[..i],
                None => return Style::default(),
            }
        }
    }

    fn exact(&self, name: &str) -> Option<Style> {
        let fg = |c: Color| Style {
            color: Some(c),
            ..Style::default()
        };
        let italic = |c: Color| Style {
            color: Some(c),
            italic: true,
            ..Style::default()
        };
        Some(match name {
            "keyword" => fg(self.magenta),
            "keyword.control" => italic(self.magenta),
            "function" | "constructor" | "tag" | "label" | "string.special" => fg(self.blue),
            "function.macro"
            | "type.builtin"
            | "special"
            | "string.regexp"
            | "escape"
            | "string.escape"
            | "constant.character.escape" => fg(self.magenta),
            "type" | "constant" | "number" | "boolean" | "attribute" => fg(self.yellow),
            "type.enum.variant" | "constant.character" | "operator" | "markup.list" => {
                fg(self.cyan)
            }
            "string" | "markup.raw" => fg(self.green),
            "comment" | "markup.quote" => italic(self.muted),
            "variable.parameter" => italic(self.magenta),
            "variable.builtin" => fg(self.red),
            "variable.other.member" | "property" => fg(self.blue),
            "namespace" | "module" => italic(self.yellow),
            // The Markdown around the words, and other punctuation: dimmed.
            "punctuation" | "markup.strikethrough" => fg(self.muted),
            "markup.heading" => Style {
                color: Some(self.accent),
                bold: true,
                ..Style::default()
            },
            "markup.bold" => Style {
                bold: true,
                ..Style::default()
            },
            "markup.italic" => Style {
                italic: true,
                ..Style::default()
            },
            "markup.link.url" => Style {
                color: Some(self.blue),
                underline: true,
                ..Style::default()
            },
            "markup.link.text" | "markup.link.label" => fg(self.magenta),
            _ => return None,
        })
    }
}

fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_omarchy_theme() {
        let text = "mode = \"dark\"\n# a comment\naccent = \"#7aa2f7\"\nbackground = \"#1a1b26\"\nforeground = \"#a9b1d6\"\nmagenta = \"#ad8ee6\"\n";
        let c = Colors::from_toml(text, None).unwrap();
        assert!(c.dark);
        assert_eq!(c.background, Color::from_rgb8(0x1a, 0x1b, 0x26));
        assert_eq!(
            c.style("keyword").color,
            Some(Color::from_rgb8(0xad, 0x8e, 0xe6))
        );
    }

    #[test]
    fn reads_an_older_theme_by_its_terminal_colours() {
        let text = "color0 = \"#ffffff\"\ncolor7 = \"#000000\"\ncolor2 = \"#00ff00\"\n";
        let c = Colors::from_toml(text, None).unwrap();
        assert!(!c.dark);
        assert_eq!(c.style("string").color, Some(Color::from_rgb8(0, 0xff, 0)));
    }

    #[test]
    fn a_name_falls_back_to_a_less_specific_one() {
        let c = Colors::builtin(Scheme::Dark);
        assert_eq!(c.style("function.method.call"), c.style("function"));
        assert!(c.style("markup.heading.1").bold);
        assert_eq!(c.style("no.such.name"), Style::default());
    }
}
