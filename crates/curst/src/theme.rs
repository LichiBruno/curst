//! the colors of the interface, by role: the built-in themes and the
//! user's own, which change some roles of a built-in one

use std::path::Path;
use std::str::FromStr;

use curst_core::model::Method;
use curst_core::store::ThemeFile;
use ratatui::style::{Color, Modifier, Style};

/// a style for each role the interface draws with
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Theme {
    pub(crate) name: String,

    /// the focused pane's border, and what is picked out, such as a
    /// row that was cut
    pub(crate) accent: Style,

    /// the other panes' borders
    pub(crate) border: Style,

    /// labels and explanations around what matters
    pub(crate) muted: Style,

    /// keys in hints and help
    pub(crate) key: Style,

    /// notices and warnings
    pub(crate) warning: Style,
    pub(crate) error: Style,

    /// a 2xx status
    pub(crate) success: Style,

    /// a 3xx status
    pub(crate) info: Style,

    /// the selected row or tab
    pub(crate) selected: Style,
    pub(crate) get: Style,
    pub(crate) post: Style,
    pub(crate) put: Style,
    pub(crate) delete: Style,

    /// HEAD and OPTIONS
    pub(crate) other: Style,
}

/// the themes curst comes with
pub(crate) const BUILT_IN: [&str; 3] = ["dark", "light", "mono"];

const fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

const fn modifier(modifier: Modifier) -> Style {
    Style::new().add_modifier(modifier)
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// for a dark terminal: the colors curst always had
    pub(crate) fn dark() -> Self {
        Self {
            name: "dark".into(),
            accent: fg(Color::Cyan),
            border: fg(Color::DarkGray),
            muted: fg(Color::DarkGray),
            key: modifier(Modifier::BOLD),
            warning: fg(Color::Yellow),
            error: fg(Color::Red),
            success: fg(Color::Green),
            info: fg(Color::Cyan),
            selected: modifier(Modifier::REVERSED),
            get: fg(Color::Green),
            post: fg(Color::Yellow),
            put: fg(Color::Blue),
            delete: fg(Color::Red),
            other: fg(Color::Magenta),
        }
    }

    /// for a light terminal: darker colors that read on white
    pub(crate) fn light() -> Self {
        let green = fg(Color::Indexed(28));
        let orange = fg(Color::Indexed(130));

        Self {
            name: "light".into(),
            accent: fg(Color::Blue),
            border: fg(Color::Indexed(245)),
            muted: fg(Color::Indexed(242)),
            warning: orange,
            success: green,
            info: fg(Color::Blue),
            get: green,
            post: orange,
            ..Self::dark()
        }
    }

    /// no color at all, for terminals without it or people who would
    /// rather not: weight and underlines tell things apart
    pub(crate) fn mono() -> Self {
        let plain = Style::new();

        Self {
            name: "mono".into(),
            accent: modifier(Modifier::BOLD),
            border: plain,
            muted: modifier(Modifier::DIM),
            key: modifier(Modifier::BOLD),
            warning: modifier(Modifier::BOLD),
            error: modifier(Modifier::BOLD | Modifier::UNDERLINED),
            success: plain,
            info: plain,
            selected: modifier(Modifier::REVERSED),
            get: plain,
            post: plain,
            put: plain,
            delete: modifier(Modifier::BOLD),
            other: plain,
        }
    }

    pub(crate) fn built_in(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "mono" => Some(Self::mono()),
            _ => None,
        }
    }

    /// the style for a method's name
    pub(crate) fn method(&self, method: Method) -> Style {
        match method {
            Method::Get => self.get,
            Method::Post => self.post,
            Method::Put | Method::Patch => self.put,
            Method::Delete => self.delete,
            Method::Head | Method::Options => self.other,
        }
    }

    /// the role a name in a theme file stands for
    fn role(&mut self, name: &str) -> Option<&mut Style> {
        Some(match name {
            "accent" => &mut self.accent,
            "border" => &mut self.border,
            "muted" => &mut self.muted,
            "key" => &mut self.key,
            "warning" => &mut self.warning,
            "error" => &mut self.error,
            "success" => &mut self.success,
            "info" => &mut self.info,
            "selected" => &mut self.selected,
            "methods.get" => &mut self.get,
            "methods.post" => &mut self.post,
            "methods.put" => &mut self.put,
            "methods.delete" => &mut self.delete,
            "methods.other" => &mut self.other,
            _ => return None,
        })
    }

    /// a built-in theme with the roles in `file` changed
    pub(crate) fn from_file(name: &str, file: &ThemeFile) -> (Self, Vec<String>) {
        let mut warnings = file.warnings.clone();
        let mut theme = match file.get("base") {
            None => Self::dark(),
            Some(base) => Self::built_in(base).unwrap_or_else(|| {
                warnings.push(format!(
                    "base {base:?} is not a built-in theme; use dark, light or mono"
                ));

                Self::dark()
            }),
        };

        name.clone_into(&mut theme.name);

        for (role, text) in &file.entries {
            if role == "base" {
                continue;
            }

            let Some(slot) = theme.role(role) else {
                warnings.push(format!("{role:?} is not a role a theme can set"));

                continue;
            };

            match parse_style(text) {
                Ok(style) => *slot = style,
                Err(err) => warnings.push(format!("{role}: {err}")),
            }
        }

        (theme, warnings)
    }

    /// the theme called `name`: a built-in one, or a file in `themes_dir`;
    /// what went wrong is reported and the dark theme takes its place
    pub(crate) fn load(name: Option<&str>, themes_dir: Option<&Path>) -> (Self, Vec<String>) {
        let name = name.unwrap_or("dark");

        if let Some(theme) = Self::built_in(name) {
            return (theme, Vec::new());
        }

        let file = themes_dir.map(|dir| ThemeFile::load(&dir.join(format!("{name}.toml"))));

        match file {
            Some(Ok(Some(file))) => {
                let (theme, warnings) = Self::from_file(name, &file);
                let warnings = warnings
                    .into_iter()
                    .map(|w| format!("theme {name}: {w}"))
                    .collect();

                (theme, warnings)
            }
            Some(Err(err)) => (Self::dark(), vec![format!("theme {name}: {err}")]),
            Some(Ok(None)) | None => (
                Self::dark(),
                vec![format!(
                    "there is no theme {name:?}: use dark, light, mono or a file in the themes folder"
                )],
            ),
        }
    }
}

