//! one key with its modifiers, as written in `config.toml` and as shown
//! in the footer

use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// a key and its modifiers, normalized so the same key always compares
/// equal however the terminal reports it: a letter carries its case and
/// no Shift, and Shift+Tab is always `BackTab`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Chord {
    pub(crate) code: KeyCode,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
}

impl Chord {
    pub(crate) const fn key(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    #[cfg(test)]
    pub(crate) const fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    #[cfg(test)]
    pub(crate) const fn shift(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: false,
            alt: false,
            shift: true,
        }
    }

    #[cfg(test)]
    pub(crate) const fn char(c: char) -> Self {
        Self::key(KeyCode::Char(c))
    }

    pub(crate) fn from_event(event: KeyEvent) -> Self {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);

        match event.code {
            // a typed character already shows Shift in its case; with Ctrl,
            // terminals differ in reporting it, so it is folded to lower case
            KeyCode::Char(c) if ctrl => Self {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                ctrl,
                alt,
                shift: false,
            },
            KeyCode::Char(c) => Self {
                code: KeyCode::Char(c),
                ctrl,
                alt,
                shift: false,
            },
            KeyCode::BackTab => Self {
                code: KeyCode::BackTab,
                ctrl,
                alt,
                shift: false,
            },
            KeyCode::Tab if shift => Self {
                code: KeyCode::BackTab,
                ctrl,
                alt,
                shift: false,
            },
            code => Self {
                code,
                ctrl,
                alt,
                shift,
            },
        }
    }

    /// reads one key, such as `ctrl+r`, `shift+up`, `G`, `?` or `f1`
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let bad = || format!("{text:?} is not a key");

        // `+` alone, or a final `+` after modifiers, is the plus key
        let (modifiers, key) = match text.rsplit_once('+') {
            Some(("", "")) => ("", "+"),
            Some((mods, "")) => match mods.strip_suffix('+') {
                Some(mods) => (mods, "+"),
                None => return Err(bad()),
            },
            Some((mods, key)) => (mods, key),
            None => ("", text),
        };
        let mut chord = Self::key(KeyCode::Null);

        for modifier in modifiers.split('+').filter(|m| !m.is_empty()) {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "alt" | "option" | "meta" => chord.alt = true,
                "shift" => chord.shift = true,
                _ => return Err(bad()),
            }
        }

        let lower = key.to_ascii_lowercase();

        chord.code = match lower.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" if chord.shift => {
                chord.shift = false;

                KeyCode::BackTab
            }
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "delete" | "del" => KeyCode::Delete,
            "backspace" => KeyCode::Backspace,
            "insert" => KeyCode::Insert,
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                KeyCode::F(f[1..].parse().map_err(|_| bad())?)
            }
            _ => {
                let mut chars = key.chars();

                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return Err(bad()),
                }
            }
        };

        // a character carries Shift in its case, as `from_event` reads it
        if let KeyCode::Char(c) = chord.code {
            if chord.ctrl {
                chord.code = KeyCode::Char(c.to_ascii_lowercase());
            } else if chord.shift {
                chord.code = KeyCode::Char(c.to_ascii_uppercase());
            }

            chord.shift = false;
        }

        Ok(chord)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }

        if self.alt {
            f.write_str("Alt+")?;
        }

        if self.shift {
            f.write_str("Shift+")?;
        }

        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) if self.ctrl || self.alt => write!(f, "{}", c.to_ascii_uppercase()),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::BackTab => f.write_str("Shift+Tab"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::PageUp => f.write_str("PgUp"),
            KeyCode::PageDown => f.write_str("PgDn"),
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Insert => f.write_str("Insert"),
            KeyCode::F(n) => write!(f, "F{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// keys pressed one after another, such as `g t`
pub(crate) type Sequence = Vec<Chord>;

/// reads a sequence: chords separated by spaces
pub(crate) fn parse_sequence(text: &str) -> Result<Sequence, String> {
    let chords: Result<Sequence, String> = text.split_whitespace().map(Chord::parse).collect();
    let chords = chords?;

    if chords.is_empty() {
        Err("an empty key".into())
    } else {
        Ok(chords)
    }
}

/// a sequence as shown to the user: `Ctrl+R`, or `g t`
pub(crate) fn label(sequence: &[Chord]) -> String {
    sequence
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn events_are_normalized() {
        let g = Chord::char('G');

        assert_eq!(
            Chord::from_event(event(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            g
        );
        assert_eq!(
            Chord::from_event(event(KeyCode::Char('G'), KeyModifiers::NONE)),
            g
        );
        assert_eq!(
            Chord::from_event(event(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Chord::key(KeyCode::BackTab)
        );
        assert_eq!(
            Chord::from_event(event(KeyCode::Tab, KeyModifiers::SHIFT)),
            Chord::key(KeyCode::BackTab)
        );
        assert_eq!(
            Chord::from_event(event(
                KeyCode::Char('R'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Chord::ctrl('r')
        );
        assert_eq!(
            Chord::from_event(event(KeyCode::Up, KeyModifiers::SHIFT)),
            Chord::shift(KeyCode::Up)
        );
    }

    #[test]
    fn keys_are_read_and_shown() {
        for (text, chord, shown) in [
            ("ctrl+r", Chord::ctrl('r'), "Ctrl+R"),
            ("Ctrl+R", Chord::ctrl('r'), "Ctrl+R"),
            ("shift+up", Chord::shift(KeyCode::Up), "Shift+↑"),
            ("G", Chord::char('G'), "G"),
            ("shift+g", Chord::char('G'), "G"),
            ("?", Chord::char('?'), "?"),
            ("space", Chord::char(' '), "Space"),
            ("shift+tab", Chord::key(KeyCode::BackTab), "Shift+Tab"),
            ("enter", Chord::key(KeyCode::Enter), "Enter"),
            ("f1", Chord::key(KeyCode::F(1)), "F1"),
            ("pgdn", Chord::key(KeyCode::PageDown), "PgDn"),
            (
                "ctrl++",
                Chord {
                    ctrl: true,
                    ..Chord::char('+')
                },
                "Ctrl++",
            ),
            ("+", Chord::char('+'), "+"),
            (
                "alt+x",
                Chord {
                    alt: true,
                    ..Chord::char('x')
                },
                "Alt+X",
            ),
        ] {
            assert_eq!(Chord::parse(text), Ok(chord), "{text}");
            assert_eq!(chord.to_string(), shown, "{text}");
        }

        for bad in ["", "hyper+x", "xyz", "ctrl+", "f99x"] {
            assert!(Chord::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sequences() {
        let gt = parse_sequence("g  t").unwrap();

        assert_eq!(gt, [Chord::char('g'), Chord::char('t')]);
        assert_eq!(label(&gt), "g t");
        assert!(parse_sequence("  ").is_err());
        assert!(parse_sequence("g nope").is_err());
    }
}
