//! the collections shown in the sidebar, how they are addressed and which
//! rows the sidebar shows

use std::collections::HashSet;
use std::path::PathBuf;

use curst_core::model::{Collection, EnvId, Item, LocalId, Method, Variable};

/// a loaded collection with what this machine knows about it
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workspace {
    pub(crate) id: LocalId,
    pub(crate) root: PathBuf,
    pub(crate) collection: Collection,
    pub(crate) active_environment: Option<EnvId>,

    /// whether its requests may be sent and its secrets used; a `.curst/`
    /// folder is untrusted until the user says otherwise
    pub(crate) trusted: bool,

    /// whether curst made its folder, in the data folder; curst never
    /// renames or deletes any other
    pub(crate) managed: bool,
}

impl Workspace {
    /// the active environment's variables, none when no environment is
    /// active or the active one no longer exists
    pub(crate) fn environment_variables(&self) -> &[Variable] {
        self.active_environment
            .as_ref()
            .and_then(|id| self.collection.environment(id))
            .map_or(&[], |e| e.variables.as_slice())
    }

    pub(crate) fn environment_name(&self) -> Option<&str> {
        self.active_environment
            .as_ref()
            .and_then(|id| self.collection.environment(id))
            .map(|e| e.name.as_str())
    }
}

/// a collection, folder or request: the collection's id and the slugs from
/// its top, empty for the collection itself
///
/// the id stays the same when the collection folder is renamed, so a key
/// outlives everything but a rename or move of the item itself
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NodeKey {
    pub(crate) collection: LocalId,
    pub(crate) path: Vec<String>,
}

impl NodeKey {
    pub(crate) fn root(collection: LocalId) -> Self {
        Self {
            collection,
            path: Vec::new(),
        }
    }

    pub(crate) fn child(&self, slug: &str) -> Self {
        let mut path = self.path.clone();

        path.push(slug.to_owned());

        Self {
            collection: self.collection.clone(),
            path,
        }
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        let (_, parent) = self.path.split_last()?;

        Some(Self {
            collection: self.collection.clone(),
            path: parent.to_vec(),
        })
    }

    /// whether `self` is `other` or lies inside it
    pub(crate) fn is_within(&self, other: &Self) -> bool {
        self.collection == other.collection && self.path.starts_with(&other.path)
    }