/// a style written as words: colors, `on` and a background color, and
/// `bold`, `dim`, `italic`, `underlined` or `reversed`; as in
/// `"bold black on cyan"`. colors are names, `#rrggbb` or a number from
/// 0 to 255. `"none"` is no style at all
pub(crate) fn parse_style(text: &str) -> Result<Style, String> {
    let mut style = Style::new();
    let mut background = false;

    for word in text.split_whitespace() {
        let lower = word.to_lowercase();
        let added = match lower.as_str() {
            "none" | "default" => Some(Modifier::empty()),
            "bold" => Some(Modifier::BOLD),
            "dim" => Some(Modifier::DIM),
            "italic" => Some(Modifier::ITALIC),
            "underlined" | "underline" => Some(Modifier::UNDERLINED),
            "reversed" | "reverse" => Some(Modifier::REVERSED),
            "on" => {
                background = true;

                continue;
            }
            _ => None,
        };

        if let Some(added) = added {
            style = style.add_modifier(added);

            continue;
        }

        let color = Color::from_str(&lower)
            .map_err(|_| format!("{word:?} is not a color, such as \"cyan\" or \"#5f87af\""))?;

        if background {
            style = style.bg(color);
            background = false;
        } else {
            style = style.fg(color);
        }
    }

    if background {
        return Err(format!("{text:?} has \"on\" with no color after it"));
    }

    Ok(style)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_read_from_words() {
        assert_eq!(parse_style("cyan"), Ok(fg(Color::Cyan)));
        assert_eq!(
            parse_style("bold Black on #5f87af"),
            Ok(Style::new()
                .fg(Color::Black)
                .bg(Color::Rgb(0x5f, 0x87, 0xaf))
                .add_modifier(Modifier::BOLD))
        );
        assert_eq!(parse_style("244"), Ok(fg(Color::Indexed(244))));
        assert_eq!(parse_style("none"), Ok(Style::new()));
        assert!(parse_style("chartreuse-ish").is_err());
        assert!(parse_style("red on").is_err());
    }

    #[test]
    fn a_file_changes_some_roles_of_its_base() {
        let file = ThemeFile {
            entries: vec![
                ("base".into(), "light".into()),
                ("accent".into(), "magenta".into()),
                ("methods.get".into(), "bold".into()),
                ("sparkle".into(), "red".into()),
                ("muted".into(), "not-a-color".into()),
            ],
            warnings: Vec::new(),
        };
        let (theme, warnings) = Theme::from_file("mine", &file);

        assert_eq!(theme.name, "mine");
        assert_eq!(theme.accent, fg(Color::Magenta));
        assert_eq!(theme.get, modifier(Modifier::BOLD));
        assert_eq!(theme.muted, Theme::light().muted);
        assert_eq!(theme.success, Theme::light().success);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn themes_are_found_by_name() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("mine.toml"), "base = \"mono\"\n").unwrap();

        assert_eq!(Theme::load(None, None), (Theme::dark(), Vec::new()));
        assert_eq!(Theme::load(Some("light"), None).0, Theme::light());
        assert_eq!(
            Theme::load(Some("mine"), Some(dir.path())).0.accent,
            Theme::mono().accent
        );

        let (theme, warnings) = Theme::load(Some("gone"), Some(dir.path()));

        assert_eq!(theme, Theme::dark());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn mono_has_no_color() {
        let theme = Theme::mono();
        let styles = [
            theme.accent,
            theme.border,
            theme.muted,
            theme.key,
            theme.warning,
            theme.error,
            theme.success,
            theme.info,
            theme.selected,
            theme.get,
            theme.post,
            theme.put,
            theme.delete,
            theme.other,
        ];

        assert!(styles.iter().all(|s| s.fg.is_none() && s.bg.is_none()));
    }
}
