//! finds values that look like credentials, so they can be turned into
//! secrets before they are saved or shared
//!
//! a heuristic: a credential in an unusual place, such as inside a JSON
//! body, can get past it. a value made only of `{{templates}}` is never a
//! finding, since it holds no credential itself

use crate::model::{Auth, Body, Request};

/// where in a request a finding is
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// the user and password part of the URL
    UrlUserInfo,

    /// a parameter in the URL's own query string, counted from 0
    UrlQuery(usize),

    /// a row of the parameters table
    Param(usize),

    /// a row of the headers table
    Header(usize),

    /// a row of a form body
    FormField(usize),

    /// the basic auth password
    Password,

    /// the bearer token
    Token,
}

/// why a value looks like a credential
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// a header that carries credentials, such as `Authorization`
    CredentialHeader,

    /// a name such as `api_key`, `token` or `password`
    CredentialName,

    /// a password in a URL or in basic auth
    Password,

    /// a `Bearer` token
    Bearer,

    /// a JSON web token
    Jwt,

    /// a token with a known vendor prefix, such as `ghp_`
    KnownPrefix(&'static str),
}

impl Reason {
    /// a short explanation for the user
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::CredentialHeader => "this header carries credentials".into(),
            Self::CredentialName => "the name suggests a credential".into(),
            Self::Password => "this is a password".into(),
            Self::Bearer => "this is a bearer token".into(),
            Self::Jwt => "this looks like a JSON web token".into(),
            Self::KnownPrefix(prefix) => format!("this looks like a token ({prefix}…)"),
        }
    }
}

/// one value that looks like a credential; the value itself is never kept
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    /// where it is
    pub place: Place,

    /// why it was flagged
    pub reason: Reason,
}

const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "proxy-authorization", "cookie"];

/// names with one of these parts are taken as credentials
const CREDENTIAL_PARTS: [&str; 9] = [
    "token", "secret", "password", "passwd", "apikey", "api-key", "api_key", "session", "auth",
];

/// exact names that are credentials although they are short
const CREDENTIAL_NAMES: [&str; 5] = ["key", "pwd", "sig", "signature", "sid"];

/// names that contain a credential part but are not credentials
const NOT_CREDENTIALS: [&str; 4] = [
    "idempotency-key",
    "sec-websocket-key",
    "author",
    "authority",
];

const PREFIXES: [&str; 17] = [
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "glpat-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "npm_",
    "AKIA",
    "AIza",
];

/// whether `value` is empty or made only of templates, perhaps after a
/// scheme word such as `Bearer`
fn holds_nothing(value: &str) -> bool {
    let mut rest = String::new();
    let mut text = value;

    while let Some(open) = text.find("{{") {
        rest.push_str(&text[..open]);

        if let Some(close) = text[open..].find("}}") {
            text = &text[open + close + 2..];
        } else {
            rest.push_str(&text[open..]);
            text = "";
        }
    }

    rest.push_str(text);

    let rest = rest.trim();

    rest.is_empty()
        || ["bearer", "basic", "token"]
            .iter()
            .any(|word| rest.eq_ignore_ascii_case(word))
}

fn is_credential_name(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();

    if NOT_CREDENTIALS.contains(&name.as_str()) {
        return false;
    }

    CREDENTIAL_NAMES.contains(&name.as_str())
        || CREDENTIAL_PARTS.iter().any(|part| name.contains(part))
}

fn is_jwt(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    let base64url = |part: &str| {
        part.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };

    parts.len() == 3
        && word.starts_with("eyJ")
        && parts[0].len() > 10
        && parts[1].len() > 10
        && parts.iter().all(|p| base64url(p))
}

/// a credential recognized from the value alone
fn value_reason(value: &str) -> Option<Reason> {
    let value = value.trim();

    if let Some(token) = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        && !holds_nothing(token)
    {
        return Some(Reason::Bearer);
    }

    value
        .split(|c: char| c.is_whitespace() || matches!(c, '&' | ';' | ',' | '=' | '"' | '\''))
        .find_map(|word| {
            if is_jwt(word) {
                return Some(Reason::Jwt);
            }

            PREFIXES
                .iter()
                .find(|prefix| word.starts_with(**prefix) && word.len() >= prefix.len() + 12)
                .map(|prefix| Reason::KnownPrefix(prefix))
        })
}

/// why a name and value pair looks like a credential, if it does; `header`
/// says whether it is a header rather than a parameter or form field
#[must_use]
pub fn check_pair(name: &str, value: &str, header: bool) -> Option<Reason> {
    if holds_nothing(value) {
        return None;
    }

    if header
        && CREDENTIAL_HEADERS
            .iter()
            .any(|h| name.trim().eq_ignore_ascii_case(h))
    {
        return Some(Reason::CredentialHeader);
    }

    if is_credential_name(name) {
        return Some(Reason::CredentialName);
    }

    value_reason(value)
}