    /// the same node after `from` was renamed or moved to `to`, also into
    /// another collection
    pub(crate) fn rebased(&self, from: &Self, to: &Self) -> Self {
        if !self.is_within(from) {
            return self.clone();
        }

        let mut path = to.path.clone();

        path.extend_from_slice(&self.path[from.path.len()..]);

        Self {
            collection: to.collection.clone(),
            path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowKind {
    Collection,
    Folder,
    Request(Method),
}

/// one line of the sidebar
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) key: NodeKey,
    pub(crate) depth: usize,
    pub(crate) name: String,
    pub(crate) kind: RowKind,
    pub(crate) expanded: bool,

    /// whether the collection the row belongs to is trusted
    pub(crate) trusted: bool,
}

/// what the sidebar shows open, which row is selected and what is cut
#[derive(Debug, Default)]
pub(crate) struct Sidebar {
    pub(crate) selected: Option<NodeKey>,
    pub(crate) expanded: HashSet<NodeKey>,
    pub(crate) cut: Option<NodeKey>,
}

impl Sidebar {
    /// follows a rename or move everywhere a key is kept
    pub(crate) fn rebase(&mut self, from: &NodeKey, to: &NodeKey) {
        self.selected = self.selected.as_ref().map(|k| k.rebased(from, to));
        self.cut = self.cut.as_ref().map(|k| k.rebased(from, to));
        self.expanded = self.expanded.iter().map(|k| k.rebased(from, to)).collect();
    }
}

/// every visible row, collections first and the open folders below them
pub(crate) fn rows(collections: &[Workspace], expanded: &HashSet<NodeKey>) -> Vec<Row> {
    let mut rows = Vec::new();

    for workspace in collections {
        let key = NodeKey::root(workspace.id.clone());
        let open = expanded.contains(&key);

        rows.push(Row {
            key: key.clone(),
            depth: 0,
            name: workspace.collection.name.clone(),
            kind: RowKind::Collection,
            expanded: open,
            trusted: workspace.trusted,
        });

        if open {
            let start = rows.len();

            add_items(&mut rows, &workspace.collection.items, &key, 1, expanded);

            for row in &mut rows[start..] {
                row.trusted = workspace.trusted;
            }
        }
    }

    rows
}

fn add_items(
    rows: &mut Vec<Row>,
    items: &[Item],
    parent: &NodeKey,
    depth: usize,
    expanded: &HashSet<NodeKey>,
) {
    for item in items {
        let key = parent.child(item.slug());

        match item {
            Item::Folder(folder) => {
                let open = expanded.contains(&key);

                rows.push(Row {
                    key: key.clone(),
                    depth,
                    name: folder.name.clone(),
                    kind: RowKind::Folder,
                    expanded: open,
                    trusted: true,
                });

                if open {
                    add_items(rows, &folder.items, &key, depth + 1, expanded);
                }
            }
            Item::Request(request) => rows.push(Row {
                key,
                depth,
                name: request.name.clone(),
                kind: RowKind::Request(request.request.method),
                expanded: false,
                trusted: true,
            }),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use curst_core::model::{Environment, Folder, Request, SavedRequest};
    use curst_core::vars::VarName;

    use super::*;

    fn id(text: &str) -> LocalId {
        LocalId::new(text).unwrap()
    }

    fn key(collection: &str, path: &[&str]) -> NodeKey {
        NodeKey {
            collection: id(collection),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    pub(crate) fn sample() -> Workspace {
        let request = |slug: &str, method| {
            Item::Request(SavedRequest {
                slug: slug.into(),
                name: slug.to_uppercase(),
                request: Request {
                    method,
                    ..Request::default()
                },
            })
        };

        Workspace {
            id: id("c1"),
            root: PathBuf::from("/c1"),
            collection: Collection {
                name: "Pets".into(),
                variables: vec![],
                items: vec![
                    Item::Folder(Folder {
                        slug: "users".into(),
                        name: "Users".into(),
                        items: vec![request("list", Method::Get)],
                    }),
                    request("create", Method::Post),
                ],
                environments: vec![Environment {
                    id: EnvId::new("e1").unwrap(),
                    slug: "dev".into(),
                    name: "Dev".into(),
                    variables: vec![Variable::new(VarName::new("a").unwrap(), "1")],
                }],
            },
            active_environment: None,
            trusted: true,
            managed: true,
        }
    }

    #[test]
    fn keys_move_with_renames() {
        let folder = key("c1", &["a"]);
        let inside = key("c1", &["a", "b"]);
        let elsewhere = key("c1", &["x"]);
        let renamed = key("c1", &["z"]);

        assert!(inside.is_within(&folder));
        assert!(!elsewhere.is_within(&folder));
        assert!(!key("c2", &["a"]).is_within(&folder));
        assert_eq!(inside.rebased(&folder, &renamed), key("c1", &["z", "b"]));
        assert_eq!(elsewhere.rebased(&folder, &renamed), elsewhere);
        assert_eq!(inside.parent(), Some(folder.clone()));
        assert_eq!(NodeKey::root(id("c1")).parent(), None);

        let mut sidebar = Sidebar {
            selected: Some(inside.clone()),
            expanded: HashSet::from([folder.clone()]),
            cut: Some(inside),
        };

        sidebar.rebase(&folder, &renamed);

        assert_eq!(sidebar.selected, Some(key("c1", &["z", "b"])));
        assert_eq!(sidebar.cut, Some(key("c1", &["z", "b"])));
        assert!(sidebar.expanded.contains(&renamed));
    }

    #[test]
    fn rows_follow_what_is_expanded() {
        let workspaces = [sample()];
        let names = |expanded: &HashSet<NodeKey>| {
            rows(&workspaces, expanded)
                .into_iter()
                .map(|r| format!("{}{}", " ".repeat(r.depth), r.name))
                .collect::<Vec<_>>()
        };

        assert_eq!(names(&HashSet::new()), ["Pets"]);

        let mut expanded = HashSet::from([key("c1", &[])]);

        assert_eq!(names(&expanded), ["Pets", " Users", " CREATE"]);

        expanded.insert(key("c1", &["users"]));

        assert_eq!(names(&expanded), ["Pets", " Users", "  LIST", " CREATE"]);

        let all = rows(&workspaces, &expanded);

        assert_eq!(all[3].kind, RowKind::Request(Method::Post));
        assert_eq!(all[1].kind, RowKind::Folder);
        assert!(all[1].expanded);
    }

    #[test]
    fn a_missing_active_environment_counts_as_none() {
        let mut workspace = sample();

        assert_eq!(workspace.environment_variables(), &[]);
        assert_eq!(workspace.environment_name(), None);

        workspace.active_environment = EnvId::new("e1");

        assert_eq!(workspace.environment_variables().len(), 1);
        assert_eq!(workspace.environment_name(), Some("Dev"));

        workspace.active_environment = EnvId::new("gone");

        assert_eq!(workspace.environment_variables(), &[]);
        assert_eq!(workspace.environment_name(), None);
    }
}
