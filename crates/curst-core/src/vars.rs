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

/// where variable values come from
pub trait Lookup {
    /// the value of `name`, or `None` when no scope defines it
    fn value(&self, name: &VarName) -> Option<&str>;
}

/// the three scopes, searched in order: environment, collection, global
#[derive(Debug, Clone, Copy, Default)]
pub struct Scopes<'a> {
    /// the active environment's variables
    pub environment: &'a [Variable],

    /// the collection's variables
    pub collection: &'a [Variable],

    /// variables shared by every collection
    pub global: &'a [Variable],
}

impl Lookup for Scopes<'_> {
    fn value(&self, name: &VarName) -> Option<&str> {
        [self.environment, self.collection, self.global]
            .into_iter()
            .flatten()
            .find(|v| v.enabled && v.name == *name)
            .map(|v| v.value.as_str())
    }
}

/// what a resolution met on the way
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// every variable that was replaced, nested ones included
    pub used: BTreeSet<VarName>,

    /// variables no scope defines, left as written
    pub missing: BTreeSet<VarName>,

    /// variables whose value leads back to themselves, left as written
    pub cyclic: BTreeSet<VarName>,
}

impl Resolution {
    /// whether every variable was replaced
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.cyclic.is_empty()
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

        let Some(value) = lookup.value(&name) else {
            out.missing.insert(name);
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
        Auth::None => Auth::None,
        Auth::Basic { username, password } => Auth::Basic {
            username: resolve(username, lookup, out),
            password: resolve(password, lookup, out),
        },
        Auth::Bearer { token } => Auth::Bearer {
            token: resolve(token, lookup, out),
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

fn resolve_pairs(pairs: &[KeyValue], lookup: &impl Lookup, out: &mut Resolution) -> Vec<KeyValue> {
    pairs
        .iter()
        .map(|p| KeyValue {
            name: resolve(&p.name, lookup, out),
            value: resolve(&p.value, lookup, out),
            enabled: p.enabled,
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
