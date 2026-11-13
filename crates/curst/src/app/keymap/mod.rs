//! which key does what: named actions bound to keys in layers, read from a
//! preset and the user's `config.toml`
//!
//! a key is looked up in the layers that apply where the focus is, the most
//! specific first; in a text field, a key no layer binds is typed. keys can
//! be sequences such as `g t`: the first key waits for the next, and a
//! sequence that goes nowhere is dropped, or typed in a text field

mod chord;
mod help;
mod hints;
mod presets;

use ratatui::crossterm::event::{KeyEvent, KeyEventKind};

pub(crate) use chord::{Chord, Sequence, label, parse_sequence};
pub(crate) use help::{Command as PaletteCommand, help_lines};
#[cfg(test)]
pub(crate) use help::{Row as HelpRow, Section as HelpSection};
pub(crate) use hints::{Hint, Hints};

use super::{Action, BodyField, Focus, Popup, RequestTab, State};
use curst_core::store::Backend;

/// a group of bindings that apply together
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Layer {
    /// everywhere, pop-ups included
    Always,

    /// wherever no pop-up is open
    Global,

    /// the panes, when nothing is being typed
    Panes,

    /// the arrows of the request and response panes
    PaneArrows,
    Url,
    Method,
    Sidebar,
    Request,

    /// a one-line field of the request pane
    RequestTyping,

    /// the multi-line body
    BodyTyping,

    /// sideways scrolling of the response, tried before the other keys
    ResponsePan,
    Response,
    Prompt,
    Passphrase,
    Confirm,
    Trust,
    Backend,
    Tabs,
    Settings,
    Approve,
    Environments,
    Variables,
    VariablesTyping,
    Help,
    Palette,
    Welcome,
}

impl Layer {
    pub(crate) const ALL: [Self; 26] = [
        Self::Always,
        Self::Global,
        Self::Panes,
        Self::PaneArrows,
        Self::Url,
        Self::Method,
        Self::Sidebar,
        Self::Request,
        Self::RequestTyping,
        Self::BodyTyping,
        Self::ResponsePan,
        Self::Response,
        Self::Prompt,
        Self::Passphrase,
        Self::Confirm,
        Self::Trust,
        Self::Backend,
        Self::Tabs,
        Self::Settings,
        Self::Approve,
        Self::Environments,
        Self::Variables,
        Self::VariablesTyping,
        Self::Help,
        Self::Palette,
        Self::Welcome,
    ];

    /// the name used in `config.toml`, as in `"sidebar.rename" = "r"`
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Global => "global",
            Self::Panes => "panes",
            Self::PaneArrows => "pane-arrows",
            Self::Url => "url",
            Self::Method => "method",
            Self::Sidebar => "sidebar",
            Self::Request => "request",
            Self::RequestTyping => "request-typing",
            Self::BodyTyping => "body-typing",
            Self::ResponsePan => "response-pan",
            Self::Response => "response",
            Self::Prompt => "prompt",
            Self::Passphrase => "passphrase",
            Self::Confirm => "confirm",
            Self::Trust => "trust",
            Self::Backend => "backend",
            Self::Tabs => "tabs",
            Self::Settings => "settings",
            Self::Approve => "approve",
            Self::Environments => "environments",
            Self::Variables => "variables",
            Self::VariablesTyping => "variables-typing",
            Self::Help => "help",
            Self::Palette => "palette",
            Self::Welcome => "welcome",
        }
    }

    /// where the layer's keys work, for the help
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Always => "every screen",
            Self::Global => "the main screen",
            Self::Panes => "the panes, when not typing",
            Self::PaneArrows => "the request and response panes",
            Self::Url => "the URL",
            Self::Method => "the method",
            Self::Sidebar => "the sidebar",
            Self::Request => "the request pane",
            Self::RequestTyping => "a request field being edited",
            Self::BodyTyping => "the body being edited",
            Self::ResponsePan => "the response, sideways",
            Self::Response => "the response pane",
            Self::Prompt => "a name prompt",
            Self::Passphrase => "the passphrase prompt",
            Self::Confirm => "a question",
            Self::Trust => "the trust question",
            Self::Backend => "the choice of secret store",
            Self::Tabs => "the tab list",
            Self::Settings => "the settings",
            Self::Approve => "the approval of a new host",
            Self::Environments => "the environments",
            Self::Variables => "the variables",
            Self::VariablesTyping => "a variable being edited",
            Self::Help => "this help",
            Self::Palette => "the command palette",
            Self::Welcome => "the welcome",
        }
    }

    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.name() == name)
    }
}

