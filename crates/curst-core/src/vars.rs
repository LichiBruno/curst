//! `{{name}}` templates and their resolution, without any I/O
//!
//! a value may itself contain `{{other}}`, which is expanded too; a variable
//! that refers back to itself, directly or not, is left as written and
//! reported as cyclic

use std::collections::BTreeSet;
use std::fmt;

use crate::model::{Auth, Body, KeyValue, Request, Variable};

const MAX_NAME: usize = 128;

/// a variable name: ASCII letters, digits, `_`, `-` and `.`
///
/// ```
/// use curst_core::vars::VarName;
///
/// assert!(VarName::new("base_url").is_some());
/// assert!(VarName::new("api.v2-token").is_some());
/// assert!(VarName::new("has space").is_none());
/// assert!(VarName::new("").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VarName(String);

impl VarName {
    /// the name, or `None` when it has a character outside the allowed set
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        let valid = !name.is_empty()
            && name.len() <= MAX_NAME
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));

        valid.then(|| Self(name.to_owned()))
    }

    /// a name made from any text, such as a header or parameter name:
    /// lower case, `_` for what a name cannot hold, never empty
    #[must_use]
    pub fn from_hint(text: &str) -> Self {
        let name: String = text
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(MAX_NAME)
            .collect();
        let name = name.trim_matches('_');

        Self(if name.is_empty() {
            "value".to_owned()
        } else {
            name.to_owned()
        })
    }

    /// the name as written
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// the scope a variable was found in
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// the active environment
    Environment,

    /// the collection
    Collection,

    /// the global variables
    Global,
}

/// what a lookup found for a name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found<'a> {
    /// the value, or `None` for a secret whose value is not available
    pub value: Option<&'a str>,

    /// the scope that defines the variable
    pub origin: Origin,

    /// whether the variable is a secret
    pub secret: bool,
}

/// where variable values come from
pub trait Lookup {
    /// the variable called `name`, or `None` when no scope defines it
    fn find(&self, name: &VarName) -> Option<Found<'_>>;
}

/// the values of secret variables, fetched from the secret store by the
/// caller before resolving
pub trait SecretValues {
    /// the value of the secret `name` defined in `origin`, if available
    fn secret(&self, origin: Origin, name: &VarName) -> Option<&str>;
}

/// the three scopes, searched in order: environment, collection, global
#[derive(Clone, Copy, Default)]
pub struct Scopes<'a> {
    /// the active environment's variables
    pub environment: &'a [Variable],

    /// the collection's variables
    pub collection: &'a [Variable],

    /// variables shared by every collection
    pub global: &'a [Variable],

    /// the values of secret variables; without it every secret is
    /// unavailable
    pub secrets: Option<&'a dyn SecretValues>,
}

impl Lookup for Scopes<'_> {
    fn find(&self, name: &VarName) -> Option<Found<'_>> {
        let scopes = [
            (Origin::Environment, self.environment),
            (Origin::Collection, self.collection),
            (Origin::Global, self.global),
        ];
        let (origin, variable) = scopes.into_iter().find_map(|(origin, variables)| {
            variables
                .iter()
                .find(|v| v.enabled && v.name == *name)
                .map(|v| (origin, v))
        })?;
        let value = if variable.secret {
            self.secrets.and_then(|s| s.secret(origin, name))
        } else {
            Some(variable.value.as_str())
        };

        Some(Found {
            value,
            origin,
            secret: variable.secret,
        })
    }
}

/// what a resolution met on the way
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// every variable that was replaced, nested ones included
    pub used: BTreeSet<VarName>,

    /// the secrets among them, with the scope each came from
    pub secrets: BTreeSet<(Origin, VarName)>,

    /// variables no scope defines, left as written
    pub missing: BTreeSet<VarName>,

    /// secrets whose value is not available, left as written
    pub unavailable: BTreeSet<(Origin, VarName)>,

    /// variables whose value leads back to themselves, left as written
    pub cyclic: BTreeSet<VarName>,
}

impl Resolution {
    /// whether every variable was replaced
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.unavailable.is_empty() && self.cyclic.is_empty()
    }
}

