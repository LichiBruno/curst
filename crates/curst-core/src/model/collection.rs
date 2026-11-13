//! collections, folders, saved requests, environments and their variables

use std::fmt;

use super::Request;
use crate::vars::VarName;

/// a variable as one scope defines it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// the name used in `{{name}}`
    pub name: VarName,

    /// the value, which may contain other `{{name}}` templates
    pub value: String,

    /// whether resolution uses it
    pub enabled: bool,
}

impl Variable {
    /// an enabled variable
    pub fn new(name: VarName, value: impl Into<String>) -> Self {
        Self {
            name,
            value: value.into(),
            enabled: true,
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// the id, or `None` unless it is 1 to 64 lower-case ASCII
            /// letters, digits and `-`
            #[must_use]
            pub fn new(id: &str) -> Option<Self> {
                valid_id(id).then(|| Self(id.to_owned()))
            }

            /// the id as written
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// identifies a collection on this machine; it lives in the local
    /// registry and is never read from a shared file
    LocalId
);

id_type!(
    /// identifies an environment inside its collection and stays the same
    /// when the environment is renamed
    EnvId
);

/// a named set of variables, chosen as the active one per collection
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// the stable id
    pub id: EnvId,

    /// the file name it is stored under, without extension
    pub slug: String,

    /// the display name
    pub name: String,

    /// the variables
    pub variables: Vec<Variable>,
}

/// a request with the name it is shown under
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRequest {
    /// the address of the request inside its folder
    pub slug: String,

    /// the display name
    pub name: String,

    /// the request
    pub request: Request,
}

/// a folder of requests and other folders
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// the address of the folder inside its parent
    pub slug: String,

    /// the display name
    pub name: String,

    /// the children, in display order
    pub items: Vec<Item>,
}

/// one entry of a folder
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// a folder
    Folder(Folder),

    /// a request
    Request(SavedRequest),
}

impl Item {
    /// the address inside the parent
    #[must_use]
    pub fn slug(&self) -> &str {
        match self {
            Self::Folder(folder) => &folder.slug,
            Self::Request(request) => &request.slug,
        }
    }

    /// the display name
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Folder(folder) => &folder.name,
            Self::Request(request) => &request.name,
        }
    }
}

/// a collection: its own variables, a tree of requests and its environments
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Collection {
    /// the display name
    pub name: String,

    /// the collection variables
    pub variables: Vec<Variable>,

    /// the top-level folders and requests, in display order
    pub items: Vec<Item>,

    /// the environments, sorted by name
    pub environments: Vec<Environment>,
}

impl Collection {
    /// the folder or request at `path`, a list of slugs from the top
    #[must_use]
    pub fn item(&self, path: &[String]) -> Option<&Item> {
        let (last, parents) = path.split_last()?;
        let mut items = &self.items;

        for slug in parents {
            match items.iter().find(|i| i.slug() == slug)? {
                Item::Folder(folder) => items = &folder.items,
                Item::Request(_) => return None,
            }
        }

        items.iter().find(|i| i.slug() == last)
    }

    /// the environment with this id
    #[must_use]
    pub fn environment(&self, id: &EnvId) -> Option<&Environment> {
        self.environments.iter().find(|e| e.id == *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(slug: &str) -> Item {
        Item::Request(SavedRequest {
            slug: slug.into(),
            name: slug.to_uppercase(),
            request: Request::default(),
        })
    }

    #[test]
    fn ids_are_validated() {
        assert!(LocalId::new("a1-b2").is_some());
        assert!(EnvId::new(&"a".repeat(64)).is_some());
        assert!(EnvId::new(&"a".repeat(65)).is_none());
        assert!(LocalId::new("").is_none());
        assert!(LocalId::new("Upper").is_none());
        assert!(LocalId::new("../x").is_none());
        assert_eq!(LocalId::new("x").unwrap().to_string(), "x");
        assert_eq!(EnvId::new("x").unwrap().to_string(), "x");
        assert_eq!(EnvId::new("x").unwrap().as_str(), "x");
    }

    #[test]
    fn items_are_found_by_path() {
        let collection = Collection {
            items: vec![
                request("top"),
                Item::Folder(Folder {
                    slug: "users".into(),
                    name: "Users".into(),
                    items: vec![request("list")],
                }),
            ],
            ..Collection::default()
        };
        let path = |p: &[&str]| p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

        assert_eq!(
            collection.item(&path(&["top"])).map(Item::name),
            Some("TOP")
        );
        assert_eq!(
            collection.item(&path(&["users", "list"])).map(Item::name),
            Some("LIST")
        );
        assert_eq!(
            collection.item(&path(&["users"])).map(Item::name),
            Some("Users")
        );
        assert!(collection.item(&path(&["top", "x"])).is_none());
        assert!(collection.item(&path(&["nope"])).is_none());
        assert!(collection.item(&[]).is_none());
    }

    #[test]
    fn environments_are_found_by_id() {
        let id = EnvId::new("e1").unwrap();
        let collection = Collection {
            environments: vec![Environment {
                id: id.clone(),
                slug: "dev".into(),
                name: "Dev".into(),
                variables: vec![],
            }],
            ..Collection::default()
        };

        assert_eq!(
            collection.environment(&id).map(|e| e.name.as_str()),
            Some("Dev")
        );
        assert!(collection.environment(&EnvId::new("e2").unwrap()).is_none());
    }
}