/// the action a name stands for, as used in presets and `config.toml`
pub(crate) fn action(name: &str) -> Option<Action> {
    let numbered = |prefix: &str| -> Option<usize> {
        let n: usize = name.strip_prefix(prefix)?.parse().ok()?;

        (1..=9).contains(&n).then(|| n - 1)
    };

    if name == "request.go-last" {
        return Some(Action::RequestAt(usize::MAX));
    }

    if let Some(index) = numbered("request.go-").filter(|i| *i < 8) {
        return Some(Action::RequestAt(index));
    }

    if let Some(index) = numbered("trust.moved-") {
        return Some(Action::Reattach(index));
    }

    Some(match name {
        "quit" => Action::Quit,
        "send" => Action::Send,
        "save" => Action::Save,
        "cancel" => Action::Cancel,
        "focus.next" => Action::FocusNext,
        "focus.previous" => Action::FocusPrevious,
        "tab.next" => Action::NextTab,
        "tab.previous" => Action::PreviousTab,
        "method.next" => Action::NextMethod,
        "method.previous" => Action::PreviousMethod,
        "up" => Action::Up,
        "down" => Action::Down,
        "page-up" => Action::PageUp,
        "page-down" => Action::PageDown,
        "top" => Action::Top,
        "bottom" => Action::Bottom,
        "left" => Action::Left,
        "right" => Action::Right,
        "edit" => Action::Edit,
        "content-type" => Action::EditContentType,
        "stop-edit" => Action::StopEdit,
        "next-cell" => Action::NextCell,
        "row.add" => Action::AddRow,
        "row.delete" => Action::DeleteRow,
        "row.toggle" => Action::ToggleRow,
        "kind" => Action::CycleKind,
        "save-body" => Action::OpenSave,
        "follow" => Action::Follow,
        "new-request" => Action::NewRequest,
        "new-folder" => Action::NewFolder,
        "new-collection" => Action::NewCollection,
        "rename" => Action::Rename,
        "delete" => Action::DeleteItem,
        "cut" => Action::Cut,
        "paste" => Action::Paste,
        "move-up" => Action::MoveUp,
        "move-down" => Action::MoveDown,
        "trust" => Action::Trust,
        "environments" => Action::Environments,
        "settings" => Action::Settings,
        "make-secret" => Action::MakeSecret,
        "edit-body" => Action::EditBody,
        "tabs" => Action::Tabs,
        "request.new" => Action::NewTab,
        "request.close" => Action::CloseTab,
        "request.next" => Action::NextRequest,
        "request.previous" => Action::PreviousRequest,
        "leave-collection" => Action::LeaveCollection,
        "confirm" => Action::ConfirmPopup,
        "close" => Action::ClosePopup,
        "env.new" => Action::NewEnvironment,
        "env.rename" => Action::RenameEnvironment,
        "env.delete" => Action::DeleteEnvironment,
        "env.variables" => Action::EditVariables,
        "migrate" => Action::Migrate,
        "help" => Action::Help,
        "palette" => Action::Palette,
        "keys" => Action::NextKeys,
        "theme" => Action::NextTheme,
        "backend.keychain" => Action::Choose(Backend::Keychain),
        "backend.file" => Action::Choose(Backend::EncryptedFile),
        _ => return None,
    })
}

/// one key, or sequence, bound to a named action in a layer
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) layer: Layer,
    pub(crate) keys: Sequence,
    pub(crate) name: String,
}

/// every binding in force
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::standard()
    }
}

/// what a run of keys means in some layers
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lookup {
    Exact(String),

    /// the start of a longer binding: wait for the next key
    Prefix,
    Nothing,
}

impl Keymap {
    fn from_table(table: &[presets::Row]) -> Self {
        let mut bindings = Vec::new();

        for (layer, name, keys) in table {
            for key in *keys {
                if let Ok(keys) = parse_sequence(key) {
                    bindings.push(Binding {
                        layer: *layer,
                        keys,
                        name: (*name).to_owned(),
                    });
                }
            }
        }

        Self { bindings }
    }

