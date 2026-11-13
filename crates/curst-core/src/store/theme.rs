//! user themes: `<config dir>/themes/<name>.toml`, colors for the roles
//! of the interface, read as text for the interface to make sense of
//!
//! ```toml
//! # every role not set here comes from this built-in theme
//! base = "dark"
//! accent = "magenta"
//! muted = "#7f7f7f"
//! selected = "black on cyan"
//!
//! [methods]
//! get = "bold green"
//! ```

use std::path::Path;

use toml_edit::DocumentMut;

use super::StoreError;
use super::fs::read_regular;

/// a theme file: each role and the text given for it, `methods.get` for
/// the ones in `[methods]`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeFile {
    /// each role and its text, in file order
    pub entries: Vec<(String, String)>,

    /// what in the file could not be read, such as a number for a color
    pub warnings: Vec<String>,
}

fn collect(prefix: &str, table: &dyn toml_edit::TableLike, file: &mut ThemeFile) {
    for (key, item) in table.iter() {
        let name = format!("{prefix}{key}");

        if let Some(text) = item.as_str() {
            file.entries.push((name, text.to_owned()));
        } else if let (true, Some(inner)) = (prefix.is_empty(), item.as_table_like()) {
            collect(&format!("{name}."), inner, file);
        } else {
            file.warnings
                .push(format!("{name} must be text, such as \"cyan\""));
        }
    }
}

impl ThemeFile {
    /// the theme in `path`, `None` when there is no such file
    ///
    /// # Errors
    /// when the file cannot be read or is not TOML
    pub fn load(path: &Path) -> Result<Option<Self>, StoreError> {
        let Some(text) = read_regular(path)? else {
            return Ok(None);
        };
        let document = text
            .parse::<DocumentMut>()
            .map_err(|e| StoreError::parse(path, e.message()))?;
        let mut file = Self::default();

        collect("", document.as_table(), &mut file);

        Ok(Some(file))
    }

    /// the text for a role, the last one when it is given twice
    #[must_use]
    pub fn get(&self, role: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(name, _)| name == role)
            .map(|(_, text)| text.as_str())
    }
}

/// the names of the themes in `dir`, sorted; a missing folder has none
#[must_use]
pub fn theme_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .filter_map(|path| path.file_stem()?.to_str().map(ToOwned::to_owned))
        .collect();

    names.sort();

    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_methods_are_read_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("night.toml");

        assert_eq!(ThemeFile::load(&path).unwrap(), None);

        std::fs::write(
            &path,
            "base = \"dark\"\naccent = \"magenta\"\nmuted = 3\n\n[methods]\nget = \"bold green\"\n",
        )
        .unwrap();

        let file = ThemeFile::load(&path).unwrap().unwrap();

        assert_eq!(file.get("base"), Some("dark"));
        assert_eq!(file.get("accent"), Some("magenta"));
        assert_eq!(file.get("methods.get"), Some("bold green"));
        assert_eq!(file.get("muted"), None);
        assert_eq!(file.warnings.len(), 1, "{:?}", file.warnings);

        std::fs::write(dir.path().join("day.toml"), "").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "").unwrap();

        assert_eq!(theme_names(dir.path()), ["day", "night"]);
        assert_eq!(theme_names(&dir.path().join("none")), Vec::<String>::new());

        std::fs::write(&path, "[methods\n").unwrap();

        assert!(ThemeFile::load(&path).is_err());
    }
}
