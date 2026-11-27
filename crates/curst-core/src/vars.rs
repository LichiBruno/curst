//! `{{name}}` templates and their resolution, without any I/O
//!
//! a value may itself contain `{{other}}`, which is expanded too; a variable
//! that refers back to itself, directly or not, is left as written and
//! reported as cyclic

use std::collections::BTreeSet;
use std::fmt::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Auth, Body, KeyValue, Request, Variable};

const MAX_NAME: usize = 128;

/// the variables curst fills in itself, each with what it gives; a name
/// starts with `$`, which no variable that is defined can
pub const BUILT_IN: [(&str, &str); 5] = [
    ("$uuid", "uuid v4"),
    ("$timestamp", "unix seconds"),
    ("$timestamp_ms", "unix milliseconds"),
    ("$iso_timestamp", "iso 8601 utc"),
    ("$random_int", "0 to 1000"),
];

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

    /// a built-in variable's name, such as `$uuid`, or `None` for any
    /// other text
    ///
    /// ```
    /// use curst_core::vars::VarName;
    ///
    /// assert!(VarName::built_in("$uuid").is_some());
    /// assert!(VarName::built_in("$nope").is_none());
    /// assert!(VarName::new("$uuid").is_none());
    /// ```
    #[must_use]
    pub fn built_in(name: &str) -> Option<Self> {
        BUILT_IN
            .iter()
            .any(|(built_in, _)| *built_in == name)
            .then(|| Self(name.to_owned()))
    }

    /// a name a template can hold: a variable's or a built-in one's
    #[must_use]
    pub fn template(name: &str) -> Option<Self> {
        Self::new(name).or_else(|| Self::built_in(name))
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

    /// the values curst fills in for one send
    BuiltIn,
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

/// the values of the built-in variables for one send: made once, so every
/// place a name appears in gets the same value
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    values: [String; BUILT_IN.len()],
}

impl Generated {
    /// values for now, from the system's clock and randomness
    ///
    /// # Errors
    ///
    /// when the system gives no randomness
    pub fn new() -> Result<Self, String> {
        let mut random = [0; 20];

        getrandom::fill(&mut random).map_err(|e| format!("no randomness: {e}"))?;

        Ok(Self::from_parts(SystemTime::now(), random))
    }

    /// values for `now`, the uuid and the number made from `random`
    ///
    /// ```
    /// use std::time::{Duration, UNIX_EPOCH};
    /// use curst_core::vars::{Generated, VarName};
    ///
    /// let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
    /// let values = Generated::from_parts(now, [0; 20]);
    /// let get = |name| values.get(&VarName::built_in(name).unwrap());
    ///
    /// assert_eq!(get("$timestamp"), Some("1700000000"));
    /// assert_eq!(get("$iso_timestamp"), Some("2023-11-14T22:13:20.123Z"));
    /// assert_eq!(get("$uuid"), Some("00000000-0000-4000-8000-000000000000"));
    /// ```
    #[must_use]
    pub fn from_parts(now: SystemTime, random: [u8; 20]) -> Self {
        let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
        let [uuid @ .., a, b, c, d] = random;
        let number = u32::from_le_bytes([a, b, c, d]) % 1001;

        Self {
            values: [
                uuid_v4(uuid),
                since.as_secs().to_string(),
                since.as_millis().to_string(),
                iso(since.as_secs(), since.subsec_millis()),
                number.to_string(),
            ],
        }
    }

    /// the value of the built-in variable `name`
    #[must_use]
    pub fn get(&self, name: &VarName) -> Option<&str> {
        BUILT_IN
            .iter()
            .position(|(built_in, _)| *built_in == name.as_str())
            .map(|index| self.values[index].as_str())
    }
}

/// a version 4 uuid from 16 random bytes
fn uuid_v4(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");

        hex
    });

    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `seconds` after the epoch as an ISO 8601 time in UTC, to the millisecond
fn iso(seconds: u64, millis: u32) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);

    // days to a date in the proleptic gregorian calendar, after Howard
    // Hinnant's civil_from_days, with eras of 400 years starting in march
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
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

    /// the built-in variables' values; without them they are not defined
    pub generated: Option<&'a Generated>,
}

impl Lookup for Scopes<'_> {
    fn find(&self, name: &VarName) -> Option<Found<'_>> {
        if let Some(value) = self.generated.and_then(|g| g.get(name)) {
            return Some(Found {
                value: Some(value),
                origin: Origin::BuiltIn,
                secret: false,
            });
        }

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

    /// names with a `$` that are not built in, such as `$nope`, left as
    /// written
    pub unknown: BTreeSet<String>,
}