    /// the default keys
    pub(crate) fn standard() -> Self {
        Self::from_table(presets::STANDARD)
    }

    /// `standard` with the vim changes
    pub(crate) fn vim() -> Self {
        let mut keymap = Self::standard();

        for (layer, name, keys) in presets::VIM {
            let keys: Vec<Sequence> = keys.iter().filter_map(|k| parse_sequence(k).ok()).collect();

            keymap.replace(*layer, name, keys);
        }

        keymap
    }

    /// the keymap for a preset and the user's own bindings, with a warning
    /// for each binding that could not be used and each conflict
    pub(crate) fn from_settings(
        preset: Option<&str>,
        bindings: &[(String, Vec<String>)],
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut keymap = match preset {
            None | Some("standard") => Self::standard(),
            Some("vim") => Self::vim(),
            Some(other) => {
                warnings.push(format!(
                    "keys.preset {other:?} is not a preset; use \"standard\" or \"vim\""
                ));

                Self::standard()
            }
        };

        for (target, keys) in bindings {
            let found = target
                .split_once('.')
                .and_then(|(layer, name)| Some((Layer::named(layer)?, name)))
                .filter(|(_, name)| action(name).is_some());
            let Some((layer, name)) = found else {
                warnings.push(format!(
                    "keys.bindings: {target:?} is not a layer and an action, such as \"global.send\""
                ));

                continue;
            };

            let mut sequences = Vec::new();

            for key in keys {
                match parse_sequence(key) {
                    Ok(sequence) => sequences.push(sequence),
                    Err(err) => warnings.push(format!("keys.bindings.{target}: {err}")),
                }
            }

            keymap.replace(layer, name, sequences);
        }

        warnings.extend(keymap.conflicts());

        (keymap, warnings)
    }

    /// binds `name` in `layer` to exactly `keys`, none unbinding it
    fn replace(&mut self, layer: Layer, name: &str, keys: Vec<Sequence>) {
        self.bindings
            .retain(|b| !(b.layer == layer && b.name == name));
        self.bindings.extend(keys.into_iter().map(|keys| Binding {
            layer,
            keys,
            name: name.to_owned(),
        }));
    }

    /// what `keys` mean in `layers`, the first layer first; a complete
    /// binding wins over a longer one that starts the same way
    pub(crate) fn lookup(&self, layers: &[Layer], keys: &[Chord]) -> Lookup {
        let mut prefix = false;

        for layer in layers {
            for binding in self.bindings.iter().filter(|b| b.layer == *layer) {
                if binding.keys == keys {
                    return Lookup::Exact(binding.name.clone());
                }

                prefix |= binding.keys.len() > keys.len() && binding.keys.starts_with(keys);
            }
        }

        if prefix {
            Lookup::Prefix
        } else {
            Lookup::Nothing
        }
    }

    /// keys bound to two different actions in the same layer, or that
    /// start a longer binding there, so the longer one can never be reached
    pub(crate) fn conflicts(&self) -> Vec<String> {
        let mut found = Vec::new();

        for (index, a) in self.bindings.iter().enumerate() {
            for b in &self.bindings[index + 1..] {
                if a.layer != b.layer || a.name == b.name {
                    continue;
                }

                let shadows = b.keys.len() > a.keys.len() && b.keys.starts_with(&a.keys)
                    || a.keys.len() > b.keys.len() && a.keys.starts_with(&b.keys);

                if a.keys == b.keys || shadows {
                    found.push(format!(
                        "{}: {} is bound to both {} and {}",
                        a.layer.name(),
                        label(&a.keys).min(label(&b.keys)),
                        a.name,
                        b.name
                    ));
                }
            }
        }

        found
    }

    /// the first key bound to `name` in `layer`, as shown to the user
    pub(crate) fn key_for(&self, layer: Layer, name: &str) -> Option<String> {
        self.bindings
            .iter()
            .find(|b| b.layer == layer && b.name == name)
            .map(|b| label(&b.keys))
    }
}