/// why a single value, such as an auth field, looks like a credential
#[must_use]
pub fn check_value(value: &str) -> Option<Reason> {
    if holds_nothing(value) {
        None
    } else {
        value_reason(value)
    }
}

/// every value in `request` that looks like a credential: in the URL,
/// parameters, headers, a form body and auth; enabled or not, since a
/// disabled row is still saved
#[must_use]
pub fn detect(request: &Request) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut found = |place, reason: Option<Reason>| {
        if let Some(reason) = reason {
            findings.push(Finding { place, reason });
        }
    };
    let (before_query, query) = request
        .url
        .split('#')
        .next()
        .unwrap_or_default()
        .split_once('?')
        .unwrap_or((&request.url, ""));
    let authority = before_query
        .split_once("://")
        .map_or("", |(_, rest)| rest.split('/').next().unwrap_or_default());

    if let Some((userinfo, _)) = authority.rsplit_once('@')
        && let Some((_, password)) = userinfo.split_once(':')
        && !holds_nothing(password)
    {
        found(Place::UrlUserInfo, Some(Reason::Password));
    }

    for (index, pair) in query.split('&').filter(|p| !p.is_empty()).enumerate() {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));

        found(Place::UrlQuery(index), check_pair(name, value, false));
    }

    for (index, pair) in request.params.iter().enumerate() {
        found(
            Place::Param(index),
            check_pair(&pair.name, &pair.value, false),
        );
    }

    for (index, pair) in request.headers.iter().enumerate() {
        found(
            Place::Header(index),
            check_pair(&pair.name, &pair.value, true),
        );
    }

    if let Body::Form(fields) = &request.body {
        for (index, pair) in fields.iter().enumerate() {
            found(
                Place::FormField(index),
                check_pair(&pair.name, &pair.value, false),
            );
        }
    }

    match &request.auth {
        Auth::Basic { password, .. } if !holds_nothing(password) => {
            found(Place::Password, Some(Reason::Password));
        }
        Auth::Bearer { token } if !holds_nothing(token) => {
            found(
                Place::Token,
                Some(value_reason(token).unwrap_or(Reason::Bearer)),
            );
        }
        Auth::Inherit | Auth::None | Auth::Basic { .. } | Auth::Bearer { .. } => {}
    }

    findings
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::model::KeyValue;

    /// a made-up token with the shape of a JWT, put together at run time so
    /// that no secret scanner mistakes this file for a leak
    fn jwt() -> String {
        // {"alg":"none"}, {"test":true}, and a fake signature
        [
            "eyJhbGciOiJub25lIn0",
            "eyJ0ZXN0Ijp0cnVlfQ",
            "ZmFrZS1zaWduYXR1cmU",
        ]
        .join(".")
    }

    /// a made-up token with a vendor prefix, put together at run time
    fn fake_token(prefix: &str) -> String {
        format!("{prefix}{}", "x0".repeat(18))
    }

    #[test]
    fn credential_headers_and_names() {
        assert_eq!(
            check_pair("Authorization", "Basic dTpw", true),
            Some(Reason::CredentialHeader)
        );
        assert_eq!(
            check_pair("COOKIE", "sid=1", true),
            Some(Reason::CredentialHeader)
        );
        assert_eq!(
            check_pair("X-API-Key", "abc", true),
            Some(Reason::CredentialName)
        );
        assert_eq!(
            check_pair("x-auth-token", "abc", true),
            Some(Reason::CredentialName)
        );
        assert_eq!(
            check_pair("api_key", "abc", false),
            Some(Reason::CredentialName)
        );
        assert_eq!(
            check_pair("access_token", "abc", false),
            Some(Reason::CredentialName)
        );
        assert_eq!(
            check_pair("signature", "abc", false),
            Some(Reason::CredentialName)
        );
        assert_eq!(
            check_pair("Password", "hunter2", false),
            Some(Reason::CredentialName)
        );

        // `Authorization` as a parameter name still matches by name
        assert_eq!(
            check_pair("authorization", "x", false),
            Some(Reason::CredentialName)
        );

        for (name, value) in [
            ("Accept", "application/json"),
            ("Idempotency-Key", "123"),
            ("author", "me"),
            ("page", "2"),
            ("Authorization", ""),
        ] {
            assert_eq!(check_pair(name, value, true), None, "{name}");
        }
    }

    #[test]
    fn templates_are_never_findings() {
        for value in [
            "{{token}}",
            "Bearer {{token}}",
            "  {{ a }}{{b}} ",
            "Basic {{creds}}",
        ] {
            assert_eq!(check_pair("Authorization", value, true), None, "{value}");
            assert_eq!(check_value(value), None, "{value}");
        }

        assert_eq!(
            check_pair("Authorization", "Bearer abc{{x}}", true),
            Some(Reason::CredentialHeader)
        );
        assert_eq!(check_value("{{unclosed"), None);
    }

    #[test]
    fn credentials_recognized_by_their_shape() {
        assert_eq!(check_pair("X-Anything", &jwt(), true), Some(Reason::Jwt));
        assert_eq!(check_value("Bearer abc.def"), Some(Reason::Bearer));
        assert_eq!(
            check_value(&fake_token("ghp_")),
            Some(Reason::KnownPrefix("ghp_"))
        );
        assert_eq!(
            check_value(&format!("key={};x", fake_token("AKIA"))),
            Some(Reason::KnownPrefix("AKIA"))
        );
        assert_eq!(check_value("ghp_short"), None);
        assert_eq!(check_value("eyJ.not.ajwt"), None);
        assert_eq!(check_value("plain words"), None);
        assert_eq!(
            Reason::KnownPrefix("ghp_").describe(),
            "this looks like a token (ghp_…)"
        );

        for reason in [
            Reason::CredentialHeader,
            Reason::CredentialName,
            Reason::Password,
            Reason::Bearer,
            Reason::Jwt,
        ] {
            assert_ne!(reason.describe(), "");
        }
    }

    #[test]
    fn every_part_of_a_request_is_checked() {
        let mut off = KeyValue::new("Cookie", "session=1");
        off.enabled = false;

        let request = Request {
            url: "https://me:pa55@h/x?page=2&api_key=abc#frag".into(),
            params: vec![KeyValue::new("q", "x"), KeyValue::new("token", "t")],
            headers: vec![KeyValue::new("Accept", "*/*"), off],
            body: Body::Form(vec![KeyValue::new("password", "p")]),
            auth: Auth::Bearer { token: jwt() },
            ..Request::default()
        };
        let places: Vec<(Place, Reason)> = detect(&request)
            .into_iter()
            .map(|f| (f.place, f.reason))
            .collect();

        assert_eq!(
            places,
            [
                (Place::UrlUserInfo, Reason::Password),
                (Place::UrlQuery(1), Reason::CredentialName),
                (Place::Param(1), Reason::CredentialName),
                (Place::Header(1), Reason::CredentialHeader),
                (Place::FormField(0), Reason::CredentialName),
                (Place::Token, Reason::Jwt),
            ]
        );

        let templated = Request {
            url: "https://{{user}}:{{pw}}@h/?api_key={{key}}".into(),
            auth: Auth::Basic {
                username: "u".into(),
                password: "{{pw}}".into(),
            },
            ..Request::default()
        };

        assert_eq!(detect(&templated), Vec::new());

        let basic = Request {
            url: "h".into(),
            auth: Auth::Basic {
                username: "u".into(),
                password: "p".into(),
            },
            ..Request::default()
        };

        assert_eq!(
            detect(&basic),
            [Finding {
                place: Place::Password,
                reason: Reason::Password
            }]
        );

        let bearer = Request {
            auth: Auth::Bearer {
                token: "opaque".into(),
            },
            ..Request::default()
        };

        assert_eq!(detect(&bearer)[0].reason, Reason::Bearer);
    }

    proptest! {
        #[test]
        fn a_template_is_never_a_finding(
            header in "[A-Za-z-]{1,20}",
            names in proptest::collection::vec("[a-z_]{1,10}", 1..4),
        ) {
            let value: String = names
                .iter()
                .flat_map(|n| ["{{", n.as_str(), "}}"])
                .collect();

            prop_assert_eq!(check_pair(&header, &value, true), None);
            prop_assert_eq!(check_pair(&header, &format!("Bearer {value}"), true), None);
        }

        #[test]
        fn a_github_token_is_found_under_any_name(
            name in "[A-Za-z-]{1,20}",
            body in "[A-Za-z0-9]{36}",
        ) {
            let token = format!("ghp_{body}");

            prop_assert!(check_pair(&name, &token, true).is_some());
        }

        #[test]
        fn detection_never_panics(url in ".{0,80}", name in ".{0,20}", value in ".{0,40}") {
            let request = Request {
                url,
                headers: vec![KeyValue::new(name.clone(), value.clone())],
                params: vec![KeyValue::new(name, value)],
                ..Request::default()
            };

            let _ = detect(&request);
        }
    }
}
