//! the footer's key hints, read from the keymap so a rebound key shows
//! its new keys and an unbound action is not offered at all

use super::{Keymap, Layer, State, layers};

/// a key, or keys, and what they do
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hint {
    pub(crate) key: String,
    pub(crate) label: &'static str,

    /// the action a click on the hint runs, for a hint of one action
    pub(crate) name: Option<String>,
}

/// collects hints, looking keys up in a chain of layers
pub(crate) struct Hints<'a> {
    keymap: &'a Keymap,
    layers: Vec<Layer>,
    list: Vec<Hint>,
}

fn arrow(key: &str) -> bool {
    matches!(key, "←" | "→" | "↑" | "↓")
}

impl<'a> Hints<'a> {
    /// hints for the keys that work where the focus is
    pub(crate) fn here(state: &'a State) -> Self {
        Self {
            keymap: &state.keymap,
            layers: layers(state).0,
            list: Vec::new(),
        }
    }

    /// hints for the keys that work whatever has the focus
    pub(crate) fn everywhere(state: &'a State) -> Self {
        Self {
            keymap: &state.keymap,
            layers: vec![Layer::Always, Layer::Global],
            list: Vec::new(),
        }
    }

    fn key(&self, name: &str) -> Option<String> {
        self.layers
            .iter()
            .find_map(|layer| self.keymap.key_for(*layer, name))
    }

    /// one action
    pub(crate) fn one(&mut self, name: &str, label: &'static str) -> &mut Self {
        if let Some(key) = self.key(name) {
            self.list.push(Hint {
                key,
                label,
                name: Some(name.to_owned()),
            });
        }

        self
    }

    /// two actions that go together, such as up and down: `↑↓` or `k j`
    pub(crate) fn pair(&mut self, first: &str, second: &str, label: &'static str) -> &mut Self {
        let key = match (self.key(first), self.key(second)) {
            (Some(a), Some(b)) if arrow(&a) && arrow(&b) => format!("{a}{b}"),
            (Some(a), Some(b)) => format!("{a} {b}"),
            (Some(key), None) | (None, Some(key)) => key,
            (None, None) => return self,
        };

        self.list.push(Hint {
            key,
            label,
            name: None,
        });

        self
    }

    /// the ends of a numbered run, such as `1-9`
    pub(crate) fn range(&mut self, first: &str, last: &str, label: &'static str) -> &mut Self {
        if let (Some(a), Some(b)) = (self.key(first), self.key(last)) {
            self.list.push(Hint {
                key: format!("{a}-{b}"),
                label,
                name: None,
            });
        }

        self
    }

    pub(crate) fn finish(&mut self) -> Vec<Hint> {
        std::mem::take(&mut self.list)
    }
}