/// replaces every `{{name}}` in `text`, recording what happened in `out`
///
/// spaces inside the braces are allowed, and braces around anything that is
/// not a valid name are kept as text
///
/// ```
/// use curst_core::model::Variable;
/// use curst_core::vars::{resolve, Resolution, Scopes, VarName};
///
/// let env = [
///     Variable::new(VarName::new("host").unwrap(), "api.example.com"),
///     Variable::new(VarName::new("base").unwrap(), "https://{{host}}"),
/// ];
/// let scopes = Scopes { environment: &env, ..Scopes::default() };
/// let mut out = Resolution::default();
///
/// assert_eq!(
///     resolve("{{ base }}/v1?q={{missing}}", &scopes, &mut out),
///     "https://api.example.com/v1?q={{missing}}"
/// );
/// assert_eq!(out.missing.len(), 1);
/// ```
pub fn resolve(text: &str, lookup: &impl Lookup, out: &mut Resolution) -> String {
    expand(text, lookup, out, &mut Vec::new())
}

fn expand(
    text: &str,
    lookup: &impl Lookup,
    out: &mut Resolution,
    stack: &mut Vec<VarName>,
) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find("{{") {
        result.push_str(&rest[..open]);

        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            result.push_str(&rest[open..]);

            return result;
        };

        // not a name: keep one brace and look again from the next one, so
        // `{{{a}}}` still finds `{{a}}`
        let Some(name) = VarName::new(after[..close].trim()) else {
            result.push('{');
            rest = &rest[open + 1..];

            continue;
        };

        let written = &rest[open..open + 2 + close + 2];

        rest = &after[close + 2..];

        if stack.contains(&name) {
            out.cyclic.insert(name);
            result.push_str(written);

            continue;
        }

        let Some(found) = lookup.find(&name) else {
            out.missing.insert(name);
            result.push_str(written);

            continue;
        };

        let Some(value) = found.value else {
            out.unavailable.insert((found.origin, name));
            result.push_str(written);

            continue;
        };

        stack.push(name.clone());

        let expanded = expand(value, lookup, out, stack);

        stack.pop();

        // a value that loops is kept as written, like a missing one
        if out.cyclic.contains(&name) {
            result.push_str(written);
        } else {
            result.push_str(&expanded);

            if found.secret {
                out.secrets.insert((found.origin, name.clone()));
            }

            out.used.insert(name);
        }
    }

    result.push_str(rest);

    result
}

/// `request` with every template in it resolved: URL, parameters, headers,
/// body and auth
#[must_use]
pub fn resolve_request(request: &Request, lookup: &impl Lookup) -> (Request, Resolution) {
    let mut out = Resolution::default();
    let out = &mut out;
    let body = match &request.body {
        Body::None => Body::None,
        Body::Json(json) => Body::Json(resolve(json, lookup, out)),
        Body::Text { content_type, text } => Body::Text {
            content_type: resolve(content_type, lookup, out),
            text: resolve(text, lookup, out),
        },
        Body::Form(fields) => Body::Form(resolve_pairs(fields, lookup, out)),
    };
    let auth = match &request.auth {
        Auth::Inherit => Auth::Inherit,
        Auth::None => Auth::None,
        Auth::Basic { username, password } => Auth::Basic {
            username: resolve(username, lookup, out),
            password: resolve(password, lookup, out),
        },
        Auth::Bearer { token } => Auth::Bearer {
            token: resolve(token, lookup, out),
        },
        Auth::ApiKey { name, value, place } => Auth::ApiKey {
            name: resolve(name, lookup, out),
            value: resolve(value, lookup, out),
            place: *place,
        },
    };
    let resolved = Request {
        method: request.method,
        url: resolve(&request.url, lookup, out),
        params: resolve_pairs(&request.params, lookup, out),
        headers: resolve_pairs(&request.headers, lookup, out),
        body,
        auth,
    };

    (resolved, std::mem::take(out))
}

