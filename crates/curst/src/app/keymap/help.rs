//! the help overlay's text: every binding, the ones where the focus is
//! first, each with what it does and the name to rebind it by

use super::{Keymap, Layer, label};

/// one action and its keys
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) keys: String,
    pub(crate) what: &'static str,

    /// the name in `config.toml`, as in `"global.send"`
    pub(crate) name: String,
}

/// a group of rows under a heading
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub(crate) title: String,
    pub(crate) rows: Vec<Row>,
}

/// what an action does, for the help and the command palette
pub(crate) fn describe(name: &str) -> &'static str {
    match name {
        "quit" => "quit",
        "send" => "send the request",
        "save" => "save",
        "cancel" => "cancel the request being sent",
        "focus.next" => "next pane",
        "focus.previous" => "previous pane",
        "tab.next" => "next tab of the pane",
        "tab.previous" => "previous tab of the pane",
        "method.next" => "next method",
        "method.previous" => "previous method",
        "up" => "up",
        "down" => "down",
        "page-up" => "page up",
        "page-down" => "page down",
        "top" => "to the top",
        "bottom" => "to the bottom",
        "left" => "left",
        "right" => "right",
        "edit" => "edit, or open",
        "content-type" => "edit the content type",
        "stop-edit" => "done editing",
        "next-cell" => "next field",
        "row.add" => "add a row",
        "row.delete" => "delete the row",
        "row.toggle" => "turn the row on or off",
        "kind" => "change the body or auth type",
        "save-body" => "save the body to a file",
        "follow" => "follow a redirect that was stopped",
        "new-request" => "new request",
        "new-folder" => "new folder",
        "new-collection" => "new collection",
        "rename" => "rename",
        "delete" => "delete",
        "cut" => "cut, to move",
        "paste" => "paste what was cut",
        "move-up" => "move up",
        "move-down" => "move down",
        "trust" => "trust a collection, or not",
        "environments" => "environments",
        "settings" => "settings",
        "make-secret" => "make it a secret, or plain again",
        "edit-body" => "edit the body in your editor",
        "tabs" => "the open requests",
        "request.new" => "new request tab",
        "request.close" => "close the request tab",
        "request.next" => "next request tab",
        "request.previous" => "previous request tab",
        "leave-collection" => "detach the request from its collection",
        "confirm" => "yes, or ok",
        "close" => "no, or close",
        "env.new" => "new environment",
        "env.rename" => "rename the environment",
        "env.delete" => "delete the environment",
        "env.variables" => "edit the variables",
        "migrate" => "move secrets to the other store",
        "backend.keychain" => "keep secrets in the keychain",
        "backend.file" => "keep secrets in an encrypted file",
        "help" => "this help",
        "palette" => "the command palette",
        "keys" => "the next keymap preset",
        "copy" => "copy what the response tab shows",
        "copy-curl" => "copy the request as a curl command",
        "theme" => "the next theme",
        _ if name.starts_with("request.go-") => "go to request 1-8, 9 the last",
        _ if name.starts_with("trust.moved-") => "it moved from the folder with this number",
        _ => "",
    }
}

/// numbered actions are shown as one row
fn group(name: &str) -> Option<&'static str> {
    if name.starts_with("request.go-") {
        Some("request.go-")
    } else if name.starts_with("trust.moved-") {
        Some("trust.moved-")
    } else {
        None
    }
}

impl Keymap {
    /// the rows of one layer, in preset order, skipping names in `seen`
    fn rows(&self, layer: Layer, seen: &mut Vec<String>) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        let mut numbered: Vec<(&'static str, String, String)> = Vec::new();

        for binding in self.bindings.iter().filter(|b| b.layer == layer) {
            if seen.contains(&binding.name) {
                continue;
            }

            let keys = label(&binding.keys);

            if let Some(prefix) = group(&binding.name) {
                match numbered.iter_mut().find(|(p, _, _)| *p == prefix) {
                    Some((_, _, last)) => *last = keys,
                    None => numbered.push((prefix, keys.clone(), keys)),
                }

                continue;
            }

            let name = format!("{}.{}", layer.name(), binding.name);

            match rows.iter_mut().find(|r| r.name == name) {
                Some(row) => {
                    row.keys.push_str(", ");
                    row.keys.push_str(&keys);
                }
                None => rows.push(Row {
                    keys,
                    what: describe(&binding.name),
                    name,
                }),
            }
        }

        for (prefix, first, last) in numbered {
            rows.push(Row {
                keys: format!("{first} … {last}"),
                what: describe(prefix),
                name: format!("{}.{prefix}N", layer.name()),
            });
        }

        seen.extend(
            self.bindings
                .iter()
                .filter(|b| b.layer == layer)
                .map(|b| b.name.clone()),
        );

        rows
    }

    /// the help: the keys where the focus is, given as its chain of
    /// layers, then those that work everywhere, then all the others
    pub(crate) fn help(&self, here: &[Layer]) -> Vec<Section> {
        let everywhere = [Layer::Always, Layer::Global];
        let mut sections = Vec::new();
        let mut seen = Vec::new();
        let mut rows = Vec::new();

        for layer in here.iter().filter(|l| !everywhere.contains(l)) {
            rows.extend(self.rows(*layer, &mut seen));
        }

        if !rows.is_empty() {
            sections.push(Section {
                title: "Here".into(),
                rows,
            });
        }

        let mut seen = Vec::new();
        let mut rows = Vec::new();

        for layer in everywhere {
            rows.extend(self.rows(layer, &mut seen));
        }

        sections.push(Section {
            title: "Everywhere".into(),
            rows,
        });

        for layer in Layer::ALL {
            if here.contains(&layer) || everywhere.contains(&layer) {
                continue;
            }

            let rows = self.rows(layer, &mut Vec::new());

            if !rows.is_empty() {
                sections.push(Section {
                    title: format!("In {}", layer.title()),
                    rows,
                });
            }
        }

        sections
    }
}

/// how many lines the help takes: each section's heading, its rows and a
/// blank line
pub(crate) fn help_lines(sections: &[Section]) -> usize {
    sections.iter().map(|s| s.rows.len() + 2).sum()
}

/// an action the command palette offers
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Command {
    /// the name in `config.toml`, as in `"global.send"`
    pub(crate) name: String,
    pub(crate) what: &'static str,

    /// its keys, `None` when it is unbound
    pub(crate) keys: Option<String>,
}

/// moving around and answering pop-ups: the palette leaves these to keys
const NOT_COMMANDS: [&str; 18] = [
    "up",
    "down",
    "left",
    "right",
    "page-up",
    "page-down",
    "top",
    "bottom",
    "focus.next",
    "focus.previous",
    "tab.next",
    "tab.previous",
    "stop-edit",
    "next-cell",
    "confirm",
    "close",
    "palette",
    "help",
];

impl Keymap {
    /// the actions that work in `here`, a chain of layers, bound or not,
    /// each once, the most specific layer first
    pub(crate) fn commands(&self, here: &[Layer]) -> Vec<Command> {
        let mut commands: Vec<Command> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();

        for layer in here {
            for (row_layer, name, _) in super::presets::STANDARD {
                let skip = *row_layer != *layer
                    || NOT_COMMANDS.contains(name)
                    || group(name).is_some()
                    || seen.contains(name);

                if skip {
                    continue;
                }

                seen.push(name);
                commands.push(Command {
                    name: format!("{}.{name}", layer.name()),
                    what: describe(name),
                    keys: self.key_for(*layer, name),
                });
            }
        }

        commands
    }
}