impl Resolution {
    /// whether every variable was replaced
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_empty() && !self.blocked()
    }

    /// whether something no secret value can fix is left: a variable not
    /// defined, one that refers to itself, or an unknown built-in one
    #[must_use]
    pub fn blocked(&self) -> bool {
        !self.missing.is_empty() || !self.cyclic.is_empty() || !self.unknown.is_empty()
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

        let inner = after[..close].trim();
        let written = &rest[open..open + 2 + close + 2];

        // a `$` name that is not built in is reported, never sent as it is
        if let Some(own) = inner.strip_prefix('$')
            && VarName::new(own).is_some()
            && VarName::built_in(inner).is_none()
        {
            out.unknown.insert(inner.to_owned());
            result.push_str(written);
            rest = &after[close + 2..];

            continue;
        }

        // not a name: keep one brace and look again from the next one, so
        // `{{{a}}}` still finds `{{a}}`
        let Some(name) = VarName::template(inner) else {
            result.push('{');
            rest = &rest[open + 1..];

            continue;
        };

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
            generated: None,
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

    fn at(millis: u64, random: [u8; 20]) -> Generated {
        Generated::from_parts(
            UNIX_EPOCH + std::time::Duration::from_millis(millis),
            random,
        )
    }

    fn built_in(generated: &Generated, text: &str) -> String {
        generated
            .get(&VarName::built_in(text).unwrap())
            .unwrap()
            .to_owned()
    }

    #[test]
    fn built_in_names_are_reserved() {
        for (name, _) in BUILT_IN {
            assert!(VarName::new(name).is_none());
            assert_eq!(VarName::template(name), VarName::built_in(name));
        }

        assert!(VarName::built_in("uuid").is_none());
        assert!(VarName::template("$nope").is_none());
        assert_eq!(VarName::template("plain"), VarName::new("plain"));
    }

    #[test]
    fn times_are_written_in_utc() {
        let iso = |millis| built_in(&at(millis, [0; 20]), "$iso_timestamp");

        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso(4_107_542_399_999), "2100-02-28T23:59:59.999Z");
        assert_eq!(iso(1_795_686_600_007), "2026-11-26T09:50:00.007Z");

        let values = at(1_795_686_600_007, [0; 20]);

        assert_eq!(built_in(&values, "$timestamp"), "1795686600");
        assert_eq!(built_in(&values, "$timestamp_ms"), "1795686600007");

        // a clock set before the epoch gives the epoch
        let early = Generated::from_parts(UNIX_EPOCH - std::time::Duration::from_secs(1), [0; 20]);

        assert_eq!(built_in(&early, "$timestamp"), "0");
    }

    #[test]
    fn uuids_are_version_4_and_numbers_stay_in_range() {
        let ones = at(0, [0xff; 20]);

        assert_eq!(
            built_in(&ones, "$uuid"),
            "ffffffff-ffff-4fff-bfff-ffffffffffff"
        );
        assert_eq!(
            built_in(&ones, "$random_int"),
            (u32::MAX % 1001).to_string()
        );

        let mut random = [0; 20];

        random[16..].copy_from_slice(&1000_u32.to_le_bytes());

        assert_eq!(built_in(&at(0, random), "$random_int"), "1000");

        random[16..].copy_from_slice(&1001_u32.to_le_bytes());

        assert_eq!(built_in(&at(0, random), "$random_int"), "0");

        let fresh = Generated::new().unwrap();
        let uuid = built_in(&fresh, "$uuid");

        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert_ne!(uuid, built_in(&Generated::new().unwrap(), "$uuid"));
    }

    #[test]
    fn a_built_in_value_is_the_same_everywhere_in_a_request() {
        let mut random = [7; 20];

        random[0] = 1;

        let generated = at(1_000, random);
        let uuid = built_in(&generated, "$uuid");
        let vars = [var("id", "<{{ $uuid }}>")];
        let scopes = Scopes {
            environment: &vars,
            generated: Some(&generated),
            ..Scopes::default()
        };
        let request = Request {
            url: "http://h/{{$uuid}}?at={{$timestamp}}".into(),
            headers: vec![KeyValue::new("x-id", "{{id}}")],
            body: Body::Json("{\"id\":\"{{$uuid}}\"}".into()),
            auth: Auth::Bearer {
                token: "{{$uuid}}".into(),
            },
            ..Request::default()
        };

        let (resolved, out) = resolve_request(&request, &scopes);

        assert_eq!(resolved.url, format!("http://h/{uuid}?at=1"));
        assert_eq!(
            resolved.headers,
            [KeyValue::new("x-id", format!("<{uuid}>"))]
        );
        assert_eq!(resolved.body, Body::Json(format!("{{\"id\":\"{uuid}\"}}")));
        assert_eq!(resolved.auth, Auth::Bearer { token: uuid });
        assert!(out.is_complete());
        assert!(out.secrets.is_empty());
    }

    #[test]
    fn unknown_and_ungenerated_built_ins_block() {
        let (text, out) = run("{{$nope}} {{ $uuid }} {{$}} {{$a b}}", &[]);

        assert_eq!(text, "{{$nope}} {{ $uuid }} {{$}} {{$a b}}");
        assert_eq!(out.unknown, BTreeSet::from(["$nope".to_owned()]));
        assert_eq!(
            out.missing,
            BTreeSet::from([VarName::built_in("$uuid").unwrap()])
        );
        assert!(out.blocked());
        assert!(!out.is_complete());
        assert!(!Resolution::default().blocked());
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