/// disabled pairs are never sent, so they are kept as written and add
/// nothing to the resolution
fn resolve_pairs(pairs: &[KeyValue], lookup: &impl Lookup, out: &mut Resolution) -> Vec<KeyValue> {
    pairs
        .iter()
        .map(|p| {
            if !p.enabled {
                return p.clone();
            }

            KeyValue {
                name: resolve(&p.name, lookup, out),
                value: resolve(&p.value, lookup, out),
                enabled: true,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn name(text: &str) -> VarName {
        VarName::new(text).unwrap()
    }

    fn var(n: &str, value: &str) -> Variable {
        Variable::new(name(n), value)
    }

    fn run(text: &str, vars: &[Variable]) -> (String, Resolution) {
        let scopes = Scopes {
            environment: vars,
            ..Scopes::default()
        };
        let mut out = Resolution::default();
        let result = resolve(text, &scopes, &mut out);

        (result, out)
    }

    #[test]
    fn names() {
        assert!(VarName::new(&"a".repeat(MAX_NAME)).is_some());
        assert!(VarName::new(&"a".repeat(MAX_NAME + 1)).is_none());
        assert!(VarName::new("é").is_none());
        assert_eq!(name("x").to_string(), "x");
    }

    #[test]
    fn plain_text_and_odd_braces_stay() {
        let vars = [var("a", "1")];

        assert_eq!(run("no templates", &vars).0, "no templates");
        assert_eq!(run("{{a", &vars).0, "{{a");
        assert_eq!(run("{{not valid}} {{a}}", &vars).0, "{{not valid}} 1");
        assert_eq!(run("{{}}", &vars).0, "{{}}");
        assert_eq!(run("{{{a}}}", &vars).0, "{1}");
        assert_eq!(run("}}{{a}}", &vars).0, "}}1");
    }

    #[test]
    fn spaces_and_repeats() {
        let (text, out) = run("{{ a }}-{{a}}", &[var("a", "1")]);

        assert_eq!(text, "1-1");
        assert_eq!(out.used, BTreeSet::from([name("a")]));
        assert!(out.is_complete());
    }

    #[test]
    fn missing_are_kept_and_reported() {
        let (text, out) = run("x={{ gone }}", &[]);

        assert_eq!(text, "x={{ gone }}");
        assert_eq!(out.missing, BTreeSet::from([name("gone")]));
        assert!(!out.is_complete());
    }

    #[test]
    fn nested_values_expand() {
        let vars = [var("a", "<{{b}}>"), var("b", "{{c}}{{c}}"), var("c", "z")];
        let (text, out) = run("{{a}}", &vars);

        assert_eq!(text, "<zz>");
        assert_eq!(out.used.len(), 3);
    }

    #[test]
    fn cycles_are_kept_and_reported() {
        let vars = [
            var("a", "{{b}}"),
            var("b", "x{{a}}"),
            var("self", "{{self}}"),
        ];
        let (text, out) = run("{{a}} {{self}}", &vars);

        assert_eq!(text, "{{a}} {{self}}");
        assert_eq!(out.cyclic, BTreeSet::from([name("a"), name("self")]));
    }

    #[test]
    fn scopes_take_precedence_in_order_and_skip_disabled() {
        let mut off = var("x", "env-off");
        off.enabled = false;

        let environment = [off, var("only_env", "e")];
        let collection = [var("x", "collection"), var("y", "collection")];
        let global = [var("x", "global"), var("y", "global"), var("z", "global")];
        let scopes = Scopes {
            environment: &environment,
            collection: &collection,
            global: &global,
            secrets: None,
        };
        let mut out = Resolution::default();

        assert_eq!(
            resolve("{{x}} {{y}} {{z}} {{only_env}}", &scopes, &mut out),
            "collection collection global e"
        );
    }

    #[test]
    fn a_whole_request_is_resolved() {
        let vars = [var("v", "1")];
        let scopes = Scopes {
            environment: &vars,
            ..Scopes::default()
        };
        let pair = |n: &str, v: &str| KeyValue::new(n, v);
        let mut request = Request {
            url: "http://h/{{v}}".into(),
            params: vec![pair("p{{v}}", "{{v}}")],
            headers: vec![pair("h", "{{v}}")],
            body: Body::Json("{\"a\":{{v}}}".into()),
            auth: Auth::Bearer {
                token: "t{{v}}".into(),
            },
            ..Request::default()
        };

        let (resolved, out) = resolve_request(&request, &scopes);

        assert_eq!(resolved.url, "http://h/1");
        assert_eq!(resolved.params, [pair("p1", "1")]);
        assert_eq!(resolved.headers, [pair("h", "1")]);
        assert_eq!(resolved.body, Body::Json("{\"a\":1}".into()));
        assert_eq!(resolved.auth, Auth::Bearer { token: "t1".into() });
        assert!(out.is_complete());

        request.body = Body::Text {
            content_type: "{{ct}}".into(),
            text: "{{v}}".into(),
        };
        request.auth = Auth::Basic {
            username: "{{v}}".into(),
            password: "{{pw}}".into(),
        };

        let (resolved, out) = resolve_request(&request, &scopes);

        assert_eq!(
            resolved.body,
            Body::Text {
                content_type: "{{ct}}".into(),
                text: "1".into()
            }
        );
        assert_eq!(
            resolved.auth,
            Auth::Basic {
                username: "1".into(),
                password: "{{pw}}".into()
            }
        );
        assert_eq!(out.missing, BTreeSet::from([name("ct"), name("pw")]));

        request.body = Body::Form(vec![pair("{{v}}", "x")]);
        request.auth = Auth::None;

        let (resolved, _) = resolve_request(&request, &scopes);

        assert_eq!(resolved.body, Body::Form(vec![pair("1", "x")]));

        request.body = Body::None;

        assert_eq!(resolve_request(&request, &scopes).0.body, Body::None);
    }

    struct Vault(Vec<(Origin, &'static str, &'static str)>);

    impl SecretValues for Vault {
        fn secret(&self, origin: Origin, name: &VarName) -> Option<&str> {
            self.0
                .iter()
                .find(|(o, n, _)| *o == origin && *n == name.as_str())
                .map(|(_, _, v)| *v)
        }
    }

    #[test]
    fn secrets_come_from_their_own_source_and_are_reported() {
        let environment = [Variable::secret(name("token"))];
        let collection = [
            var("auth", "Bearer {{token}}"),
            Variable::secret(name("key")),
        ];
        let vault = Vault(vec![(Origin::Environment, "token", "t0k")]);
        let scopes = Scopes {
            environment: &environment,
            collection: &collection,
            secrets: Some(&vault),
            ..Scopes::default()
        };
        let mut out = Resolution::default();

        assert_eq!(
            resolve("{{auth}} {{key}}", &scopes, &mut out),
            "Bearer t0k {{key}}"
        );
        assert_eq!(
            out.secrets,
            BTreeSet::from([(Origin::Environment, name("token"))])
        );
        assert_eq!(
            out.unavailable,
            BTreeSet::from([(Origin::Collection, name("key"))])
        );
        assert!(!out.is_complete());

        // without a source every secret is unavailable, never empty
        let mut out = Resolution::default();
        let locked = Scopes {
            environment: &environment,
            ..Scopes::default()
        };

        assert_eq!(resolve("x{{token}}", &locked, &mut out), "x{{token}}");
        assert_eq!(
            out.unavailable,
            BTreeSet::from([(Origin::Environment, name("token"))])
        );
    }

    #[test]
    fn disabled_pairs_are_left_alone() {
        let mut off = KeyValue::new("x-{{gone}}", "{{gone}}");
        off.enabled = false;

        let request = Request {
            headers: vec![off.clone()],
            params: vec![off.clone()],
            ..Request::default()
        };
        let (resolved, out) = resolve_request(&request, &Scopes::default());

        assert_eq!(resolved.headers, [off.clone()]);
        assert_eq!(resolved.params, [off]);
        assert_eq!(out, Resolution::default());
    }

    proptest! {
        #[test]
        fn text_without_open_braces_is_unchanged(text in "[^{]*") {
            let (result, out) = run(&text, &[var("a", "1")]);

            prop_assert_eq!(result, text);
            prop_assert_eq!(out, Resolution::default());
        }

        #[test]
        fn defined_variables_never_survive(
            values in proptest::collection::vec("[a-z ]{0,8}", 1..5),
            filler in "[a-z /?=&]{0,10}",
        ) {
            let vars: Vec<Variable> = values
                .iter()
                .enumerate()
                .map(|(i, v)| var(&format!("v{i}"), v))
                .collect();
            let template: String = (0..vars.len())
                .flat_map(|i| [filler.clone(), format!("{{{{v{i}}}}}")])
                .collect();
            let expected: String = values.iter().flat_map(|v| [filler.as_str(), v]).collect();
            let (result, out) = run(&template, &vars);

            prop_assert_eq!(result, expected);
            prop_assert!(out.is_complete());
            prop_assert_eq!(out.used.len(), vars.len());
        }

        #[test]
        fn resolution_never_panics(text in ".{0,64}", value in ".{0,16}") {
            let _ = run(&text, &[var("a", &value), var("b", "{{a}}{{b}}")]);
        }
    }
}
