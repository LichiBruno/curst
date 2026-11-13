//! the key a secret is stored under

use std::fmt;
use std::str::FromStr;

use crate::model::{EnvId, LocalId};
use crate::vars::{Origin, VarName};

/// where a secret belongs; its text form is the key in the store
///
/// | scope | text |
/// |---|---|
/// | environment variable | `curst/<local-id>/env/<env-id>/<name>` |
/// | collection variable | `curst/<local-id>/collection/<name>` |
/// | global variable | `curst/global/<name>` |
///
/// the local id comes from this machine's registry, never from a shared
/// file, so a collection cannot reach another collection's secrets
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretKey {
    /// a variable of one environment
    Environment {
        /// the collection's local id
        collection: LocalId,

        /// the environment's id
        environment: EnvId,

        /// the variable
        name: VarName,
    },

    /// a collection variable
    Collection {
        /// the collection's local id
        collection: LocalId,

        /// the variable
        name: VarName,
    },

    /// a global variable
    Global {
        /// the variable
        name: VarName,
    },

    /// the entry a store probe writes and deletes
    Probe,
}

/// a group of secrets deleted together
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretScope {
    /// everything of one collection, its environments included
    Collection(LocalId),

    /// the variables of one environment
    Environment(LocalId, EnvId),
}

impl SecretKey {
    /// the key of a variable found in `origin`; `None` for an environment
    /// variable without an environment, or a collection one without a
    /// collection
    #[must_use]
    pub fn for_variable(
        origin: Origin,
        collection: Option<&LocalId>,
        environment: Option<&EnvId>,
        name: VarName,
    ) -> Option<Self> {
        match origin {
            Origin::Environment => Some(Self::Environment {
                collection: collection?.clone(),
                environment: environment?.clone(),
                name,
            }),
            Origin::Collection => Some(Self::Collection {
                collection: collection?.clone(),
                name,
            }),
            Origin::Global => Some(Self::Global { name }),
        }
    }

    /// the variable name, `None` for the probe
    #[must_use]
    pub fn name(&self) -> Option<&VarName> {
        match self {
            Self::Environment { name, .. }
            | Self::Collection { name, .. }
            | Self::Global { name } => Some(name),
            Self::Probe => None,
        }
    }

    /// the collection it belongs to, `None` for globals and the probe
    #[must_use]
    pub fn collection(&self) -> Option<&LocalId> {
        match self {
            Self::Environment { collection, .. } | Self::Collection { collection, .. } => {
                Some(collection)
            }
            Self::Global { .. } | Self::Probe => None,
        }
    }

    /// the same key for a variable called `name`
    #[must_use]
    pub fn renamed(&self, name: VarName) -> Self {
        match self {
            Self::Environment {
                collection,
                environment,
                ..
            } => Self::Environment {
                collection: collection.clone(),
                environment: environment.clone(),
                name,
            },
            Self::Collection { collection, .. } => Self::Collection {
                collection: collection.clone(),
                name,
            },
            Self::Global { .. } | Self::Probe => Self::Global { name },
        }
    }

    /// whether the key belongs to `scope`, compared field by field
    #[must_use]
    pub fn is_in(&self, scope: &SecretScope) -> bool {
        match (self, scope) {
            (
                Self::Environment {
                    collection,
                    environment,
                    ..
                },
                SecretScope::Environment(c, e),
            ) => collection == c && environment == e,
            (_, SecretScope::Collection(c)) => self.collection() == Some(c),
            _ => false,
        }
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment {
                collection,
                environment,
                name,
            } => write!(f, "curst/{collection}/env/{environment}/{name}"),
            Self::Collection { collection, name } => {
                write!(f, "curst/{collection}/collection/{name}")
            }
            Self::Global { name } => write!(f, "curst/global/{name}"),
            Self::Probe => f.write_str("curst/probe"),
        }
    }
}

/// a text that is not a secret key
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a secret key: {0:?}")]
pub struct BadKey(pub String);

impl FromStr for SecretKey {
    type Err = BadKey;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || BadKey(text.to_owned());
        let parts: Vec<&str> = text.split('/').collect();
        let name = |part: &str| VarName::new(part).ok_or_else(bad);
        let collection = |part: &str| LocalId::new(part).ok_or_else(bad);

