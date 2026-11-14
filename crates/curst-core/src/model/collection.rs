//! collections, folders, saved requests, environments and their variables

use std::fmt;

use super::{Auth, KeyValue, Request};
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

    /// whether the value lives in the secret store; `value` is then empty
    /// and never written to a file
    pub secret: bool,
}

impl Variable {
    /// an enabled variable that is not secret
    pub fn new(name: VarName, value: impl Into<String>) -> Self {
        Self {
            name,
            value: value.into(),
            enabled: true,
            secret: false,
        }
    }

    /// an enabled secret variable, its value kept elsewhere
    #[must_use]
    pub fn secret(name: VarName) -> Self {
        Self {
            name,
            value: String::new(),
            enabled: true,
            secret: true,
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

    /// the spec operation it was made from, for a request imported from
    /// an `OpenAPI` spec
    pub spec: Option<SpecOrigin>,
}

/// where an imported request came from in its spec
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecOrigin {
    /// the operation, as `GET /pets/{petId}`
    pub operation: String,

    /// a fingerprint of the operation as the spec described it when the
    /// request was made; a different one on a refresh means the spec
    /// changed, whatever the user changed in the request
    pub fingerprint: String,
}

/// where a collection imported from a spec fetches it again
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Source {
    /// what the user gave: a URL, or an absolute file or folder path
    pub location: String,

    /// headers sent with a URL, which may hold `{{variables}}`
    pub headers: Vec<KeyValue>,

    /// the spec chosen in a folder that has several
    pub picked: Option<String>,
}

/// a folder of requests and other folders
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// the address of the folder inside its parent
    pub slug: String,

    /// the display name
    pub name: String,

    /// the auth its requests inherit; [`Auth::Inherit`] passes on what is
    /// above it
    pub auth: Auth,

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

    /// the spec it was imported from, to fetch it again
    pub source: Option<Source>,

    /// the auth its requests and folders inherit
    pub auth: Auth,

    /// requests sent one after another, keeping values from their answers
    pub workflows: Vec<Workflow>,
}

/// requests sent one after another, each answer giving values to keep,
/// such as a token to log in with
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Workflow {
    /// the name it is run under
    pub name: String,

    /// the steps, in order
    pub steps: Vec<Step>,
}

/// one request of a workflow and what to keep from its answer
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    /// the saved request, as a path of slugs from the top
    pub request: Vec<String>,

    /// the values kept from the answer, each as a secret variable
    pub keep: Vec<Capture>,
}

/// a value taken from an answer into a variable
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// the variable it goes into
    pub variable: VarName,

    /// where in the answer it is
    pub from: Extract,
}

/// where in an answer a value is
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extract {
    /// a path into a JSON body, such as `$.data.token` or `items[0].id`
    Json(String),

    /// a header, its name in any case
    Header(String),
}

impl Collection {
    /// the auth a request in the folder at `parent` inherits, with the
    /// name of the folder or collection it comes from; [`Auth::None`] when
    /// nothing above sets one
    #[must_use]
    pub fn inherited_auth(&self, parent: &[String]) -> (Auth, String) {
        let mut found = (self.auth.clone(), self.name.clone());
        let mut items = &self.items;

        for slug in parent {
            let Some(Item::Folder(folder)) = items.iter().find(|i| i.slug() == slug) else {
                break;
            };

            if folder.auth != Auth::Inherit {
                found = (folder.auth.clone(), folder.name.clone());
            }

            items = &folder.items;
        }

        if found.0 == Auth::Inherit {
            found.0 = Auth::None;
        }

        found
    }

    /// `request` at `path` with the auth it inherits in place of
    /// [`Auth::Inherit`]
    #[must_use]
    pub fn with_inherited_auth(&self, path: &[String], request: &Request) -> Request {
        let mut request = request.clone();

        if request.auth == Auth::Inherit {
            let parent = &path[..path.len().saturating_sub(1)];

            request.auth = self.inherited_auth(parent).0;
        }

        request
    }

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
            spec: None,
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
                    auth: Auth::Inherit,
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