/// the layers that apply where the focus is, the most specific first, and
/// whether unbound keys are typed into a field
pub(crate) fn layers(state: &State) -> (Vec<Layer>, bool) {
    use Layer as L;

    if let Some(popup) = &state.popup {
        let (layer, text) = match popup {
            Popup::Prompt { .. } => (L::Prompt, true),
            Popup::Passphrase { .. } => (L::Passphrase, true),
            Popup::Confirm { .. } => (L::Confirm, false),
            Popup::Trust { .. } => (L::Trust, false),
            Popup::ChooseBackend => (L::Backend, false),
            Popup::Tabs { .. } => (L::Tabs, false),
            Popup::Settings => (L::Settings, false),
            Popup::Approve(_) => (L::Approve, false),
            Popup::Environments { .. } => (L::Environments, false),
            Popup::Variables(editor) if editor.editing() => (L::VariablesTyping, true),
            Popup::Variables(_) => (L::Variables, false),
            Popup::Help { .. } => (L::Help, false),
            Popup::Palette(_) => (L::Palette, true),
            Popup::Welcome { .. } => (L::Welcome, false),
        };

        return (vec![L::Always, layer], text);
    }

    match state.focus {
        Focus::Url => (vec![L::Always, L::Url, L::Global], true),
        Focus::Request if state.request_typing() => {
            let body =
                state.request_tab == RequestTab::Body && state.body.typing == Some(BodyField::Text);
            let layer = if body {
                L::BodyTyping
            } else {
                L::RequestTyping
            };

            (vec![L::Always, layer, L::Global], true)
        }
        Focus::Sidebar => (vec![L::Always, L::Sidebar, L::Panes, L::Global], false),
        Focus::Method => (vec![L::Always, L::Method, L::Panes, L::Global], false),
        Focus::Request => (
            vec![L::Always, L::Request, L::Panes, L::PaneArrows, L::Global],
            false,
        ),
        Focus::Response => (
            vec![
                L::Always,
                L::ResponsePan,
                L::Panes,
                L::PaneArrows,
                L::Response,
                L::Global,
            ],
            false,
        ),
    }
}

/// the actions a key press stands for: usually one, none for an unbound
/// key, and more when a sequence that went nowhere is replayed
impl State {
    /// the start of a key sequence typed so far, such as `g`, for the
    /// footer to show while the next key is awaited
    pub(crate) fn pending_label(&self) -> Option<String> {
        let chords: Vec<Chord> = self
            .pending_keys
            .iter()
            .map(|k| Chord::from_event(*k))
            .collect();

        (!chords.is_empty()).then(|| label(&chords))
    }

    /// the key for an action in `layer`, to name in a message; an unbound
    /// action is named by its config name instead
    pub(crate) fn key(&self, layer: Layer, name: &str) -> String {
        self.keymap
            .key_for(layer, name)
            .unwrap_or_else(|| format!("{}.{name} (unbound)", layer.name()))
    }
}

pub(crate) fn action_for(state: &State, key: KeyEvent) -> Vec<Action> {
    if key.kind == KeyEventKind::Release {
        return Vec::new();
    }

    let (layers, text) = layers(state);
    let keymap = &state.keymap;
    let mut keys: Vec<Chord> = state
        .pending_keys
        .iter()
        .map(|k| Chord::from_event(*k))
        .collect();

    keys.push(Chord::from_event(key));

    match keymap.lookup(&layers, &keys) {
        Lookup::Exact(name) => action(&name).into_iter().collect(),
        Lookup::Prefix => vec![Action::Pending(key)],
        Lookup::Nothing if state.pending_keys.is_empty() => {
            if text {
                vec![Action::Type(key)]
            } else {
                Vec::new()
            }
        }

        // the sequence went nowhere: in a text field its keys are typed,
        // elsewhere they are dropped, and this key counts on its own
        Lookup::Nothing => {
            let mut actions = vec![Action::ClearPending];

            if text {
                actions.extend(state.pending_keys.iter().map(|k| Action::Type(*k)));
            }

            match keymap.lookup(&layers, &[Chord::from_event(key)]) {
                Lookup::Exact(name) => actions.extend(action(&name)),
                Lookup::Prefix => actions.push(Action::Pending(key)),
                Lookup::Nothing if text => actions.push(Action::Type(key)),
                Lookup::Nothing => {}
            }

            actions
        }
    }
}

#[cfg(test)]
mod tests;