        match parts.as_slice() {
            ["curst", "probe"] => Ok(Self::Probe),
            ["curst", "global", n] => Ok(Self::Global { name: name(n)? }),
            ["curst", c, "collection", n] => Ok(Self::Collection {
                collection: collection(c)?,
                name: name(n)?,
            }),
            ["curst", c, "env", e, n] => Ok(Self::Environment {
                collection: collection(c)?,
                environment: EnvId::new(e).ok_or_else(bad)?,
                name: name(n)?,
            }),
            _ => Err(bad()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lid(text: &str) -> LocalId {
        LocalId::new(text).unwrap()
    }

    fn eid(text: &str) -> EnvId {
        EnvId::new(text).unwrap()
    }

    fn name(text: &str) -> VarName {
        VarName::new(text).unwrap()
    }

    #[test]
    fn keys_round_trip_through_text() {
        let keys = [
            SecretKey::Environment {
                collection: lid("abc"),
                environment: eid("e1"),
                name: name("token"),
            },
            SecretKey::Collection {
                collection: lid("abc"),
                name: name("api.key"),
            },
            SecretKey::Global { name: name("g") },
            SecretKey::Probe,
        ];
        let texts = [
            "curst/abc/env/e1/token",
            "curst/abc/collection/api.key",
            "curst/global/g",
            "curst/probe",
        ];

        for (key, text) in keys.iter().zip(texts) {
            assert_eq!(key.to_string(), text);
            assert_eq!(text.parse::<SecretKey>().as_ref(), Ok(key));
        }

        for bad in [
            "",
            "curst",
            "other/global/g",
            "curst/global/a b",
            "curst/ABC/collection/x",
            "curst/abc/env/E/x",
            "curst/abc/files/x",
            "curst/abc/env/e1/x/y",
        ] {
            assert_eq!(bad.parse::<SecretKey>(), Err(BadKey(bad.into())), "{bad}");
        }
    }

    #[test]
    fn scopes_match_whole_ids_not_prefixes() {
        let env = SecretKey::Environment {
            collection: lid("abcd"),
            environment: eid("e1"),
            name: name("t"),
        };
        let collection = SecretKey::Collection {
            collection: lid("abcd"),
            name: name("t"),
        };
        let global = SecretKey::Global { name: name("t") };

        assert!(env.is_in(&SecretScope::Collection(lid("abcd"))));
        assert!(!env.is_in(&SecretScope::Collection(lid("abc"))));
        assert!(env.is_in(&SecretScope::Environment(lid("abcd"), eid("e1"))));
        assert!(!env.is_in(&SecretScope::Environment(lid("abcd"), eid("e"))));
        assert!(collection.is_in(&SecretScope::Collection(lid("abcd"))));
        assert!(!collection.is_in(&SecretScope::Environment(lid("abcd"), eid("e1"))));
        assert!(!global.is_in(&SecretScope::Collection(lid("abcd"))));
        assert!(!SecretKey::Probe.is_in(&SecretScope::Collection(lid("abcd"))));
    }

    #[test]
    fn keys_for_variables_and_renames() {
        let c = lid("c");
        let e = eid("e");

        assert_eq!(
            SecretKey::for_variable(Origin::Environment, Some(&c), Some(&e), name("t")),
            Some(SecretKey::Environment {
                collection: c.clone(),
                environment: e.clone(),
                name: name("t"),
            })
        );
        assert_eq!(
            SecretKey::for_variable(Origin::Environment, Some(&c), None, name("t")),
            None
        );
        assert_eq!(
            SecretKey::for_variable(Origin::Collection, None, None, name("t")),
            None
        );
        assert_eq!(
            SecretKey::for_variable(Origin::Global, None, None, name("t")),
            Some(SecretKey::Global { name: name("t") })
        );

        let key =
            SecretKey::for_variable(Origin::Environment, Some(&c), Some(&e), name("t")).unwrap();

        assert_eq!(key.renamed(name("u")).to_string(), "curst/c/env/e/u");
        assert_eq!(key.name(), Some(&name("t")));
        assert_eq!(key.collection(), Some(&c));
        assert_eq!(
            SecretKey::Collection {
                collection: c.clone(),
                name: name("t")
            }
            .renamed(name("u"))
            .to_string(),
            "curst/c/collection/u"
        );
        assert_eq!(
            SecretKey::Global { name: name("t") }
                .renamed(name("u"))
                .to_string(),
            "curst/global/u"
        );
        assert_eq!(SecretKey::Probe.name(), None);
        assert_eq!(SecretKey::Probe.collection(), None);
    }
}
